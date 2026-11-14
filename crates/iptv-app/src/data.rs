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

pub(crate) mod assign;
pub(crate) mod combined;
pub(crate) use crate::runtime::guides;
pub use combined::switch_all;
mod library;
mod opening;
pub(crate) mod organization;
pub(crate) mod pages;
pub(crate) mod stored;
pub(crate) use crate::runtime::wizard_map;
pub(crate) use crate::runtime::workers;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::unbounded_channel;

use iptv_core::wizard::{FailureKind, ImportFailure};
use iptv_epg::GuideStore;
use iptv_guide::EpgSchedule;
use iptv_import::CancelToken;
use iptv_library::SourceId;
use nsh_core::prelude::*;

pub use library::{
    Activity, Library, Playlist, delete, edit_guides, playlists, refresh, refresh_all, rename,
    set_policy, set_user_agent, source_details, switch_to, try_details,
};
pub use wizard_map::source_from_draft;

use crate::Ui;

/// A development playlist the wizard's M3U form starts with, set when the
/// app is built (`IPTV_RS_DEV_PLAYLIST=http://10.0.2.2:8089/iptv/playlist.m3u`
/// for the mock server, `tools/mock-server.py`, from an emulator). Without
/// it, as in every release build, the link field starts empty.
pub use crate::runtime::imports::DEV_PLAYLIST;

/// The shortest time between two progress reports shown while an import
/// runs. Reports wake the UI themselves (an awaited channel, no polling);
/// a fast local import would otherwise repaint for each.
const PROGRESS_FRAME: Duration = Duration::from_millis(100);
/// Guide checks wait at most this long at a time (a sleeping device can
/// delay timers; an early check finds nothing due and waits again)...
const MAX_WAIT: Duration = Duration::from_secs(6 * 3600);
/// ...and at least this long.
const MIN_WAIT: Duration = Duration::from_secs(60);

use crate::runtime::imports::{
    GuideRead, Imported, Msg, Streams, guide_pipeline, now_unix, run, with_runtime,
};
pub use crate::runtime::imports::{Job, Purpose, Update};

thread_local! {
    /// The app's root owner: import tasks outlive the view that started them.
    static APP_OWNER: Cell<Option<Owner>> = const { Cell::new(None) };
}

/// Where the app keeps its files: the Android app's files directory,
/// `~/Library/Application Support/iptv-rs` on macOS and in the iOS app's
/// container, or
/// `~/.local/share/iptv-rs` on the other desktops.
pub fn data_dir() -> PathBuf {
    crate::runtime().data_dir().to_owned()
}

/// Runs `future` on the UI thread under the app's root owner.
pub fn spawn_in_app(future: impl Future<Output = ()> + 'static) {
    let owner = APP_OWNER.with(Cell::get).unwrap_or_else(Owner::current);
    let future = crate::in_runtime(future);
    owner.run(move || {
        // Dropping the handle detaches the task; it lives with the owner.
        drop(spawn(future));
    });
}

/// Startup: browse enabled playlists together when there is more than one,
/// otherwise refresh the active source or open the wizard.
pub fn start(ui: Ui) {
    APP_OWNER.with(|o| o.set(Some(Owner::current())));
    crate::runtime().bind_tasks(spawn_in_app, |duration| Box::pin(timer(duration)));
    crate::runtime().bind(move |event| crate::runtime_events::apply(ui, event));
    crate::runtime().start_services();
    // NSH drops cleanup callbacks without invoking them when the entire Rust
    // runtime exits. Capture an RAII guard so both owner disposal and runtime
    // destruction drain accepted writes while the engine thread still exists.
    struct Drain(Rc<crate::runtime::Runtime>);
    impl Drop for Drain {
        fn drop(&mut self) {
            crate::with_runtime(self.0.clone(), || {
                if !crate::runtime().flush_files(Duration::from_secs(5)) {
                    log::error!("iptv persistence: queued file writes did not finish at shutdown");
                }
                if !crate::favorites::flush_on_shutdown() {
                    log::error!(
                        "iptv persistence: queued favourite changes could not be saved at shutdown"
                    );
                }
                if !crate::runtime().drain_services() {
                    log::error!("iptv persistence: store did not drain at shutdown");
                }
            });
        }
    }
    let drain = Drain(crate::runtime());
    nsh_core::prelude::cleanup(move || drop(drain));
    // In the background the system may end the process without a teardown:
    // a zap or a settings change made just before must already be on disk.
    // Usually nothing is queued and this returns at once. The Android host
    // also waits for them in `onStop` (`TvActivity.flushPendingWrites`).
    effect(|| {
        let background = lifecycle() == nsh_core::platform::Lifecycle::Background;
        crate::runtime().foreground().set(!background);
        if background && !crate::runtime().flush_files(Duration::from_secs(2)) {
            log::warn!("iptv persistence: queued file writes did not finish in the background");
        }
    });
    APP_OWNER.with(|o| o.set(Some(Owner::current())));
    combined::reset_single();
    library::reset_imports();
    stored::bind(ui);
    crate::search_view::load_saved_history(ui);
    let started = std::time::Instant::now();
    let startup = crate::runtime::commands::startup_library();
    spawn_in_app(async move {
        let startup = startup.await.map(|(source, combined, any, snapshot)| {
            library::install(snapshot);
            (source, combined, any)
        });
        crate::startup::mark_with(
            "store opened",
            format_args!("{} ms", started.elapsed().as_millis()),
        );
        match startup {
            Ok((_, true, _)) => combined::start(ui),
            Ok((Some(stored), false, _)) => {
                opening::begin(ui, &stored).await;
                library::run(ui, stored, Purpose::Startup, false);
            }
            Ok((None, false, true)) => {
                show_nothing(ui);
                crate::runtime().emit(crate::runtime::RuntimeEvent::OpenPlaylists);
            }
            Ok((None, false, false)) => {
                crate::runtime().emit(crate::runtime::RuntimeEvent::FirstRun)
            }
            Err(error) => {
                log::warn!("iptv data: store unavailable ({error}); opening the wizard");
                crate::runtime().emit(crate::runtime::RuntimeEvent::FirstRun);
                crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
                    "The saved library could not be opened. Unlock the device and restart the app.",
                ));
            }
        }
        crate::runtime::library::start_scheduler();
    });
}

/// A new generation owns the guide: older imports no longer show and
/// their guide timers stop.
fn next_generation() -> u64 {
    crate::runtime().cancel_guide_checks();
    crate::runtime().generation().with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}

/// Imports `job` on its own thread. `on_update` hears progress (coalesced,
/// at most one per [`PROGRESS_FRAME`]) and then one [`Update::Done`]; on
/// success (unless the job is a [`Purpose::Background`] refresh) the guide
/// switches to the source at once and again when its guide data arrives.
/// Cancel with the returned token.
pub fn start_import(ui: Ui, job: Job, on_update: impl Fn(Update) + 'static) -> CancelToken {
    let cancel = CancelToken::new();
    start_import_with(ui, job, cancel.clone(), on_update);
    cancel
}

/// [`start_import`] with the caller's cancel token.
fn start_import_with(ui: Ui, job: Job, cancel: CancelToken, on_update: impl Fn(Update) + 'static) {
    let purpose = job.purpose;
    let mut import_source = job.stored;
    let had_check = import_source.is_some_and(|source| crate::runtime().has_guide_check(source));
    let completion = library::import_completion(cancel.clone());
    if let Some(source) = import_source {
        library::track_import(source, &completion);
    }
    let previous = Cell::new(
        (purpose == Purpose::Add)
            .then(combined::ready_single)
            .flatten(),
    );
    let shows = purpose.shows() && !combined::active(ui);
    let generation = if shows {
        next_generation()
    } else {
        crate::runtime().generation().with(Cell::get)
    };
    // Only the startup refresh may start the last channel.
    let resume = purpose == Purpose::Startup;
    let (tx, mut rx) = unbounded_channel();
    {
        let cancel = cancel.clone();
        let visible = job
            .stored
            .is_some_and(|source| source_is_visible(ui, source))
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
        let _ = library::refresh_cache(ui).await;
        if crate::runtime().generation().with(Cell::get) != generation {
            return;
        }
        if shows || combined::active(ui) {
            let merge = purpose == Purpose::Add
                && !combined::active(ui)
                && !merge_requested.get()
                && combined::has_multiple_enabled();
            let source = imported.source;
            show_part(ui, imported, generation, resume, held).await;
            if merge {
                merge_requested.set(true);
                combined::added(ui, source, generation, previous.take());
            }
        }
    };
    spawn_in_app(async move {
        let mut finished = false;
        let mut failed = false;
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
                            && crate::runtime().generation().with(Cell::get) == generation
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
                        failed = result.is_err();
                        on_update(Update::Done(result));
                    }
                    Ok(Msg::Guide(imported)) => {
                        if library::import_is_current(imported.source, &completion) {
                            show(imported, None).await;
                        }
                    }
                    Ok(Msg::FirstScreen(imported, held)) => show(imported, Some(held)).await,
                    Ok(Msg::NextCheck { source, at })
                        if (shows || combined::includes(ui, source))
                            && library::import_is_current(source, &completion) =>
                    {
                        schedule_check(ui, source, at, generation);
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
                    failed = true;
                    log::warn!("iptv data: import thread ended without a result");
                    on_update(Update::Done(Err(ImportFailure::new(FailureKind::Network))));
                }
                break;
            }
            if reported {
                // Progress comes as fast as a local file reads: repaint it
                // at most this often. No wait while nothing comes.
                timer(PROGRESS_FRAME).await;
            }
        }
        completion.finish();
        // A failed refresh may exit before producing NextCheck. Keep a bounded
        // retry for a retained source instead of losing its guide timer forever.
        if let Some(source) = import_source
            && failed
            && had_check
            && library::import_is_current(source, &completion)
            && !completion.cancel.is_cancelled()
            && !crate::runtime().has_guide_check(source)
            && (shows || combined::includes(ui, source))
        {
            schedule_check(
                ui,
                source,
                now_unix() + MIN_WAIT.as_secs() as i64,
                generation,
            );
        }
    });
}

/// Puts `imported` on screen unless a newer import owns the guide.
async fn show(ui: Ui, imported: Box<Imported>, generation: u64, resume: bool) {
    show_part(ui, imported, generation, resume, None).await;
}

/// [`show`]; with `held`, its guide holds only those programmes (a first
/// screen, `opening.rs`).
async fn show_part(
    ui: Ui,
    imported: Box<Imported>,
    generation: u64,
    resume: bool,
    held: Option<opening::Held>,
) {
    if crate::runtime().generation().with(Cell::get) != generation {
        return;
    }
    if combined::active(ui) {
        combined::show(ui, imported, held).await;
        if resume {
            combined::resume(ui);
        }
        return;
    }
    if Some(imported.source) != ui.library.shown.get_untracked() {
        source_changed(ui, Some(imported.source));
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
    if crate::runtime().generation().with(Cell::get) != generation || combined::active(ui) {
        return;
    }
    let built = started.elapsed();
    let mut schedule = match playback {
        Streams::New(context, allowed) => {
            crate::runtime().emit(crate::runtime::RuntimeEvent::AccountStreams(allowed));
            schedule.with_playback(context)
        }
        Streams::Current => {
            let current = crate::schedule();
            let shared = current
                .as_any()
                .and_then(|s| s.downcast_ref::<EpgSchedule>())
                .and_then(EpgSchedule::shared_playback);
            schedule.with_shared_playback(shared)
        }
    };
    if let Some(lookup) = stored::description_lookup(ui, source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    // Its guide is the stored one's recent part (`stored::load_guide`), or
    // only its first screen until that arrives.
    schedule = match held {
        Some(held) => schedule
            .held_only(held.from, held.to)
            .held_channels(held.keys),
        None => pages::paged(ui, schedule, source, now).await,
    };
    if crate::runtime().generation().with(Cell::get) != generation || combined::active(ui) {
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
    if crate::runtime().generation().with(Cell::get) != generation || combined::active(ui) {
        return;
    }
    let favorites = started.elapsed();
    install_favorites();
    let schedule = Rc::new(schedule);
    combined::remember_single(source, schedule.clone());
    crate::runtime().emit(crate::runtime::RuntimeEvent::ScheduleReady(schedule));
    pages::replaced();
    crate::startup::shown(&[
        ("schedule", built),
        ("pages", wrapped),
        ("favourites", favorites),
        ("swapped", started.elapsed()),
    ]);
    crate::runtime().emit(crate::runtime::RuntimeEvent::SourceReady(resume && resumes));
}

/// Another stored source (or none) is about to be shown: what played and
/// the favourites belong to the old one, and the choice is remembered.
fn source_changed(ui: Ui, source: Option<SourceId>) {
    log::info!("iptv data: showing source {:?}", source.map(|id| id.0));
    crate::runtime().emit(crate::runtime::RuntimeEvent::SourceChanged);
    crate::runtime().emit(crate::runtime::RuntimeEvent::FavoritesChanged {
        channels: BTreeSet::new(),
        has_vod: ui.has_vod.get_untracked(),
    });
    crate::runtime::library::set_shown(source);
    if let Some(id) = source {
        library::remember_shown(id);
    }
}

/// No stored source is left: the guide shows nothing real and no timers
/// run (the wizard's first-run state goes over it).
fn show_nothing(ui: Ui) {
    combined::reset_single();
    next_generation();
    source_changed(ui, None);
    crate::runtime().emit(crate::runtime::RuntimeEvent::ScheduleReady(Rc::new(
        EpgSchedule::new(Vec::new(), GuideStore::default(), now_unix()),
    )));
}

/// Looks at `source`'s guide again at `at` (unix time), unless a newer
/// import started meanwhile.
fn schedule_check(ui: Ui, source: SourceId, at: i64, generation: u64) {
    let Some((ticket, cancel)) = crate::runtime().schedule_guide_check(source, generation) else {
        return;
    };
    let wait =
        Duration::from_secs(u64::try_from(at - now_unix()).unwrap_or(0)).clamp(MIN_WAIT, MAX_WAIT);
    log::info!("iptv data: next guide check in {} min", wait.as_secs() / 60);
    spawn_in_app(async move {
        let mut elapsed = std::pin::pin!(timer(wait));
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
            && crate::runtime().guide_check_current(source, ticket)
            && !library::running(source)
            && crate::runtime().generation().with(Cell::get) == generation
            && (!combined::active(ui) || combined::includes(ui, source))
        {
            check_guide(ui, source, generation);
        }
    });
}

/// A guide check on its own thread: downloads the guide again when the
/// refresh policy says so, then schedules the next check.
fn check_guide(ui: Ui, source: SourceId, generation: u64) {
    spawn_in_app(read_guide(ui, source, generation, GuideRead::Check, None));
}

/// Reattach a retained source after changing generations. A first-screen
/// snapshot may still hold only a few channels/hours, so install the full
/// cached window even when the refresh policy does not request a download.
fn restore_guide(ui: Ui, source: SourceId, generation: u64) {
    spawn_in_app(read_guide(
        ui,
        source,
        generation,
        GuideRead::RestoreAndCheck,
        None,
    ));
}

/// Prefer the source being browsed, including the focused child of All playlists.
fn source_is_visible(ui: Ui, source: SourceId) -> bool {
    if ui.library.shown.get_untracked() == Some(source) {
        return true;
    }
    if !combined::active(ui) {
        return false;
    }
    crate::runtime_events::focused_channel(ui)
        .and_then(|channel| crate::schedule().channel_key(channel))
        .and_then(|key| combined::owner(ui, &key))
        .is_some_and(|(owner, _)| owner == source)
}

async fn read_guide(
    ui: Ui,
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
    let had_check = crate::runtime().has_guide_check(source);
    let Some((ticket, cancel)) = crate::runtime().schedule_guide_check(source, generation) else {
        return;
    };
    let (tx, mut rx) = unbounded_channel();
    if !workers::submit(source_is_visible(ui, source), move || {
        with_runtime(|| guide_pipeline(source, &tx, read, &cancel));
    }) {
        crate::runtime().cancel_guide_check(source);
        if had_check {
            schedule_check(
                ui,
                source,
                now_unix() + MIN_WAIT.as_secs() as i64,
                generation,
            );
        }
        crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
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
        if !crate::runtime().guide_check_current(source, ticket) {
            return;
        }
        match message {
            Some(Msg::Guide(imported)) => show(ui, imported, generation, false).await,
            Some(Msg::NextCheck { source, at }) => schedule_check(ui, source, at, generation),
            Some(_) => {}
            None => break,
        }
    }
    if crate::runtime().guide_check_current(source, ticket) {
        crate::runtime().cancel_guide_check(source);
    }
}

/// Runs `future` on a small tokio runtime on this thread; false when the
/// runtime cannot start.
pub use crate::runtime::imports::keep_picked_file;
#[cfg(test)]
use crate::runtime::imports::kept_file_name;

/// Current browser generation for async state installation guards.
pub(crate) fn generation() -> u64 {
    crate::runtime().generation().with(Cell::get)
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
