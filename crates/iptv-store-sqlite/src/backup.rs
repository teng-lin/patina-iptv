//! Complete SQLite snapshots, including committed pages still in the WAL.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, backup::Backup, params};

use crate::{Result, SCHEMA_VERSION, Store, StoreError, migrate};

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData(message.to_owned())
}

// Archive metadata may describe another OS's paths. Match whole components,
// never string prefixes, and reject traversal before using native path rules.
fn archived_relative(local: &Path, root: &Path) -> Result<PathBuf> {
    fn portable(path: &Path) -> Result<String> {
        let mut s = path
            .to_str()
            .ok_or_else(|| invalid("backup path is not UTF-8"))?
            .to_owned();
        // file:///C:/... on a Unix restore host.
        if s.starts_with('/')
            && s.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
            && s.as_bytes().get(2) == Some(&b':')
            && s.as_bytes().get(3) == Some(&b'/')
        {
            s.remove(0);
        }
        iptv_import::backup_manifest::portable_absolute_path(&s)
            .ok_or_else(|| invalid("backup path is not an absolute storage path"))
    }
    let local = portable(local)?;
    let root = portable(root)?;
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let relative = local
        .strip_prefix(&prefix)
        .ok_or_else(|| invalid("local resource is outside the backup directory"))?;
    if relative
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == ".." || p.contains([':', '\0']))
    {
        return Err(invalid("invalid local resource location"));
    }
    Ok(PathBuf::from(relative))
}

impl Store {
    /// Writes a complete, consistent snapshot to a new private file. Existing
    /// files are never replaced. The destination has no dependency on the live
    /// database's WAL or on any still-open connection.
    pub fn snapshot_to(&self, path: &Path) -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|_| invalid("cannot create snapshot file"))?;
        drop(file);
        let result = (|| {
            iptv_sqlcipher_init::initialize()?;
            let mut destination = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            destination.busy_timeout(Duration::from_secs(5))?;
            if self.key.is_some() {
                // SQLCipher deliberately rejects sqlite3_backup across codec
                // boundaries. User-requested portable exports use its supported
                // export API; the archive is explicitly plaintext.
                self.conn.execute(
                    "ATTACH DATABASE ?1 AS portable_backup KEY ''",
                    [path
                        .to_str()
                        .ok_or_else(|| invalid("snapshot path is not UTF-8"))?],
                )?;
                let exported = (|| -> Result<()> {
                    let transaction = self.conn.unchecked_transaction()?;
                    transaction.query_row(
                        "SELECT sqlcipher_export('portable_backup')",
                        [],
                        |_| Ok(()),
                    )?;
                    let version: u32 =
                        transaction.pragma_query_value(None, "user_version", |r| r.get(0))?;
                    transaction.pragma_update(Some("portable_backup"), "user_version", version)?;
                    transaction.commit()?;
                    Ok(())
                })();
                let detached = self.conn.execute("DETACH DATABASE portable_backup", []);
                exported?;
                detached?;
            } else {
                Backup::new(&self.conn, &mut destination)?.run_to_completion(
                    256,
                    Duration::from_millis(5),
                    None,
                )?;
            }
            // Media and host-local jobs are outside portable catalogue backups.
            destination.execute("DELETE FROM recording_rules", [])?;
            destination.execute("DELETE FROM recording_jobs", [])?;
            // A standalone file, even when the source uses WAL.
            destination.pragma_update(None, "journal_mode", "DELETE")?;
            drop(destination);
            Self::validate_snapshot(path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }

    /// Checks an untrusted snapshot without migrating or modifying it. Rejects
    /// unknown schema objects as well as damaged, foreign-key-invalid and newer
    /// files. Migrations run only after the caller accepts the validated file.
    pub fn validate_snapshot(path: &Path) -> Result<()> {
        iptv_sqlcipher_init::initialize()?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::validate_snapshot_connection(&conn)
    }

    pub fn validate_encrypted_snapshot(path: &Path, key: &crate::DatabaseKey) -> Result<()> {
        iptv_sqlcipher_init::initialize()?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        key.apply(&conn)?;
        Self::validate_snapshot_connection(&conn)
    }

    /// Converts validated portable restore content to the live key before it
    /// becomes a persistent pending restore. Plaintext stores are test-only.
    pub fn seal_snapshot(&self, path: &Path) -> Result<()> {
        if let Some(key) = &self.key {
            drop(Self::open_encrypted(path, key)?);
            Self::validate_encrypted_snapshot(path, key)
        } else {
            Self::validate_snapshot(path)
        }
    }

    fn validate_snapshot_connection(conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "trusted_schema", false)?;
        let version = migrate::current_version(conn)?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if version == 0 {
            return Err(invalid("snapshot is not an application database"));
        }
        let user_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if user_version != version {
            return Err(invalid("snapshot schema version mismatch"));
        }
        iptv_sqlcipher_init::initialize()?;
        let mut expected = Connection::open_in_memory()?;
        migrate::migrate_to(&mut expected, version, 0)?;
        if migrate::schema_dump(conn)? != migrate::schema_dump(&expected)? {
            return Err(invalid("snapshot has an unexpected database schema"));
        }
        // schema_dump deliberately omits the derived FTS tables. Compare those
        // independently, so an object merely prefixed channel_fts cannot evade
        // validation. A snapshot from a host without FTS is also valid.
        let derived = |db: &Connection| -> Result<Vec<(String, String, String)>> {
            let mut statement = db.prepare(
                "SELECT type, name, sql FROM sqlite_master
                 WHERE sql IS NOT NULL AND name LIKE 'channel_fts%' ORDER BY name",
            )?;
            Ok(statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        };
        let actual_fts = derived(conn)?;
        if !actual_fts.is_empty() {
            crate::search::prepare(&mut expected)?;
            if actual_fts != derived(&expected)? {
                return Err(invalid("snapshot has an unexpected search schema"));
            }
        }
        for migration in migrate::MIGRATIONS.iter().take(version as usize) {
            let name: String = conn.query_row(
                "SELECT name FROM schema_migrations WHERE version = ?1",
                [migration.version],
                |r| r.get(0),
            )?;
            if name != migration.name {
                return Err(invalid("snapshot migration history mismatch"));
            }
        }
        let mut check = conn.prepare("PRAGMA integrity_check")?;
        let results = check
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if results.as_slice() != ["ok"] {
            return Err(invalid("snapshot integrity check failed"));
        }
        let mut check = conn.prepare("PRAGMA foreign_key_check")?;
        if check.query([])?.next()?.is_some() {
            return Err(invalid("snapshot foreign key check failed"));
        }
        Ok(())
    }

    /// Rewrites imported local playlist locations when moving an entire data
    /// directory to another device/home. All local playlists must be under the
    /// archived root; unrelated filesystem paths cannot silently become valid.
    /// The caller copies those files separately before activating the snapshot.
    pub fn rebase_local_playlists(&mut self, old_root: &Path, new_root: &Path) -> Result<()> {
        let mut explicit_guides = std::collections::HashMap::new();
        for summary in self.sources()? {
            if let Some(source) = self.source(summary.id)?
                && let iptv_providers::model::SourceConfig::Xtream(credentials) =
                    source.source.config
                && let Ok(client) =
                    iptv_providers::xtream::XtreamClient::from_credentials(&credentials)
            {
                explicit_guides.insert(summary.id.0, client.xmltv_url());
            }
        }
        let tx = self.conn.transaction()?;
        let locations = {
            let mut statement = tx.prepare(
                "SELECT c.source_id, c.value FROM source_credentials c
                 JOIN sources s ON s.id = c.source_id
                 WHERE s.kind = 'm3u' AND c.name = 'location'",
            )?;
            statement
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (source, location) in locations {
            let Some(local) = iptv_providers::content_url::local_path(&location)
                .map_err(|_| invalid("invalid local playlist URL"))?
            else {
                continue;
            };
            let relative = archived_relative(&local, old_root)?;
            let rebased = new_root.join(relative);
            let rebased = rebased
                .to_str()
                .ok_or_else(|| invalid("local playlist location is not UTF-8"))?;
            tx.execute(
                "UPDATE source_credentials SET value = ?1
                 WHERE source_id = ?2 AND name = 'location'",
                params![rebased, source],
            )?;
        }
        // Guide URLs are just as capable of naming local resources as a
        // playlist location. Every local reference must stay inside the archive.
        for table in ["guide_sources", "epg_feeds"] {
            let guides = {
                let provenance = if table == "guide_sources" {
                    "provider_index IS NOT NULL"
                } else {
                    "content_supplied"
                };
                let mut statement = tx.prepare(&format!(
                    "SELECT source_id, feed_id, url, {provenance} FROM {table}"
                ))?;
                statement
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, u32>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, bool>(3)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (source, feed, location, content_supplied) in guides {
                let local = iptv_providers::content_url::local_path(&location)
                    .map_err(|_| invalid("invalid guide URL"))?;
                if let Some(local) = local {
                    let relative = archived_relative(&local, old_root)?;
                    let rebased = new_root.join(relative);
                    tx.execute(
                        &format!("UPDATE {table} SET url=?1 WHERE source_id=?2 AND feed_id=?3"),
                        params![
                            rebased
                                .to_str()
                                .ok_or_else(|| invalid("guide path is not UTF-8"))?,
                            source,
                            feed
                        ],
                    )?;
                } else {
                    let explicit = !content_supplied
                        || explicit_guides
                            .get(&source)
                            .is_some_and(|url| url == &location);
                    let validate = if explicit {
                        iptv_providers::content_url::validate_picked
                    } else {
                        iptv_providers::content_url::validate
                    };
                    validate(&location)
                        .map_err(|_| invalid("backup guide URL violates its source policy"))?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn restores_windows_paths_on_any_host_without_allowing_escape() {
        let root = Path::new(r"C:\Users\Synthetic\Patina");
        for path in [
            r"C:\Users\Synthetic\Patina\playlists\a.m3u",
            "/C:/Users/Synthetic/Patina/playlists/a.m3u",
        ] {
            assert_eq!(
                archived_relative(Path::new(path), root).unwrap(),
                Path::new("playlists/a.m3u")
            );
        }
        for path in [
            "C:/Users/Synthetic/Patina/../outside",
            "C:/Users/Synthetic/Patina-other/a",
            "C:/Users/Synthetic/Patina/playlists/C:/a",
        ] {
            assert!(archived_relative(Path::new(path), root).is_err());
        }
    }

    #[test]
    fn restores_unc_and_verbatim_paths_with_either_absolute_spelling() {
        for (roots, locations) in [
            (
                [
                    r"C:\Users\Synthetic\Patina",
                    r"\\?\C:\Users\Synthetic\Patina",
                ],
                [
                    r"C:\Users\Synthetic\Patina\playlists\a.m3u",
                    r"\\?\C:\Users\Synthetic\Patina\playlists\a.m3u",
                ],
            ),
            (
                [
                    r"\\synthetic-server\share\Patina",
                    r"\\?\UNC\synthetic-server\share\Patina",
                ],
                [
                    r"\\synthetic-server\share\Patina\playlists\a.m3u",
                    r"\\?\UNC\synthetic-server\share\Patina\playlists\a.m3u",
                ],
            ),
        ] {
            for root in roots {
                for location in locations {
                    assert_eq!(
                        archived_relative(Path::new(location), Path::new(root)).unwrap(),
                        Path::new("playlists/a.m3u"),
                        "{location} under {root}",
                    );
                }
            }
        }
    }

    #[test]
    fn unc_rebasing_rejects_other_shares_devices_and_traversal() {
        let root = Path::new(r"\\synthetic-server\share\Patina");
        for location in [
            r"\\synthetic-server\share-other\Patina\playlists\a.m3u",
            r"\\other-server\share\Patina\playlists\a.m3u",
            r"\\?\UNC\synthetic-server\share\Patina\..\outside.m3u",
            r"\\?\UNC\synthetic-server\share\Patina\playlists\C:\outside.m3u",
            r"\\.\UNC\synthetic-server\share\Patina\playlists\a.m3u",
            r"\synthetic-server\share\Patina\playlists\a.m3u",
        ] {
            assert!(
                archived_relative(Path::new(location), root).is_err(),
                "{location}"
            );
        }
    }
}
