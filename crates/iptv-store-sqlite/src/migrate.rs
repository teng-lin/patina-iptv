//! Forward-only numbered migrations, applied at open.
//!
//! Each migration is a SQL file under `migrations/` that is never edited once
//! shipped. `schema_migrations` records every applied version; all pending
//! migrations run in one `BEGIN IMMEDIATE` transaction, so a failure leaves
//! the file at its previous version and two processes opening the same file
//! serialise instead of racing. Users skip releases, so opening any older
//! file applies every missing step in order (tested from each historical
//! schema in `tests/fixtures/`).
//!
//! Rules for new migrations: add `000N_name.sql`, append it to
//! `MIGRATIONS`, snapshot the resulting schema into
//! `tests/fixtures/schema_vN.sql` with
//! `cargo test -p iptv-store-sqlite --lib -- --ignored write_missing_schema_snapshots`,
//! and add a seed-data fixture for the previous version (see
//! `tests/it/migrations.rs`). A migration that rebuilds a
//! table must not rely on `PRAGMA foreign_keys` (it cannot change inside a
//! transaction): copy rows explicitly and keep ids stable.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::error::{Result, StoreError};

pub(crate) struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "provider_data",
        sql: include_str!("../migrations/0001_provider_data.sql"),
    },
    Migration {
        version: 2,
        name: "overlays",
        sql: include_str!("../migrations/0002_overlays.sql"),
    },
    Migration {
        version: 3,
        name: "organisation",
        sql: include_str!("../migrations/0003_organisation.sql"),
    },
    Migration {
        version: 4,
        name: "epg",
        sql: include_str!("../migrations/0004_epg.sql"),
    },
    Migration {
        version: 5,
        name: "source_settings",
        sql: include_str!("../migrations/0005_source_settings.sql"),
    },
    Migration {
        version: 6,
        name: "guide_sources",
        sql: include_str!("../migrations/0006_guide_sources.sql"),
    },
    Migration {
        version: 7,
        name: "library_preferences",
        sql: include_str!("../migrations/0007_library_preferences.sql"),
    },
    Migration {
        version: 8,
        name: "content_url_policy",
        sql: include_str!("../migrations/0008_content_url_policy.sql"),
    },
    Migration {
        version: 9,
        name: "search_history",
        sql: include_str!("../migrations/0009_search_history.sql"),
    },
    Migration {
        version: 10,
        name: "guide_revisions",
        sql: include_str!("../migrations/0010_guide_revisions.sql"),
    },
    Migration {
        version: 11,
        name: "recordings",
        sql: include_str!("../migrations/0011_recordings.sql"),
    },
];

/// The schema version this build writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

const BOOKKEEPING: &str = "CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    applied_at INTEGER NOT NULL
)";

/// The highest applied version, `0` for a new file. Checks that the
/// recorded versions are exactly `1..=max`.
pub(crate) fn current_version(conn: &Connection) -> Result<u32> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Ok(0);
    }
    let (count, max): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if count != max || max < 0 {
        return Err(StoreError::SchemaCorrupt(format!(
            "{count} recorded migrations but highest is {max}"
        )));
    }
    u32::try_from(max).map_err(|_| StoreError::SchemaCorrupt("version overflow".into()))
}

/// Applies every migration up to `target`. Returns the version reached.
/// A no-op (and no write lock) when the file is already current.
pub(crate) fn migrate_to(conn: &mut Connection, target: u32, now: i64) -> Result<u32> {
    let target = target.min(SCHEMA_VERSION);
    let found = current_version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found >= target {
        return Ok(found);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(BOOKKEEPING)?;
    // Another connection may have migrated while we waited for the lock.
    let current = current_version(&tx)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|m| m.version > current && m.version <= target)
    {
        let applied = tx.execute_batch(migration.sql).and_then(|()| {
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![migration.version, migration.name, now],
            )
        });
        applied.map_err(|source| StoreError::Migration {
            version: migration.version,
            source,
        })?;
    }
    let reached = current.max(target);
    tx.pragma_update(None, "user_version", reached)?;
    tx.commit()?;
    Ok(reached)
}

/// `sqlite_master` DDL in a stable, executable order (tables, then
/// indexes, then the rest), without SQLite internals and the derived search
/// index. Used for the historical snapshots and to compare a migrated file
/// with a fresh one.
pub fn schema_dump(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT type, name, sql FROM sqlite_master
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'channel_fts%'
         ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(format!(
            "-- {} {}\n{};\n",
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?
        ))
    })?;
    let mut out = String::new();
    for row in rows {
        // SQLite preserves the DDL's line endings. Compare schemas across OS checkouts.
        out.push_str(&row?.replace("\r\n", "\n"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_sequential() {
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(m.version as usize, i + 1, "{}", m.name);
        }
    }

    fn snapshot_path(version: u32) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/schema_v{version}.sql"))
    }

    /// The schema a fresh file has after migrating to `version`.
    fn schema_at(version: u32) -> String {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate_to(&mut conn, version, 0).unwrap(), version);
        schema_dump(&conn).unwrap()
    }

    /// A shipped migration must never change: migrating a fresh file to
    /// version N must reproduce the committed snapshot of version N.
    #[test]
    fn historical_snapshots_match_migrations() {
        for version in 1..=SCHEMA_VERSION {
            let path = snapshot_path(version);
            let snapshot = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "{}: {e}; create it with `cargo test -p iptv-store-sqlite --lib \
                     -- --ignored write_missing_schema_snapshots`",
                    path.display()
                )
            });
            assert_eq!(
                schema_at(version).replace("\r\n", "\n"),
                snapshot.replace("\r\n", "\n"),
                "schema v{version} drifted from {}",
                path.display()
            );
        }
    }

    /// Tool: writes `tests/fixtures/schema_vN.sql` for every version that
    /// has no snapshot yet, exactly as migrating a fresh file to N produces
    /// it. Never overwrites a snapshot: those guard shipped migrations.
    #[test]
    #[ignore = "tool: writes missing schema snapshots"]
    fn write_missing_schema_snapshots() {
        for version in 1..=SCHEMA_VERSION {
            let path = snapshot_path(version);
            if !path.exists() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, schema_at(version)).unwrap();
                println!("wrote {}", path.display());
            }
        }
    }

    #[test]
    fn rerun_is_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(
            migrate_to(&mut conn, SCHEMA_VERSION, 1).unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            migrate_to(&mut conn, SCHEMA_VERSION, 2).unwrap(),
            SCHEMA_VERSION
        );
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, i64::from(SCHEMA_VERSION));
    }

    #[test]
    fn newer_schema_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, SCHEMA_VERSION, 0).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations VALUES (?1, 'future', 0)",
            [SCHEMA_VERSION + 1],
        )
        .unwrap();
        assert!(matches!(
            migrate_to(&mut conn, SCHEMA_VERSION, 0),
            Err(StoreError::SchemaTooNew { .. })
        ));
    }

    #[test]
    fn holes_are_reported() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, SCHEMA_VERSION, 0).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 1", [])
            .unwrap();
        conn.execute("INSERT INTO schema_migrations VALUES (99, 'x', 0)", [])
            .unwrap();
        assert!(matches!(
            current_version(&conn),
            Err(StoreError::SchemaCorrupt(_))
        ));
    }
}
