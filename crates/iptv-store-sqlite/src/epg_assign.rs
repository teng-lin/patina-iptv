//! How each guide feed's last download went, the channels the stored feeds
//! list, and choosing a channel's guide by hand (schema v6).
//!
//! A manual choice is a channel overlay (`EpgOverride`, kept by channel
//! key across refreshes) that EPG matching applies at the next download.
//! [`Store::rebind_epg`] applies it to the stored guide at once when that
//! is possible without a download: turning a channel's guide off, or
//! pointing it at an EPG channel whose programmes are stored.

use iptv_epg::{ChannelKey, MatchKind};
use iptv_providers::ContentId;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::epg::{kind_from, kind_slug, to_u64};
use crate::error::Result;
use crate::overlay::EpgOverride;
use crate::sources::{ImportErrorKind, ensure_source};
use crate::{SourceId, Store};

pub use iptv_library::{FeedResult, GuideChannel};

impl Store {
    /// Records how each configured feed's download went (replacing what
    /// was recorded for `source`). Call after [`Store::replace_epg`] or
    /// [`Store::record_epg_failure`].
    pub fn record_epg_feed_results(
        &mut self,
        source: SourceId,
        results: &[(iptv_epg::FeedId, Option<ImportErrorKind>)],
    ) -> Result<()> {
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_source(&tx, source)?;
        tx.execute(
            "INSERT INTO epg_guides (source_id, attempted_at) VALUES (?1, ?2)
             ON CONFLICT (source_id) DO NOTHING",
            params![source.0, now],
        )?;
        tx.execute(
            "DELETE FROM epg_feed_results WHERE source_id = ?1",
            [source.0],
        )?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO epg_feed_results (source_id, feed_id, attempted_at,
                     error_kind)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (feed, error) in results {
                insert.execute(params![source.0, feed.0, now, error.map(|k| k.slug())])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// How each feed's last download went, with what was stored from it.
    pub fn epg_feed_results(&self, source: SourceId) -> Result<Vec<FeedResult>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.feed_id, r.attempted_at, r.error_kind, COALESCE(f.channel_count, 0),
                 COALESCE(f.programme_count, 0),
                 (SELECT COUNT(*) FROM epg_feed_channels c
                  WHERE c.source_id = r.source_id AND c.feed_id = r.feed_id)
             FROM epg_feed_results r
             LEFT JOIN epg_feeds f ON f.source_id = r.source_id AND f.feed_id = r.feed_id
             WHERE r.source_id = ?1 ORDER BY r.feed_id",
        )?;
        let rows = stmt.query_map([source.0], |r| {
            Ok(FeedResult {
                feed: iptv_epg::FeedId(r.get(0)?),
                attempted_at: r.get(1)?,
                error: r
                    .get::<_, Option<String>>(2)?
                    .map(|s| ImportErrorKind::from_slug(&s)),
                channel_count: to_u64(r.get(3)?),
                programme_count: to_u64(r.get(4)?),
                listed_count: to_u64(r.get(5)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every channel the stored feeds of `source` list (a guide stored
    /// before schema v6 lists only its bound ones until the next download),
    /// by feed, then name.
    pub fn guide_channels(&self, source: SourceId) -> Result<Vec<GuideChannel>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT feed_id, epg_channel, MAX(name), MAX(assigned) FROM (
                 SELECT feed_id, epg_channel, name, 0 AS assigned
                 FROM epg_feed_channels WHERE source_id = ?1
                 UNION ALL
                 SELECT c.feed_id, c.epg_channel, NULL,
                     EXISTS (SELECT 1 FROM epg_bindings b WHERE b.epg_channel_id = c.id)
                 FROM epg_channels c WHERE c.source_id = ?1)
             GROUP BY feed_id, epg_channel
             ORDER BY feed_id, lower(COALESCE(MAX(name), epg_channel)), epg_channel",
        )?;
        let rows = stmt.query_map([source.0], |r| {
            Ok(GuideChannel {
                feed: iptv_epg::FeedId(r.get(0)?),
                epg_channel: r.get(1)?,
                name: r.get(2)?,
                assigned: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The EPG channel playlist channel `key` takes its stored guide from,
    /// and how it was matched.
    pub fn epg_binding(
        &self,
        source: SourceId,
        key: &ChannelKey,
    ) -> Result<Option<(iptv_epg::FeedId, String, MatchKind)>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT c.feed_id, c.epg_channel, b.match_kind FROM epg_bindings b
                 JOIN epg_channels c ON c.id = b.epg_channel_id
                 WHERE b.source_id = ?1 AND b.channel_key = ?2",
            )?
            .query_row(params![source.0, key.as_str()], |r| {
                Ok((
                    iptv_epg::FeedId(r.get(0)?),
                    r.get::<_, String>(1)?,
                    kind_from(&r.get::<_, String>(2)?),
                ))
            })
            .optional()?)
    }

    /// The channels of `source` whose guide the user chose by hand.
    pub fn epg_overrides(&self, source: SourceId) -> Result<Vec<(ContentId, EpgOverride)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, epg_mode, epg_source, epg_channel FROM channel_overlays
             WHERE source_id = ?1 AND epg_mode IS NOT NULL ORDER BY channel_id",
        )?;
        let rows = stmt.query_map([source.0], |r| {
            Ok((
                r.get::<_, String>(0)?,
                EpgOverride::from_columns(r.get(1)?, r.get(2)?, r.get(3)?),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, epg) = row?;
            if let Some(epg) = epg {
                out.push((ContentId::from_stored(id), epg));
            }
        }
        Ok(out)
    }

    /// Points playlist channel `key` of the stored guide at EPG channel
    /// `to` (`None`: no guide), without a download. False (and nothing
    /// changed) when there is no stored guide or `to`'s programmes are not
    /// stored. `match_fingerprint`: the fingerprint of the channels and
    /// overrides the bindings now follow.
    pub fn rebind_epg(
        &mut self,
        source: SourceId,
        key: &ChannelKey,
        to: Option<(iptv_epg::FeedId, &str)>,
        match_fingerprint: u64,
    ) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: Option<Option<i64>> = tx
            .query_row(
                "SELECT replaced_at FROM epg_guides WHERE source_id = ?1",
                [source.0],
                |r| r.get(0),
            )
            .optional()?;
        if !matches!(stored, Some(Some(_))) {
            return Ok(false);
        }
        match to {
            None => {
                tx.execute(
                    "DELETE FROM epg_bindings WHERE source_id = ?1 AND channel_key = ?2",
                    params![source.0, key.as_str()],
                )?;
            }
            Some((feed, epg_channel)) => {
                let channel: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM epg_channels
                         WHERE source_id = ?1 AND feed_id = ?2 AND epg_channel = ?3",
                        params![source.0, feed.0, epg_channel],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(channel) = channel else {
                    return Ok(false);
                };
                tx.execute(
                    "INSERT OR REPLACE INTO epg_bindings (source_id, channel_key, epg_channel_id,
                         match_kind)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        source.0,
                        key.as_str(),
                        channel,
                        kind_slug(MatchKind::Override)
                    ],
                )?;
            }
        }
        tx.execute(
            "UPDATE epg_guides SET match_fingerprint = ?2,
                 bound_channels = (SELECT COUNT(*) FROM epg_bindings WHERE source_id = ?1)
             WHERE source_id = ?1",
            params![source.0, match_fingerprint as i64],
        )?;
        tx.execute(
            "DELETE FROM store_meta WHERE key=?1",
            [crate::epg::matching_projection_key(source)],
        )?;
        tx.commit()?;
        Ok(true)
    }
}
