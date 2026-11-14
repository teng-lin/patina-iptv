//! SQLCipher database encryption. Keys belong to a platform credential broker.
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use zeroize::Zeroizing;

use crate::{OpenOptions as StoreOptions, Result, Store, StoreError};

/// A random database key. Copies erase their bytes on drop; Debug never reveals it.
#[derive(Clone)]
pub struct DatabaseKey(Zeroizing<[u8; 32]>);
impl DatabaseKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    fn sql_key(&self) -> Zeroizing<String> {
        use std::fmt::Write;
        let mut value = Zeroizing::new(String::with_capacity(67));
        value.push_str("x'");
        for byte in self.0.iter() {
            write!(&mut *value, "{byte:02x}").expect("writing to String");
        }
        value.push('\'');
        value
    }
    pub(super) fn apply(&self, conn: &Connection) -> Result<()> {
        let version: String = conn
            .pragma_query_value(None, "cipher_version", |r| r.get(0))
            .map_err(|_| StoreError::Unavailable("SQLCipher is not available"))?;
        if version.is_empty() {
            return Err(StoreError::Unavailable("SQLCipher is not available"));
        }
        conn.pragma_update(None, "key", &*self.sql_key())
            .map_err(|_| StoreError::Unavailable("could not initialize database encryption"))?;
        // Temporary sort/index pages must not spill unencrypted data to disk.
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    }
}
impl std::fmt::Debug for DatabaseKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DatabaseKey([REDACTED])")
    }
}
fn io_error(_: std::io::Error) -> StoreError {
    StoreError::Unavailable("encrypted database filesystem operation failed")
}
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
fn private_file(path: &Path, exclusive: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(io_error)
}
impl Store {
    /// Opens an encrypted database, converting a legacy plaintext file atomically.
    /// The original remains usable if export, validation, or migration fails.
    pub fn open_encrypted(path: impl AsRef<Path>, key: &DatabaseKey) -> Result<Self> {
        let path = path.as_ref();
        let lock = private_file(&sibling(path, ".encryption-lock"), false)?;
        lock.lock().map_err(io_error)?;
        let mut header = [0; 16];
        let legacy = match File::open(path) {
            Ok(mut file) => file.read_exact(&mut header).is_ok() && &header == b"SQLite format 3\0",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(io_error(e)),
        };
        let encrypted_marker = sibling(path, ".encrypted");
        if legacy {
            if encrypted_marker.exists() {
                return Err(StoreError::Unavailable(
                    "refusing plaintext database in place of encrypted installation",
                ));
            }
            Self::encrypt_legacy(path, key)?;
        }
        // Create with private permissions before SQLite opens a new file.
        drop(private_file(path, false)?);
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        key.apply(&conn)?;
        let mut store = Self::init(conn, &StoreOptions::default())?;
        store.key = Some(key.clone());
        if !encrypted_marker.exists() {
            private_file(&encrypted_marker, false)?
                .sync_all()
                .map_err(io_error)?;
            #[cfg(unix)]
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                File::open(parent)
                    .and_then(|dir| dir.sync_all())
                    .map_err(io_error)?;
            }
        }
        Ok(store)
    }
    /// Opens scratch storage with exactly the live store's encryption policy.
    /// Plaintext is possible only for an explicitly plaintext/test Store.
    pub fn open_staging_connection(&self, path: &Path) -> Result<Connection> {
        drop(private_file(path, false)?);
        let conn = Connection::open(path)?;
        if let Some(key) = &self.key {
            key.apply(&conn)?;
        }
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(conn)
    }
    /// Attaches a scratch database created by this store. SQLCipher must receive
    /// the raw key again: inheriting a main database codec also inherits its salt.
    pub fn attach_staging_database(&self, path: &Path, schema: &str) -> Result<()> {
        if schema.is_empty()
            || !schema
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(StoreError::Unavailable(
                "invalid scratch database schema name",
            ));
        }
        let key = self
            .key
            .as_ref()
            .map(DatabaseKey::sql_key)
            .unwrap_or_default();
        self.conn
            .execute(
                &format!("ATTACH DATABASE ?1 AS \"{schema}\" KEY ?2"),
                params![
                    path.to_str().ok_or(StoreError::Unavailable(
                        "scratch database path is not UTF-8"
                    ))?,
                    &*key
                ],
            )
            .map_err(|_| StoreError::Unavailable("could not attach scratch database"))?;
        Ok(())
    }
    fn encrypt_legacy(path: &Path, key: &DatabaseKey) -> Result<()> {
        let destination = sibling(path, ".encrypting");
        // A prior crash may leave an encrypted, incomplete scratch copy. Never
        // treat it as authoritative: the original path is the commit marker.
        for stale in [
            destination.clone(),
            sibling(&destination, "-wal"),
            sibling(&destination, "-shm"),
            sibling(&destination, "-journal"),
        ] {
            match std::fs::remove_file(stale) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_error(e)),
            }
        }
        drop(private_file(&destination, true)?);
        let result = (|| {
            let original = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            original.busy_timeout(std::time::Duration::from_secs(5))?;
            original.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
            // Checkpoint committed legacy WAL pages before removing its sidecars.
            original.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            let mode: String =
                original.pragma_update_and_check(None, "journal_mode", "DELETE", |r| r.get(0))?;
            if !mode.eq_ignore_ascii_case("delete") {
                return Err(StoreError::Unavailable("legacy database is in use"));
            }
            original.execute_batch("BEGIN EXCLUSIVE;")?;
            let version: i64 = original.pragma_query_value(None, "user_version", |r| r.get(0))?;
            original
                .execute(
                    "ATTACH DATABASE ?1 AS encrypted KEY ?2",
                    params![
                        destination
                            .to_str()
                            .ok_or(StoreError::Unavailable("database path is not UTF-8"))?,
                        &*key.sql_key()
                    ],
                )
                .map_err(|_| {
                    StoreError::Unavailable("could not attach encrypted migration database")
                })?;
            original.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
            original.pragma_update(Some("encrypted"), "user_version", version)?;
            original.execute_batch("COMMIT; DETACH DATABASE encrypted;")?;
            // Run every pending schema migration on the encrypted copy before
            // replacing the legacy database. An unsupported schema keeps the old file.
            let conn = Connection::open(&destination)?;
            key.apply(&conn)?;
            let checked = Self::init(conn, &StoreOptions::default())?;
            let integrity: String = checked
                .conn
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            if integrity != "ok" {
                return Err(StoreError::Unavailable(
                    "encrypted database integrity check failed",
                ));
            }
            checked
                .conn
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            checked.conn.pragma_update(None, "journal_mode", "DELETE")?;
            drop(checked);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&destination)
                .and_then(|file| file.sync_all())
                .map_err(io_error)?;
            #[cfg(windows)]
            drop(original);
            std::fs::rename(&destination, path).map_err(io_error)?;
            #[cfg(unix)]
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                File::open(parent)
                    .and_then(|dir| dir.sync_all())
                    .map_err(io_error)?;
            }
            #[cfg(not(windows))]
            drop(original);
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&destination);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SCHEMA_VERSION, migrate};

    fn key() -> DatabaseKey {
        DatabaseKey::from_bytes([0x54; 32])
    }
    fn no_plaintext(path: &Path) {
        let bytes = std::fs::read(path).unwrap();
        assert!(!bytes.starts_with(b"SQLite format 3"));
        assert!(
            !bytes
                .windows(b"synthetic-secret".len())
                .any(|part| part == b"synthetic-secret")
        );
    }
    #[test]
    fn encrypted_database_and_wal_reject_plaintext_and_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let store = Store::open_encrypted(&path, &key()).unwrap();
        store.conn.execute_batch("CREATE TABLE secret_test(value TEXT); INSERT INTO secret_test VALUES('https://synthetic.invalid/synthetic-secret');").unwrap();
        no_plaintext(&path);
        no_plaintext(&sibling(&path, "-wal"));
        assert!(Store::open(&path).is_err());
        assert!(Store::open_encrypted(&path, &DatabaseKey::from_bytes([0x55; 32])).is_err());
        drop(store);
        let reopened = Store::open_encrypted(&path, &key()).unwrap();
        let value: String = reopened
            .conn
            .query_row("SELECT value FROM secret_test", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "https://synthetic.invalid/synthetic-secret");
    }
    #[test]
    fn encrypted_installation_refuses_plaintext_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let store = Store::open_encrypted(&path, &key()).unwrap();
        drop(store);

        // Replace the encrypted database with a plaintext SQLite database.
        let _ = std::fs::remove_file(&path);
        let plain_conn = Connection::open(&path).unwrap();
        plain_conn
            .execute_batch("CREATE TABLE untrusted(id INTEGER);")
            .unwrap();
        drop(plain_conn);

        // Attempting to reopen as encrypted must refuse the plaintext file.
        let err = Store::open_encrypted(&path, &key()).unwrap_err();
        assert!(matches!(err, StoreError::Unavailable(_)));
        let header = std::fs::read(&path).unwrap();
        assert!(
            header.starts_with(b"SQLite format 3"),
            "file was not adopted or converted"
        );
    }
    #[test]
    fn every_legacy_schema_migrates_without_plaintext_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        for version in 1..=SCHEMA_VERSION {
            let path = dir.path().join(format!("v{version}.sqlite"));
            let mut conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            migrate::migrate_to(&mut conn, version, 0).unwrap();
            for seed in 1..=version {
                let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/fixtures/data_v{seed}.sql"));
                conn.execute_batch(&std::fs::read_to_string(fixture).unwrap())
                    .unwrap();
            }
            let credentials = |db: &Connection| -> Vec<(i64, String, String)> {
                db.prepare(
                    "SELECT source_id,name,value FROM source_credentials ORDER BY source_id,name",
                )
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
            };
            let before = credentials(&conn);
            assert!(!before.is_empty());
            drop(conn);
            let store = Store::open_encrypted(&path, &key()).unwrap();
            assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
            assert_eq!(credentials(&store.conn), before);
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(b"fixture-pass".len())
                    .any(|part| part == b"fixture-pass")
            );
            no_plaintext(&path);
            for suffix in ["-wal", "-journal", ".encrypting"] {
                let side = sibling(&path, suffix);
                if side.exists() {
                    no_plaintext(&side);
                }
            }
        }
    }
    #[test]
    fn newer_legacy_schema_is_not_replaced_on_migration_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO schema_migrations VALUES(?1, 'future', 0)",
                [SCHEMA_VERSION + 1],
            )
            .unwrap();
        drop(store);
        assert!(Store::open_encrypted(&path, &key()).is_err());
        assert!(
            std::fs::read(&path)
                .unwrap()
                .starts_with(b"SQLite format 3\0")
        );
        assert_eq!(
            migrate::current_version(&Connection::open(&path).unwrap()).unwrap(),
            SCHEMA_VERSION + 1
        );
        assert!(!sibling(&path, ".encrypting").exists());
    }
    #[test]
    fn migration_includes_committed_legacy_wal_pages() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("legacy-live.sqlite");
        let path = dir.path().join("legacy-crash.sqlite");
        let source = Store::open(&live).unwrap();
        source.conn.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE wal_secret(value TEXT); INSERT INTO wal_secret VALUES('synthetic-secret');").unwrap();
        // Simulate an interrupted process by copying a stable main+WAL pair
        // while the source connection remains open (no close checkpoint).
        std::fs::copy(&live, &path).unwrap();
        std::fs::copy(sibling(&live, "-wal"), sibling(&path, "-wal")).unwrap();
        let migrated = Store::open_encrypted(&path, &key()).unwrap();
        let secret: String = migrated
            .conn
            .query_row("SELECT value FROM wal_secret", [], |r| r.get(0))
            .unwrap();
        assert_eq!(secret, "synthetic-secret");
        no_plaintext(&path);
        no_plaintext(&sibling(&path, "-wal"));
    }
    #[test]
    fn encrypted_store_exports_portable_snapshot_and_reencrypts_on_import() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.sqlite");
        let archive = dir.path().join("export.sqlite");
        let mut store = Store::open_encrypted(&path, &key()).unwrap();
        store.set_combined_last_channel("synthetic-secret").unwrap();
        store.snapshot_to(&archive).unwrap();
        Store::validate_snapshot(&archive).unwrap();
        assert_eq!(
            Store::open(&archive)
                .unwrap()
                .combined_last_channel()
                .unwrap()
                .as_deref(),
            Some("synthetic-secret")
        );
        let restored =
            Store::open_encrypted(&archive, &DatabaseKey::from_bytes([0x57; 32])).unwrap();
        assert_eq!(
            restored.combined_last_channel().unwrap().as_deref(),
            Some("synthetic-secret")
        );
        no_plaintext(&archive);
    }
    #[test]
    fn encrypted_guide_stage_attaches_with_main_store_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_encrypted(dir.path().join("main.sqlite"), &key()).unwrap();
        let mut stage = crate::EpgStage::create_in_store(&store, dir.path()).unwrap();
        stage.flush().unwrap();
        store
            .attach_staging_database(stage.path(), "stage_test")
            .unwrap();
        let count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM stage_test.programmes", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        no_plaintext(stage.path());
        store
            .conn
            .execute("DETACH DATABASE stage_test", [])
            .unwrap();
    }
    #[test]
    fn encrypted_catalog_stage_publishes_through_keyed_attachment() {
        use iptv_import::{CatalogRow, CatalogSink};
        use iptv_providers::model::{M3uSourceConfig, Source, SourceConfig};

        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.sqlite");
        let mut store = Store::open_encrypted(&main, &key()).unwrap();
        let source = store
            .add_source(&Source {
                name: "Synthetic".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: iptv_providers::SecretUrl::new("https://synthetic.invalid/list.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let parsed = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1 group-title=\"Synthetic\",Synthetic channel\nhttps://synthetic.invalid/synthetic-secret.ts\n",
        );
        let channel = parsed.channels.items.into_iter().next().unwrap();
        let mut stage_path = None;
        let mut stage = crate::CatalogStage::create_in_with(dir.path(), |path| {
            stage_path = Some(path.to_owned());
            store.open_staging_connection(path)
        })
        .unwrap();
        for group in parsed.groups {
            stage.push(CatalogRow::Group(group)).unwrap();
        }
        stage.push(CatalogRow::Channel(channel.clone())).unwrap();
        stage.finish().unwrap();
        let stage_path = stage_path.unwrap();
        no_plaintext(&stage_path);
        assert!(
            Connection::open(&stage_path)
                .unwrap()
                .query_row("SELECT count(*) FROM channels", [], |r| r.get::<_, i64>(0))
                .is_err()
        );
        assert_eq!(stage.load_channels().unwrap(), vec![channel.clone()]);
        // Publication must use ATTACH with the live key, not a plaintext connection.
        assert_eq!(
            store.replace_catalog_stage(source, stage).unwrap().channels,
            1
        );
        assert!(!stage_path.exists());
        drop(store);
        let reopened = Store::open_encrypted(&main, &key()).unwrap();
        let rows = reopened
            .load_channels(source, &crate::ChannelQuery::default())
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].channel, channel);
        no_plaintext(&main);
    }
    #[test]
    fn scratch_database_uses_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_encrypted(dir.path().join("main.sqlite"), &key()).unwrap();
        let path = dir.path().join("stage.sqlite");
        let scratch = store.open_staging_connection(&path).unwrap();
        scratch
            .execute_batch(
                "CREATE TABLE secrets(value TEXT); INSERT INTO secrets VALUES('synthetic-secret');",
            )
            .unwrap();
        no_plaintext(&path);
        assert!(
            Connection::open(&path)
                .unwrap()
                .query_row("SELECT count(*) FROM secrets", [], |r| r.get::<_, i64>(0))
                .is_err()
        );
    }
}
