//! The guide (XMLTV) sources of a playlist source as the user arranged
//! them (schema v6): the provider's own guides and any the user added, each
//! with a switch, a place in the priority order and a time shift.
//!
//! The list is replaced as a whole ([`Store::set_guide_sources`]); the app
//! decides what changes (adding, removing, moving, the provider's guides
//! after a playlist import) and turns the list into the configs of a guide
//! download.

use iptv_providers::SecretUrl;
use rusqlite::{TransactionBehavior, params};

use crate::error::Result;
use crate::sources::ensure_source;
use crate::{SourceId, Store};

pub use iptv_library::GuideSource;

pub(crate) fn set_guide_sources_in(
    conn: &rusqlite::Connection,
    source: SourceId,
    sources: &[GuideSource],
    now: i64,
) -> Result<()> {
    ensure_source(conn, source)?;
    // Preserve equal configured rows and their incarnations on a physical
    // reimport. Delete only removed feeds; triggers record actual changes.
    let retained: std::collections::HashSet<_> = sources.iter().map(|g| g.feed.0).collect();
    if retained.len() != sources.len() {
        return Err(crate::StoreError::InvalidData(
            "duplicate guide feed ID".into(),
        ));
    }
    let existing = {
        let mut statement =
            conn.prepare_cached("SELECT feed_id FROM guide_sources WHERE source_id = ?1")?;
        statement
            .query_map([source.0], |row| row.get::<_, u32>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for feed in existing {
        if !retained.contains(&feed) {
            conn.execute(
                "DELETE FROM guide_sources WHERE source_id = ?1 AND feed_id = ?2",
                rusqlite::params![source.0, feed],
            )?;
        }
    }
    // Release changed provider slots before upserting: two retained feed IDs
    // may swap provider indices in one otherwise valid replacement.
    for feed in sources {
        conn.execute(
            "UPDATE guide_sources SET provider_index = NULL
             WHERE source_id = ?1 AND feed_id = ?2 AND provider_index IS NOT ?3",
            rusqlite::params![source.0, feed.feed.0, feed.provider_index],
        )?;
    }
    {
        let mut insert = conn.prepare_cached(
            "INSERT INTO guide_sources (source_id, feed_id, provider_index, url, name,
                     enabled, position, shift_minutes, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (source_id, feed_id) DO UPDATE SET
                     provider_index = excluded.provider_index, url = excluded.url,
                     name = excluded.name, enabled = excluded.enabled,
                     position = excluded.position, shift_minutes = excluded.shift_minutes,
                     updated_at = excluded.updated_at",
        )?;
        for (position, g) in sources.iter().enumerate() {
            insert.execute(params![
                source.0,
                g.feed.0,
                g.provider_index,
                g.url.expose(),
                g.name,
                g.enabled,
                position as i64,
                g.shift_minutes,
                now
            ])?;
        }
    }
    Ok(())
}

impl Store {
    /// `source`'s guide sources in the user's order (first wins). Empty for
    /// a source whose guides were never arranged (a new one).
    pub fn guide_sources(&self, source: SourceId) -> Result<Vec<GuideSource>> {
        ensure_source(&self.conn, source)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT feed_id, provider_index, url, name, enabled, shift_minutes
             FROM guide_sources WHERE source_id = ?1 ORDER BY position, feed_id",
        )?;
        let rows = stmt.query_map([source.0], |r| {
            Ok(GuideSource {
                feed: iptv_epg::FeedId(r.get(0)?),
                provider_index: r.get(1)?,
                url: SecretUrl::new(r.get::<_, String>(2)?),
                name: r.get(3)?,
                enabled: r.get(4)?,
                shift_minutes: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces `source`'s guide sources with `sources`, in that order.
    pub fn set_guide_sources(&mut self, source: SourceId, sources: &[GuideSource]) -> Result<()> {
        let now = self.now();
        // Immediate: it reads before it writes (see `update_source`).
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        set_guide_sources_in(&tx, source, sources, now)?;
        tx.commit()?;
        Ok(())
    }
}
