//! Guide replacement as a resumable job ([`EpgReplacement`]); the one-shot
//! [`Store::replace_epg`] runs one to completion.

use std::borrow::{BorrowMut, Cow};
use std::collections::{HashMap, HashSet};

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
    /// The old channels' descriptions, then programs, channel by channel,
    /// in the order of the index on (source_id, feed_id, epg_channel), after
    /// the channel whose row id is `after`.
    DeleteSchedules {
        after: Option<i64>,
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
    /// The last start copied; `None` before the first slice.
    after: Option<i64>,
    /// Copying descriptions (programs are done).
    descriptions: bool,
    finished: bool,
    programs_copied: u64,
    descriptions_copied: u64,
}

/// The first of a source's EPG channels, and the one after channel `?2`, in
/// the order of the unique index on (source_id, feed_id, epg_channel).
const FIRST_CHANNEL: &str = "SELECT id FROM epg_channels WHERE source_id = ?1
     ORDER BY feed_id, epg_channel LIMIT 1";
///
/// A row-value comparison `(feed_id, epg_channel) > (?, ?)` would seek only
/// on feed_id and read the feed's earlier channels, so the next channel is
/// the next one of the same feed, else the first of a later feed: two
/// seeks.
const NEXT_CHANNEL: &str = "SELECT coalesce(
         (SELECT n.id FROM epg_channels n WHERE n.source_id = ?1 AND n.feed_id = c.feed_id
              AND n.epg_channel > c.epg_channel ORDER BY n.epg_channel LIMIT 1),
         (SELECT n.id FROM epg_channels n WHERE n.source_id = ?1 AND n.feed_id > c.feed_id
              ORDER BY n.feed_id, n.epg_channel LIMIT 1))
     FROM epg_channels c WHERE c.id = ?2";

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
/// The job holds its stage (borrowed, [`Self::new`], or owned,
/// [`Self::with_stage`]) until it ends. `S` is the store it writes,
/// borrowed or owned (a browser publish copy); [`Self::into_store`] gives an
/// owned one back.
pub struct EpgReplacement<'a, S: BorrowMut<Store>> {
    store: Option<S>,
    source: SourceId,
    import: Cow<'a, EpgImport>,
    feeds: Cow<'a, [EpgFeedMeta]>,
    match_fingerprint: u64,
    /// Per binding, the first binding of the same EPG channel: where its
    /// row id is kept in `ids` (no key strings are copied).
    first: Vec<usize>,
    /// The EPG channel row id written for a binding (at its `first`).
    ids: HashMap<usize, i64>,
    /// Held until the job ends: nothing else can write it, or drop it,
    /// while it is attached.
    stage: StageHold<'a>,
    attached: bool,
    /// Foreign keys and the page cache were changed for the copy.
    tuned: bool,
    phase: Phase,
    progress: Progress,
    now: i64,
    counts: EpgCounts,
    known: HashSet<u32>,
    /// Per feed: channels and programs written.
    per_feed: HashMap<u32, (u64, u64)>,
    ends_at: Option<i64>,
    /// This job committed slices on a browser publish copy
    /// (`replacement::pause`).
    copy_claim: bool,
    /// Tests: an error the cleanup after the commit reports.
    #[cfg(test)]
    pub(crate) fail_cleanup: Option<StoreError>,
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
        stage: &'a mut EpgStage,
        import: Cow<'a, EpgImport>,
        feeds: impl Into<Cow<'a, [EpgFeedMeta]>>,
        match_fingerprint: u64,
    ) -> Result<Self> {
        Self::holding(
            store,
            source,
            StageHold::Borrowed(stage),
            import,
            feeds.into(),
            match_fingerprint,
        )
    }

    /// [`Self::new`] owning its stage, for a job kept between event-loop
    /// turns (the browser's slices); the stage goes with the job.
    pub fn with_stage(
        store: S,
        source: SourceId,
        stage: EpgStage,
        import: Cow<'a, EpgImport>,
        feeds: impl Into<Cow<'a, [EpgFeedMeta]>>,
        match_fingerprint: u64,
    ) -> Result<Self> {
        Self::holding(
            store,
            source,
            StageHold::Owned(Box::new(stage)),
            import,
            feeds.into(),
            match_fingerprint,
        )
    }

    fn holding(
        store: S,
        source: SourceId,
        mut stage: StageHold<'a>,
        import: Cow<'a, EpgImport>,
        feeds: Cow<'a, [EpgFeedMeta]>,
        match_fingerprint: u64,
    ) -> Result<Self> {
        stage.get_mut().flush()?;
        let first = {
            let mut seen: HashMap<(u32, &str), usize> = HashMap::new();
            import
                .bindings
                .iter()
                .enumerate()
                .map(|(i, (_, epg))| *seen.entry((epg.source.0, epg.epg_id.as_str())).or_insert(i))
                .collect()
        };
        // One entry per EPG channel written, as master's map had.
        let ids = HashMap::new();
        let known = feeds.iter().map(|f| f.config.id.0).collect();
        Ok(Self {
            store: Some(store),
            source,
            import,
            feeds,
            match_fingerprint,
            first,
            ids,
            stage,
            attached: false,
            tuned: false,
            phase: Phase::Begin,
            progress: Progress::Running,
            now: 0,
            counts: EpgCounts::default(),
            known,
            per_feed: HashMap::new(),
            ends_at: None,
            copy_claim: false,
            #[cfg(test)]
            fail_cleanup: None,
        })
    }

    /// Writes until about `rows` rows are written (at least one), then
    /// stops between statements with the transaction still open. On a
    /// browser publish copy the step commits instead: its rows stay in the
    /// copy until the copy is discarded, and if the job then ends without
    /// its last commit the copy is never swapped in.
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

    /// Tests: the phase the next step continues in.
    #[cfg(test)]
    pub(crate) fn phase(&self) -> String {
        format!("{:?}", self.phase)
    }

    /// Whether the replacement committed.
    pub fn is_done(&self) -> bool {
        self.progress == Progress::Committed
    }

    /// The store back: after the commit, or abandoning (rolling back) an
    /// unfinished replacement. A browser publish copy keeps the slices it
    /// committed, so an unfinished job leaves it unpublishable: discard it
    /// (`abandon_publish`) and begin a new publish.
    pub fn into_store(mut self) -> S {
        let _ = self.end();
        self.store
            .take()
            .expect("the store stays until the job is dropped")
    }

    pub(crate) fn store(&mut self) -> &mut Store {
        self.store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut()
    }

    fn run(&mut self, mut meter: Meter) -> Result<Step<EpgCounts>> {
        if self.progress != Progress::Running {
            return Err(replacement::finished_error());
        }
        let changes = self.store().conn.total_changes();
        let stepped = self.resume().and_then(|()| self.advance(&mut meter));
        let stepped = match stepped {
            Ok(Step::Pending) => {
                let store = self
                    .store
                    .as_mut()
                    .expect("the store stays until the job is dropped")
                    .borrow_mut();
                let changed = store.conn.total_changes() != changes;
                replacement::pause(store, &mut self.copy_claim, changed).map(|()| Step::Pending)
            }
            other => other,
        };
        match stepped {
            Ok(Step::Done(counts)) => {
                let cleanup = self.end();
                #[cfg(test)]
                let cleanup = cleanup.and(self.fail_cleanup.take().map_or(Ok(()), Err));
                replacement::after_commit(self.store(), cleanup)?;
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

    /// Reopens the transaction a browser publish copy committed between
    /// steps (`replacement::pause`).
    fn resume(&mut self) -> Result<()> {
        if matches!(self.phase, Phase::Begin) {
            return Ok(());
        }
        replacement::resume(self.store())
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
        let store = store.borrow_mut();
        if running {
            replacement::roll_back(&store.conn);
        }
        if self.progress != Progress::Committed {
            replacement::abandoned(store, &mut self.copy_claim);
        }
        let conn = &store.conn;
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
        let path = self.stage.get().path().to_owned();
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
                    after: None,
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
        after: Option<i64>,
        current: Option<i64>,
        programs: bool,
    ) -> Result<()> {
        let source = self.source.0;
        let conn = &self.store().conn;
        let Some(channel) = current else {
            // A seek on the source's channels in the unique index, never a
            // walk past another source's rows; each visit costs a row.
            let next = match after {
                None => conn
                    .prepare_cached(FIRST_CHANNEL)?
                    .query_row(params![source], |r| r.get::<_, Option<i64>>(0)),
                Some(after) => conn
                    .prepare_cached(NEXT_CHANNEL)?
                    .query_row(params![source, after], |r| r.get::<_, Option<i64>>(0)),
            };
            let next: Option<i64> = match next {
                Ok(id) => id,
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(e.into()),
            };
            meter.spend(1);
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
                    after: Some(channel),
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
                self.ids.insert(self.first[index], copy.channel);
                copy.channel
            }
            None => {
                if !self.known.contains(&feed) {
                    self.counts.skipped_bindings += 1;
                    self.phase = next;
                    return Ok(());
                }
                match self.ids.get(&self.first[index]).copied() {
                    Some(id) => id,
                    None => {
                        conn.prepare_cached(
                            "INSERT INTO epg_channels (source_id, feed_id, epg_channel)
                             VALUES (?1, ?2, ?3)",
                        )?
                        .execute(params![source.0, feed, epg.epg_id])?;
                        let id = conn.last_insert_rowid();
                        meter.spend(1);
                        // The stage is held by the job: look its schedule up now.
                        match self.stage.get().channel(epg.source, &epg.epg_id) {
                            Some(staged) if !meter.unlimited() => {
                                self.phase = Phase::Binding {
                                    index,
                                    copy: Some(Transfer {
                                        channel: id,
                                        staged,
                                        after: None,
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
                                self.ids.insert(self.first[index], id);
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
        let store = self
            .store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut();
        replacement::completed(store, &mut self.copy_claim);
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
    // The first slice has no lower bound (a start can be any i64); later
    // ones continue after the last start, a seek on the stage's primary key
    // (ch, start), so each slice reads only what it copies.
    // The slice is a bounded run of program keys, for descriptions too
    // (most programs may have none: counting only described ones could scan
    // a whole schedule in one step), and the step pays for every key.
    let mut end = conn.prepare_cached(&slice_end_sql(copy.after.is_some()))?;
    let row = |r: &rusqlite::Row<'_>| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?));
    let (last, keys) = match copy.after {
        None => end.query_row(params![copy.staged.id, meter.limit()], row)?,
        Some(after) => end.query_row(params![copy.staged.id, meter.limit(), after], row)?,
    };
    drop(end);
    let Some(last) = last else {
        // A schedule without staged descriptions needs no description pass.
        if copy.descriptions || !copy.staged.described {
            copy.finished = true;
        } else {
            copy.descriptions = true;
            copy.after = None;
        }
        return Ok(copy);
    };
    let sql = slice_insert_sql(copy.descriptions, copy.after.is_some());
    let mut insert = conn.prepare_cached(&sql)?;
    let n = match copy.after {
        None => insert.execute(params![copy.channel, copy.staged.id, last])?,
        Some(after) => insert.execute(params![copy.channel, copy.staged.id, last, after])?,
    };
    if copy.descriptions {
        copy.descriptions_copied += n as u64;
    } else {
        copy.programs_copied += n as u64;
    }
    meter.spend(n.max(keys as usize).max(1));
    copy.after = Some(last);
    Ok(copy)
}

/// The last start of a schedule slice and how many program keys it spans:
/// `?1` the stage channel, `?2` the limit, `?3` the last start copied (when
/// continuing).
fn slice_end_sql(continuing: bool) -> String {
    let lower = if continuing { " AND start > ?3" } else { "" };
    format!(
        "SELECT max(start), count(*) FROM (SELECT start FROM epg_stage.programs
         WHERE ch = ?1{lower} ORDER BY start LIMIT ?2)"
    )
}

/// Copies a schedule slice: `?1` the channel, `?2` the stage channel, `?3`
/// the slice's last start, `?4` the last start copied (when continuing).
fn slice_insert_sql(descriptions: bool, continuing: bool) -> String {
    let lower = if continuing { " AND start > ?4" } else { "" };
    // `OR FAIL`, with foreign keys off: see `copy_programs`.
    if descriptions {
        format!(
            "INSERT OR FAIL INTO main.epg_descriptions (channel_id, start, description)
             SELECT ?1, start, description FROM epg_stage.programs
             WHERE ch = ?2 AND start <= ?3{lower} AND description IS NOT NULL
             ORDER BY start"
        )
    } else {
        format!(
            "INSERT OR FAIL INTO main.epg_programs (channel_id, start, stop, title, sub_title,
                 category, icon, rating, catchup_id, season, episode)
             SELECT ?1, start, stop, title, sub_title, category, icon, rating, catchup_id,
                 season, episode
             FROM epg_stage.programs WHERE ch = ?2 AND start <= ?3{lower}
             ORDER BY start"
        )
    }
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

/// The stage a job publishes from, borrowed or owned.
enum StageHold<'a> {
    Borrowed(&'a mut EpgStage),
    Owned(Box<EpgStage>),
}

impl StageHold<'_> {
    fn get(&self) -> &EpgStage {
        match self {
            Self::Borrowed(stage) => stage,
            Self::Owned(stage) => stage,
        }
    }

    fn get_mut(&mut self) -> &mut EpgStage {
        match self {
            Self::Borrowed(stage) => stage,
            Self::Owned(stage) => stage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A continuing slice seeks the stage's primary key past its last start
    /// instead of rescanning the channel's earlier programs.
    #[test]
    fn continuing_slices_seek_past_the_last_start() {
        let store = Store::open_in_memory().unwrap();
        store
            .conn
            .execute_batch(&format!(
                "ATTACH DATABASE ':memory:' AS epg_stage;
                 {}",
                crate::epg_stage::SCHEMA
                    .replace("CREATE TABLE programs", "CREATE TABLE epg_stage.programs")
            ))
            .unwrap();
        let plan = |sql: &str| -> String {
            let mut stmt = store
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(std::iter::repeat_n(0i64, stmt.parameter_count())),
                    |r| r.get::<_, String>(3),
                )
                .unwrap();
            rows.map(Result::unwrap).collect::<Vec<_>>().join("; ")
        };
        let end = plan(&slice_end_sql(true));
        assert!(end.contains("ch=? AND start>?"), "{end}");
        // The next old channel to clear: a seek in the source's part of the
        // unique index, not a walk over every source's channels.
        for sql in [FIRST_CHANNEL, NEXT_CHANNEL] {
            let next = plan(sql);
            assert!(
                next.contains("INDEX sqlite_autoindex_epg_channels_1 (source_id=?"),
                "{next}"
            );
            assert!(!next.contains("SCAN epg_channels"), "{next}");
            if sql == NEXT_CHANNEL {
                assert!(next.contains("epg_channel>?"), "{next}");
            }
        }
        // Measured, not only planned: finding the next of 5,000 channels of one
        // feed reads a few index entries, not the feed's earlier channels.
        store
            .conn
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000)
                 INSERT INTO epg_channels (source_id, feed_id, epg_channel)
                 SELECT 1, 1, printf('c%05d', i) FROM n;",
            )
            .unwrap();
        let mut next = store.conn.prepare(NEXT_CHANNEL).unwrap();
        let id: Option<i64> = next.query_row(params![1, 4000], |r| r.get(0)).unwrap();
        let id = id.unwrap();
        assert_eq!(id, 4001);
        let steps = next.get_status(rusqlite::StatementStatus::VmStep);
        assert!(steps < 200, "{steps} VM steps");
        for descriptions in [false, true] {
            let insert = plan(&slice_insert_sql(descriptions, true));
            assert!(insert.contains("ch=? AND start>? AND start<?"), "{insert}");
        }
    }
}
