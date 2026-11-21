//! Guide replacement as a resumable job ([`EpgReplacement`]); the one-shot
//! [`Store::replace_epg`] runs one to completion.

use std::borrow::{BorrowMut, Cow};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use iptv_import::EpgImport;
use rusqlite::params;

use crate::epg::{
    COPY_CACHE_KIB, EpgCounts, EpgFeedMeta, copy_programs, delete_guide_rows, insert_feed,
    insert_feed_channels, kind_slug, matching_projection_key,
};
use crate::epg_stage::{EpgStage, StagedChannel};
use crate::replacement::{self, Meter, Progress, Step, delete_some};
use crate::sources::ensure_source;
use crate::{Result, SourceId, Store, StoreError};

#[derive(Clone, Copy, Debug)]
enum Phase {
    Begin,
    /// `epg_feed_results`, `epg_feed_channels`.
    DeleteFeedRows {
        table: usize,
    },
    /// The old channels' descriptions, then programs, channel by channel
    /// (by `epg_channels.id` after `after`).
    DeleteSchedules {
        after: i64,
        current: Option<i64>,
        programs: bool,
    },
    /// `epg_bindings`, `epg_channels`, `epg_feeds`.
    DeleteRest {
        table: usize,
    },
    Guide,
    /// Feed `index`: its row, then its listed channels from `listed`.
    Feed {
        index: usize,
        listed: Option<usize>,
    },
    /// Binding `index`, possibly in the middle of copying its schedule.
    Binding {
        index: usize,
        copy: Option<Transfer>,
    },
    Finish,
}

/// A schedule being copied from the stage.
#[derive(Clone, Copy, Debug)]
struct Transfer {
    channel: i64,
    staged: StagedChannel,
    /// The last start copied.
    after: i64,
    /// Copying descriptions (programs are done).
    descriptions: bool,
    finished: bool,
    programs_copied: u64,
    descriptions_copied: u64,
}

const DELETE_FEED_ROWS: [(&str, &str); 2] = [
    ("epg_feed_results", "source_id, feed_id"),
    ("epg_feed_channels", "source_id, feed_id, epg_channel"),
];
const DELETE_REST: [(&str, &str); 3] = [
    ("epg_bindings", "source_id, channel_key"),
    ("epg_channels", "id"),
    ("epg_feeds", "source_id, feed_id"),
];

/// A guide replacement that runs until a row budget is spent and is
/// continued later ([`Self::step`]), or to completion ([`Self::finish`]):
/// the resumable form of [`Store::replace_epg`]. See `replacement.rs`.
///
/// The stage stays attached until the job ends; keep it alive until then.
/// `S` is the store it writes, borrowed or owned (a browser publish copy);
/// [`Self::into_store`] gives an owned one back.
pub struct EpgReplacement<'a, S: BorrowMut<Store>> {
    store: Option<S>,
    source: SourceId,
    import: Cow<'a, EpgImport>,
    feeds: Cow<'a, [EpgFeedMeta]>,
    match_fingerprint: u64,
    /// The staged schedule of each binding's EPG channel.
    staged: Vec<Option<StagedChannel>>,
    stage_path: PathBuf,
    attached: bool,
    /// Foreign keys and the page cache were changed for the copy.
    tuned: bool,
    phase: Phase,
    progress: Progress,
    now: i64,
    counts: EpgCounts,
    known: HashSet<u32>,
    /// EPG channel -> row id.
    channels: HashMap<(u32, String), i64>,
    /// Per feed: channels and programs written.
    per_feed: HashMap<u32, (u64, u64)>,
    ends_at: Option<i64>,
}

impl<S: BorrowMut<Store>> std::fmt::Debug for EpgReplacement<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpgReplacement")
            .field("source", &self.source)
            .field("phase", &self.phase)
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}

impl<'a, S: BorrowMut<Store>> EpgReplacement<'a, S> {
    /// Replaces `source`'s stored guide with a downloaded one, as
    /// [`Store::replace_epg`] documents. Waits for the stage's writes.
    pub fn new(
        store: S,
        source: SourceId,
        stage: &mut EpgStage,
        import: Cow<'a, EpgImport>,
        feeds: impl Into<Cow<'a, [EpgFeedMeta]>>,
        match_fingerprint: u64,
    ) -> Result<Self> {
        stage.flush()?;
        let feeds = feeds.into();
        let staged = import
            .bindings
            .iter()
            .map(|(_, epg)| stage.channel(epg.source, &epg.epg_id))
            .collect();
        let known = feeds.iter().map(|f| f.config.id.0).collect();
        Ok(Self {
            store: Some(store),
            source,
            import,
            feeds,
            match_fingerprint,
            staged,
            stage_path: stage.path().to_owned(),
            attached: false,
            tuned: false,
            phase: Phase::Begin,
            progress: Progress::Running,
            now: 0,
            counts: EpgCounts::default(),
            known,
            channels: HashMap::new(),
            per_feed: HashMap::new(),
            ends_at: None,
        })
    }

    /// Writes until about `rows` rows are written (at least one), then
    /// stops between statements with the transaction still open.
    pub fn step(&mut self, rows: u32) -> Result<Step<EpgCounts>> {
        self.run(Meter::rows(rows))
    }

    /// Runs to completion: exactly the statements of the one-shot form.
    pub fn finish(mut self) -> Result<EpgCounts> {
        match self.run(Meter::UNLIMITED)? {
            Step::Done(counts) => Ok(counts),
            Step::Pending => Err(StoreError::Unavailable("an unlimited step stopped early")),
        }
    }

    /// Whether the replacement committed.
    pub fn is_done(&self) -> bool {
        self.progress == Progress::Committed
    }

    /// The store back: after the commit, or abandoning (rolling back) an
    /// unfinished replacement.
    pub fn into_store(mut self) -> S {
        let _ = self.end();
        self.store
            .take()
            .expect("the store stays until the job is dropped")
    }

    fn store(&mut self) -> &mut Store {
        self.store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut()
    }

    fn run(&mut self, mut meter: Meter) -> Result<Step<EpgCounts>> {
        if self.progress != Progress::Running {
            return Err(replacement::finished_error());
        }
        match self.advance(&mut meter) {
            Ok(Step::Done(counts)) => {
                self.end()?;
                Ok(Step::Done(counts))
            }
            Ok(Step::Pending) => Ok(Step::Pending),
            Err(e) => {
                let _ = self.end();
                self.progress = Progress::Failed;
                Err(e)
            }
        }
    }

    /// Rolls back an open transaction, restores the connection's settings
    /// and detaches the stage; the first failure of the last two.
    fn end(&mut self) -> Result<()> {
        let running = self.progress == Progress::Running;
        let (tuned, attached) = (
            std::mem::take(&mut self.tuned),
            std::mem::take(&mut self.attached),
        );
        let Some(store) = self.store.as_mut() else {
            return Ok(());
        };
        let conn = &store.borrow_mut().conn;
        if running {
            replacement::roll_back(conn);
        }
        let restored = if tuned {
            conn.pragma_update(None, "foreign_keys", true)
                .and_then(|()| conn.pragma_update(None, "cache_size", -crate::CACHE_KIB))
        } else {
            Ok(())
        };
        let detached = if attached {
            conn.execute("DETACH DATABASE epg_stage", []).map(drop)
        } else {
            Ok(())
        };
        restored?;
        detached?;
        Ok(())
    }

    fn advance(&mut self, meter: &mut Meter) -> Result<Step<EpgCounts>> {
        loop {
            if meter.exhausted() {
                return Ok(Step::Pending);
            }
            match self.phase {
                Phase::Begin => self.begin()?,
                Phase::DeleteFeedRows { table } => self.delete_feed_rows(meter, table)?,
                Phase::DeleteSchedules {
                    after,
                    current,
                    programs,
                } => self.delete_schedules(meter, after, current, programs)?,
                Phase::DeleteRest { table } => self.delete_rest(meter, table)?,
                Phase::Guide => self.guide()?,
                Phase::Feed { index, listed } => self.feed(meter, index, listed)?,
                Phase::Binding { index, copy } => self.binding(meter, index, copy)?,
                Phase::Finish => return self.commit().map(Step::Done),
            }
        }
    }

    fn begin(&mut self) -> Result<()> {
        let source = self.source;
        let path = self.stage_path.clone();
        let store = self.store();
        store.attach_staging_database(&path, "epg_stage")?;
        self.attached = true;
        // Every row references a row written just before it in the same
        // transaction: consistent by construction. Off for the copy's sake
        // (`copy_programs`); it cannot change inside a transaction. A
        // larger page cache holds more of `epg_programs_by_stop`, which
        // every channel's programs sweep (a third faster).
        self.tuned = true;
        let store = self.store();
        store.conn.pragma_update(None, "foreign_keys", false)?;
        store
            .conn
            .pragma_update(None, "cache_size", -COPY_CACHE_KIB)?;
        let now = store.now();
        replacement::begin(&store.conn)?;
        ensure_source(&store.conn, source)?;
        self.now = now;
        self.phase = Phase::DeleteFeedRows { table: 0 };
        Ok(())
    }

    fn delete_feed_rows(&mut self, meter: &mut Meter, table: usize) -> Result<()> {
        let source = self.source;
        if meter.unlimited() {
            delete_guide_rows(&self.store().conn, source)?;
            self.phase = Phase::Guide;
            return Ok(());
        }
        let (name, key) = DELETE_FEED_ROWS[table];
        let n = delete_some(
            &self.store().conn,
            name,
            key,
            "source_id = ?1",
            &[&source.0],
            meter.limit(),
        )?;
        meter.spend(n);
        if n == 0 {
            self.phase = if table + 1 < DELETE_FEED_ROWS.len() {
                Phase::DeleteFeedRows { table: table + 1 }
            } else {
                Phase::DeleteSchedules {
                    after: i64::MIN,
                    current: None,
                    programs: false,
                }
            };
        }
        Ok(())
    }

    fn delete_schedules(
        &mut self,
        meter: &mut Meter,
        after: i64,
        current: Option<i64>,
        programs: bool,
    ) -> Result<()> {
        let source = self.source.0;
        let conn = &self.store().conn;
        let Some(channel) = current else {
            let next: Option<i64> = conn
                .prepare_cached(
                    "SELECT id FROM epg_channels WHERE +source_id = ?1 AND id > ?2
                     ORDER BY id LIMIT 1",
                )?
                .query_row(params![source, after], |r| r.get(0))
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })?;
            self.phase = match next {
                Some(channel) => Phase::DeleteSchedules {
                    after,
                    current: Some(channel),
                    programs: false,
                },
                None => Phase::DeleteRest { table: 0 },
            };
            return Ok(());
        };
        let (table, key) = if programs {
            ("epg_programs", "channel_id, start")
        } else {
            ("epg_descriptions", "channel_id, start")
        };
        let n = delete_some(
            conn,
            table,
            key,
            "channel_id = ?1",
            &[&channel],
            meter.limit(),
        )?;
        meter.spend(n);
        if n == 0 {
            self.phase = if programs {
                Phase::DeleteSchedules {
                    after: channel,
                    current: None,
                    programs: false,
                }
            } else {
                Phase::DeleteSchedules {
                    after,
                    current: Some(channel),
                    programs: true,
                }
            };
        }
        Ok(())
    }

    fn delete_rest(&mut self, meter: &mut Meter, table: usize) -> Result<()> {
        let source = self.source.0;
        let (name, key) = DELETE_REST[table];
        let n = delete_some(
            &self.store().conn,
            name,
            key,
            "source_id = ?1",
            &[&source],
            meter.limit(),
        )?;
        meter.spend(n);
        if n == 0 {
            self.phase = if table + 1 < DELETE_REST.len() {
                Phase::DeleteRest { table: table + 1 }
            } else {
                Phase::Guide
            };
        }
        Ok(())
    }

    fn guide(&mut self) -> Result<()> {
        let (source, now, fingerprint) = (self.source, self.now, self.match_fingerprint);
        self.store().conn.execute(
            "INSERT INTO epg_guides (source_id, replaced_at, attempted_at, last_error_kind,
                 match_fingerprint)
             VALUES (?1, ?2, ?2, NULL, ?3)
             ON CONFLICT (source_id) DO UPDATE SET replaced_at = excluded.replaced_at,
                 attempted_at = excluded.attempted_at, last_error_kind = NULL,
                 match_fingerprint = excluded.match_fingerprint",
            params![source.0, now, fingerprint as i64],
        )?;
        self.phase = Phase::Feed {
            index: 0,
            listed: None,
        };
        Ok(())
    }

    fn feed(&mut self, meter: &mut Meter, index: usize, listed: Option<usize>) -> Result<()> {
        let source = self.source;
        let Some(meta) = self.feeds.get(index) else {
            self.phase = Phase::Binding {
                index: 0,
                copy: None,
            };
            return Ok(());
        };
        let feed = meta.config.id;
        let store = self
            .store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut();
        let conn = &store.conn;
        let Some(at) = listed else {
            insert_feed(conn, source, meta)?;
            let origin_key = format!("inspection.feed.{}.{}", source.0, feed.0);
            conn.execute("DELETE FROM store_meta WHERE key=?1", [&origin_key])?;
            conn.execute(
                "INSERT INTO store_meta(key,value)
                 SELECT ?1,i.incarnation FROM guide_feed_incarnations i JOIN guide_sources g
                 ON g.source_id=i.source_id AND g.feed_id=i.feed_id
                 WHERE g.source_id=?2 AND g.feed_id=?3 AND g.url=?4 AND g.shift_minutes=?5",
                params![
                    origin_key,
                    source.0,
                    meta.config.id.0,
                    meta.config.url.expose(),
                    meta.config.time.shift_minutes
                ],
            )?;
            meter.spend(1);
            self.phase = Phase::Feed {
                index,
                listed: Some(0),
            };
            return Ok(());
        };
        let all = self.import.listed(feed);
        let n = meter.take(all.len() - at);
        insert_feed_channels(conn, source, feed, &all[at..at + n])?;
        meter.spend(n);
        self.phase = if at + n < all.len() {
            Phase::Feed {
                index,
                listed: Some(at + n),
            }
        } else {
            self.counts.feeds += 1;
            Phase::Feed {
                index: index + 1,
                listed: None,
            }
        };
        Ok(())
    }

    fn binding(&mut self, meter: &mut Meter, index: usize, copy: Option<Transfer>) -> Result<()> {
        let source = self.source;
        let Some((key, epg)) = self.import.bindings.get(index) else {
            self.phase = Phase::Finish;
            return Ok(());
        };
        let feed = epg.source.0;
        let next = Phase::Binding {
            index: index + 1,
            copy: None,
        };
        let store = self
            .store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut();
        let conn = &store.conn;
        let channel = match copy {
            Some(copy) => {
                let copy = copy_slice(conn, meter, copy)?;
                if !copy.finished {
                    self.phase = Phase::Binding {
                        index,
                        copy: Some(copy),
                    };
                    return Ok(());
                }
                let staged = copy.staged;
                record_channel(
                    &mut self.counts,
                    &mut self.per_feed,
                    &mut self.ends_at,
                    feed,
                    (
                        copy.programs_copied,
                        copy.descriptions_copied,
                        staged.last_stop,
                    ),
                );
                self.channels
                    .insert((feed, epg.epg_id.clone()), copy.channel);
                copy.channel
            }
            None => {
                if !self.known.contains(&feed) {
                    self.counts.skipped_bindings += 1;
                    self.phase = next;
                    return Ok(());
                }
                match self.channels.get(&(feed, epg.epg_id.clone())) {
                    Some(&id) => id,
                    None => {
                        conn.prepare_cached(
                            "INSERT INTO epg_channels (source_id, feed_id, epg_channel)
                             VALUES (?1, ?2, ?3)",
                        )?
                        .execute(params![source.0, feed, epg.epg_id])?;
                        let id = conn.last_insert_rowid();
                        meter.spend(1);
                        match self.staged[index] {
                            Some(staged) if !meter.unlimited() => {
                                self.phase = Phase::Binding {
                                    index,
                                    copy: Some(Transfer {
                                        channel: id,
                                        staged,
                                        after: i64::MIN,
                                        descriptions: false,
                                        finished: false,
                                        programs_copied: 0,
                                        descriptions_copied: 0,
                                    }),
                                };
                                return Ok(());
                            }
                            staged => {
                                let (n, d, last) = match staged {
                                    Some(staged) => {
                                        let (n, d) = copy_programs(conn, id, staged.id)?;
                                        (n, d, staged.last_stop)
                                    }
                                    None => (0, 0, None),
                                };
                                record_channel(
                                    &mut self.counts,
                                    &mut self.per_feed,
                                    &mut self.ends_at,
                                    feed,
                                    (n, d, last),
                                );
                                self.channels.insert((feed, epg.epg_id.clone()), id);
                                id
                            }
                        }
                    }
                }
            }
        };
        conn.prepare_cached(
            "INSERT INTO epg_bindings (source_id, channel_key, epg_channel_id, match_kind)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![
            source.0,
            key.as_str(),
            channel,
            kind_slug(epg.kind)
        ])?;
        meter.spend(1);
        self.counts.bindings += 1;
        self.phase = next;
        Ok(())
    }

    fn commit(&mut self) -> Result<EpgCounts> {
        let source = self.source;
        let fingerprint = self.match_fingerprint;
        let counts = self.counts;
        let ends_at = self.ends_at;
        let per_feed = std::mem::take(&mut self.per_feed);
        let conn = &self.store().conn;
        for (feed, (channels, programs)) in per_feed {
            conn.execute(
                "UPDATE epg_feeds SET channel_count = ?3, program_count = ?4
                 WHERE source_id = ?1 AND feed_id = ?2",
                params![source.0, feed, channels as i64, programs as i64],
            )?;
        }
        conn.execute(
            "UPDATE epg_guides SET bound_channels = ?2, ends_at = ?3 WHERE source_id = ?1",
            params![source.0, counts.bindings as i64, ends_at],
        )?;
        // Only a complete binding replacement proves the whole automatic
        // projection. A one-row cached rebind cannot renew this authority.
        conn.execute(
            "INSERT INTO store_meta(key,value)
             SELECT ?1,?2 || ':' || configuration_revision FROM guide_revisions WHERE source_id=?3
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                matching_projection_key(source),
                format!("{fingerprint:016x}"),
                source.0
            ],
        )?;
        conn.execute_batch("COMMIT")?;
        self.progress = Progress::Committed;
        Ok(counts)
    }
}

impl<S: BorrowMut<Store>> Drop for EpgReplacement<'_, S> {
    fn drop(&mut self) {
        let _ = self.end();
    }
}

/// Copies the next slice of a staged schedule: programs in start order,
/// then descriptions.
fn copy_slice(
    conn: &rusqlite::Connection,
    meter: &mut Meter,
    mut copy: Transfer,
) -> Result<Transfer> {
    let description = if copy.descriptions {
        " AND description IS NOT NULL"
    } else {
        ""
    };
    let last: Option<i64> = conn
        .prepare_cached(&format!(
            "SELECT max(start) FROM (SELECT start FROM epg_stage.programs
             WHERE ch = ?1 AND start > ?2{description} ORDER BY start LIMIT ?3)"
        ))?
        .query_row(params![copy.staged.id, copy.after, meter.limit()], |r| {
            r.get(0)
        })?;
    let Some(last) = last else {
        if copy.descriptions {
            copy.finished = true;
        } else {
            copy.descriptions = true;
            copy.after = i64::MIN;
        }
        return Ok(copy);
    };
    // `OR FAIL`, with foreign keys off: see `copy_programs`.
    let n = if copy.descriptions {
        conn.prepare_cached(
            "INSERT OR FAIL INTO main.epg_descriptions (channel_id, start, description)
             SELECT ?1, start, description FROM epg_stage.programs
             WHERE ch = ?2 AND start > ?3 AND start <= ?4 AND description IS NOT NULL
             ORDER BY start",
        )?
        .execute(params![copy.channel, copy.staged.id, copy.after, last])?
    } else {
        conn.prepare_cached(
            "INSERT OR FAIL INTO main.epg_programs (channel_id, start, stop, title, sub_title,
                 category, icon, rating, catchup_id, season, episode)
             SELECT ?1, start, stop, title, sub_title, category, icon, rating, catchup_id,
                 season, episode
             FROM epg_stage.programs WHERE ch = ?2 AND start > ?3 AND start <= ?4
             ORDER BY start",
        )?
        .execute(params![copy.channel, copy.staged.id, copy.after, last])?
    };
    if copy.descriptions {
        copy.descriptions_copied += n as u64;
    } else {
        copy.programs_copied += n as u64;
    }
    meter.spend(n.max(1));
    copy.after = last;
    Ok(copy)
}

/// Adds a channel's copied schedule to the counts.
fn record_channel(
    counts: &mut EpgCounts,
    per_feed: &mut HashMap<u32, (u64, u64)>,
    ends_at: &mut Option<i64>,
    feed: u32,
    (programs, descriptions, last): (u64, u64, Option<i64>),
) {
    counts.channels += 1;
    counts.programs += programs;
    counts.descriptions += descriptions;
    *ends_at = (*ends_at).max(last);
    let entry = per_feed.entry(feed).or_default();
    entry.0 += 1;
    entry.1 += programs;
}
