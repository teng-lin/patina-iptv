//! Private encrypted raw probe snapshots and bounded channel indexes.
//! Programme schedules are parsed on demand from these exact retained bytes.
use crate::{DatabaseKey, Result, StoreError};
use iptv_epg::XmltvChannel;
use rusqlite::{Connection, OpenFlags, params};
use std::path::Path;

pub struct InspectionStage {
    conn: Connection,
    next: i64,
    index_bytes: u64,
}
impl InspectionStage {
    pub fn create(path: &Path, key: &DatabaseKey) -> Result<Self> {
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
            Some(move || cancel.is_cancelled() || std::time::Instant::now() >= deadline),
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
