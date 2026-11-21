//! Stored guide data (schema v4): a playlist source's whole guide replaced
//! per refresh from a download staged on disk ([`EpgStage`]), read back as
//! a time window into an [`iptv_epg::GuideStore`] without descriptions, and
//! descriptions read on demand.
//!
//! Only EPG channels some playlist channel is bound to are stored (a feed
//! often lists far more channels than the playlist has); playlist channels
//! sharing one EPG channel share its rows. Feed rows keep the settings the
//! feed was fetched with, so a background refresh needs no playlist
//! download, and [`StoredGuide::refresh_state`] feeds the refresh policy
//! (`iptv_import::decide`).
//!
//! # Wiring the app (`data.rs`)
//!
//! `channels` / `overrides` are the ones EPG matching uses (the catalog's
//! channels, or `load_channels` rows when the playlist is not downloaded at
//! start); the fingerprint must be computed from exactly what is passed to
//! `import_epg`. `settings`/`at`: `iptv_settings::Settings`, `Target::Global`.
//!
//! At start, on the import thread (before any network):
//!
//! ```ignore
//! use iptv_import::{Decision, RefreshPolicy, matching_fingerprint, plan_startup, store_options};
//! let stored = store.epg_guide(source)?;                     // Option<StoredGuide>
//! let mut configs = match &catalog {                         // the playlist's feeds now
//!     Some(c) => c.epg_sources(1)?,
//!     None => stored.as_ref().map(StoredGuide::configs).unwrap_or_default(),
//! };
//! let opts = store_options(&settings, &at);                  // descriptions, keep_past
//! configs.iter_mut().for_each(|c| c.store = opts);
//! let fp = matching_fingerprint(&channels, &overrides);
//! let policy = RefreshPolicy::from_settings(&settings, &at);
//! let state = stored.as_ref().map(StoredGuide::refresh_state);
//! let plan = plan_startup(&policy, &configs, fp, state.as_ref(), now);
//! let keep = opts.past_days_to_keep.unwrap_or(2);
//! if plan.use_stored {
//!     let guide = store.load_epg(source, &EpgWindow::from(now, keep))?; // no descriptions
//!     // -> UI thread (fast path, no download):
//!     //    let reader = Store::open(path)?;   // a reader connection for the UI thread
//!     //    let schedule = EpgSchedule::from_catalog(&groups, &channels, guide, now)
//!     //        .with_description_lookup(move |key, start| {
//!     //            reader.epg_description(source, key, start).ok().flatten()
//!     //        });
//!     //    set_schedule(&ui, Rc::new(schedule));
//! }
//! match plan.decision {
//!     Decision::Refresh(_) => refresh(),                     // below, still off the UI thread
//!     Decision::Discard => { store.delete_epg(source)?; }
//!     Decision::Keep { next_check: Some(at) } | Decision::Backoff { retry_at: at } => {
//!         // timer: at `at`, decide(.., Trigger::Timer, ..) again
//!     }
//!     Decision::Keep { next_check: None } => {}              // only on request
//! }
//! ```
//!
//! Background refresh (writer store), then swap. The download streams
//! into a stage file next to the store (memory stays bounded whatever the
//! guide's size, and the store is not locked meanwhile); the stored guide
//! changes only in `replace_epg`:
//!
//! ```ignore
//! let mut stage = EpgStage::create_in(data_dir)?;            // deleted on drop
//! match import_epg(&net, &configs, &channels, &overrides, now, &mut stage, &mut progress, &cancel).await {
//!     Ok(epg) => {
//!         let feeds = EpgFeedMeta::from_import(&epg, &configs, now);
//!         store.replace_epg(source, &mut stage, &epg, &feeds, fp)?;
//!         if let Some(e) = epg.sources.iter().find_map(|o| o.result.as_ref().err()) {
//!             store.record_epg_failure(source, e.kind().into())?; // partial: back off
//!         }
//!         drop(stage);                                       // deletes the stage file
//!         let guide = store.load_epg(source, &EpgWindow::from(now, keep))?;
//!         // -> UI thread: build the EpgSchedule as above and set_schedule (the swap)
//!     }
//!     Err(e) => store.record_epg_failure(source, e.kind().into())?, // stored guide kept
//! }
//! store.prune_epg(now - i64::from(keep) * iptv_epg::DAY)?;
//! ```
//!
//! At start, before any import: `EpgStage::remove_stale(data_dir)`.
//!
//! After a playlist refresh, ask again with `Trigger::PlaylistChanged`
//! only when the user added or edited the playlist or a scheduled playlist
//! update ran (the setting's meaning); a changed channel list is detected
//! by the fingerprint anyway. Do not pass it for the per-start playlist
//! import the dev build does, or every start downloads the guide.

use std::collections::{HashMap, HashSet};

use iptv_epg::{
    ChannelKey, EpgRef, GuideStore, MatchKind, SettledBuilder, StoreOptions, TimePolicy, UnixTime,
};
use iptv_import::{EpgImport, EpgSourceConfig, GuideState};

use crate::epg_stage::EpgStage;
use iptv_providers::SecretUrl;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::{Result, StoreError};
use crate::sources::{ImportErrorKind, ensure_source};
use crate::{SourceId, Store};

/// One feed of a guide as written: its settings and fetch metadata.
/// `Debug` is safe (the URL is a [`SecretUrl`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpgFeedMeta {
    pub config: EpgSourceConfig,
    pub fetched_at: UnixTime,
    /// Response validators, when the network layer exposes them.
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Bytes downloaded (compressed size for gzip).
    pub bytes: u64,
}

impl EpgFeedMeta {
    /// The feeds of an import that loaded, with their configs.
    pub fn from_import(
        import: &EpgImport,
        configs: &[EpgSourceConfig],
        fetched_at: UnixTime,
    ) -> Vec<Self> {
        import
            .sources
            .iter()
            .filter_map(|o| {
                let stats = o.result.as_ref().ok()?;
                let config = configs.iter().find(|c| c.id == o.id)?.clone();
                Some(Self {
                    config,
                    fetched_at,
                    etag: None,
                    last_modified: None,
                    bytes: stats.bytes,
                })
            })
            .collect()
    }
}

/// A stored feed with what was written for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEpgFeed {
    pub meta: EpgFeedMeta,
    /// EPG channels stored (bound ones only).
    pub channel_count: u64,
    pub program_count: u64,
}

/// A playlist source's stored guide (or its failed attempts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredGuide {
    pub replaced_at: Option<UnixTime>,
    pub attempted_at: Option<UnixTime>,
    pub last_error: Option<ImportErrorKind>,
    pub match_fingerprint: Option<u64>,
    pub bound_channels: u64,
    /// Latest programme end written.
    pub ends_at: Option<UnixTime>,
    /// Highest priority first.
    pub feeds: Vec<StoredEpgFeed>,
}

impl StoredGuide {
    /// The input of `iptv_import::decide` / `plan_startup`.
    pub fn refresh_state(&self) -> GuideState {
        GuideState {
            fetched_at: self.replaced_at,
            attempted_at: self.attempted_at,
            last_attempt_failed: self.last_error.is_some(),
            feeds: self.configs(),
            match_fingerprint: self.match_fingerprint,
            ends_at: self.ends_at,
            bound_channels: usize::try_from(self.bound_channels).unwrap_or(usize::MAX),
        }
    }

    /// The feeds' configs, to refresh without downloading the playlist.
    pub fn configs(&self) -> Vec<EpgSourceConfig> {
        self.feeds.iter().map(|f| f.meta.config.clone()).collect()
    }
}

/// Counts written by [`Store::replace_epg`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EpgCounts {
    pub feeds: u64,
    pub channels: u64,
    pub bindings: u64,
    pub programs: u64,
    pub descriptions: u64,
    /// Bound playlist channels whose feed was not in `feeds` (not stored).
    pub skipped_bindings: u64,
}

/// What [`Store::load_epg`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpgWindow<'a> {
    /// Programmes overlapping `[from, to)`.
    pub from: UnixTime,
    pub to: UnixTime,
    /// Only these playlist channels; `None`: every bound channel.
    pub keys: Option<&'a [ChannelKey]>,
    /// Keep descriptions in memory (normally off: read them on demand
    /// with [`Store::epg_description`]).
    pub descriptions: bool,
}

impl EpgWindow<'static> {
    /// Every channel from `past_days` before `now` on, without descriptions.
    pub fn from(now: UnixTime, past_days: u32) -> Self {
        Self {
            from: now - i64::from(past_days) * iptv_epg::DAY,
            to: UnixTime::MAX,
            keys: None,
            descriptions: false,
        }
    }
}

pub(crate) fn kind_slug(kind: MatchKind) -> &'static str {
    match kind {
        MatchKind::Override => "override",
        MatchKind::TvgId => "tvg_id",
        MatchKind::Name => "name",
    }
}

pub(crate) fn kind_from(slug: &str) -> MatchKind {
    match slug {
        "override" => MatchKind::Override,
        "name" => MatchKind::Name,
        _ => MatchKind::TvgId,
    }
}

pub(crate) fn to_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// Page cache while a guide is copied in (KiB; the store's is
/// [`crate::CACHE_KIB`]).
const COPY_CACHE_KIB: i64 = 32 * 1024;

const DELETE_GUIDE: &[&str] = &[
    "DELETE FROM epg_feed_results WHERE source_id = ?1",
    "DELETE FROM epg_feed_channels WHERE source_id = ?1",
    "DELETE FROM epg_descriptions WHERE channel_id IN
         (SELECT id FROM epg_channels WHERE source_id = ?1)",
    "DELETE FROM epg_programs WHERE channel_id IN
         (SELECT id FROM epg_channels WHERE source_id = ?1)",
    "DELETE FROM epg_bindings WHERE source_id = ?1",
    "DELETE FROM epg_channels WHERE source_id = ?1",
    "DELETE FROM epg_feeds WHERE source_id = ?1",
];

fn delete_guide_rows(conn: &Connection, source: SourceId) -> Result<()> {
    for sql in DELETE_GUIDE {
        conn.execute(sql, [source.0])?;
    }
    Ok(())
}

fn insert_feed(conn: &Connection, source: SourceId, f: &EpgFeedMeta) -> Result<()> {
    let c = &f.config;
    conn.prepare_cached(
        "INSERT INTO epg_feeds (source_id, feed_id, url, priority, user_agent,
             assumed_offset_minutes, shift_minutes, store_descriptions, past_days_to_keep,
             fetched_at, etag, last_modified, bytes, content_supplied)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?
    .execute(params![
        source.0,
        c.id.0,
        c.url.expose(),
        c.priority,
        c.user_agent,
        c.time.assumed_offset_minutes,
        c.time.shift_minutes,
        c.store.store_descriptions,
        c.store.past_days_to_keep,
        f.fetched_at,
        f.etag,
        f.last_modified,
        i64::try_from(f.bytes).unwrap_or(i64::MAX),
        c.content_supplied,
    ])?;
    Ok(())
}

/// Lists every channel feed `feed` lists (id and first display name), for
/// choosing a channel's guide by hand.
fn insert_feed_channels(
    conn: &Connection,
    source: SourceId,
    feed: iptv_epg::FeedId,
    import: &EpgImport,
) -> Result<()> {
    let mut insert = conn.prepare_cached(
        "INSERT OR IGNORE INTO epg_feed_channels (source_id, feed_id, epg_channel, name)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for c in import.listed(feed) {
        insert.execute(params![source.0, feed.0, c.id, c.name])?;
    }
    Ok(())
}

/// Copies one staged channel's schedule (`stage`) to store channel
/// `channel`; returns (programmes, descriptions).
///
/// `OR FAIL`, with foreign keys off ([`Store::replace_epg`]): a multi-row
/// statement that may abort half-way needs a statement journal, which
/// copies every page of `epg_programs_by_stop` it touches first (3-4x
/// slower, and temporary files).
fn copy_programs(conn: &Connection, channel: i64, stage: i64) -> Result<(u64, u64)> {
    let n = conn
        .prepare_cached(
            "INSERT OR FAIL INTO main.epg_programs (channel_id, start, stop, title, sub_title,
                 category, icon, rating, catchup_id, season, episode)
             SELECT ?1, start, stop, title, sub_title, category, icon, rating, catchup_id,
                 season, episode
             FROM epg_stage.programmes WHERE ch = ?2 ORDER BY start",
        )?
        .execute(params![channel, stage])?;
    let d = conn
        .prepare_cached(
            "INSERT OR FAIL INTO main.epg_descriptions (channel_id, start, description)
             SELECT ?1, start, description FROM epg_stage.programmes
             WHERE ch = ?2 AND description IS NOT NULL ORDER BY start",
        )?
        .execute(params![channel, stage])?;
    Ok((n as u64, d as u64))
}

/// A bound channel as stored.
pub(crate) struct Binding {
    pub key: String,
    pub kind: MatchKind,
    /// The EPG channel's row id.
    pub channel: i64,
    pub feed: u32,
    pub epg_channel: String,
}

impl Store {
    /// Replaces `source`'s stored guide with a downloaded one in one
    /// transaction: `import` (how it matched) and `stage` (its programmes,
    /// from the same `iptv_import::import_epg` run).
    ///
    /// Stores every bound EPG channel of the feeds in `feeds` (all of its
    /// programmes, descriptions apart), the bindings and each feed's
    /// channel list; bindings to a feed missing from `feeds` are skipped
    /// and counted. `match_fingerprint`: `iptv_import::matching_fingerprint`
    /// of the channels and overrides `import` was matched with. Clears a
    /// recorded failure. On error the stored guide is unchanged.
    pub fn replace_epg(
        &mut self,
        source: SourceId,
        stage: &mut EpgStage,
        import: &EpgImport,
        feeds: &[EpgFeedMeta],
        match_fingerprint: u64,
    ) -> Result<EpgCounts> {
        stage.flush()?;
        self.attach_staging_database(stage.path(), "epg_stage")?;
        // Every row references a row written just before it in the same
        // transaction: consistent by construction. Off for the copy's sake
        // (`copy_programs`); it cannot change inside a transaction. A
        // larger page cache holds more of `epg_programs_by_stop`, which
        // every channel's programmes sweep (a third faster).
        let replaced = self
            .conn
            .pragma_update(None, "foreign_keys", false)
            .and_then(|()| self.conn.pragma_update(None, "cache_size", -COPY_CACHE_KIB))
            .map_err(StoreError::from)
            .and_then(|()| self.replace_staged(source, stage, import, feeds, match_fingerprint));
        let restored = self
            .conn
            .pragma_update(None, "foreign_keys", true)
            .and_then(|()| {
                self.conn
                    .pragma_update(None, "cache_size", -crate::CACHE_KIB)
            });
        let detached = self.conn.execute("DETACH DATABASE epg_stage", []);
        let counts = replaced?;
        restored?;
        detached?;
        Ok(counts)
    }

    fn replace_staged(
        &mut self,
        source: SourceId,
        stage: &EpgStage,
        import: &EpgImport,
        feeds: &[EpgFeedMeta],
        match_fingerprint: u64,
    ) -> Result<EpgCounts> {
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_source(&tx, source)?;
        delete_guide_rows(&tx, source)?;

        tx.execute(
            "INSERT INTO epg_guides (source_id, replaced_at, attempted_at, last_error_kind,
                 match_fingerprint)
             VALUES (?1, ?2, ?2, NULL, ?3)
             ON CONFLICT (source_id) DO UPDATE SET replaced_at = excluded.replaced_at,
                 attempted_at = excluded.attempted_at, last_error_kind = NULL,
                 match_fingerprint = excluded.match_fingerprint",
            params![source.0, now, match_fingerprint as i64],
        )?;
        let mut counts = EpgCounts::default();
        for f in feeds {
            insert_feed(&tx, source, f)?;
            let origin_key = format!("inspection.feed.{}.{}", source.0, f.config.id.0);
            tx.execute("DELETE FROM store_meta WHERE key=?1", [&origin_key])?;
            tx.execute(
                "INSERT INTO store_meta(key,value)
                 SELECT ?1,i.incarnation FROM guide_feed_incarnations i JOIN guide_sources g
                 ON g.source_id=i.source_id AND g.feed_id=i.feed_id
                 WHERE g.source_id=?2 AND g.feed_id=?3 AND g.url=?4 AND g.shift_minutes=?5",
                params![
                    origin_key,
                    source.0,
                    f.config.id.0,
                    f.config.url.expose(),
                    f.config.time.shift_minutes
                ],
            )?;

            insert_feed_channels(&tx, source, f.config.id, import)?;
            counts.feeds += 1;
        }
        let known: HashSet<u32> = feeds.iter().map(|f| f.config.id.0).collect();
        // EPG channel -> row id, and per-feed counts.
        let mut channels: HashMap<(u32, &str), i64> = HashMap::new();
        let mut per_feed: HashMap<u32, (u64, u64)> = HashMap::new();
        let mut ends_at: Option<i64> = None;
        // By key (`import.bindings` is sorted): deterministic row ids.
        for (key, epg) in &import.bindings {
            if !known.contains(&epg.source.0) {
                counts.skipped_bindings += 1;
                continue;
            }
            let id = match channels.get(&(epg.source.0, epg.epg_id.as_str())) {
                Some(&id) => id,
                None => {
                    tx.prepare_cached(
                        "INSERT INTO epg_channels (source_id, feed_id, epg_channel)
                         VALUES (?1, ?2, ?3)",
                    )?
                    .execute(params![source.0, epg.source.0, epg.epg_id])?;
                    let id = tx.last_insert_rowid();
                    let (n, d, last) = match stage.channel(epg.source, &epg.epg_id) {
                        Some(staged) => {
                            let (n, d) = copy_programs(&tx, id, staged.id)?;
                            (n, d, staged.last_stop)
                        }
                        None => (0, 0, None),
                    };
                    counts.channels += 1;
                    counts.programs += n;
                    counts.descriptions += d;
                    ends_at = ends_at.max(last);
                    let feed = per_feed.entry(epg.source.0).or_default();
                    feed.0 += 1;
                    feed.1 += n;
                    channels.insert((epg.source.0, epg.epg_id.as_str()), id);
                    id
                }
            };
            tx.prepare_cached(
                "INSERT INTO epg_bindings (source_id, channel_key, epg_channel_id, match_kind)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![source.0, key.as_str(), id, kind_slug(epg.kind)])?;
            counts.bindings += 1;
        }
        for (feed, (channels, programs)) in per_feed {
            tx.execute(
                "UPDATE epg_feeds SET channel_count = ?3, program_count = ?4
                 WHERE source_id = ?1 AND feed_id = ?2",
                params![source.0, feed, channels as i64, programs as i64],
            )?;
        }
        tx.execute(
            "UPDATE epg_guides SET bound_channels = ?2, ends_at = ?3 WHERE source_id = ?1",
            params![source.0, counts.bindings as i64, ends_at],
        )?;
        // Only a complete binding replacement proves the whole automatic
        // projection. A one-row cached rebind cannot renew this authority.
        tx.execute(
            "INSERT INTO store_meta(key,value)
             SELECT ?1,?2 || ':' || configuration_revision FROM guide_revisions WHERE source_id=?3
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                matching_projection_key(source),
                format!("{match_fingerprint:016x}"),
                source.0
            ],
        )?;
        tx.commit()?;
        Ok(counts)
    }

    /// Records a failed (or partly failed) guide download. Stored
    /// programmes stay; the refresh policy backs off.
    pub fn record_epg_failure(&mut self, source: SourceId, kind: ImportErrorKind) -> Result<()> {
        ensure_source(&self.conn, source)?;
        let now = self.now();
        self.conn.execute(
            "INSERT INTO epg_guides (source_id, attempted_at, last_error_kind) VALUES (?1, ?2, ?3)
             ON CONFLICT (source_id) DO UPDATE SET attempted_at = excluded.attempted_at,
                 last_error_kind = excluded.last_error_kind",
            params![source.0, now, kind.slug()],
        )?;
        Ok(())
    }

    /// What is stored about `source`'s guide; `None` when nothing is.
    pub fn epg_guide(&self, source: SourceId) -> Result<Option<StoredGuide>> {
        self.read_snapshot(|store| store.epg_guide_inner(source))
    }
    fn epg_guide_inner(&self, source: SourceId) -> Result<Option<StoredGuide>> {
        let guide = self
            .conn
            .query_row(
                "SELECT replaced_at, attempted_at, last_error_kind, match_fingerprint,
                     bound_channels, ends_at
                 FROM epg_guides WHERE source_id = ?1",
                [source.0],
                |r| {
                    Ok(StoredGuide {
                        replaced_at: r.get(0)?,
                        attempted_at: r.get(1)?,
                        last_error: r
                            .get::<_, Option<String>>(2)?
                            .map(|s| ImportErrorKind::from_slug(&s)),
                        match_fingerprint: r.get::<_, Option<i64>>(3)?.map(|n| n as u64),
                        bound_channels: to_u64(r.get(4)?),
                        ends_at: r.get(5)?,
                        feeds: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut guide) = guide else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT feed_id, url, priority, user_agent, assumed_offset_minutes, shift_minutes,
                 store_descriptions, past_days_to_keep, fetched_at, etag, last_modified, bytes,
                 channel_count, program_count, content_supplied
             FROM epg_feeds WHERE source_id = ?1 ORDER BY priority DESC, feed_id",
        )?;
        let rows = stmt.query_map([source.0], |r| {
            let config = EpgSourceConfig {
                content_supplied: r.get(14)?,
                id: iptv_epg::FeedId(r.get(0)?),
                url: SecretUrl::new(r.get::<_, String>(1)?),
                priority: r.get(2)?,
                user_agent: r.get(3)?,
                time: TimePolicy {
                    assumed_offset_minutes: r.get(4)?,
                    shift_minutes: r.get(5)?,
                },
                store: StoreOptions {
                    store_descriptions: r.get(6)?,
                    past_days_to_keep: r.get(7)?,
                },
            };
            Ok(StoredEpgFeed {
                meta: EpgFeedMeta {
                    config,
                    fetched_at: r.get(8)?,
                    etag: r.get(9)?,
                    last_modified: r.get(10)?,
                    bytes: to_u64(r.get(11)?),
                },
                channel_count: to_u64(r.get(12)?),
                program_count: to_u64(r.get(13)?),
            })
        })?;
        for row in rows {
            guide.feeds.push(row?);
        }
        Ok(Some(guide))
    }

    /// Reads a window of `source`'s stored guide into a [`GuideStore`]
    /// bound to the playlist channel keys. Empty when nothing is stored.
    pub fn load_epg(&self, source: SourceId, window: &EpgWindow<'_>) -> Result<GuideStore> {
        self.read_snapshot(|store| store.load_epg_inner(source, window))
    }
    fn load_epg_inner(&self, source: SourceId, window: &EpgWindow<'_>) -> Result<GuideStore> {
        let wanted: Option<HashSet<&str>> = window
            .keys
            .map(|keys| keys.iter().map(ChannelKey::as_str).collect());
        let mut stmt = self.conn.prepare_cached(
            "SELECT b.channel_key, b.match_kind, c.id, c.feed_id, c.epg_channel
             FROM epg_bindings b JOIN epg_channels c ON c.id = b.epg_channel_id
             WHERE b.source_id = ?1",
        )?;
        let mut bindings = Vec::new();
        let mut rows = stmt.query([source.0])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            if wanted.as_ref().is_some_and(|w| !w.contains(key.as_str())) {
                continue;
            }
            bindings.push(Binding {
                key,
                kind: kind_from(&r.get::<_, String>(1)?),
                channel: r.get(2)?,
                feed: r.get(3)?,
                epg_channel: r.get(4)?,
            });
        }
        drop(rows);
        // Stored rows are settled (`replace_epg` wrote them so): they go
        // straight into their slots, in start order.
        let mut builders: HashMap<u32, SettledBuilder> = HashMap::new();
        let mut feeds = self
            .conn
            .prepare_cached("SELECT feed_id, priority FROM epg_feeds WHERE source_id = ?1")?;
        let mut rows = feeds.query([source.0])?;
        while let Some(r) = rows.next()? {
            let id: u32 = r.get(0)?;
            let b = SettledBuilder::new(iptv_epg::FeedId(id), r.get(1)?, window.descriptions);
            builders.insert(id, b);
        }
        drop(rows);
        crate::epg_window::read_programs(&self.conn, &bindings, &mut builders, window)?;
        let mut guide = GuideStore::new();
        for (_, b) in builders {
            guide.replace_source(b.finish());
        }
        let matches = bindings
            .into_iter()
            .map(|b| {
                let cand = EpgRef {
                    source: iptv_epg::FeedId(b.feed),
                    epg_id: b.epg_channel,
                    kind: b.kind,
                };
                (ChannelKey(b.key), vec![cand])
            })
            .collect();
        guide.set_matches(matches);
        Ok(guide)
    }

    /// The description of the programme starting at `start` on playlist
    /// channel `key`, read on demand.
    pub fn epg_description(
        &self,
        source: SourceId,
        key: &ChannelKey,
        start: UnixTime,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT d.description FROM epg_bindings b
                 JOIN epg_descriptions d ON d.channel_id = b.epg_channel_id
                 WHERE b.source_id = ?1 AND b.channel_key = ?2 AND d.start = ?3",
            )?
            .query_row(params![source.0, key.as_str(), start], |r| r.get(0))
            .optional()?)
    }

    /// Deletes programmes (of every source) that ended at or before
    /// `before`. Returns how many.
    pub fn prune_epg(&mut self, before: UnixTime) -> Result<u64> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM epg_descriptions WHERE (channel_id, start) IN
                 (SELECT channel_id, start FROM epg_programs WHERE stop <= ?1)",
            [before],
        )?;
        let n = tx.execute("DELETE FROM epg_programs WHERE stop <= ?1", [before])?;
        tx.commit()?;
        Ok(n as u64)
    }

    /// Deletes `source`'s stored guide ("clear EPG", or no feed is
    /// configured any more). Returns whether there was one.
    pub fn delete_epg(&mut self, source: SourceId) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        delete_guide_rows(&tx, source)?;
        let n = tx.execute("DELETE FROM epg_guides WHERE source_id = ?1", [source.0])?;
        tx.commit()?;
        Ok(n > 0)
    }
}

/// Authority for a complete projection, separate from the legacy refresh hash.
pub(crate) fn matching_projection_key(source: SourceId) -> String {
    format!("epg.match.projection.{}", source.0)
}
