//! Private encrypted raw probe snapshots and bounded channel indexes.
//! Program schedules are parsed on demand from these exact retained bytes.
use crate::{DatabaseKey, Result, StoreError};
use iptv_epg::XmltvChannel;
use rusqlite::{Connection, OpenFlags, params};
use std::path::Path;

/// Includes raw chunks, channel indexes and SQLite page overhead.
pub const INSPECTION_STAGE_BYTES: u64 = 128 << 20;

fn page_bytes(conn: &Connection) -> Result<u64> {
    let value: rusqlite::types::Value =
        conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let bytes = match value {
        rusqlite::types::Value::Integer(value) => u64::try_from(value).ok(),
        // SQLCipher exposes its cipher_page_size alias as text.
        rusqlite::types::Value::Text(value) => value.parse().ok(),
        _ => None,
    };
    bytes
        .filter(|bytes| *bytes >= 512 && *bytes <= 65536)
        .ok_or_else(|| StoreError::InvalidData("probe page size".into()))
}

pub struct InspectionStage {
    conn: Connection,
    next: i64,
    index_bytes: u64,
}
impl InspectionStage {
    pub fn create(path: &Path, key: &DatabaseKey) -> Result<Self> {
        iptv_sqlcipher_init::initialize()?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(
            options
                .open(path)
                .map_err(|_| StoreError::Unavailable("cannot create probe snapshot"))?,
        );
        let conn = Connection::open(path)?;
        key.apply(&conn)?;
        let page_size = page_bytes(&conn)?;
        conn.pragma_update(
            None,
            "max_page_count",
            (INSPECTION_STAGE_BYTES / page_size) as i64,
        )?;
        conn.execute_batch(
            "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
            PRAGMA cache_size=-2048; BEGIN;
            CREATE TABLE chunks(ordinal INTEGER PRIMARY KEY,bytes BLOB NOT NULL);
            CREATE TABLE channels(id TEXT PRIMARY KEY,names TEXT NOT NULL);",
        )?;
        Ok(Self {
            conn,
            next: 0,
            index_bytes: 0,
        })
    }
    pub fn open(path: &Path, key: &DatabaseKey) -> Result<Self> {
        iptv_sqlcipher_init::initialize()?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        key.apply(&conn)?;
        Ok(Self {
            conn,
            next: 0,
            index_bytes: 0,
        })
    }
    pub fn chunk(&mut self, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(64 * 1024) {
            self.conn.execute(
                "INSERT INTO chunks VALUES(?1,?2)",
                params![self.next, chunk],
            )?;
            self.next += 1;
        }
        Ok(())
    }
    pub fn channel(&mut self, channel: &XmltvChannel) -> Result<()> {
        let names = serde_json::to_string(&channel.display_names)?;
        self.index_bytes = self
            .index_bytes
            .saturating_add((channel.id.len() + names.len()) as u64);
        if self.index_bytes > 64 << 20
            || channel.id.len() > 4096
            || channel.display_names.len() > 16
            || channel.display_names.iter().any(|n| n.len() > 4096)
        {
            return Err(StoreError::Unavailable(
                "probe channel index limit exceeded",
            ));
        }
        self.conn.execute(
            "INSERT INTO channels VALUES(?1,?2)",
            params![channel.id, names],
        )?;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        self.conn.execute_batch("COMMIT; PRAGMA optimize;")?;
        Ok(())
    }
    pub fn bounded<T>(
        &mut self,
        cancel: iptv_import::CancelToken,
        deadline: std::time::Instant,
        operation: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        self.conn.progress_handler(
            1000,
            Some(move || cancel.is_canceled() || std::time::Instant::now() >= deadline),
        )?;
        let result = operation(self);
        self.conn.progress_handler(0, None::<fn() -> bool>)?;
        result
    }
    pub fn channels(&self, after: &str, text: &str, limit: u32) -> Result<Vec<XmltvChannel>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT id,names FROM channels WHERE id>?1 AND
             (id LIKE ?2 ESCAPE '\\' OR names LIKE ?2 ESCAPE '\\') ORDER BY id LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![after, crate::search::like_pattern(text), limit],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        rows.map(|row| {
            let (id, names) = row?;
            Ok(XmltvChannel {
                id,
                display_names: serde_json::from_str(&names)?,
                icon: None,
            })
        })
        .collect()
    }
    pub fn has_channel(&self, id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM channels WHERE id=?1)",
            [id],
            |row| row.get(0),
        )?)
    }
    pub fn for_each_chunk(&self, mut read: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT bytes FROM chunks ORDER BY ordinal")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let bytes: Vec<u8> = row.get(0)?;
            if bytes.is_empty() || bytes.len() > 64 * 1024 {
                return Err(StoreError::InvalidData("probe chunk".into()));
            }
            read(&bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::initialization_support;
    #[test]
    fn probe_stage_initialization_precedes_its_first_encrypted_connection() {
        const CHILD: &str = "IPTV_INSPECTION_FIRST_CIPHER_CHILD";
        if std::env::var_os(CHILD).is_none() {
            for _ in 0..8 {
                let output = initialization_support::child(
                    "inspection_stage::tests::probe_stage_initialization_precedes_its_first_encrypted_connection",
                    CHILD,
                    std::time::Duration::from_secs(20),
                )
                .expect("probe initialization child could not complete");
                assert!(
                    output.status.success(),
                    "fresh probe opener failed:\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let begin = std::sync::Barrier::new(32);
        std::thread::scope(|threads| {
            for index in 0..32 {
                let directory = directory.path();
                let begin = &begin;
                threads.spawn(move || {
                    begin.wait();
                    if index % 4 == 0 {
                        // A foreign rusqlite consumer bypasses the owned adapter.
                        let connection=Connection::open_in_memory().unwrap();
                        connection.execute_batch("SELECT 1").unwrap();
                        return;
                    }
                    let path = directory.join(format!("generated-probe-{index}.sqlite"));
                    let key = DatabaseKey::from_bytes([0x37; 32]);
                    // No live Store has opened in this process. A foreign first
                    // opener must not leave this connection without cipher exports.
                    let mut stage = InspectionStage::create(&path, &key).unwrap();
                    let export: bool = stage.conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_function_list WHERE name='sqlcipher_export')",
                        [],|row|row.get(0)).unwrap();
                    assert!(export,"first probe connection missed native cipher initialization");
                    stage.chunk(b"generated first-opener snapshot").unwrap();
                    stage.finish().unwrap();
                    assert!(!std::fs::read(&path).unwrap().starts_with(b"SQLite format 3\0"));
                    let reopened = InspectionStage::open(&path, &key).unwrap();
                    let mut actual = Vec::new();
                    reopened.for_each_chunk(|bytes| {actual.extend_from_slice(bytes);Ok(())}).unwrap();
                    assert_eq!(actual,b"generated first-opener snapshot");
                });
            }
        });
    }
    #[test]
    fn encrypted_stage_has_a_physical_page_ceiling_and_round_trips_raw_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("probe.sqlite");
        let key = DatabaseKey::from_bytes([0x37; 32]);
        let mut stage = InspectionStage::create(&path, &key).unwrap();
        let pages: i64 = stage
            .conn
            .pragma_query_value(None, "max_page_count", |r| r.get(0))
            .unwrap();
        let size = page_bytes(&stage.conn).unwrap();
        assert!((pages as u64) * size <= INSPECTION_STAGE_BYTES);
        stage.chunk(b"generated snapshot").unwrap();
        stage.finish().unwrap();
        let stage = InspectionStage::open(&path, &key).unwrap();
        let mut actual = Vec::new();
        stage
            .for_each_chunk(|bytes| {
                actual.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap();
        assert_eq!(actual, b"generated snapshot");
    }
}
