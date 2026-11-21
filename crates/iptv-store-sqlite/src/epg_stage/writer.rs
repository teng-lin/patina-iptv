//! The stage's own thread: it owns the stage connection and writes the
//! batches the import thread sends (so parsing and SQLite overlap), and
//! settles a feed's channels when asked.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use iptv_epg::{IngestReport, ProgramData, ScheduleRules, Settled, UnixTime};
use rusqlite::{Connection, params};

use super::StagedChannel;
use crate::error::{Result, StoreError};

/// A missing stop, as stored (sorts below every real stop).
const NO_STOP: i64 = i64::MIN;

/// Same start as a staged row: the longer one stays (equal: the first).
const UPSERT: &str = "
    INSERT INTO programmes (ch, start, stop, title, sub_title, category, icon, rating,
        catchup_id, season, episode, description)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
    ON CONFLICT (ch, start) DO UPDATE SET stop = excluded.stop, title = excluded.title,
        sub_title = excluded.sub_title, category = excluded.category, icon = excluded.icon,
        rating = excluded.rating, catchup_id = excluded.catchup_id, season = excluded.season,
        episode = excluded.episode, description = excluded.description
    WHERE excluded.stop > programmes.stop";

/// Text fields of a staged row, in [`Batch`] order.
const TEXTS: usize = 7;

/// One staged programme; texts are spans of [`Batch::text`].
struct Row {
    ch: i64,
    start: UnixTime,
    stop: i64,
    season: Option<u16>,
    episode: Option<u16>,
    /// title, sub-title, category, icon, rating, catch-up id, description.
    text: [Option<(u32, u32)>; TEXTS],
}

/// Programmes on their way to the writer: one string buffer for all
/// their texts.
#[derive(Default)]
pub(super) struct Batch {
    text: String,
    rows: Vec<Row>,
}

impl Batch {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn text_bytes(&self) -> usize {
        self.text.len()
    }

    /// Adds `p` (already [`ProgramData::as_stored`]) for stage channel
    /// `ch`.
    pub fn push(&mut self, ch: i64, p: &ProgramData<'_>) {
        let mut span = |t: Option<&str>| {
            t.map(|t| {
                let at = self.text.len() as u32;
                self.text.push_str(t);
                (at, t.len() as u32)
            })
        };
        let text = [
            span(Some(p.title)),
            span(p.sub_title),
            span(p.category),
            span(p.icon),
            span(p.rating),
            span(p.catchup_id),
            span(p.desc),
        ];
        self.rows.push(Row {
            ch,
            start: p.start,
            stop: p.stop.unwrap_or(NO_STOP),
            season: p.season,
            episode: p.episode,
            text,
        });
    }

    fn text(&self, row: &Row, i: usize) -> Option<&str> {
        row.text[i].map(|(at, len)| &self.text[at as usize..(at + len) as usize])
    }
}

/// A feed's staged channels: (EPG id, stage channel id, programmes
/// admitted).
pub(super) type FeedChannels = Vec<(Box<str>, i64, u64)>;

/// What a settled feed left: its channels and the counters of settling.
pub(super) type SettledFeed = (HashMap<Box<str>, StagedChannel>, IngestReport);

pub(super) enum Cmd {
    Write(Batch),
    Settle {
        channels: FeedChannels,
        rules: ScheduleRules,
        reply: Sender<Result<SettledFeed>>,
    },
    /// Deletes stage channels `first..end`.
    Delete {
        first: i64,
        end: i64,
        reply: Sender<Result<()>>,
    },
    /// Answers once everything sent before is written.
    Sync(Sender<Result<()>>),
}

/// The writer loop; ends when the stage is dropped. After a failed write
/// it sets `failed`, writes nothing more and answers the next request with
/// the error.
pub(super) fn run(conn: Connection, rx: Receiver<Cmd>, failed: Arc<AtomicBool>) {
    let mut error: Option<StoreError> = None;
    let fail = |e: StoreError, error: &mut Option<StoreError>| {
        failed.store(true, Ordering::Relaxed);
        error.get_or_insert(e);
    };
    let take = |error: &mut Option<StoreError>| match error.take() {
        Some(e) => Err(e),
        None if failed.load(Ordering::Relaxed) => {
            Err(StoreError::InvalidData("guide stage failed earlier".into()))
        }
        None => Ok(()),
    };
    for cmd in rx {
        match cmd {
            Cmd::Write(batch) => {
                if !failed.load(Ordering::Relaxed)
                    && let Err(e) = write(&conn, &batch)
                {
                    fail(e, &mut error);
                }
            }
            Cmd::Settle {
                channels,
                rules,
                reply,
            } => {
                let r = take(&mut error).and_then(|()| settle(&conn, channels, rules));
                if r.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                let _ = reply.send(r);
            }
            Cmd::Delete { first, end, reply } => {
                let r = take(&mut error).and_then(|()| {
                    conn.execute(
                        "DELETE FROM programmes WHERE ch >= ?1 AND ch < ?2",
                        params![first, end],
                    )?;
                    Ok(())
                });
                let _ = reply.send(r);
            }
            Cmd::Sync(reply) => {
                let _ = reply.send(take(&mut error));
            }
        }
    }
}

/// One transaction per batch. Never ROLLBACK: without a journal it is
/// undefined.
fn write(conn: &Connection, batch: &Batch) -> Result<()> {
    conn.execute_batch("BEGIN")?;
    let written = (|| {
        let mut upsert = conn.prepare_cached(UPSERT)?;
        for r in &batch.rows {
            let t = |i| batch.text(r, i);
            upsert.execute(params![
                r.ch,
                r.start,
                r.stop,
                t(0).unwrap_or_default(),
                t(1),
                t(2),
                t(3),
                t(4),
                t(5),
                r.season,
                r.episode,
                t(6),
            ])?;
        }
        Ok(())
    })();
    conn.execute_batch("COMMIT")?;
    written
}

/// Settles every channel of a feed (module docs of `epg_stage`).
fn settle(conn: &Connection, channels: FeedChannels, rules: ScheduleRules) -> Result<SettledFeed> {
    let mut report = IngestReport::default();
    let mut staged = HashMap::with_capacity(channels.len());
    let mut rows: Vec<(UnixTime, Option<UnixTime>)> = Vec::new();
    let mut changes: Vec<(UnixTime, Option<UnixTime>)> = Vec::new();
    conn.execute_batch("BEGIN")?;
    let settled = (|| {
        let mut select =
            conn.prepare_cached("SELECT start, stop FROM programmes WHERE ch = ?1 ORDER BY start")?;
        let mut update =
            conn.prepare_cached("UPDATE programmes SET stop = ?3 WHERE ch = ?1 AND start = ?2")?;
        let mut delete =
            conn.prepare_cached("DELETE FROM programmes WHERE ch = ?1 AND start = ?2")?;
        for (epg_id, ch, admitted) in channels {
            rows.clear();
            let mut q = select.query([ch])?;
            while let Some(r) = q.next()? {
                let stop: i64 = r.get(1)?;
                rows.push((r.get(0)?, (stop != NO_STOP).then_some(stop)));
            }
            drop(q);
            report.dropped_duplicate += admitted.saturating_sub(rows.len() as u64);
            changes.clear();
            let (mut kept, mut last_stop) = (0, None);
            rules.settle(&rows, &mut report, |i, fate| match fate {
                Settled::Keep { stop } => {
                    kept += 1;
                    last_stop = Some(stop);
                    if rows[i].1 != Some(stop) {
                        changes.push((rows[i].0, Some(stop)));
                    }
                }
                _ => changes.push((rows[i].0, None)),
            });
            for &(start, stop) in &changes {
                match stop {
                    Some(stop) => update.execute(params![ch, start, stop])?,
                    None => delete.execute(params![ch, start])?,
                };
            }
            let channel = StagedChannel {
                id: ch,
                kept,
                last_stop,
            };
            staged.insert(epg_id, channel);
        }
        Ok(())
    })();
    conn.execute_batch("COMMIT")?;
    settled.map(|()| (staged, report))
}
