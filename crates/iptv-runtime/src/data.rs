//! Real data: imports a source and its EPG off the UI thread, keeps it in
//! the SQLite store, and swaps the guide to the imported schedule.
//!
//! At startup enabled sources are browsed together when several are saved;
//! otherwise the stored source shown last (else the newest) is refreshed.
//! With none, the add-playlist wizard opens on its empty state (`wizard_view`).
//! The wizard adds sources
//! through [`start_import`]. A new source is stored only once its import
//! succeeded, so a failed or cancelled attempt leaves nothing behind. The
//! playlists screen switches, refreshes, renames and deletes stored
//! sources through [`library`].
//!
//! A stored source's saved channels and guide are on screen before its
//! refresh reaches the network, and stay when the refresh fails. Its guide
//! is downloaded again only when the refresh policy says so (`stored`).
//! Only the source on screen has guide timers: each shown import starts a
//! new generation, and timers of an older one stop.

pub mod assign;
pub mod combined;
pub use crate::guides;
pub use combined::switch_all;
pub mod library;
pub mod opening;
pub mod organization;
pub mod pages;
pub mod stored;
pub use crate::wizard_map;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::unbounded_channel;

use iptv_core::wizard::{FailureKind, ImportFailure};
use iptv_epg::GuideStore;
use iptv_guide::EpgSchedule;
use iptv_import::CancelToken;
use iptv_library::SourceId;

pub use library::{
    Activity, Playlist, delete, edit_guides, playlists, refresh, refresh_all, rename, set_policy,
    set_user_agent, source_details, switch_to, try_details,
};
pub use wizard_map::source_from_draft;

/// A development playlist the wizard's M3U form starts with, set when the
/// app is built (`IPTV_RS_DEV_PLAYLIST=http://10.0.2.2:8089/iptv/playlist.m3u`
/// for the mock server, `tools/mock-server.py`, from an emulator). Without
/// it, as in every release build, the link field starts empty.
pub use crate::imports::DEV_PLAYLIST;

/// The shortest time between two progress reports shown while an import
/// runs. Reports wake the UI themselves (an awaited channel, no polling);
/// a fast local import would otherwise repaint for each.
const PROGRESS_FRAME: Duration = Duration::from_millis(100);
/// Guide checks wait at most this long at a time (a sleeping device can
/// delay timers; an early check finds nothing due and waits again)...
const MAX_WAIT: Duration = Duration::from_secs(6 * 3600);
/// ...and at least this long.
const MIN_WAIT: Duration = Duration::from_secs(60);

use crate::GuideWork;
use crate::imports::{GuideRead, Imported, Msg, Streams, guide_pipeline, now_unix, run};
pub use crate::imports::{Job, Purpose, Update};

pub fn start(owner: &std::rc::Rc<crate::Runtime>) {
    combined::reset_single(owner);
    library::reset_imports(owner);
    let started = std::time::Instant::now();
    let startup = crate::commands::startup_library(owner);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let startup = startup.await.map(|(source, combined, any, snapshot)| {
                library::install(&owner, snapshot);
                (source, combined, any)
            });
            owner.services().mark(
                "store opened",
                format_args!("{} ms", started.elapsed().as_millis()),
            );
            match startup {
                Ok((_, true, _)) => combined::start(&owner),
                Ok((Some(stored), false, _)) => {
                    opening::begin(&owner, &stored).await;
                    library::run(&owner, stored, Purpose::Startup, false);
                }
                Ok((None, false, true)) => {
                    show_nothing(&owner);
                    owner.emit(crate::RuntimeEvent::OpenPlaylists);
                }
                Ok((None, false, false)) => owner.emit(crate::RuntimeEvent::FirstRun),
                Err(error) => {
                    log::warn!("iptv data: store unavailable ({error}); opening the wizard");
                    owner.emit(crate::RuntimeEvent::FirstRun);
                    owner.emit(crate::RuntimeEvent::Notify(
                    "The saved library could not be opened. Unlock the device and restart the app.",
                ));
                }
            }
            crate::library::start_scheduler(&owner);
        }
    });
}

/// A new generation owns the guide: older imports no longer show and
/// their guide timers stop. What `shown`, the playlist on screen, is owed
/// by then stays owed: the import that begins the generation may fail and
/// leave it there (`GuideChecks::clear`).
fn next_generation(owner: &std::rc::Rc<crate::Runtime>, shown: Option<SourceId>) -> u64 {
    owner.cancel_guide_checks_keeping(shown);
    owner.generation.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}

/// Imports `job` on its own thread. `on_update` hears progress (coalesced,
/// at most one per [`PROGRESS_FRAME`]) and then one [`Update::Done`]; on
/// success (unless the job is a [`Purpose::Background`] refresh) the guide
/// switches to the source at once and again when its guide data arrives.
/// Cancel with the returned token.
pub fn start_import(
    owner: &std::rc::Rc<crate::Runtime>,
    job: Job,
    on_update: impl Fn(Update) + 'static,
) -> CancelToken {
    let cancel = CancelToken::new();
    start_import_with(owner, job, cancel.clone(), on_update);
    cancel
}

/// [`start_import`] with the caller's cancel token.
fn start_import_with(
    owner: &std::rc::Rc<crate::Runtime>,
    job: Job,
    cancel: CancelToken,
    on_update: impl Fn(Update) + 'static,
) {
    let purpose = job.purpose;
    let mut import_source = job.stored;
    let completion = library::import_completion(cancel.clone());
    if let Some(source) = import_source {
        library::track_import(owner, source, &completion);
    }
    let previous = Cell::new(
        (purpose == Purpose::Add)
            .then(|| combined::ready_single(owner))
            .flatten(),
    );
    let shows = purpose.shows() && !combined::active(owner);
    let generation = if shows {
        next_generation(owner, owner.shown.get())
    } else {
        owner.generation.with(Cell::get)
    };
    // Only the startup refresh may start the last channel.
    let resume = purpose == Purpose::Startup;
    let (tx, mut rx) = unbounded_channel();
    {
        let cancel = cancel.clone();
        let revoked = completion.guide.clone();
        let visible = job
            .stored
            .is_some_and(|source| source_is_visible(owner, source))
            || matches!(
                purpose,
                Purpose::Add | Purpose::Startup | Purpose::Switch | Purpose::Refresh
            );
        owner.spawn({
            let owner = owner.clone();
            async move {
                let services = owner.services();
                if job.purpose.shows_stored_first()
                    && let Some(id) = job.stored
                    && opening::send_stored(&services, &job, id, now_unix(), &tx, &cancel, &revoked)
                        .await
                {
                    return;
                }
                if cancel.is_cancelled() {
                    return;
                }
                let failed = tx.clone();
                let worker = services.clone();
                if !services.submit(visible, move || run(&worker, job, &tx, &cancel, &revoked)) {
                    let _ = failed.send(Msg::Playlist(Err(ImportFailure::new(FailureKind::Busy))));
                }
            }
        });
    }
    let merge_requested = Cell::new(false);
    let job = completion.clone();
    let show = {
        let owner = owner.clone();
        async move |imported: Box<Imported>, held: Option<opening::Held>| {
            let _ = library::refresh_cache(&owner).await;
            if owner.generation.with(Cell::get) != generation {
                return;
            }
            if shows || combined::active(&owner) {
                let merge = purpose == Purpose::Add
                    && !combined::active(&owner)
                    && !merge_requested.get()
                    && combined::has_multiple_enabled(&owner);
                let source = imported.source;
                show_part(&owner, imported, generation, resume, held, Some(&job)).await;
                if merge {
                    merge_requested.set(true);
                    combined::added(&owner, source, generation, previous.take());
                }
            }
        }
    };
    owner.spawn({
        let owner = owner.clone();
        async move {
            let mut finished = false;
            let mut failed = false;
            // Whether this job set the source's next guide check, or settled
            // that the policy asks for none.
            let mut answered = false;
            // The next check this job asked for after its handoff was revoked.
            let mut unscheduled = None;
            loop {
                let mut progress = None;
                let mut open = true;
                // Sleeps until the import sends: its send wakes this task.
                let mut next = rx.recv().await.ok_or(TryRecvError::Disconnected);
                loop {
                    match next {
                        Ok(Msg::Progress(p)) => progress = Some(p),
                        Ok(Msg::Playlist(result)) => {
                            completion.playlist_done();
                            if import_source.is_none()
                                && owner.generation.with(Cell::get) == generation
                                && let Ok((_, imported)) = &result
                            {
                                import_source = Some(imported.source);
                                library::track_added_import(&owner, imported.source, &completion);
                            }
                            if import_source.is_some_and(|source| {
                                !library::import_is_current(&owner, source, &completion)
                            }) {
                                // A guide action can revoke handoff while authentication
                                // is still running. Keep its successful playback metadata,
                                // then cancel the superseded post-playlist guide phase.
                                completion.cancel_guide();
                            }
                            if let Some(p) = progress.take() {
                                on_update(Update::Progress(p));
                            }
                            finished = true;
                            let result = match result {
                                Ok((counts, imported)) => {
                                    show(imported, None).await;
                                    Ok(counts)
                                }
                                Err(error) => Err(error),
                            };
                            failed = result.is_err();
                            on_update(Update::Done(result));
                        }
                        Ok(Msg::Guide(imported)) => {
                            if library::import_is_current(&owner, imported.source, &completion) {
                                show(imported, None).await;
                            }
                        }
                        Ok(Msg::FirstScreen(imported, held)) => show(imported, Some(held)).await,
                        Ok(Msg::NextCheck { source, at })
                            if (shows || combined::includes(&owner, source))
                                && library::import_is_current(&owner, source, &completion) =>
                        {
                            // Not when a newer generation began meanwhile
                            // (an Add, a switch): should that fail and leave
                            // this playlist on screen, this is what it is owed.
                            if !schedule_check(&owner, source, at, generation) {
                                owner.owe_guide_check(source, at);
                            }
                            answered = true;
                        }
                        Ok(Msg::NextCheck { at, .. }) => unscheduled = Some(at),
                        Ok(Msg::NoCheck { source }) => {
                            if library::import_is_current(&owner, source, &completion) {
                                // Nothing is owed any more: the policy answered.
                                owner.take_owed_guide_check(source);
                                answered = true;
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            open = false;
                            break;
                        }
                    }
                    // What else came meanwhile, in one go (progress coalesces).
                    next = rx.try_recv();
                }
                let reported = progress.is_some();
                if let Some(p) = progress.filter(|_| !finished) {
                    on_update(Update::Progress(p));
                }
                if !open {
                    if !finished {
                        failed = true;
                        log::warn!("iptv data: import thread ended without a result");
                        on_update(Update::Done(Err(ImportFailure::new(FailureKind::Network))));
                    }
                    break;
                }
                if reported {
                    // Progress comes as fast as a local file reads: repaint it
                    // at most this often. No wait while nothing comes.
                    owner.delay(PROGRESS_FRAME).await;
                }
            }
            completion.finish();
            // This job cancelled the source's timer when it started and did
            // not answer for it: it failed, a guide action revoked its handoff
            // (its NextCheck was dropped), or it could not read the store.
            // Newer work that replaced it answers instead.
            if let Some(source) = import_source
                && !answered
                && (shows || combined::includes(&owner, source))
                && !completion.superseded.get()
            {
                keep_guide_check(&owner, source, generation, unscheduled);
            }
            // A shown job starts a generation, which ends every timer. When it
            // fails before showing its source (a wizard's Add, a switch), the
            // playlist that stays on screen would have none.
            if failed
                && shows
                && let Some(still_shown) = owner.shown.get()
                && Some(still_shown) != import_source
            {
                keep_shown_check(&owner, still_shown, generation);
            }
        }
    });
}

/// Puts `imported` on screen unless a newer import owns the guide.
async fn show(
    owner: &std::rc::Rc<crate::Runtime>,
    imported: Box<Imported>,
    generation: u64,
    resume: bool,
) {
    show_part(owner, imported, generation, resume, None, None).await;
}

/// [`show`]; with `held`, its guide holds only those programmes (a first
/// screen, `opening.rs`).
///
/// Shows of one source take turns in the order they began: each has
/// several waits, and one that began earlier (with older data) would
/// otherwise replace a later one when it happens to finish last. With
/// `job`, the import that read `imported`: when a guide action revoked it
/// meanwhile ("Delete stored guide data", a channel's guide chosen), the
/// guide it read may predate what that action deleted or matched anew.
/// Its catalogue and playback are still shown, with the guide as stored
/// now (read after that action's writes) in place of the one it read.
async fn show_part(
    owner: &std::rc::Rc<crate::Runtime>,
    mut imported: Box<Imported>,
    generation: u64,
    resume: bool,
    mut held: Option<opening::Held>,
    job: Option<&library::ImportCompletion>,
) {
    if owner.generation.with(Cell::get) != generation {
        return;
    }
    let _turn = owner.show_turn(imported.source).await;
    if owner.generation.with(Cell::get) != generation {
        return;
    }
    if let Some(job) = job
        && !library::import_is_current(owner, imported.source, job)
    {
        let stored = crate::commands::stored_guide(owner, imported.source, imported.now).await;
        // Unreadable: without programmes rather than with ones that may
        // just have been deleted, until the stored guide can be read.
        imported.guide = stored.unwrap_or_else(|error| {
            log::warn!("iptv data: the stored guide cannot be read: {error}");
            restore_guide(owner, imported.source, generation);
            GuideStore::default()
        });
        held = None;
        if owner.generation.with(Cell::get) != generation {
            return;
        }
    }
    if combined::active(owner) {
        combined::show(owner, imported, held).await;
        if resume {
            combined::resume(owner);
        }
        return;
    }
    if Some(imported.source) != owner.shown.get() {
        source_changed(owner, Some(imported.source));
    }
    stored::note_shown(owner, Some(imported.source));
    let channels = imported.channels.len();
    let started = std::time::Instant::now();
    let Imported {
        groups,
        channels: catalog,
        playback,
        guide,
        now,
        source,
        resume: resumes,
    } = *imported;
    let mut organization_revision = organization::revision(owner);
    let schedule = organization::build(owner, source, groups, catalog, guide, now).await;
    if owner.generation.with(Cell::get) != generation || combined::active(owner) {
        return;
    }
    let built = started.elapsed();
    let mut schedule = match playback {
        Streams::New(context, allowed) => {
            owner.emit(crate::RuntimeEvent::AccountStreams(allowed));
            schedule.with_playback(context)
        }
        Streams::Current => {
            let current = owner.schedule();
            let shared = current
                .as_any()
                .and_then(|s| s.downcast_ref::<EpgSchedule>())
                .and_then(EpgSchedule::shared_playback);
            schedule.with_shared_playback(shared)
        }
    };
    if let Some(lookup) = stored::description_lookup(owner, source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    // Its guide is the stored one's recent part (`stored::load_guide`), or
    // only its first screen until that arrives.
    schedule = match held {
        Some(held) => schedule
            .held_only(held.from, held.to)
            .held_channels(held.keys),
        None => pages::paged(owner, schedule, source, now).await,
    };
    if owner.generation.with(Cell::get) != generation || combined::active(owner) {
        return;
    }
    let wrapped = started.elapsed();
    log::info!("iptv data: guide switched to {channels} imported channels");
    let Some(install_favorites) = crate::favorites::prepare_source(owner, Some(source)).await
    else {
        return;
    };
    while organization::revision(owner) != organization_revision {
        organization_revision = organization::revision(owner);
        if let Some(replacement) = organization::apply(owner, &schedule, source).await {
            schedule = replacement;
        }
    }
    if owner.generation.with(Cell::get) != generation || combined::active(owner) {
        return;
    }
    let favorites = started.elapsed();
    install_favorites();
    let schedule = Rc::new(schedule);
    combined::remember_single(owner, source, schedule.clone());
    owner.emit(crate::RuntimeEvent::ScheduleReady(schedule));
    pages::replaced(owner);
    owner.emit(crate::RuntimeEvent::SchedulePresented(vec![
        ("schedule", built),
        ("pages", wrapped),
        ("favourites", favorites),
        ("swapped", started.elapsed()),
    ]));
    owner.emit(crate::RuntimeEvent::SourceReady(resume && resumes));
}

/// Another stored source (or none) is about to be shown: what played and
/// the favourites belong to the old one, and the choice is remembered.
fn source_changed(owner: &std::rc::Rc<crate::Runtime>, source: Option<SourceId>) {
    log::info!("iptv data: showing source {:?}", source.map(|id| id.0));
    owner.emit(crate::RuntimeEvent::SourceChanged);
    owner.emit(crate::RuntimeEvent::FavoritesChanged {
        channels: BTreeSet::new(),
        has_vod: owner.browser().has_vod,
    });
    crate::library::set_shown(owner, source);
    if let Some(id) = source {
        library::remember_shown(owner, id);
    }
}

/// No stored source is left: the guide shows nothing real and no timers
/// run (the wizard's first-run state goes over it).
fn show_nothing(owner: &std::rc::Rc<crate::Runtime>) {
    combined::reset_single(owner);
    next_generation(owner, None);
    source_changed(owner, None);
    owner.emit(crate::RuntimeEvent::ScheduleReady(Rc::new(
        EpgSchedule::new(Vec::new(), GuideStore::default(), now_unix()),
    )));
}

/// Work that cancelled `source`'s guide timer ended without answering for
/// it: keeps a check unless one is registered or newer work for the source
/// will set it. `asked` is the check the work asked for itself before its
/// handoff was revoked (or the time a guide action gives a check it
/// stopped).
///
/// Without one, with automatic updates the guide is looked at again
/// shortly: the policy is in memory, so an unavailable store cannot
/// suppress its own recovery. With updates on request only
/// (`epg.update_interval` = 0) the policy is not asked anew, which would
/// make up a check: only the one it had asked for before this work, and
/// which this work cancelled, is kept at its own time. The refresh policy
/// (`iptv_import::refresh`) sets such checks whatever the interval, for a
/// download that failed (rule 3, the back-off) or is still due (rule 4: a
/// guide never fetched or with changed feeds "is not an update").
fn keep_guide_check(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    asked: Option<i64>,
) {
    if owner.has_guide_check(source)
        || library::pending_import(owner, source).is_some()
        || !(owner.shown.get() == Some(source) || combined::includes(owner, source))
    {
        return;
    }
    if owner.generation.with(Cell::get) != generation {
        // A newer generation began meanwhile (an Add, a switch): no timer
        // can be set under this one, and what the playlist is owed stays
        // for when that import fails and leaves it on screen
        // (`keep_shown_check`), with what this work asked for.
        if let Some(at) = asked {
            owner.owe_guide_check(source, at);
        }
        return;
    }
    let owed = owner.take_owed_guide_check(source);
    let automatic = || {
        iptv_import::RefreshPolicy::from_settings(&owner.settings(), &iptv_settings::Target::Global)
            .interval
            .is_some()
    };
    match asked.or_else(|| owed.filter(|_| !automatic())) {
        Some(at) => {
            schedule_check(owner, source, at, generation);
        }
        None if automatic() => defer_guide_check(owner, source, generation),
        None => {}
    }
}

/// An import that started a generation failed and left `source` on
/// screen: it keeps a check ([`keep_guide_check`], with what it was owed
/// when that generation ended its timers: [`next_generation`]). Work of
/// its own that began under the older generation cannot set a timer in
/// this one, and what it answers decides what is owed: it is waited for.
/// A guide that work stored could not be shown under its generation
/// either, so the stored guide is read again.
fn keep_shown_check(owner: &std::rc::Rc<crate::Runtime>, source: SourceId, generation: u64) {
    owner.spawn({
        let owner = owner.clone();
        async move {
            while let Some(pending) = library::pending_import(&owner, source) {
                pending.until_finished().await;
            }
            if owner.generation.with(Cell::get) == generation {
                keep_guide_check(&owner, source, generation, None);
                // The read sets the timer kept just now again when it ends.
                read_guide(&owner, source, generation, GuideRead::CacheOnly, None, true).await;
            }
        }
    });
}

/// A check that was due cannot run yet (work on the source is under way,
/// or the workers are busy): it comes due again shortly, whatever the
/// refresh policy, since the check itself was already owed.
fn defer_guide_check(owner: &std::rc::Rc<crate::Runtime>, source: SourceId, generation: u64) {
    schedule_check(
        owner,
        source,
        now_unix() + MIN_WAIT.as_secs() as i64,
        generation,
    );
}

/// Looks at `source`'s guide again at `at` (unix time), unless a newer
/// import started meanwhile (false: no timer was set).
fn schedule_check(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    at: i64,
    generation: u64,
) -> bool {
    let work = GuideWork::Timer(at);
    let Some((ticket, cancel)) = owner.schedule_guide_work(source, generation, work) else {
        return false;
    };
    let wait =
        Duration::from_secs(u64::try_from(at - now_unix()).unwrap_or(0)).clamp(MIN_WAIT, MAX_WAIT);
    log::info!("iptv data: next guide check in {} min", wait.as_secs() / 60);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let mut elapsed = std::pin::pin!(owner.delay(wait));
            let mut cancelled = std::pin::pin!(cancel.cancelled());
            let fired = std::future::poll_fn(|cx| {
                use std::task::Poll;
                if cancelled.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(false);
                }
                elapsed.as_mut().poll(cx).map(|_| true)
            })
            .await;
            if !fired || !owner.guide_check_current(source, ticket) {
                return;
            }
            if owner.generation.with(Cell::get) != generation
                || (combined::active(&owner) && !combined::includes(&owner, source))
            {
                // No longer this timer's to check (the source left All
                // playlists): its entry must not outlive it.
                owner.end_guide_check(source);
            } else if library::running(&owner, source)
                || stored::updating(&owner, source)
                || library::pending_import(&owner, source).is_some()
            {
                // The work under way owns the guide: a check now would only
                // read the cache and end without a next check.
                defer_guide_check(&owner, source, generation);
            } else {
                check_guide(&owner, source, generation);
            }
        }
    });
    true
}

/// A guide check on its own thread: downloads the guide again when the
/// refresh policy says so, then schedules the next check.
fn check_guide(owner: &Rc<crate::Runtime>, source: SourceId, generation: u64) {
    let task = owner.clone();
    owner.spawn(async move {
        read_guide(&task, source, generation, GuideRead::Check, None, false).await;
    });
}

/// Restore the retained source's complete cache and then its guide policy.
fn restore_guide(owner: &Rc<crate::Runtime>, source: SourceId, generation: u64) {
    let task = owner.clone();
    owner.spawn(async move {
        read_guide(
            &task,
            source,
            generation,
            GuideRead::RestoreAndCheck,
            None,
            false,
        )
        .await;
    });
}

/// Prefer the source being browsed, including the focused child of All playlists.
fn source_is_visible(owner: &std::rc::Rc<crate::Runtime>, source: SourceId) -> bool {
    if owner.shown.get() == Some(source) {
        return true;
    }
    if !combined::active(owner) {
        return false;
    }
    owner
        .browser()
        .focused_key
        .and_then(|key| combined::owner(owner, &key))
        .is_some_and(|(owner, _)| owner == source)
}

/// Reads `source`'s stored guide for `read` on a worker and shows what it
/// sends. `handoff`: the job the read restores for, which ends it when a
/// guide action revokes that job. `quiet`: nobody asked for this read (a
/// recovery of the app's own), so busy workers are not reported.
async fn read_guide(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    read: GuideRead,
    handoff: Option<library::ImportCompletion>,
    quiet: bool,
) {
    let requested = read;
    let read = if library::running(owner, source) {
        GuideRead::CacheOnly
    } else {
        read
    };
    let cached = matches!(read, GuideRead::CacheOnly);
    let work = if cached {
        GuideWork::Cache
    } else {
        GuideWork::Check
    };
    let Some((ticket, cancel)) = owner.schedule_guide_work(source, generation, work) else {
        return;
    };
    // A read of the cache answers for no check: the timer it displaced is
    // set again when it ends.
    let restore_timer = || {
        if let Some(at) = owner.take_owed_guide_check(source) {
            schedule_check(owner, source, at, generation);
        }
    };
    let (tx, mut rx) = unbounded_channel();
    let services = owner.services();
    let worker = services.clone();
    if !services.submit(source_is_visible(owner, source), move || {
        worker.block_on(guide_pipeline(&worker, source, &tx, read, &cancel));
    }) {
        owner.end_guide_check(source);
        // What the caller asked for decides, not what running work reduced
        // it to: a read meant to check is owed one, and a background check
        // comes back quietly instead of repeating the message each time.
        if !matches!(requested, GuideRead::CacheOnly) {
            defer_guide_check(owner, source, generation);
        } else {
            restore_timer();
        }
        if !quiet && !matches!(requested, GuideRead::Check) {
            owner.emit(crate::RuntimeEvent::Notify(
                "Too many playlists are updating. Try again shortly.",
            ));
        }
        return;
    }
    // Whether the read said when to look again, or that the policy asks
    // for no check.
    let mut answered = false;
    loop {
        if handoff
            .as_ref()
            .is_some_and(|job| !library::import_is_current(owner, source, job))
        {
            // A guide action revoked the job this read restores for: end
            // the read with its entry, and keep the source a check.
            if owner.guide_check_current(source, ticket) {
                owner.end_guide_check(source);
                keep_guide_check(owner, source, generation, None);
            }
            return;
        }
        let message = rx.recv().await;
        if !owner.guide_check_current(source, ticket) {
            return;
        }
        match message {
            Some(Msg::Guide(imported)) => show(owner, imported, generation, false).await,
            Some(Msg::NextCheck { source, at }) => {
                schedule_check(owner, source, at, generation);
                answered = true;
            }
            Some(Msg::NoCheck { .. }) => answered = true,
            Some(_) => {}
            None => break,
        }
    }
    if owner.guide_check_current(source, ticket) {
        owner.end_guide_check(source);
        if cached && !matches!(requested, GuideRead::CacheOnly) {
            // Running work reduced a read that was to check the guide to
            // its cache: the check is still owed once that work finished.
            defer_guide_check(owner, source, generation);
        } else if cached {
            restore_timer();
        } else if !answered {
            // The check could not say when to look again (it could not
            // read the store, or its worker ended early): it was owed and
            // still is, later. Whatever the refresh policy: a check that
            // ran had been asked for.
            let later = now_unix() + stored::RETRY_DUE;
            schedule_check(owner, source, later, generation);
        }
    }
}

#[cfg(test)]
use crate::imports::kept_file_name;

/// Current browser generation for async state installation guards.
pub fn generation(owner: &crate::Runtime) -> u64 {
    owner.generation.get()
}

#[cfg(test)]
mod kept_file_tests {
    use super::kept_file_name;

    #[test]
    fn kept_playlist_names_are_portable() {
        assert_eq!(kept_file_name("my list (1).m3u"), "my_list__1_.m3u");
        assert_eq!(kept_file_name("..hidden.m3u"), "hidden.m3u");
        assert_eq!(kept_file_name("list."), "list");
        assert_eq!(kept_file_name("///"), "___");
        assert_eq!(kept_file_name(""), "playlist.m3u");
        for reserved in ["con.m3u", "NUL", "com1.m3u8", "Lpt9.txt"] {
            let kept = kept_file_name(reserved);
            assert_eq!(kept, format!("playlist-{reserved}"));
            assert!(
                iptv_import::backup_manifest::portable_component(&kept),
                "{kept}"
            );
        }
    }
}
