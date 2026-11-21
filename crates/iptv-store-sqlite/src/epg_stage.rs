//! A guide being downloaded, staged in its own SQLite file
//! ([`EpgStage`]): `iptv_import::import_epg` streams each feed's
//! programs into it in bounded batches, and [`crate::Store::replace_epg`]
//! copies the bound channels' schedules into the store in one transaction.
//!
//! The stage is a separate file so a long download never holds the
//! store's write lock, and so a failed or canceled download leaves the
//! stored guide untouched: dropping the stage deletes its file. The file
//! is scratch space (no journal, no fsync): a crash leaves a stale file
//! that [`EpgStage::remove_stale`] deletes at the next start.
//!
//! A writer thread owns the stage connection, so parsing and writing
//! overlap. Memory stays bounded: SQLite's page cache ([`CACHE_KIB`]), at
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
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use iptv_epg::{FeedId, IngestReport, ProgramData, ScheduleRules, StoreOptions, UnixTime};
use iptv_import::{GuideSink, SinkError};
use rusqlite::{Connection, OpenFlags};

use crate::error::{Result, StoreError};
use writer::{Batch, Cmd};

/// Page cache of the stage connection.
const CACHE_KIB: i64 = 4096;
/// Programs per batch (one stage transaction each)...
const BATCH: usize = 4096;
/// ...or fewer when their texts reach this many bytes.
const BATCH_TEXT: usize = 1 << 20;
/// Batches queued for the writer before the import waits for it.
const IN_FLIGHT: usize = 2;
const PREFIX: &str = "epg-stage-";

const SCHEMA: &str = "
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
}

/// The feed being received.
struct Current {
    feed: FeedId,
    rules: ScheduleRules,
    descriptions: bool,
    /// EPG channel id -> (stage channel id, programs admitted).
    channels: HashMap<Box<str>, (i64, u64)>,
    first: i64,
    report: IngestReport,
}

/// A loaded feed's staged channels (ids `first..end`).
struct Loaded {
    channels: HashMap<Box<str>, StagedChannel>,
    first: i64,
    end: i64,
}

/// A guide download staged on disk; see the module docs. Implements
/// [`GuideSink`]. Dropping it deletes the file.
pub struct EpgStage {
    path: PathBuf,
    tx: Option<SyncSender<Cmd>>,
    thread: Option<JoinHandle<()>>,
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
        let name = format!("{PREFIX}{}-{n}.sqlite", std::process::id());
        dir.join(name)
    }

    /// A new stage at `path` (an existing file there is replaced).
    pub fn create(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        remove(&path);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        iptv_sqlcipher_init::initialize()?;
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
        let (tx, rx): (SyncSender<Cmd>, Receiver<Cmd>) = mpsc::sync_channel(IN_FLIGHT);
        let failed = Arc::new(AtomicBool::new(false));
        let thread = {
            let failed = failed.clone();
            std::thread::Builder::new()
                .name("epg-stage".into())
                .spawn(move || writer::run(conn, rx, failed))
                .map_err(|e| StoreError::InvalidData(format!("guide stage thread: {e}")))?
        };
        Ok(Self {
            path,
            tx: Some(tx),
            thread: Some(thread),
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
        self.tx
            .as_ref()
            .and_then(|tx| tx.send(cmd).ok())
            .ok_or_else(|| StoreError::InvalidData("guide stage writer stopped".into()))
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
        let ch = match cur.channels.get_mut(channel) {
            Some((ch, admitted)) => {
                *admitted += 1;
                *ch
            }
            None => {
                let ch = self.next_channel;
                self.next_channel += 1;
                cur.channels.insert(channel.into(), (ch, 1));
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
        let mut channels: Vec<_> = cur
            .channels
            .drain()
            .map(|(id, (ch, admitted))| (id, ch, admitted))
            .collect();
        // In stage order: one pass through the file.
        channels.sort_by_key(|&(_, ch, _)| ch);
        let rules = cur.rules;
        let (staged, settled) = self.request(|reply| Cmd::Settle {
            channels,
            rules,
            reply,
        })?;
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
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
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
