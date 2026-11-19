//! How far back a stored guide reaches: the guide can be browsed back to
//! its earliest programme and no further (older days were never stored or
//! were pruned, `Store::prune_epg`).

use std::collections::HashMap;

use iptv_epg::{ChannelKey, UnixTime};

use crate::error::Result;
use crate::{SourceId, Store};

impl Store {
    /// Per-channel first listing that overlaps or follows the retention cutoff.
    pub fn epg_listing_starts(
        &self,
        source: SourceId,
        cutoff: UnixTime,
    ) -> Result<HashMap<ChannelKey, UnixTime>> {
        // Use the same ordered seek as a guide window. The retained first
        // programme may cross the cutoff, so skip older rows by start before
        // checking its end.
        let seek = crate::epg_window::seek("b.epg_channel_id");
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT b.channel_key, (
                 SELECT p.start FROM epg_programmes p
                 WHERE p.channel_id = b.epg_channel_id AND p.stop > ?2 AND {seek}
                 ORDER BY p.start LIMIT 1
             ) FROM epg_bindings b WHERE b.source_id = ?1",
        ))?;
        let mut starts = HashMap::new();
        let mut rows = stmt.query(rusqlite::params![source.0, cutoff])?;
        while let Some(row) = rows.next()? {
            if let Some(start) = row.get::<_, Option<UnixTime>>(1)? {
                starts.insert(ChannelKey::new(row.get::<_, String>(0)?), start);
            }
        }
        Ok(starts)
    }

    /// When the earliest stored programme of `source`'s bound channels
    /// starts; `None` when none is stored. One index seek per bound EPG
    /// channel (the programmes' key is `(channel_id, start)`).
    pub fn epg_first_start(&self, source: SourceId) -> Result<Option<UnixTime>> {
        let first = self
            .conn
            .prepare_cached(
                "SELECT MIN((SELECT p.start FROM epg_programmes p
                             WHERE p.channel_id = b.epg_channel_id
                             ORDER BY p.start LIMIT 1))
                 FROM epg_bindings b WHERE b.source_id = ?1",
            )?
            .query_row([source.0], |r| r.get::<_, Option<i64>>(0))?;
        Ok(first)
    }
}
