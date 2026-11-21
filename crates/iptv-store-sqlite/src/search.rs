//! Channel name search: an FTS5 index when the SQLite build has it (the
//! bundled one does), else `LIKE`.
//!
//! The index is derived data, not part of the numbered schema: it is
//! (re)built at open when missing or when [`FTS_VERSION`] changes, and kept
//! in step by every write that changes a channel's name. It is addressed by
//! `channels.rowid`. Every connection maintains it when it exists, whatever
//! backend that connection queries with.

use iptv_providers::ContentId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::SourceId;
use crate::error::Result;

/// Bump to rebuild every file's index (tokenizer or column change).
const FTS_VERSION: &str = "1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchBackend {
    /// Token-prefix match over names (`"new yo"` finds `New York`),
    /// diacritics- and case-insensitive.
    Fts5,
    /// Substring match per word; case-insensitive for ASCII only.
    Like,
}

/// Rows to index: the effective (displayed) name, plus the provider name
/// when a custom name hides it.
const INDEX_ROWS: &str = "SELECT c.rowid, COALESCE(o.custom_name, c.name),
        CASE WHEN o.custom_name IS NOT NULL THEN c.name END
    FROM channels c LEFT JOIN channel_overlays o
        ON o.source_id = c.source_id AND o.channel_id = c.id";

fn fts5_available(conn: &Connection) -> Result<bool> {
    let used: i64 = conn.query_row("SELECT sqlite_compileoption_used('ENABLE_FTS5')", [], |r| {
        r.get(0)
    })?;
    Ok(used == 1)
}

fn index_is_current(conn: &Connection) -> Result<bool> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'channel_fts_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = 'channel_fts'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(exists.is_some() && version.as_deref() == Some(FTS_VERSION))
}

/// Creates or rebuilds the index if needed. Returns whether it exists.
pub(crate) fn prepare(conn: &mut Connection) -> Result<bool> {
    if !fts5_available(conn)? {
        return Ok(false);
    }
    if index_is_current(conn)? {
        return Ok(true);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !index_is_current(&tx)? {
        tx.execute_batch(
            "DROP TABLE IF EXISTS channel_fts;
             CREATE VIRTUAL TABLE channel_fts USING fts5(
                 name, original, tokenize = 'unicode61 remove_diacritics 2'
             );",
        )?;
        tx.execute(
            &format!("INSERT INTO channel_fts (rowid, name, original) {INDEX_ROWS}"),
            [],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO store_meta (key, value) VALUES ('channel_fts_version', ?1)",
            [FTS_VERSION],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

/// Drops a source's rows from the index (before its channels are deleted).
pub(crate) fn forget_source(conn: &Connection, fts: bool, source: SourceId) -> Result<()> {
    if fts {
        conn.execute(
            "DELETE FROM channel_fts WHERE rowid IN (SELECT rowid FROM channels WHERE source_id = ?1)",
            [source.0],
        )?;
    }
    Ok(())
}

/// Indexes a source's current channels (after they were inserted).
pub(crate) fn index_source(conn: &Connection, fts: bool, source: SourceId) -> Result<()> {
    if fts {
        conn.execute(
            &format!(
                "INSERT INTO channel_fts (rowid, name, original) {INDEX_ROWS} WHERE c.source_id = ?1"
            ),
            [source.0],
        )?;
    }
    Ok(())
}

/// Drops the index rows of `source`'s channels with rowids in
/// `(after, last]` (a slice of [`forget_source`]).
pub(crate) fn forget_channels(
    conn: &Connection,
    source: SourceId,
    after: i64,
    last: i64,
) -> Result<usize> {
    Ok(conn
        .prepare_cached(
            "DELETE FROM channel_fts WHERE rowid IN (SELECT rowid FROM channels
             WHERE +source_id = ?1 AND rowid > ?2 AND rowid <= ?3)",
        )?
        .execute(params![source.0, after, last])?)
}

/// Indexes `source`'s channels with rowids in `(after, last]` (a slice of
/// [`index_source`]).
pub(crate) fn index_channels(
    conn: &Connection,
    source: SourceId,
    after: i64,
    last: i64,
) -> Result<usize> {
    Ok(conn
        .prepare_cached(&format!(
            "INSERT INTO channel_fts (rowid, name, original) {INDEX_ROWS}
             WHERE +c.source_id = ?1 AND c.rowid > ?2 AND c.rowid <= ?3"
        ))?
        .execute(params![source.0, after, last])?)
}

/// Re-indexes one channel after its custom name changed. A no-op for an
/// id that is not in the catalog.
pub(crate) fn reindex_channel(
    conn: &Connection,
    fts: bool,
    source: SourceId,
    id: &ContentId,
) -> Result<()> {
    if fts {
        conn.execute(
            "DELETE FROM channel_fts WHERE rowid = (SELECT rowid FROM channels WHERE source_id = ?1 AND id = ?2)",
            params![source.0, id.as_str()],
        )?;
        conn.execute(
            &format!(
                "INSERT INTO channel_fts (rowid, name, original) {INDEX_ROWS} WHERE c.source_id = ?1 AND c.id = ?2"
            ),
            params![source.0, id.as_str()],
        )?;
    }
    Ok(())
}

/// Words of a query: runs of letters and digits.
pub(crate) fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// An FTS5 query matching rows that contain every word as a token prefix.
pub(crate) fn fts_query(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `LIKE` pattern for one word, with `\` as the escape character.
pub(crate) fn like_pattern(word: &str) -> String {
    let mut out = String::with_capacity(word.len() + 2);
    out.push('%');
    for c in word.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_building() {
        assert_eq!(words("  News-24 (HD) "), ["News", "24", "HD"]);
        assert_eq!(fts_query(&["new", "yo"]), r#""new"* "yo"*"#);
        assert_eq!(like_pattern("50%_a"), r"%50\%\_a%");
    }

    #[test]
    fn bundled_sqlite_has_fts5() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(fts5_available(&conn).unwrap());
    }
}
