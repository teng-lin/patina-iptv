//! A guide being downloaded, staged in its own SQLite file
//! ([`EpgStage`]): `iptv_import::import_epg` streams each feed's
//! programs into it in bounded batches, and [`crate::Store::replace_epg`]
//! copies the bound channels' schedules into the store in one transaction.
//!
//! The stage is a separate file so a long download never holds the
//! store's write lock, and so a failed or canceled download leaves the
//! stored guide untouched: dropping the stage deletes its file. The file
//! is scratch space (no journal, no fsync): a crash leaves a stale file
//! that [`EpgStage::remove_stale`] deletes at the next start. A browser
//! store's stage ([`EpgStage::create_in_store`]) is a scratch file among
//! its memory files instead, gone with its last connection.
//!
//! Natively a writer thread owns the stage connection, so parsing and
//! writing overlap; the browser's one thread writes in place. Memory stays
//! bounded: SQLite's page cache ([`CACHE_KIB`]), at
//! most [`IN_FLIGHT`] batches of at most [`BATCH`] programs or
//! [`BATCH_TEXT`] bytes of text on their way, and per feed a map of its
//! staged EPG channel ids; the guide itself is never held.
//!
//! Each feed is settled with [`iptv_epg::ScheduleRules`] as it finishes:
//! duplicates by start are resolved as rows arrive (an upsert keeping the
//! longest; a missing stop is shortest), then each channel's rows are read
//! in start order to fill missing stops, trim overlaps and drop what the
//! rules drop. The result is exactly the schedule
//! `iptv_epg::SourceBuilder` would build from the same programs.

mod writer;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;

use iptv_epg::{FeedId, IngestReport, ProgramData, ScheduleRules, StoreOptions, UnixTime};
use iptv_import::{GuideSink, SinkError};
use rusqlite::Connection;
#[cfg(not(target_family = "wasm"))]
use rusqlite::OpenFlags;

use crate::error::{Result, StoreError};
use writer::{Batch, Cmd, Writer};

/// Page cache of the stage connection.
const CACHE_KIB: i64 = 4096;
/// Programs per batch (one stage transaction each)...
const BATCH: usize = 4096;
/// ...or fewer when their texts reach this many bytes.
const BATCH_TEXT: usize = 1 << 20;
/// Batches queued for the writer before the import waits for it.
#[cfg(not(target_family = "wasm"))]
const IN_FLIGHT: usize = 2;
const PREFIX: &str = "epg-stage-";

pub(crate) const SCHEMA: &str = "
    CREATE TABLE programs (
        ch INTEGER NOT NULL,
        start INTEGER NOT NULL,
        stop INTEGER NOT NULL,
        title TEXT NOT NULL,
        sub_title TEXT,
        category TEXT,
        icon TEXT,
        rating TEXT,
        catchup_id TEXT,
        season INTEGER,
        episode INTEGER,
        description TEXT,
        PRIMARY KEY (ch, start)
    ) WITHOUT ROWID;";

/// One staged EPG channel of a loaded feed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StagedChannel {
    pub id: i64,
    /// Programs kept after settling.
    pub kept: u64,
    /// Latest kept stop.
    pub last_stop: Option<UnixTime>,
    /// Some staged program of it has a description: without one, a sliced
    /// copy skips the description pass.
    pub described: bool,
}

/// The feed being received.
struct Current {
    feed: FeedId,
    rules: ScheduleRules,
    descriptions: bool,
    /// EPG channel id -> (stage channel id, programs admitted, any of them
    /// described).
    channels: HashMap<Box<str>, (i64, u64, bool)>,
    first: i64,
    report: IngestReport,
}

/// A loaded feed's staged channels (ids `first..end`).
struct Loaded {
    channels: HashMap<Box<str>, StagedChannel>,
    first: i64,
    end: i64,
}

/// Where the stage's commands go.
enum Lane {
    /// The writer's own thread (native), behind a bounded queue.
    #[cfg(not(target_family = "wasm"))]
    Thread {
        tx: mpsc::SyncSender<Cmd>,
        thread: std::thread::JoinHandle<()>,
    },
    /// In place, on the caller's thread (the browser has one thread).
    #[cfg(target_family = "wasm")]
    Inline(std::sync::Mutex<Writer>),
}

/// A guide download staged in its own database; see the module docs.
/// Implements [`GuideSink`]. Dropping it deletes the file.
pub struct EpgStage {
    path: PathBuf,
    lane: Option<Lane>,
    /// The writer failed; the next request returns its error.
    failed: Arc<AtomicBool>,
    batch: Batch,
    current: Option<Current>,
    loaded: HashMap<FeedId, Loaded>,
    next_channel: i64,
}

impl std::fmt::Debug for EpgStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpgStage")
            .field("path", &self.path)
            .field("feeds", &self.loaded.len())
            .finish_non_exhaustive()
    }
}

impl EpgStage {
    /// A new stage in directory `dir` (next to the store), under a name no
    /// other stage of this process uses.
    pub fn create_in(dir: impl AsRef<Path>) -> Result<Self> {
        Self::create(Self::next_path(dir.as_ref()))
    }

    /// Stages guide data using the live store's key, including icon/stream URLs.
    pub fn create_in_store(store: &crate::Store, dir: impl AsRef<Path>) -> Result<Self> {
        let path = Self::next_path(dir.as_ref());
        let conn = store.open_staging_connection(&path)?;
        Self::create_with_connection(path, conn)
    }

    fn next_path(dir: &Path) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Natively the process id keeps names of concurrent processes apart;
        // a browser store's memory files belong to one instance.
        #[cfg(not(target_family = "wasm"))]
        let name = format!("{PREFIX}{}-{n}.sqlite", std::process::id());
        #[cfg(target_family = "wasm")]
        let name = format!("{PREFIX}{n}.sqlite");
        dir.join(name)
    }

    /// A new stage at `path` (an existing file there is replaced). The
    /// browser has no files: [`Self::create_in_store`] makes the stage among
    /// a browser store's memory files there, and this refuses.
    pub fn create(path: impl Into<PathBuf>) -> Result<Self> {
        #[cfg(target_family = "wasm")]
        {
            let _ = path.into();
            Err(StoreError::Unavailable(
                "the browser build stages through its store (create_in_store)",
            ))
        }
        #[cfg(not(target_family = "wasm"))]
        Self::create_file(path.into())
    }

    #[cfg(not(target_family = "wasm"))]
    fn create_file(path: PathBuf) -> Result<Self> {
        remove(&path);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        crate::initialize_sqlite()?;
        let conn = Connection::open_with_flags(&path, flags)?;
        Self::create_with_connection(path, conn)
    }

    fn create_with_connection(path: PathBuf, conn: Connection) -> Result<Self> {
        let opened = (|| -> Result<()> {
            // Scratch data: a crash deletes the file anyway.
            let _mode: String =
                conn.pragma_update_and_check(None, "journal_mode", "OFF", |r| r.get(0))?;
            conn.pragma_update(None, "synchronous", "OFF")?;
            conn.pragma_update(None, "cache_size", -CACHE_KIB)?;
            conn.execute_batch(SCHEMA)?;
            Ok(())
        })();
        if let Err(e) = opened {
            drop(conn);
            remove(&path);
            return Err(e);
        }
        let failed = Arc::new(AtomicBool::new(false));
        let writer = Writer::new(conn, failed.clone());
        #[cfg(not(target_family = "wasm"))]
        let lane = {
            let (tx, rx) = mpsc::sync_channel(IN_FLIGHT);
            let thread = std::thread::Builder::new()
                .name("epg-stage".into())
                .spawn(move || writer::run(writer, rx))
                .map_err(|e| StoreError::InvalidData(format!("guide stage thread: {e}")))?;
            Lane::Thread { tx, thread }
        };
        #[cfg(target_family = "wasm")]
        let lane = Lane::Inline(std::sync::Mutex::new(writer));
        Ok(Self {
            path,
            lane: Some(lane),
            failed,
            batch: Batch::default(),
            current: None,
            loaded: HashMap::new(),
            next_channel: 1,
        })
    }

    /// Deletes the stage files a crashed process left in `dir`. Call at
    /// start, before any import.
    pub fn remove_stale(dir: impl AsRef<Path>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(PREFIX) {
                remove(&entry.path());
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The staged channel `epg_id` of loaded feed `feed`.
    pub(crate) fn channel(&self, feed: FeedId, epg_id: &str) -> Option<StagedChannel> {
        self.loaded.get(&feed)?.channels.get(epg_id).copied()
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        let stopped = || StoreError::InvalidData("guide stage writer stopped".into());
        match self.lane.as_ref().ok_or_else(stopped)? {
            #[cfg(not(target_family = "wasm"))]
            Lane::Thread { tx, .. } => tx.send(cmd).map_err(|_| stopped()),
            #[cfg(target_family = "wasm")]
            Lane::Inline(writer) => {
                writer.lock().map_err(|_| stopped())?.handle(cmd);
                Ok(())
            }
        }
    }

    /// Sends `cmd` built around a reply channel and waits for the answer.
    fn request<T>(&self, cmd: impl FnOnce(mpsc::Sender<Result<T>>) -> Cmd) -> Result<T> {
        let (reply, answer) = mpsc::channel();
        self.send(cmd(reply))?;
        answer
            .recv()
            .map_err(|_| StoreError::InvalidData("guide stage writer stopped".into()))?
    }

    /// Hands the batch being built to the writer.
    fn send_batch(&mut self) -> Result<()> {
        if self.batch.len() > 0 {
            let batch = std::mem::take(&mut self.batch);
            self.send(Cmd::Write(batch))?;
        }
        Ok(())
    }

    /// Waits until everything sent is written; the writer's error, if any.
    pub(crate) fn flush(&mut self) -> Result<()> {
        self.send_batch()?;
        self.request(Cmd::Sync)
    }

    fn program_inner(&mut self, channel: &str, p: &ProgramData<'_>) -> Result<()> {
        if self.failed.load(Ordering::Relaxed) {
            return self.request(Cmd::Sync);
        }
        let Some(cur) = self.current.as_mut() else {
            return Err(StoreError::InvalidData("no guide feed started".into()));
        };
        if !cur.rules.admit(p.start, p.stop, &mut cur.report) {
            return Ok(());
        }
        let p = p.as_stored(cur.descriptions);
        let described = p.desc.is_some();
        let ch = match cur.channels.get_mut(channel) {
            Some((ch, admitted, any)) => {
                *admitted += 1;
                *any |= described;
                *ch
            }
            None => {
                let ch = self.next_channel;
                self.next_channel += 1;
                cur.channels.insert(channel.into(), (ch, 1, described));
                ch
            }
        };
        self.batch.push(ch, &p);
        if self.batch.len() >= BATCH || self.batch.text_bytes() >= BATCH_TEXT {
            self.send_batch()?;
        }
        Ok(())
    }

    fn finish_inner(&mut self) -> Result<IngestReport> {
        self.send_batch()?;
        let Some(mut cur) = self.current.take() else {
            return Err(StoreError::InvalidData("no guide feed started".into()));
        };
        let mut described = std::collections::HashSet::new();
        let mut channels: Vec<_> = cur
            .channels
            .drain()
            .map(|(id, (ch, admitted, any))| {
                if any {
                    described.insert(ch);
                }
                (id, ch, admitted)
            })
            .collect();
        // In stage order: one pass through the file.
        channels.sort_by_key(|&(_, ch, _)| ch);
        let rules = cur.rules;
        let (mut staged, settled) = self.request(|reply| Cmd::Settle {
            channels,
            rules,
            reply,
        })?;
        for channel in staged.values_mut() {
            channel.described = described.contains(&channel.id);
        }
        let r = &mut cur.report;
        r.kept += settled.kept;
        r.channels_with_programs += settled.channels_with_programs;
        r.dropped_bad_duration += settled.dropped_bad_duration;
        r.dropped_no_stop += settled.dropped_no_stop;
        r.dropped_retention += settled.dropped_retention;
        r.dropped_duplicate += settled.dropped_duplicate;
        r.overlaps_trimmed += settled.overlaps_trimmed;
        let loaded = Loaded {
            channels: staged,
            first: cur.first,
            end: self.next_channel,
        };
        if let Some(old) = self.loaded.insert(cur.feed, loaded) {
            // The same feed loaded twice: the later one replaces it.
            self.request(|reply| Cmd::Delete {
                first: old.first,
                end: old.end,
                reply,
            })?;
        }
        Ok(cur.report)
    }

    fn discard_inner(&mut self) -> Result<()> {
        self.batch = Batch::default();
        match self.current.take() {
            Some(cur) => {
                let end = self.next_channel;
                self.request(|reply| Cmd::Delete {
                    first: cur.first,
                    end,
                    reply,
                })
            }
            None => Ok(()),
        }
    }
}

impl Drop for EpgStage {
    fn drop(&mut self) {
        // Ends the writer, which closes the connection.
        match self.lane.take() {
            #[cfg(not(target_family = "wasm"))]
            Some(Lane::Thread { tx, thread }) => {
                drop(tx);
                let _ = thread.join();
            }
            #[cfg(target_family = "wasm")]
            Some(Lane::Inline(writer)) => drop(writer),
            None => {}
        }
        remove(&self.path);
    }
}

fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
    let mut journal = path.as_os_str().to_owned();
    journal.push("-journal");
    let _ = std::fs::remove_file(journal);
}

fn sink_error(e: StoreError) -> SinkError {
    SinkError(format!("guide stage: {e}"))
}

impl GuideSink for EpgStage {
    fn begin_feed(
        &mut self,
        feed: FeedId,
        _priority: i32,
        store: StoreOptions,
        now: UnixTime,
    ) -> std::result::Result<(), SinkError> {
        self.current = Some(Current {
            feed,
            rules: ScheduleRules::new(store, now),
            descriptions: store.store_descriptions,
            channels: HashMap::new(),
            first: self.next_channel,
            report: IngestReport::default(),
        });
        Ok(())
    }

    fn program(
        &mut self,
        channel: &str,
        p: &ProgramData<'_>,
    ) -> std::result::Result<(), SinkError> {
        self.program_inner(channel, p).map_err(sink_error)
    }

    fn finish_feed(&mut self) -> std::result::Result<IngestReport, SinkError> {
        self.finish_inner().map_err(sink_error)
    }

    fn discard_feed(&mut self) -> std::result::Result<(), SinkError> {
        self.discard_inner().map_err(sink_error)
    }

    fn has_programs(&self, feed: FeedId, epg_id: &str) -> bool {
        self.channel(feed, epg_id).is_some_and(|c| c.kept > 0)
    }
}
