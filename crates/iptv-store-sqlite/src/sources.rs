//! Sources: non-secret config in `sources.config`, secrets in
//! `source_credentials`, and per-source import status.

use std::collections::HashMap;

use iptv_providers::model::{
    M3uSourceConfig, MacAddress, Source, SourceConfig, SourceKind, StalkerCredentials,
    XtreamCredentials,
};
use iptv_providers::{Secret, SecretUrl};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

use crate::codec::{source_kind_from, source_kind_slug};
use crate::error::{Result, StoreError};
use crate::{SourceId, Store};

pub use iptv_library::StoredSource;

/// A source list row. Carries no secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSummary {
    pub id: SourceId,
    pub kind: SourceKind,
    pub name: String,
    pub position: i64,
    pub import: ImportStatus,
}

/// Rows written by the last successful [`Store::replace_catalog`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogCounts {
    pub groups: u32,
    pub channels: u32,
    pub vod: u32,
    pub series: u32,
    /// Input rows dropped because their id repeated an earlier row.
    pub duplicates: u32,
}

/// Per-source import metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportStatus {
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub counts: CatalogCounts,
    /// Kind of the last failure, cleared by the next success. Only the kind
    /// is stored: messages could quote portal text or URLs.
    pub last_error: Option<ImportErrorKind>,
    pub last_error_at: Option<i64>,
}

pub use iptv_library::ImportErrorKind;

/// Splits a source into (non-secret config JSON, secret fields).
fn split(config: &SourceConfig) -> (Value, Vec<(&'static str, String)>) {
    let mut secrets = Vec::new();
    let mut put = |name: &'static str, value: Option<&Secret>| {
        if let Some(v) = value {
            secrets.push((name, v.expose().to_owned()));
        }
    };
    let json = match config {
        SourceConfig::M3u(c) => {
            put("location", Some(&Secret::new(c.location.expose())));
            json!({"user_agent": c.user_agent, "referrer": c.referrer})
        }
        SourceConfig::Xtream(c) => {
            put("username", Some(&c.username));
            put("password", Some(&c.password));
            json!({"server_url": c.server_url, "user_agent": c.user_agent})
        }
        SourceConfig::Stalker(c) => {
            put("mac", Some(&Secret::new(c.mac.expose())));
            put("serial", c.serial.as_ref());
            put("device_id", c.device_id.as_ref());
            put("device_id2", c.device_id2.as_ref());
            put("signature", c.signature.as_ref());
            put("signature2", c.signature2.as_ref());
            put("login", c.login.as_ref());
            put("password", c.password.as_ref());
            json!({"portal_url": c.portal_url})
        }
    };
    (json, secrets)
}

fn join(
    kind: SourceKind,
    config: &Value,
    mut secrets: HashMap<String, String>,
) -> Result<SourceConfig> {
    let text = |key: &str| config.get(key).and_then(Value::as_str).map(str::to_owned);
    let mut take = |name: &str| secrets.remove(name).map(Secret::new);
    let required = |value: Option<Secret>, name: &str| {
        value.ok_or_else(|| StoreError::InvalidData(format!("missing credential '{name}'")))
    };
    Ok(match kind {
        SourceKind::M3u => SourceConfig::M3u(M3uSourceConfig {
            location: SecretUrl::new(required(take("location"), "location")?.expose()),
            user_agent: text("user_agent"),
            referrer: text("referrer"),
        }),
        SourceKind::Xtream => SourceConfig::Xtream(XtreamCredentials {
            server_url: text("server_url").unwrap_or_default(),
            username: required(take("username"), "username")?,
            password: required(take("password"), "password")?,
            user_agent: text("user_agent"),
        }),
        SourceKind::Stalker => SourceConfig::Stalker(StalkerCredentials {
            portal_url: text("portal_url").unwrap_or_default(),
            mac: MacAddress::from_stored(required(take("mac"), "mac")?.expose()),
            serial: take("serial"),
            device_id: take("device_id"),
            device_id2: take("device_id2"),
            signature: take("signature"),
            signature2: take("signature2"),
            login: take("login"),
            password: take("password"),
        }),
    })
}

fn write_credentials(conn: &Connection, id: SourceId, config: &SourceConfig) -> Result<Value> {
    let (json, secrets) = split(config);
    conn.execute(
        "DELETE FROM source_credentials WHERE source_id = ?1",
        [id.0],
    )?;
    let mut stmt = conn.prepare_cached(
        "INSERT INTO source_credentials (source_id, name, value) VALUES (?1, ?2, ?3)",
    )?;
    for (name, value) in secrets {
        stmt.execute(params![id.0, name, value])?;
    }
    Ok(json)
}

pub(crate) fn ensure_source(conn: &Connection, id: SourceId) -> Result<()> {
    let found: Option<i64> = conn
        .query_row("SELECT 1 FROM sources WHERE id = ?1", [id.0], |r| r.get(0))
        .optional()?;
    found.map(|_| ()).ok_or(StoreError::SourceNotFound(id))
}

fn import_status(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<ImportStatus> {
    let count = |i: usize| -> rusqlite::Result<u32> {
        Ok(row
            .get::<_, Option<i64>>(at + i)?
            .unwrap_or(0)
            .clamp(0, u32::MAX.into()) as u32)
    };
    Ok(ImportStatus {
        last_attempt_at: row.get(at)?,
        last_success_at: row.get(at + 1)?,
        counts: CatalogCounts {
            groups: count(2)?,
            channels: count(3)?,
            vod: count(4)?,
            series: count(5)?,
            duplicates: count(6)?,
        },
        last_error: row
            .get::<_, Option<String>>(at + 7)?
            .map(|s| ImportErrorKind::from_slug(&s)),
        last_error_at: row.get(at + 8)?,
    })
}

const IMPORT_COLUMNS: &str = "i.last_attempt_at, i.last_success_at, i.group_count, \
    i.channel_count, i.vod_count, i.series_count, i.duplicate_count, i.last_error_kind, \
    i.last_error_at";

pub(crate) fn update_source_in(
    conn: &Connection,
    id: SourceId,
    source: &Source,
    now: i64,
) -> Result<()> {
    ensure_source(conn, id)?;
    let config = write_credentials(conn, id, &source.config)?;
    conn.execute(
        "UPDATE sources SET kind = ?2, name = ?3, config = ?4, updated_at = ?5 WHERE id = ?1",
        params![
            id.0,
            source_kind_slug(source.kind()),
            source.name,
            config.to_string(),
            now
        ],
    )?;
    Ok(())
}

impl Store {
    /// Adds a source at the end of the list.
    pub fn add_source(&mut self, source: &Source) -> Result<SourceId> {
        let now = self.now();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO sources (kind, name, config, position, created_at, updated_at)
             VALUES (?1, ?2, '{}', (SELECT COALESCE(MAX(position) + 1, 0) FROM sources), ?3, ?3)",
            params![source_kind_slug(source.kind()), source.name, now],
        )?;
        let id = SourceId(tx.last_insert_rowid());
        let config = write_credentials(&tx, id, &source.config)?;
        tx.execute(
            "UPDATE sources SET config = ?2 WHERE id = ?1",
            params![id.0, config.to_string()],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Replaces name, config and credentials. Changing the kind of a source
    /// is allowed but its catalog ids will no longer match, so overlays
    /// become orphans until the next import produces matching ids.
    pub fn update_source(&mut self, id: SourceId, source: &Source) -> Result<()> {
        let now = self.now();
        // Immediate: it reads before it writes, and in WAL mode a deferred
        // transaction whose snapshot another writer's commit made stale
        // fails at once ("database is locked") instead of waiting.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        update_source_in(&tx, id, source, now)?;
        tx.commit()?;
        Ok(())
    }

    pub fn rename_source(&mut self, id: SourceId, name: &str) -> Result<()> {
        let now = self.now();
        let changed = self.conn.execute(
            "UPDATE sources SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.0, name, now],
        )?;
        if changed == 0 {
            return Err(StoreError::SourceNotFound(id));
        }
        Ok(())
    }

    /// Deletes the source with its catalog, credentials, settings, stored
    /// guide, customisations and history (and forgets it as the shown
    /// source). Returns `false` if it did not exist.
    pub fn delete_source(&mut self, id: SourceId) -> Result<bool> {
        let tx = self.conn.transaction()?;
        crate::search::forget_source(&tx, self.fts_index, id)?;
        crate::source_settings::forget_active(&tx, id)?;
        let deleted = tx.execute("DELETE FROM sources WHERE id = ?1", [id.0])?;
        tx.commit()?;
        Ok(deleted > 0)
    }

    /// One source with its secrets.
    pub fn source(&self, id: SourceId) -> Result<Option<StoredSource>> {
        let row = self
            .conn
            .query_row(
                "SELECT kind, name, config, position, created_at, updated_at FROM sources WHERE id = ?1",
                [id.0],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((kind, name, config, position, created_at, updated_at)) = row else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT name, value FROM source_credentials WHERE source_id = ?1")?;
        let secrets = stmt
            .query_map([id.0], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<HashMap<String, String>>>()?;
        let config: Value = serde_json::from_str(&config)?;
        Ok(Some(StoredSource {
            id,
            source: Source {
                name,
                config: join(source_kind_from(&kind)?, &config, secrets)?,
            },
            position,
            created_at,
            updated_at,
        }))
    }

    /// All sources in list order, without secrets.
    pub fn sources(&self) -> Result<Vec<SourceSummary>> {
        let sql = format!(
            "SELECT s.id, s.kind, s.name, s.position, {IMPORT_COLUMNS}
             FROM sources s LEFT JOIN source_imports i ON i.source_id = s.id
             ORDER BY s.position, s.id"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                import_status(r, 4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, kind, name, position, import) = row?;
            Ok(SourceSummary {
                id: SourceId(id),
                kind: source_kind_from(&kind)?,
                name,
                position,
                import,
            })
        })
        .collect()
    }

    /// Import status; all-`None` if the source never imported.
    pub fn import_status(&self, id: SourceId) -> Result<ImportStatus> {
        ensure_source(&self.conn, id)?;
        let sql = format!("SELECT {IMPORT_COLUMNS} FROM source_imports i WHERE i.source_id = ?1");
        Ok(self
            .conn
            .query_row(&sql, [id.0], |r| import_status(r, 0))
            .optional()?
            .unwrap_or_default())
    }

    /// Records a failed import attempt. The catalog and the last success
    /// stay as they were, so a failure never looks like empty content.
    pub fn record_import_failure(&mut self, id: SourceId, kind: ImportErrorKind) -> Result<()> {
        ensure_source(&self.conn, id)?;
        let now = self.now();
        self.conn.execute(
            "INSERT INTO source_imports (source_id, last_attempt_at, last_error_kind, last_error_at)
             VALUES (?1, ?2, ?3, ?2)
             ON CONFLICT (source_id) DO UPDATE SET last_attempt_at = excluded.last_attempt_at,
                 last_error_kind = excluded.last_error_kind, last_error_at = excluded.last_error_at",
            params![id.0, now, kind.slug()],
        )?;
        Ok(())
    }
}

pub(crate) fn record_import_success(
    conn: &Connection,
    id: SourceId,
    counts: &CatalogCounts,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO source_imports (source_id, last_attempt_at, last_success_at, group_count,
             channel_count, vod_count, series_count, duplicate_count, last_error_kind, last_error_at)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)
         ON CONFLICT (source_id) DO UPDATE SET last_attempt_at = excluded.last_attempt_at,
             last_success_at = excluded.last_success_at, group_count = excluded.group_count,
             channel_count = excluded.channel_count, vod_count = excluded.vod_count,
             series_count = excluded.series_count, duplicate_count = excluded.duplicate_count,
             last_error_kind = NULL, last_error_at = NULL",
        params![
            id.0,
            now,
            counts.groups,
            counts.channels,
            counts.vod,
            counts.series,
            counts.duplicates
        ],
    )?;
    Ok(())
}
