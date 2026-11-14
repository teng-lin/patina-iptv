//! App-owned documents stored as bounded chunks in an encrypted database.
use crate::{Result, Store, StoreError};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    io::{self, Read},
    path::Path,
};
const CHUNK: usize = 64 * 1024;

/// A bounded reader over an app-owned encrypted document.
pub struct PrivateFileReader {
    conn: Connection,
    length: u64,
    next: i64,
    consumed: u64,
    chunk: io::Cursor<Vec<u8>>,
}
impl PrivateFileReader {
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
}
impl Read for PrivateFileReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.chunk.position() == self.chunk.get_ref().len() as u64 {
            let chunk: Option<Vec<u8>> = self
                .conn
                .query_row(
                    "SELECT CASE WHEN typeof(bytes)='blob' AND length(bytes)<=65536 THEN bytes END FROM private_chunks WHERE ordinal=?1",
                    [self.next],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| io::Error::other("private document cannot be read"))?;
            let Some(chunk) = chunk else {
                return if self.consumed == self.length {
                    Ok(0)
                } else {
                    Err(io::Error::other("private document is incomplete"))
                };
            };
            if chunk.len() > CHUNK || chunk.is_empty() {
                return Err(io::Error::other("private document chunk is invalid"));
            }
            self.consumed = self
                .consumed
                .checked_add(chunk.len() as u64)
                .filter(|count| *count <= self.length)
                .ok_or_else(|| io::Error::other("private document length is invalid"))?;
            self.next += 1;
            self.chunk = io::Cursor::new(chunk);
        }
        self.chunk.read(output)
    }
}
impl Store {
    pub fn private_documents_sealed(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM store_meta WHERE key='private_documents_sealed'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .as_deref()
            == Some("1"))
    }
    /// Durably reset the marker before converting a portable pending restore.
    pub fn mark_private_documents_unsealed(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM store_meta WHERE key='private_documents_sealed'",
            [],
        )?;
        Ok(())
    }
    pub fn mark_private_documents_sealed(&self) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO store_meta(key,value) VALUES('private_documents_sealed','1')",
            [],
        )?;
        Ok(())
    }
    /// Writes a new encrypted document; callers atomically replace old copies
    /// only after this succeeds. No plaintext intermediate is created.
    pub fn write_private_file(&self, path: &Path, input: &mut impl Read) -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(
            options
                .open(path)
                .map_err(|_| StoreError::Unavailable("cannot create private document"))?,
        );
        let result = (|| {
            let mut conn = self.open_staging_connection(path)?;
            conn.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE private_chunks(ordinal INTEGER PRIMARY KEY, bytes BLOB NOT NULL); CREATE TABLE private_metadata(length INTEGER NOT NULL);")?;
            let tx = conn.transaction()?;
            let mut buffer = vec![0; CHUNK];
            let mut ordinal = 0i64;
            let mut length = 0u64;
            loop {
                let count = input
                    .read(&mut buffer)
                    .map_err(|_| StoreError::Unavailable("cannot read private document input"))?;
                if count == 0 {
                    break;
                }
                tx.execute(
                    "INSERT INTO private_chunks VALUES(?1,?2)",
                    params![ordinal, &buffer[..count]],
                )?;
                ordinal += 1;
                length = length
                    .checked_add(count as u64)
                    .ok_or(StoreError::Unavailable("private document too large"))?;
            }
            tx.execute(
                "INSERT INTO private_metadata VALUES(?1)",
                [i64::try_from(length)
                    .map_err(|_| StoreError::Unavailable("private document too large"))?],
            )?;
            tx.commit()?;
            drop(conn);
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .and_then(|file| file.sync_all())
                .map_err(|_| StoreError::Unavailable("cannot finish private document"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
    pub fn read_private_file(&self, path: &Path) -> Result<PrivateFileReader> {
        iptv_sqlcipher_init::initialize()?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        if let Some(key) = &self.key {
            key.apply(&conn)?;
        }
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let length: i64 =
            conn.query_row("SELECT length FROM private_metadata", [], |row| row.get(0))?;
        let length = u64::try_from(length)
            .map_err(|_| StoreError::Unavailable("private document length is invalid"))?;
        Ok(PrivateFileReader {
            conn,
            length,
            next: 0,
            consumed: 0,
            chunk: io::Cursor::new(Vec::new()),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_is_encrypted_and_reads_in_bounded_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let key = crate::DatabaseKey::from_bytes([0x51; 32]);
        let store = Store::open_encrypted(dir.path().join("main"), &key).unwrap();
        let path = dir.path().join("playlist");
        let bytes = b"#EXTM3U\nhttp://synthetic.test/user/password\n".repeat(6000);
        store
            .write_private_file(&path, &mut bytes.as_slice())
            .unwrap();
        assert!(
            !std::fs::read(&path)
                .unwrap()
                .windows(8)
                .any(|b| b == b"password")
        );
        let mut reader = store.read_private_file(&path).unwrap();
        assert_eq!(reader.len(), bytes.len() as u64);
        let mut restored = Vec::new();
        reader.read_to_end(&mut restored).unwrap();
        assert_eq!(restored, bytes);
        let wrong = Store::open_encrypted(
            dir.path().join("wrong"),
            &crate::DatabaseKey::from_bytes([0x52; 32]),
        )
        .unwrap();
        assert!(wrong.read_private_file(&path).is_err());
    }
}
