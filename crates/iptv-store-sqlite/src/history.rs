//! Watch history: one row per play, keyed by stable id, kept across
//! refreshes and pruned to the newest [`HISTORY_LIMIT`] rows per source
//! (amortized: a source may briefly hold up to [`PRUNE_SLACK`] more).

use iptv_providers::ContentId;
use iptv_providers::model::ContentKind;
use rusqlite::params;

use crate::codec::{content_kind_from, narrow};
use crate::error::Result;
use crate::sources::ensure_source;
use crate::{SourceId, Store};

/// Rows kept per source; older plays are dropped on append.
pub const HISTORY_LIMIT: u32 = 2000;

/// Extra rows tolerated before a prune runs.
pub const PRUNE_SLACK: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub source_id: SourceId,
    pub item_id: ContentId,
    pub kind: ContentKind,
    /// Unix seconds.
    pub watched_at: i64,
    /// Resume position for VOD/catch-up.
    pub position_secs: Option<u32>,
}

impl Store {
    /// Appends a play. Callers pass the time (the player knows when it
    /// started).
    pub fn record_watch(
        &mut self,
        source: SourceId,
        item: &ContentId,
        kind: ContentKind,
        watched_at: i64,
        position_secs: Option<u32>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        ensure_source(&tx, source)?;
        tx.execute(
            "INSERT INTO watch_history (source_id, item_id, kind, watched_at, position_secs)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                source.0,
                item.as_str(),
                kind.slug(),
                watched_at,
                position_secs
            ],
        )?;
        let rows: i64 = tx.query_row(
            "SELECT COUNT(*) FROM watch_history WHERE source_id = ?1",
            [source.0],
            |r| r.get(0),
        )?;
        if rows > i64::from(HISTORY_LIMIT + PRUNE_SLACK) {
            tx.execute(
                "DELETE FROM watch_history WHERE id IN (
                     SELECT id FROM watch_history WHERE source_id = ?1
                     ORDER BY watched_at DESC, id DESC LIMIT -1 OFFSET ?2)",
                params![source.0, HISTORY_LIMIT],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Adds observed live playback time for a channel.
    pub fn add_channel_watch_time(
        &mut self,
        source: SourceId,
        channel: &ContentId,
        elapsed_ms: u64,
    ) -> Result<()> {
        if elapsed_ms == 0 {
            return Ok(());
        }
        ensure_source(&self.conn, source)?;
        self.conn.execute(
            "INSERT INTO channel_watch_time(source_id, channel_id, watched_ms)
             VALUES (?1, ?2, ?3) ON CONFLICT(source_id, channel_id) DO UPDATE
             SET watched_ms = MIN(watched_ms + excluded.watched_ms, 9223372036854775807)",
            params![
                source.0,
                channel.as_str(),
                i64::try_from(elapsed_ms).unwrap_or(i64::MAX)
            ],
        )?;
        Ok(())
    }

    /// Most recently watched items, latest first, each item once (its
    /// latest play). Filters by source and kind when given.
    pub fn recent(
        &self,
        source: Option<SourceId>,
        kind: Option<ContentKind>,
        limit: u32,
    ) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT h.source_id, h.item_id, h.kind, h.watched_at, h.position_secs
             FROM watch_history h
             WHERE (?1 IS NULL OR h.source_id = ?1) AND (?2 IS NULL OR h.kind = ?2)
               AND h.id = (SELECT h2.id FROM watch_history h2
                           WHERE h2.source_id = h.source_id AND h2.item_id = h.item_id
                           ORDER BY h2.watched_at DESC, h2.id DESC LIMIT 1)
             ORDER BY h.watched_at DESC, h.id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![source.map(|s| s.0), kind.map(ContentKind::slug), limit],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (source_id, item, kind, watched_at, position) = row?;
            Ok(HistoryEntry {
                source_id: SourceId(source_id),
                item_id: ContentId::from_stored(item),
                kind: content_kind_from(&kind)?,
                watched_at,
                position_secs: narrow(position, "watch_history.position_secs")?,
            })
        })
        .collect()
    }

    /// Clears one source's history, or everyone's.
    pub fn clear_history(&mut self, source: Option<SourceId>) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let source = [source.map(|s| s.0)];
        let deleted = tx.execute(
            "DELETE FROM watch_history WHERE ?1 IS NULL OR source_id = ?1",
            source,
        )?;
        tx.execute(
            "DELETE FROM channel_watch_time WHERE ?1 IS NULL OR source_id = ?1",
            source,
        )?;
        tx.commit()?;
        Ok(deleted)
    }
}
