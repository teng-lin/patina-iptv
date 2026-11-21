//! Program titles of a stored guide, streamed for program search: every
//! day the store keeps can be searched without holding those days as a
//! guide in memory (the app holds only the last day: `Store::load_epg`).

use std::collections::{BTreeMap, HashMap};

use iptv_epg::{ChannelKey, UnixTime};
use rusqlite::params;

use crate::error::{Result, StoreError};
use crate::{SourceId, Store};

impl Store {
    /// Calls `f(position, start, stop, title)` for every stored program
    /// of `source` overlapping `[from, to)` on the playlist channels
    /// `keys` (`position` is the key's index in `keys`; channels sharing
    /// an EPG channel each hear its programs). Channel by channel, each
    /// in start order. Returns how many programs were read.
    pub fn for_each_epg_title(
        &self,
        source: SourceId,
        keys: &[ChannelKey],
        from: UnixTime,
        to: UnixTime,
        f: impl FnMut(usize, UnixTime, UnixTime, &str),
    ) -> Result<u64> {
        self.read_snapshot(|store| store.for_each_epg_title_inner(source, keys, from, to, f))
    }
    fn for_each_epg_title_inner(
        &self,
        source: SourceId,
        keys: &[ChannelKey],
        from: UnixTime,
        to: UnixTime,
        mut f: impl FnMut(usize, UnixTime, UnixTime, &str),
    ) -> Result<u64> {
        let positions: HashMap<&str, usize> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        // Store channel -> the positions bound to it (ordered: the same
        // reads every time).
        let mut channels: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        let mut bound = self.conn.prepare_cached(
            "SELECT channel_key, epg_channel_id FROM epg_bindings WHERE source_id = ?1",
        )?;
        let mut rows = bound.query([source.0])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            if let Some(&at) = positions.get(key.as_str()) {
                channels.entry(r.get(1)?).or_default().push(at);
            }
        }
        drop(rows);
        let mut titles = self.conn.prepare_cached(
            "SELECT start, stop, title FROM epg_programs
             WHERE channel_id = ?1 AND start < ?3 AND stop > ?2 ORDER BY start",
        )?;
        let mut n = 0;
        for (channel, at) in &channels {
            let mut rows = titles.query(params![channel, from, to])?;
            while let Some(r) = rows.next()? {
                let (start, stop) = (r.get(0)?, r.get(1)?);
                let title = match r.get_ref(2)?.as_str_or_null() {
                    Ok(t) => t.unwrap_or_default(),
                    Err(_) => return Err(StoreError::InvalidData("epg_programs title".into())),
                };
                for &position in at {
                    f(position, start, stop, title);
                }
                n += 1;
            }
        }
        Ok(n)
    }
}
