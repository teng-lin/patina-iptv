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
pub use crate::workers;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::future::Future;
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

use crate::imports::{
    GuideRead, Imported, Msg, Streams, guide_pipeline, now_unix, run, with_runtime,
};
pub use crate::imports::{Job, Purpose, Update};

pub fn spawn_in_app(future: impl Future<Output = ()> + 'static) {
    let owner = crate::current();
    owner.spawn(crate::in_runtime(future));
}
async fn delay(duration: Duration) {
    crate::current().delay(duration).await;
}
pub fn start() {
    combined::reset_single();
    library::reset_imports();
    let started = std::time::Instant::now();
    let startup = crate::commands::startup_library();
    spawn_in_app(async move {
        let startup = startup.await.map(|(source, combined, any, snapshot)| {
            library::install(snapshot);
            (source, combined, any)
        });
        crate::ports::mark_with(
            "store opened",
            format_args!("{} ms", started.elapsed().as_millis()),
        );
        match startup {
            Ok((_, true, _)) => combined::start(),
            Ok((Some(stored), false, _)) => {
                opening::begin(&stored).await;
                library::run(stored, Purpose::Startup, false);
            }
            Ok((None, false, true)) => {
                show_nothing();
                crate::current().emit(crate::RuntimeEvent::OpenPlaylists);
            }
            Ok((None, false, false)) => crate::current().emit(crate::RuntimeEvent::FirstRun),
            Err(error) => {
                log::warn!("iptv data: store unavailable ({error}); opening the wizard");
                crate::current().emit(crate::RuntimeEvent::FirstRun);
                crate::current().emit(crate::RuntimeEvent::Notify(
                    "The saved library could not be opened. Unlock the device and restart the app.",
                ));
            }
        }
        crate::library::start_scheduler();
    });
}

/// A new generation owns the guide: older imports no longer show and
/// their guide timers stop.
fn next_generation() -> u64 {
    crate::current().cancel_guide_checks();
    crate::current().generation.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}

/// Imports `job` on its own thread. `on_update` hears progress (coalesced,
/// at most one per [`PROGRESS_FRAME`]) and then one [`Update::Done`]; on
/// success (unless the job is a [`Purpose::Background`] refresh) the guide
/// switches to the source at once and again when its guide data arrives.
/// Cancel with the returned token.
pub fn start_import(job: Job, on_update: impl Fn(Update) + 'static) -> CancelToken {
    let cancel = CancelToken::new();
    start_import_with(job, cancel.clone(), on_update);
    cancel
}

/// [`start_import`] with the caller's cancel token.
fn start_import_with(job: Job, cancel: CancelToken, on_update: impl Fn(Update) + 'static) {
    let purpose = job.purpose;
    let mut import_source = job.stored;
    let completion = library::import_completion(cancel.clone());
    if let Some(source) = import_source {
        library::track_import(source, &completion);
    }
    let previous = Cell::new(
        (purpose == Purpose::Add)
            .then(combined::ready_single)
            .flatten(),
    );
    let shows = purpose.shows() && !combined::active();
    let generation = if shows {
        next_generation()
    } else {
        crate::current().generation.with(Cell::get)
    };
    // Only the startup refresh may start the last channel.
    let resume = purpose == Purpose::Startup;
    let (tx, mut rx) = unbounded_channel();
    {
        let cancel = cancel.clone();
        let visible = job.stored.is_some_and(source_is_visible)
            || matches!(
                purpose,
                Purpose::Add | Purpose::Startup | Purpose::Switch | Purpose::Refresh
            );
        spawn_in_app(async move {
            if job.purpose.shows_stored_first()
                && let Some(id) = job.stored
                && opening::send_stored(&job, id, now_unix(), &tx).await
            {
                return;
            }
            if cancel.is_cancelled() {
                return;
            }
            let failed = tx.clone();
            if !workers::submit(visible, move || run(job, &tx, &cancel)) {
                let _ = failed.send(Msg::Playlist(Err(ImportFailure::new(FailureKind::Busy))));
            }
        });
    }
    let merge_requested = Cell::new(false);
    let show = async move |imported: Box<Imported>, held: Option<opening::Held>| {
        let _ = library::refresh_cache().await;
        if crate::current().generation.with(Cell::get) != generation {
            return;
        }
        if shows || combined::active() {
            let merge = purpose == Purpose::Add
                && !combined::active()
                && !merge_requested.get()
                && combined::has_multiple_enabled();
            let source = imported.source;
            show_part(imported, generation, resume, held).await;
            if merge {
                merge_requested.set(true);
                combined::added(source, generation, previous.take());
            }
        }
    };
    spawn_in_app(async move {
        let mut finished = false;
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
                            && crate::current().generation.with(Cell::get) == generation
                            && let Ok((_, imported)) = &result
                        {
                            import_source = Some(imported.source);
                            library::track_added_import(imported.source, &completion);
                        }
                        if import_source
                            .is_some_and(|source| !library::import_is_current(source, &completion))
                        {
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
                        on_update(Update::Done(result));
                    }
                    Ok(Msg::Guide(imported)) => {
                        if library::import_is_current(imported.source, &completion) {
                            show(imported, None).await;
                        }
                    }
                    Ok(Msg::FirstScreen(imported, held)) => show(imported, Some(held)).await,
                    Ok(Msg::NextCheck { source, at })
                        if (shows || combined::includes(source))
                            && library::import_is_current(source, &completion) =>
                    {
                        schedule_check(source, at, generation);
                    }
                    Ok(Msg::NextCheck { .. }) => {}
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
                    log::warn!("iptv data: import thread ended without a result");
                    on_update(Update::Done(Err(ImportFailure::new(FailureKind::Network))));
                }
                break;
            }
            if reported {
                // Progress comes as fast as a local file reads: repaint it
                // at most this often. No wait while nothing comes.
                crate::data::delay(PROGRESS_FRAME).await;
            }
        }
        completion.finish();
        // A failed refresh may exit before producing NextCheck. Keep a bounded
        // retry for a retained source instead of losing its guide timer forever.
        if let Some(source) = import_source
            && library::import_is_current(source, &completion)
            && !completion.cancel.is_cancelled()
            && !crate::current().has_guide_check(source)
            && (shows || combined::includes(source))
        {
            schedule_check(source, now_unix() + MIN_WAIT.as_secs() as i64, generation);
        }
    });
}

/// Puts `imported` on screen unless a newer import owns the guide.
async fn show(imported: Box<Imported>, generation: u64, resume: bool) {
    show_part(imported, generation, resume, None).await;
}

/// [`show`]; with `held`, its guide holds only those programmes (a first
/// screen, `opening.rs`).
async fn show_part(
    imported: Box<Imported>,
    generation: u64,
    resume: bool,
    held: Option<opening::Held>,
) {
    if crate::current().generation.with(Cell::get) != generation {
        return;
    }
    if combined::active() {
        combined::show(imported, held).await;
        if resume {
            combined::resume();
        }
        return;
    }
    if Some(imported.source) != crate::current().shown.get() {
        source_changed(Some(imported.source));
    }
    stored::note_shown(Some(imported.source));
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
    let mut organization_revision = organization::revision();
    let schedule = organization::build(source, groups, catalog, guide, now).await;
    if crate::current().generation.with(Cell::get) != generation || combined::active() {
        return;
    }
    let built = started.elapsed();
    let mut schedule = match playback {
        Streams::New(context, allowed) => {
            crate::current().emit(crate::RuntimeEvent::AccountStreams(allowed));
            schedule.with_playback(context)
        }
        Streams::Current => {
            let current = crate::current().schedule();
            let shared = current
                .as_any()
                .and_then(|s| s.downcast_ref::<EpgSchedule>())
                .and_then(EpgSchedule::shared_playback);
            schedule.with_shared_playback(shared)
        }
    };
    if let Some(lookup) = stored::description_lookup(source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    // Its guide is the stored one's recent part (`stored::load_guide`), or
    // only its first screen until that arrives.
    schedule = match held {
        Some(held) => schedule
            .held_only(held.from, held.to)
            .held_channels(held.keys),
        None => pages::paged(schedule, source, now).await,
    };
    if crate::current().generation.with(Cell::get) != generation || combined::active() {
        return;
    }
    let wrapped = started.elapsed();
    log::info!("iptv data: guide switched to {channels} imported channels");
    let Some(install_favorites) = crate::favorites::prepare_source(Some(source)).await else {
        return;
    };
    while organization::revision() != organization_revision {
        organization_revision = organization::revision();
        if let Some(replacement) = organization::apply(&schedule, source).await {
            schedule = replacement;
        }
    }
    if crate::current().generation.with(Cell::get) != generation || combined::active() {
        return;
    }
    let favorites = started.elapsed();
    install_favorites();
    let schedule = Rc::new(schedule);
    combined::remember_single(source, schedule.clone());
    crate::current().emit(crate::RuntimeEvent::ScheduleReady(schedule));
    pages::replaced();
    crate::current().emit(crate::RuntimeEvent::SchedulePresented(vec![
        ("schedule", built),
        ("pages", wrapped),
        ("favourites", favorites),
        ("swapped", started.elapsed()),
    ]));
    crate::current().emit(crate::RuntimeEvent::SourceReady(resume && resumes));
}

/// Another stored source (or none) is about to be shown: what played and
/// the favourites belong to the old one, and the choice is remembered.
fn source_changed(source: Option<SourceId>) {
    log::info!("iptv data: showing source {:?}", source.map(|id| id.0));
    crate::current().emit(crate::RuntimeEvent::SourceChanged);
    crate::current().emit(crate::RuntimeEvent::FavoritesChanged {
        channels: BTreeSet::new(),
        has_vod: crate::current().browser().has_vod,
    });
    crate::library::set_shown(source);
    if let Some(id) = source {
        library::remember_shown(id);
    }
}

/// No stored source is left: the guide shows nothing real and no timers
/// run (the wizard's first-run state goes over it).
fn show_nothing() {
    combined::reset_single();
    next_generation();
    source_changed(None);
    crate::current().emit(crate::RuntimeEvent::ScheduleReady(Rc::new(
        EpgSchedule::new(Vec::new(), GuideStore::default(), now_unix()),
    )));
}

/// Looks at `source`'s guide again at `at` (unix time), unless a newer
/// import started meanwhile.
fn schedule_check(source: SourceId, at: i64, generation: u64) {
    let Some((ticket, cancel)) = crate::current().schedule_guide_check(source, generation) else {
        return;
    };
    let wait =
        Duration::from_secs(u64::try_from(at - now_unix()).unwrap_or(0)).clamp(MIN_WAIT, MAX_WAIT);
    log::info!("iptv data: next guide check in {} min", wait.as_secs() / 60);
    spawn_in_app(async move {
        let mut elapsed = std::pin::pin!(crate::data::delay(wait));
        let mut cancelled = std::pin::pin!(cancel.cancelled());
        let fired = std::future::poll_fn(|cx| {
            use std::future::Future;
            use std::task::Poll;
            if cancelled.as_mut().poll(cx).is_ready() {
                return Poll::Ready(false);
            }
            elapsed.as_mut().poll(cx).map(|_| true)
        })
        .await;
        if fired
            && crate::current().guide_check_current(source, ticket)
            && !library::running(source)
            && crate::current().generation.with(Cell::get) == generation
            && (!combined::active() || combined::includes(source))
        {
            check_guide(source, generation);
        }
    });
}

/// A guide check on its own thread: downloads the guide again when the
/// refresh policy says so, then schedules the next check.
fn check_guide(source: SourceId, generation: u64) {
    spawn_in_app(read_guide(source, generation, GuideRead::Check, None));
}

/// Reattach a retained source after changing generations. A first-screen
/// snapshot may still hold only a few channels/hours, so install the full
/// cached window even when the refresh policy does not request a download.
fn restore_guide(source: SourceId, generation: u64) {
    spawn_in_app(read_guide(
        source,
        generation,
        GuideRead::RestoreAndCheck,
        None,
    ));
}

/// Prefer the source being browsed, including the focused child of All playlists.
fn source_is_visible(source: SourceId) -> bool {
    if crate::current().shown.get() == Some(source) {
        return true;
    }
    if !combined::active() {
        return false;
    }
    crate::current()
        .browser()
        .focused_key
        .and_then(|key| combined::owner(&key))
        .is_some_and(|(owner, _)| owner == source)
}

async fn read_guide(
    source: SourceId,
    generation: u64,
    read: GuideRead,
    handoff: Option<library::ImportCompletion>,
) {
    let read = if library::running(source) {
        GuideRead::CacheOnly
    } else {
        read
    };
    let (ticket, cancel) = crate::current().replace_guide_check(source);
    let (tx, mut rx) = unbounded_channel();
    if !workers::submit(source_is_visible(source), move || {
        with_runtime(|| guide_pipeline(source, &tx, read, &cancel));
    }) {
        crate::current().cancel_guide_check(source);
        crate::current().emit(crate::RuntimeEvent::Notify(
            "Too many playlists are updating. Try again shortly.",
        ));
        return;
    }
    loop {
        if handoff
            .as_ref()
            .is_some_and(|job| !library::import_is_current(source, job))
        {
            return;
        }
        let message = rx.recv().await;
        if !crate::current().guide_check_current(source, ticket) {
            return;
        }
        match message {
            Some(Msg::Guide(imported)) => show(imported, generation, false).await,
            Some(Msg::NextCheck { source, at }) => schedule_check(source, at, generation),
            Some(_) => {}
            None => break,
        }
    }
    if crate::current().guide_check_current(source, ticket) {
        crate::current().cancel_guide_check(source);
    }
}

/// Runs `future` on a small tokio runtime on this thread; false when the
/// runtime cannot start.
pub use crate::imports::keep_picked_file;
#[cfg(test)]
use crate::imports::kept_file_name;

/// Current browser generation for async state installation guards.
pub fn generation() -> u64 {
    crate::current().generation.with(Cell::get)
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
