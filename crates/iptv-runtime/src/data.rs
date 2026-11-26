//! Real data: imports a source and its EPG off the UI thread, keeps it in
//! the SQLite store, and swaps the guide to the imported schedule.
//!
//! At startup enabled sources are browsed together when several are saved;
//! otherwise the stored source shown last (else the newest) is refreshed.
//! With none, the add-playlist wizard opens on its empty state (`wizard_view`).
//! The wizard adds sources
//! through [`start_import`]. A new source is stored only once its import
//! succeeded, so a failed or canceled attempt leaves nothing behind. The
//! playlists screen switches, refreshes, renames and deletes stored
//! sources through this module. The library prepares imports and persists changes,
//! and this module applies the corresponding guide and browser transitions.
//!
//! A stored source's saved channels and guide are on screen before its
//! refresh reaches the network, and stay when the refresh fails. Its guide
//! is downloaded again only when the refresh policy says so (`stored`).
//! Only the source on screen has guide timers: each shown import starts a
//! new generation, and timers of an older one stop.

pub mod assign;
pub mod combined;
pub use crate::guides;
pub mod library;
pub mod opening;
mod organization;
pub mod pages;
pub mod stored;
pub use crate::wizard_map;

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::unbounded_channel;

use iptv_core::add_playlist::{FailureKind, ImportFailure};
use iptv_core::guide::Schedule;
use iptv_epg::GuideStore;
use iptv_guide::EpgSchedule;
use iptv_guide::combined::{CombinedSchedule, SourceSchedule};
use iptv_import::CancelToken;
use iptv_library::SourceId;
use iptv_providers::model::{Source, SourceConfig};

pub use library::{Activity, Playlist, playlists, source_details};
pub use wizard_map::source_from_draft;

/// The shortest time between two progress reports shown while an import
/// runs. Reports wake the UI themselves (an awaited channel, no polling);
/// a fast local import would otherwise repaint for each.
const PROGRESS_FRAME: Duration = Duration::from_millis(100);
/// Guide checks wait at most this long at a time (a sleeping device can
/// delay timers; an early check finds nothing due and waits again)...
const MAX_WAIT: Duration = Duration::from_secs(6 * 3600);
/// ...and at least this long.
const MIN_WAIT: Duration = Duration::from_secs(60);
/// Let multiple edits to the guide's feeds settle before checking them.
const GUIDE_EDIT_SETTLE: Duration = Duration::from_secs(2);

use crate::imports::{GuideRead, Imported, Msg, Streams, guide_pipeline, pipeline};
pub use crate::imports::{Job, Purpose, Update};
use crate::model::{StoreError, StoredSource};
use crate::state::SingleSchedule;
use crate::time::now_unix;
use crate::{GuideWork, Runtime, RuntimeEvent, ScheduleCause};

enum ScheduleUpdate<'a> {
    Source {
        source: SourceId,
        generation: u64,
        guide_revision: u64,
        schedule: Rc<EpgSchedule>,
    },
    Combined {
        generation: u64,
        revision: u64,
        sources: Vec<SourceSchedule>,
        collapsed: Vec<i64>,
    },
    Organization {
        source: SourceId,
        ticket: u64,
        expected: &'a EpgSchedule,
        schedule: Rc<EpgSchedule>,
    },
    /// Opening, an empty guide, or an expansion toggle leaves page waiters pending.
    Catalog(Rc<dyn Schedule>),
}

#[derive(Debug, Eq, PartialEq)]
enum CommitOutcome {
    Refused,
    Installed,
    Superseded,
}

/// Install and reconcile synchronously. Source imports and combined rebuilds
/// pass prepared favorites. The sink may read the new schedule or install
/// another one during delivery, so hold no state borrows across emits.
fn commit_schedule(
    owner: &Rc<Runtime>,
    update: ScheduleUpdate<'_>,
    install_favorites: impl FnOnce(),
) -> CommitOutcome {
    let catalog_changed = !matches!(update, ScheduleUpdate::Organization { .. });
    let wake_waiters = matches!(
        update,
        ScheduleUpdate::Source { .. } | ScheduleUpdate::Combined { .. }
    );
    let current: Rc<dyn Schedule> = match update {
        ScheduleUpdate::Source {
            source,
            generation,
            guide_revision,
            schedule,
        } => {
            if owner.generation.get() != generation
                || combined::active(owner)
                || crate::stored::guide_edit_revision(owner, source) != guide_revision
            {
                return CommitOutcome::Refused;
            }
            install_favorites();
            remember_single(owner, source, schedule.clone());
            schedule
        }
        ScheduleUpdate::Combined {
            generation,
            revision,
            sources,
            collapsed,
        } => {
            if !combined::active(owner)
                || owner.generation.get() != generation
                || owner.rebuild.get() != revision
            {
                return CommitOutcome::Refused;
            }
            install_favorites();
            let schedule =
                CombinedSchedule::new(sources, owner.epoch.get()).with_collapsed_sources(collapsed);
            Rc::new(schedule)
        }
        ScheduleUpdate::Organization {
            source,
            ticket,
            expected,
            schedule,
        } => {
            let actual = owner.schedule();
            let same_catalog = actual
                .as_any()
                .and_then(|schedule| schedule.downcast_ref::<EpgSchedule>())
                .is_some_and(|actual| expected.same_catalog(actual));
            if owner.shown.get() != Some(source)
                || !owner.organization_edit_current(source, ticket)
                || !same_catalog
            {
                return CommitOutcome::Refused;
            }
            remember_single(owner, source, schedule.clone());
            schedule
        }
        ScheduleUpdate::Catalog(schedule) => schedule,
    };
    let previous = owner.set_schedule(current.clone());
    let revision = owner.schedule_revision.get().wrapping_add(1);
    owner.schedule_revision.set(revision);
    let cause = if catalog_changed {
        crate::favorites::rebuild_index(owner, &*current);
        let (has_vod, any_vod) = crate::favorites::vod_availability(owner);
        ScheduleCause::Catalog { has_vod, any_vod }
    } else {
        ScheduleCause::Organization
    };
    owner.emit(RuntimeEvent::ScheduleChanged {
        previous,
        current,
        cause,
    });
    if catalog_changed && owner.schedule_revision.get() == revision {
        crate::favorites::publish(owner);
    }
    // A nested projection may supersede this commit without waking waiters.
    // Finish the replacement's wakeup even in that case.
    if wake_waiters {
        pages::replaced(owner);
    }
    if owner.schedule_revision.get() == revision {
        CommitOutcome::Installed
    } else {
        CommitOutcome::Superseded
    }
}

pub async fn set_policy(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
    policy: crate::model::SourcePolicy,
) -> Result<(), StoreError> {
    update_policy(owner, id, move |stored| *stored = policy).await
}

/// Edit the latest stored policy on the library writer, preserving other controls.
pub async fn update_policy(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
    edit: impl FnOnce(&mut crate::model::SourcePolicy) + Send + 'static,
) -> Result<(), StoreError> {
    let (policy, visibility_changed) = library::update_policy(owner, id, edit).await?;
    if !visibility_changed {
        return Ok(());
    }
    crate::favorites::refresh_vod_availability(owner).await;
    // The checkbox can be changed again while metadata and availability
    // reload. Only apply playback changes for the current visibility.
    if !library::policy(owner, id).is_ok_and(|current| current.enabled == policy.enabled) {
        return Ok(());
    }
    if combined::active(owner) {
        if !policy.enabled {
            crate::library::cancel_running(owner, id);
        }
        rebuild_combined(owner);
        if policy.enabled
            && let Some(source) = crate::library::load(owner, id)
        {
            run_import(owner, source, Purpose::Combined, false);
        }
        return Ok(());
    }
    if !policy.enabled {
        crate::library::cancel_running(owner, id);
        if owner.shown.get() == Some(id) {
            let next = crate::library::newest(owner)?;
            show_nothing(owner);
            match next {
                Some(next) => run_import(owner, next, Purpose::Switch, false),
                None => owner.emit(crate::RuntimeEvent::OpenPlaylists),
            }
        }
    }
    Ok(())
}

/// Shows stored playlist `id`: its stored channels and guide at once,
/// refreshed in the background as at startup.
pub fn switch_to(owner: &Rc<crate::Runtime>, id: SourceId) {
    owner.spawn({
        let owner = owner.clone();
        async move {
            if crate::favorites::flush(&owner).await {
                switch_to_ready(&owner, id);
            } else {
                owner.emit(crate::RuntimeEvent::Notify(
                    "Recent changes could not be saved; try again",
                ));
            }
        }
    });
}
fn switch_to_ready(owner: &Rc<crate::Runtime>, id: SourceId) {
    if library::policy(owner, id).is_ok_and(|p| !p.enabled) {
        return;
    }
    if !combined::active(owner) && owner.shown.get() == Some(id) {
        return;
    }
    let Some(stored) = crate::library::load(owner, id) else {
        return;
    };
    leave_combined(owner);
    // A refresh of it in the background gives way to the shown one.
    crate::library::replace_running(owner, id);
    run_import(owner, stored, Purpose::Switch, false);
}

/// Downloads stored playlist `id` again (unless that already runs): the
/// shown one in place, another one into the store only.
pub fn refresh(owner: &Rc<crate::Runtime>, id: SourceId) {
    if crate::library::running(owner, id) {
        return;
    }
    let Some(stored) = crate::library::load(owner, id) else {
        return;
    };
    run_import(owner, stored, refresh_purpose(owner, id), false);
}

/// The shown playlist refreshes in place, another one into the store only.
fn refresh_purpose(owner: &Rc<crate::Runtime>, id: SourceId) -> Purpose {
    if combined::active(owner) {
        Purpose::Background
    } else if owner.shown.get() == Some(id) {
        Purpose::Refresh
    } else {
        Purpose::Background
    }
}

/// Refreshes every stored playlist (each one not refreshing already).
pub fn refresh_all(owner: &Rc<crate::Runtime>) {
    match playlists(owner) {
        Ok(all) => all
            .iter()
            .filter(|p| p.policy.enabled)
            .for_each(|p| refresh(owner, p.id)),
        Err(e) => log::warn!("iptv data: the store is unavailable: {e}"),
    }
}

pub fn try_details(owner: &Rc<crate::Runtime>, id: SourceId, source: Source) {
    let Some(mut stored) = crate::library::load(owner, id) else {
        return;
    };
    // Only the details change; the name is the stored one.
    stored.source.config = source.config;
    crate::library::replace_running(owner, id);
    run_import(owner, stored, refresh_purpose(owner, id), true);
}

/// Sets the User-Agent of playlist `id` (`None`: the app's own), tried
/// like other new details.
pub fn set_user_agent(owner: &Rc<crate::Runtime>, id: SourceId, user_agent: Option<String>) {
    let Some(mut source) = source_details(owner, id) else {
        return;
    };
    match &mut source.config {
        SourceConfig::M3u(c) => c.user_agent = user_agent,
        SourceConfig::Xtream(c) => c.user_agent = user_agent,
        SourceConfig::Stalker(_) => return,
    }
    try_details(owner, id, source);
}

pub async fn rename(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
    name: &str,
) -> Result<(), StoreError> {
    library::rename(owner, id, name).await?;
    if combined::active(owner) {
        rebuild_combined(owner);
    }
    Ok(())
}

/// Puts the playlists in `order` (their hand order); lists showing
/// several follow it unless they sort by name.
pub async fn reorder(owner: &Rc<crate::Runtime>, order: Vec<SourceId>) -> Result<(), StoreError> {
    library::reorder(owner, order).await?;
    order_changed(owner);
    Ok(())
}

/// The playlists' order changed (arranged, or `playlists.sort`
/// switched): lists showing several follow.
pub fn order_changed(owner: &Rc<crate::Runtime>) {
    library::order_changed(owner);
    if combined::active(owner) {
        rebuild_combined(owner);
    }
}

/// Changes the guide sources of playlist `id`; false when `edit` changes
/// nothing. The guide on screen is checked again once the edits settle
/// (changed feeds are downloaded); another playlist's when it is shown.
pub async fn edit_guides(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
    edit: &guides::Edit,
) -> Result<bool, StoreError> {
    let edited = library::edit_guides(owner, id, edit).await?;
    if edited {
        guides_changed(owner, id);
    }
    Ok(edited)
}

/// The fallback the guide would show after deleting the current playlist.
pub fn next_playlist_after_deletion(owner: &Runtime, id: SourceId) -> Option<String> {
    crate::library::newest_excluding(owner, Some(id))
        .ok()
        .flatten()
        .map(|next| next.source.name)
}

/// Deletes stored playlist `id` with its channels, guide and favorites.
/// The shown one gives way to the newest other playlist, or to the
/// first-run state when none is left.
pub async fn delete(owner: &Rc<crate::Runtime>, id: SourceId) -> Result<(), StoreError> {
    let generation = generation(owner);
    if let Err(error) = library::delete(owner, id).await {
        keep_guide_check(owner, id, generation, None);
        return Err(error);
    }
    if combined::active(owner) {
        // Keep the explicitly selected aggregate usable when its last source
        // is deleted; the playlist screen still offers Add playlist.
        rebuild_combined(owner);
        return Ok(());
    }
    if playlists(owner)?.is_empty() {
        show_nothing(owner);
        owner.emit(crate::RuntimeEvent::ClosePlaylists);
        owner.emit(crate::RuntimeEvent::FirstRun);
        return Ok(());
    }
    if owner.shown.get() != Some(id) {
        return Ok(());
    }
    match crate::library::newest(owner)? {
        Some(next) => run_import(owner, next, Purpose::Switch, false),
        None => {
            show_nothing(owner);
            owner.emit(crate::RuntimeEvent::ClosePlaylists);
            if playlists(owner)?.is_empty() {
                owner.emit(crate::RuntimeEvent::FirstRun);
            } else {
                owner.emit(crate::RuntimeEvent::OpenPlaylists);
            }
        }
    }
    Ok(())
}

fn run_import(owner: &Rc<Runtime>, source: StoredSource, purpose: Purpose, edit: bool) {
    let (job, cancel, report) = library::prepare_import(owner, source, purpose, edit);
    start_import_with(owner, job, cancel, report);
}

pub fn refresh_guide_status(owner: &Rc<Runtime>) {
    let loaded = stored::load_status(owner);
    let owner = owner.clone();
    owner.clone().spawn(async move {
        if let Some(library_changed) = loaded.await {
            if library_changed && combined::active(&owner) {
                rebuild_combined(&owner);
            }
            owner.emit(RuntimeEvent::SettingsChanged);
        }
    });
}

/// Requeue the guide check interrupted by a clear, assignment, or repair.
pub(crate) fn keep_interrupted_check(
    owner: &Rc<Runtime>,
    source: SourceId,
    generation: u64,
    at: i64,
    work: Option<GuideWork>,
) {
    match work {
        Some(GuideWork::Check) => keep_guide_check(owner, source, generation, Some(at)),
        Some(_) => keep_guide_check(owner, source, generation, None),
        None => {}
    }
}

/// "Update the guide now" of the source on screen: see [`update_source_guide`].
pub fn update_guide_now(owner: &Rc<crate::Runtime>, done: impl FnOnce() + 'static) -> bool {
    let sources = crate::stored::shown_sources(owner);
    if sources.is_empty() {
        return false;
    }
    let remaining = Rc::new(Cell::new(sources.len()));
    let done = Rc::new(RefCell::new(Some(done)));
    let mut started = false;
    for source in sources {
        let left = remaining.clone();
        let complete = done.clone();
        let finish = move || {
            left.set(left.get() - 1);
            if left.get() == 0
                && let Some(done) = complete.borrow_mut().take()
            {
                done();
            }
        };
        if crate::stored::update_current(owner, source) {
            remaining.set(remaining.get() - 1);
            started = true;
            continue;
        }
        if update_source_guide(owner, source, move |_| finish()) {
            started = true;
        } else {
            remaining.set(remaining.get() - 1);
        }
    }
    if started
        && remaining.get() == 0
        && let Some(done) = done.borrow_mut().take()
    {
        done();
    }
    started
}

/// Downloads `source`'s guide now (every guide source switched on, with
/// the storage settings and guide choices of now); the guide on screen
/// changes when it is `source`'s. `done` runs on the UI thread when it
/// finished. False when there is no guide to download.
///
/// A request joins an update that runs already, unless a guide action
/// superseded that one: it then downloads nothing any more, and this
/// request starts an update of its own once that one has unwound.
pub fn update_source_guide(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    if crate::stored::update_current(owner, source) {
        return true;
    }
    start_update(owner, source, None, false, done)
}

/// [`update_source_guide`] for a change the download has to include (a
/// channel's guide choice just saved): an update that runs already read
/// the store before it, and is replaced instead of joined. `after`: work
/// that change canceled, which the download starts after.
fn restart_update(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    after: Option<crate::library::ImportCompletion>,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    start_update(owner, source, after, false, done)
}

pub(crate) fn restart_repair_update(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    after: Option<crate::library::ImportCompletion>,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    start_update(owner, source, after, true, done)
}

fn start_update(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    after: Option<crate::library::ImportCompletion>,
    force: bool,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    if !force && !stored::has_enabled_guides(owner, source) {
        return false;
    }
    // This update owns the source's guide from now on: work it replaces
    // is revoked. What that canceled, and an update a guide action
    // superseded before, only unwinds: this one starts after it.
    let mut unwinding = crate::stored::unwinding_updates(owner, source);
    unwinding.extend(crate::library::replace_import(owner, source));
    unwinding.extend(after);
    let cancel = CancelToken::new();
    let completion = crate::library::import_completion(cancel.clone());
    completion.playlist_done(); // This worker owns guide work only.
    crate::library::track_import(owner, source, &completion);
    crate::stored::begin_update(owner, source, &completion);
    let generation = owner.generation.with(Cell::get);
    owner.spawn({
        let owner = owner.clone();
        async move {
            for job in unwinding {
                job.until_finished().await;
            }
            let guides = stored::configured_guides(&owner, source).await;
            let (accepted, mut rx) = stored::download(&owner, source, guides, cancel, || {
                source_is_visible(&owner, source)
            });
            // Whether this update set the next check, or settled that the
            // policy asks for none.
            let mut answered = false;
            // The next check this update asked for after it was superseded.
            let mut unscheduled = None;
            loop {
                let shown = || {
                    owner.shown.with(Cell::get) == Some(source) || combined_includes(&owner, source)
                };
                let current = || crate::library::import_is_current(&owner, source, &completion);
                match rx.recv().await {
                    Some(Msg::Guide(imported)) if shown() && current() => {
                        show(&owner, imported, generation, false).await
                    }
                    Some(Msg::NextCheck { source, at }) if shown() && current() => {
                        // Not when a newer generation began meanwhile: should
                        // that fail and leave this playlist on screen, this
                        // is what it is owed.
                        if !schedule_check(&owner, source, at, generation) {
                            owner.owe_guide_check(source, at);
                        }
                        answered = true;
                    }
                    Some(Msg::NextCheck { at, .. }) => unscheduled = Some(at),
                    Some(Msg::NoCheck { .. }) if current() => {
                        owner.take_owed_guide_check(source);
                        answered = true;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            completion.finish();
            crate::stored::end_update(&owner, source, &completion);
            // This update canceled the source's timer and did not answer for
            // it: a guide action superseded it (its NextCheck was dropped),
            // the worker pool refused it, or the store could not be read.
            // Newer work that replaced it answers instead.
            if !answered && !completion.superseded.get() {
                keep_guide_check(&owner, source, generation, unscheduled);
            }
            owner.emit(crate::RuntimeEvent::LibraryChanged);
            done(accepted);
        }
    });
    true
}

/// `source`'s guide sources changed: its guide is checked once the edits
/// settle (the refresh policy downloads changed feeds). Only for the
/// source on screen; another one is checked when it is shown.
fn guides_changed(owner: &std::rc::Rc<crate::Runtime>, source: SourceId) {
    let previous = crate::library::pending_import(owner, source);
    crate::library::revoke_import_handoff(owner, source);
    if owner.shown.with(Cell::get) != Some(source) && !combined_includes(owner, source) {
        return;
    }
    // A check under way would download and store the feeds as they were.
    // The check below answers for it, or the update that runs instead.
    owner.interrupt_guide_read(source);
    let edit = crate::stored::next_guide_edit(owner, source);
    let generation = owner.generation.with(Cell::get);
    owner.spawn({
        let owner = owner.clone();
        async move {
            owner.delay(GUIDE_EDIT_SETTLE).await;
            // A refresh may still be installing its old guide configuration. Wait
            // for that handoff before checking the edited feeds, rather than turning
            // this check into a cache-only read and losing the edit's refresh.
            if let Some(previous) = previous {
                previous.until_finished().await;
            }
            while let Some(pending) = crate::library::pending_import(&owner, source) {
                pending.until_finished().await;
            }
            let current = owner.generation.with(Cell::get) == generation
                && crate::stored::guide_edit_is_current(&owner, source, edit);
            if current && !crate::stored::updating(&owner, source) {
                log::info!("iptv data: guide sources changed; checking the guide");
                check_guide(&owner, source, generation);
            }
        }
    });
}

/// "Delete stored guide data": the source on screen keeps its channels
/// without programs until its guide is downloaded again (at the next
/// start, refresh, or "Update the guide now"). The store works off the UI
/// thread; `done` hears whether the guide was deleted. False when no
/// playlist is on screen.
pub fn clear_guide(owner: &std::rc::Rc<crate::Runtime>, done: impl FnOnce(bool) + 'static) -> bool {
    clear_sources(owner, crate::stored::shown_sources(owner), done)
}

/// Delete one playlist’s stored guide, independent of the combined view.
/// Source links, channel lists and other playlists’ programs are retained.
pub fn clear_source_guide(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    clear_sources(owner, vec![source], done)
}

fn clear_sources(
    owner: &std::rc::Rc<crate::Runtime>,
    sources: Vec<SourceId>,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    if sources.is_empty() {
        return false;
    }
    // Work under way would show the guide again or store it anew.
    let superseded: Vec<_> = sources
        .iter()
        .map(|source| (*source, stored::supersede(owner, *source)))
        .collect();
    let generation = owner.generation.with(Cell::get);
    let saved = crate::commands::clear_guides(owner, sources);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let outcome = saved.await;
            // A check stopped for it stays owed, as after a download that
            // was canceled: still due, later, not at once.
            let later = crate::time::now_unix() + crate::stored::RETRY_DUE;
            for (source, superseded) in superseded {
                keep_interrupted_check(&owner, source, generation, later, superseded.read);
            }
            let Ok((deleted, imports)) = outcome else {
                return done(false);
            };
            for imported in imports {
                // Clearing an unshown playlist must not navigate to it. In
                // All playlists every enabled child still needs its empty guide.
                if owner.shown_source() == Some(imported.source)
                    || combined_includes(&owner, imported.source)
                {
                    show(&owner, Box::new(imported), generation, false).await;
                }
            }
            owner.emit(crate::RuntimeEvent::LibraryChanged);
            done(deleted);
        }
    });
    true
}

fn remember_single(owner: &Rc<Runtime>, source: SourceId, schedule: Rc<EpgSchedule>) {
    combined::remember_single(
        owner,
        source,
        schedule,
        crate::library::import_revision(owner, source),
    );
}

/// Snapshot before Add advances the generation. An unfinished account refresh
/// can still replace a cached source's provisional playback metadata, so that
/// source must go through the normal combined loader instead of being retained.
fn ready_single(owner: &Rc<crate::Runtime>) -> Option<SingleSchedule> {
    combined::single(owner)
        .filter(|s| !crate::library::running(owner, s.source) && s.schedule.playback().is_some())
        .map(|mut s| {
            s.pending = crate::library::pending_import(owner, s.source);
            s.revision = crate::library::import_revision(owner, s.source);
            s
        })
}
/// Add already owns the current generation. Keep that generation so its guide
/// download and future guide checks continue after the browser joins sources.
fn join_after_add(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    previous: Option<SingleSchedule>,
) {
    owner.spawn({
        let owner = owner.clone();
        async move {
            if !crate::favorites::flush(&owner).await {
                owner.emit(crate::RuntimeEvent::Notify(
                    "Recent changes could not be saved; try again",
                ));
                return;
            }
            if combined::active(&owner)
                || owner.generation.with(Cell::get) != generation
                || !has_multiple_enabled(&owner)
            {
                return;
            }
            // Guide messages may have arrived while writes flushed: take the latest
            // schedule, rather than the first channel-only Add result.
            let Some(added) = combined::single(&owner).filter(|s| s.source == source) else {
                return;
            };
            if let Err(e) = crate::library::save_all(&owner, true).await {
                log::warn!("iptv combined: cannot save browsing mode: {e}");
                return;
            }
            if owner.generation.with(Cell::get) != generation || combined::active(&owner) {
                return;
            }
            let Ok(playlists) = library::playlists(&owner) else {
                return;
            };
            let enabled = |id| playlists.iter().any(|p| p.id == id && p.policy.enabled);
            source_changed(&owner, None);
            crate::stored::note_shown(&owner, None);
            crate::library::set_all(&owner, true);
            combined::clear_parts(&owner);
            let previous = previous.filter(|s| {
                enabled(s.source)
                    && crate::library::import_revision_is_current(&owner, s.source, &s.revision)
            });
            let previous_guide = previous.as_ref().map(|s| (s.source, s.pending.clone()));
            for kept in previous.into_iter().chain(std::iter::once(added)) {
                combined::retain_single(&owner, kept);
            }
            combined::reset_single(&owner);
            owner.emit(crate::RuntimeEvent::AccountStreams(None));
            combined::reset_epoch(&owner, now_unix());
            rebuild_combined(&owner);
            // The previous single source's timer belonged to its older generation.
            // Reattach guide work without fetching its playlist/account again.
            if let Some((previous, pending)) = previous_guide.filter(|(id, _)| *id != source) {
                reattach_guide(&owner, previous, generation, pending);
            }
            for playlist in playlists.into_iter().filter(|p| p.policy.enabled) {
                if combined::part(&owner, playlist.id).is_none()
                    && let Some(stored) = crate::library::load(&owner, playlist.id)
                {
                    run_import(&owner, stored, Purpose::Combined, false);
                }
            }
        }
    });
}

/// A previous source can finish its original EPG download after Add has moved
/// the browser to a new generation. Keep the worker identity until its final
/// cache is restored; newer imports and explicit guide changes revoke it.
fn reattach_guide(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    pending: Option<crate::library::ImportCompletion>,
) {
    let Some(pending) = pending else {
        restore_guide(owner, source, generation);
        return;
    };
    owner.spawn({
        let owner = owner.clone();
        async move {
            let current = || {
                owner.generation.with(Cell::get) == generation
                    && combined_includes(&owner, source)
                    && crate::library::import_is_current(&owner, source, &pending)
            };
            if !current() {
                return;
            }
            // Complete this initial restore before the final one so an older cache
            // read cannot arrive after the original download's successful result.
            read_guide(
                &owner,
                source,
                generation,
                GuideRead::CacheOnly,
                Some(pending.clone()),
                false,
            )
            .await;
            if current() {
                pending.until_finished().await;
            }
            if current() {
                read_guide(
                    &owner,
                    source,
                    generation,
                    GuideRead::RestoreAndCheck,
                    Some(pending),
                    false,
                )
                .await;
            }
        }
    });
}

/// Whether the combined guide may include the enabled playlist.
pub fn combined_includes(owner: &Rc<crate::Runtime>, id: SourceId) -> bool {
    combined::active(owner) && library::policy(owner, id).is_ok_and(|p| p.enabled)
}
/// The normal browser includes every enabled source once there is a choice.
pub fn has_multiple_enabled(owner: &Rc<crate::Runtime>) -> bool {
    library::playlists(owner)
        .is_ok_and(|sources| sources.iter().filter(|s| s.policy.enabled).take(2).count() == 2)
}

fn start_combined(owner: &Rc<crate::Runtime>) {
    owner.resume.with(|r| r.set(true));
    // Startup precedes user edits; install the empty real guide at once.
    owner.spawn({
        let owner = owner.clone();
        async move {
            switch_all_ready(&owner).await;
        }
    });
}

pub fn switch_all(owner: &Rc<crate::Runtime>) {
    owner.spawn({
        let owner = owner.clone();
        async move {
            if crate::favorites::flush(&owner).await {
                switch_all_ready(&owner).await;
            } else {
                owner.emit(crate::RuntimeEvent::Notify(
                    "Recent changes could not be saved; try again",
                ));
            }
        }
    });
}
async fn switch_all_ready(owner: &Rc<crate::Runtime>) {
    let generation = owner.generation.with(Cell::get);
    if combined::active(owner) {
        return;
    }
    if let Err(e) = crate::library::save_all(owner, true).await {
        log::warn!("iptv combined: cannot save browsing mode: {e}");
        return;
    }
    if owner.generation.with(Cell::get) != generation || combined::active(owner) {
        return;
    }
    next_generation(owner, None);
    combined::reset_single(owner);
    source_changed(owner, None);
    crate::stored::note_shown(owner, None);
    crate::library::set_all(owner, true);
    combined::clear_parts(owner);
    owner.emit(crate::RuntimeEvent::AccountStreams(None));
    combined::reset_epoch(owner, now_unix());
    rebuild_combined(owner);
    if let Ok(playlists) = library::playlists(owner) {
        for playlist in playlists
            .into_iter()
            .filter(|playlist| playlist.policy.enabled)
        {
            if let Some(source) = crate::library::load(owner, playlist.id) {
                run_import(owner, source, Purpose::Combined, false);
            }
        }
    }
}

fn leave_combined(owner: &Rc<crate::Runtime>) {
    combined::reset_single(owner);
    owner.resume.with(|r| r.set(false));
    crate::library::set_all(owner, false);
    combined::clear_parts(owner);
    let saved = crate::library::save_all(owner, false);
    owner.spawn(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv combined: cannot save browsing mode: {error}");
        }
    });
}

async fn show_combined(
    owner: &Rc<crate::Runtime>,
    imported: Box<Imported>,
    held: Option<opening::Held>,
) {
    if !combined_includes(owner, imported.source) {
        return;
    }
    let source = imported.source;
    let generation = owner.generation.with(Cell::get);
    let revision = crate::library::import_revision(owner, source);
    let guide_revision = crate::stored::guide_edit_revision(owner, source);
    let current = || {
        combined_includes(owner, source)
            && owner.generation.with(Cell::get) == generation
            && crate::library::import_revision_is_current(owner, source, &revision)
            && crate::stored::guide_edit_revision(owner, source) == guide_revision
    };
    let Imported {
        groups,
        channels,
        playback,
        guide,
        now,
        resume: can_resume,
        ..
    } = *imported;
    let mut organization_revision = organization::revision(owner);
    let mut schedule = organization::build(owner, source, groups, channels, guide, now).await;
    if !current() {
        return;
    }
    schedule = match playback {
        Streams::New(context, allowed, authority) => {
            combined::note_account_limit(owner, source, allowed);
            schedule
                .with_playback(context)
                .with_source_authority(Some(authority))
        }
        Streams::Current => {
            let (context, authority) = combined::part(owner, source)
                .map(|s| (s.shared_playback(), s.source_authority()))
                .unwrap_or_default();
            schedule
                .with_shared_playback(context)
                .with_source_authority(authority)
        }
    };
    if let Some(lookup) = stored::description_lookup(owner, source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    schedule = match held {
        Some(h) => schedule.held_only(h.from, h.to).held_channels(h.keys),
        None => pages::paged(owner, schedule, source, now).await,
    };
    while current() && organization::revision(owner) != organization_revision {
        organization_revision = organization::revision(owner);
        if let Some(replacement) = organization::apply(owner, &schedule, source).await {
            schedule = replacement;
        }
    }
    if !current() {
        return;
    }
    combined::store_part(owner, source, Rc::new(schedule));
    rebuild_combined_ready(owner).await;
    if can_resume && owner.resume.with(Cell::get) {
        resume_combined(owner);
    }
}

fn rebuild_combined(owner: &Rc<crate::Runtime>) {
    let task = owner.clone();
    owner.spawn(async move {
        rebuild_combined_ready(&task).await;
    });
}

async fn rebuild_combined_ready(owner: &Rc<crate::Runtime>) -> bool {
    let generation = owner.generation.with(Cell::get);
    let revision = combined::invalidate_rebuild(owner);
    if !combined::active(owner) {
        return false;
    }
    let Ok(playlists) = library::playlists(owner) else {
        return false;
    };
    let enabled = playlists
        .into_iter()
        .filter(|playlist| playlist.policy.enabled)
        .map(|playlist| (playlist.id, playlist.name))
        .collect();
    let sources = combined::source_schedules(owner, enabled);
    let Some(install_favorites) = crate::favorites::prepare_combined(owner).await else {
        return false;
    };
    if !combined::active(owner) {
        return false;
    }
    let collapsed = crate::library::collapsed(owner).await.unwrap_or_default();
    commit_schedule(
        owner,
        ScheduleUpdate::Combined {
            generation,
            revision,
            sources,
            collapsed,
        },
        install_favorites,
    ) == CommitOutcome::Installed
}

async fn replace_combined_organization(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    ticket: u64,
) -> bool {
    let Some(schedule) = combined::part(owner, source) else {
        return false;
    };
    let Some(replacement) = organization::apply(owner, &schedule, source).await else {
        return false;
    };
    let current = combined::part(owner, source).is_some_and(|s| s.same_catalog(&schedule));
    if !current || !owner.organization_edit_current(source, ticket) {
        return false;
    }
    combined::store_part(owner, source, Rc::new(replacement));
    rebuild_combined_ready(owner).await
}

fn resume_combined(owner: &Rc<crate::Runtime>) {
    if !crate::settings::flag(&owner.settings(), iptv_settings::keys::RESUME_LAST_CHANNEL)
        || owner.browser().playing
    {
        owner.resume.with(|r| r.set(false));
        return;
    }
    let loaded = crate::commands::combined_resume_key(owner);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let key = loaded.await.ok().flatten();
            if !owner.resume.with(Cell::get) || owner.browser().playing || !combined::active(&owner)
            {
                return;
            }
            let schedule = owner.schedule();
            if let Some(c) = key.and_then(|k| schedule.find_channel(&k)) {
                // Another playlist's import may call resume while the target portal is
                // still only cached rows. Wait for that owning source's authentication.
                if iptv_guide::combined::epg_channel(&*schedule, c)
                    .is_none_or(|(child, _)| child.playback().is_none())
                {
                    return;
                }
                owner.resume.with(|r| r.set(false));
                if schedule.channel_block(c) != iptv_core::organization::Block::None {
                    return;
                }
                owner.emit(crate::RuntimeEvent::ResumeChannel(c));
                owner.emit(crate::RuntimeEvent::ResumeFullscreen);
            }
        }
    });
}

/// Toggle a parent row without changing the channel catalog or player surface.
pub fn toggle_playlist(owner: &Rc<crate::Runtime>, group: usize) -> bool {
    let Some((projection, collapsed)) = combined::toggled_playlist(owner, group) else {
        return false;
    };
    let saved = crate::library::save_collapsed(owner, collapsed);
    // A following key must see the new rows, even while persistence waits.
    // Invalidate assemblies that began before this projection was chosen.
    combined::invalidate_rebuild(owner);
    commit_schedule(owner, ScheduleUpdate::Catalog(projection), || {});
    owner.spawn({
        let owner = owner.clone();
        async move {
            if let Err(error) = saved.await {
                log::warn!("iptv browser: could not save expansion: {error}");
                owner.emit(crate::RuntimeEvent::Notify(
                    "The change could not be saved. Reopen the list and try again.",
                ));
                // Restore persisted expansion on failure. This read follows all
                // newer queued choices, and later choices invalidate its revision.
                rebuild_combined(&owner);
            }
        }
    });
    true
}

pub fn organization_changed(owner: &Rc<crate::Runtime>, source: SourceId) {
    reload_organization(owner, source, false);
}

/// A watch sort remains eligible for the next guide entry until installed.
pub(crate) fn watch_order_changed(owner: &Rc<crate::Runtime>, source: SourceId) {
    reload_organization(owner, source, true);
}

fn reload_organization(owner: &Rc<crate::Runtime>, source: SourceId, retry_watch_order: bool) {
    owner
        .organization_revision
        .set(owner.organization_revision.get().wrapping_add(1));
    let ticket = owner.begin_organization_edit(source);
    owner.emit(crate::RuntimeEvent::LibraryChanged);
    let task = owner.clone();
    owner.spawn(async move {
        let installed = if combined::active(&task) {
            replace_combined_organization(&task, source, ticket).await
        } else {
            replace_single_organization(&task, source, ticket).await
        };
        if retry_watch_order && !installed {
            task.watch_sort_dirty.borrow_mut().insert(source);
        }
    });
}

async fn replace_single_organization(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    ticket: u64,
) -> bool {
    if owner.shown.get() != Some(source) {
        return false;
    }
    let schedule = owner.schedule();
    let Some(epg) = schedule
        .as_any()
        .and_then(|s| s.downcast_ref::<EpgSchedule>())
    else {
        return false;
    };
    let Some(replacement) = organization::apply(owner, epg, source).await else {
        return false;
    };
    commit_schedule(
        owner,
        ScheduleUpdate::Organization {
            source,
            ticket,
            expected: epg,
            schedule: Rc::new(replacement),
        },
        || {},
    ) == CommitOutcome::Installed
}

pub fn start(owner: &std::rc::Rc<crate::Runtime>) {
    start_library(owner, false);
}

/// Retry the startup library read after a user has addressed a storage failure.
pub fn retry_startup(owner: &std::rc::Rc<crate::Runtime>) {
    start_library(owner, true);
}

fn start_library(owner: &std::rc::Rc<crate::Runtime>, recovering: bool) {
    combined::reset_single(owner);
    crate::library::reset_imports(owner);
    let started = crate::time::Instant::now();
    let services = owner.services();
    let startup = crate::commands::startup_library(&services);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let startup = match startup.await {
                Ok(snapshot) if recovering => {
                    async {
                        // The startup writer and the parental reader have separate
                        // backoffs. Prepare the reader before resuming library work.
                        services.read_at_startup(|_| Ok(())).await?;
                        owner.parental_refresh().await.map_err(|_| {
                            crate::Error::unavailable(iptv_core::message::key(
                                "Parental settings are unavailable",
                            ))
                        })?;
                        Ok(snapshot)
                    }
                    .await
                }
                result => result,
            };
            if !services.is_current() {
                return;
            }
            let startup = startup.map(|(source, combined, any, snapshot)| {
                let stored: Vec<_> = snapshot.playlists.iter().map(|p| p.id).collect();
                crate::library::install(&owner, snapshot);
                // A deletion whose settings save was lost must not hand its
                // playlist's own settings to a new playlist with its id.
                if let Err(error) = crate::settings::forget_import_switches(&owner, None, &stored) {
                    log::warn!("iptv data: deleted playlists' settings stay: {error}");
                }
                if let Err(error) = crate::settings::adopt_global_connection_limit(&owner, &stored)
                {
                    log::warn!("iptv data: the shared streams-at-once value stays: {error}");
                }
                (source, combined, any)
            });
            if startup.is_ok() {
                services.mark(
                    "store opened",
                    format_args!("{} ms", started.elapsed().as_millis()),
                );
                if recovering {
                    owner.emit(crate::RuntimeEvent::StoreStartupRecovered);
                }
            }
            match startup {
                Ok((_, true, _)) => start_combined(&owner),
                Ok((Some(stored), false, _)) => {
                    opening::begin(&owner, &stored).await;
                    run_import(&owner, stored, Purpose::Startup, false);
                }
                Ok((None, false, true)) => {
                    show_nothing(&owner);
                    owner.emit(crate::RuntimeEvent::OpenPlaylists);
                }
                Ok((None, false, false)) => owner.emit(crate::RuntimeEvent::FirstRun),
                Err(error) => {
                    log::warn!("iptv data: store unavailable ({error})");
                    owner.emit(crate::RuntimeEvent::StoreStartupFailed {
                        path: services.file_database_path(),
                        error,
                    });
                    return;
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
/// at most one per `PROGRESS_FRAME`) and then one [`Update::Done`]; on
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
    let completion = crate::library::import_completion(cancel.clone());
    if let Some(source) = import_source {
        crate::library::track_import(owner, source, &completion);
    }
    let previous = Cell::new(
        (purpose == Purpose::Add)
            .then(|| ready_single(owner))
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
        let services = owner.services();
        owner.spawn({
            let owner = owner.clone();
            async move {
                let opens_stored = job.purpose.shows_stored_first() && job.stored.is_some();
                if !services.can_import() && !opens_stored {
                    // A new import cannot run on this host; nothing is read
                    // for it.
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(
                        FailureKind::Unavailable,
                    ))));
                    return;
                }
                let authorization = owner.parental_snapshot();
                let admission = job.clone();
                let auth = authorization.clone();
                let admission = services
                    .read(move |store| crate::parental::import_allowed(store, &admission, &auth))
                    .await;
                let failure = match admission {
                    _ if cancel.is_canceled() || !services.is_current() => {
                        Some(FailureKind::Canceled)
                    }
                    Ok(true) => None,
                    Ok(false) => Some(FailureKind::Auth),
                    Err(error) => {
                        log::warn!("iptv data: import admission could not read the store: {error}");
                        Some(FailureKind::Storage)
                    }
                };
                if let Some(kind) = failure {
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(kind))));
                    return;
                }
                if job.purpose.shows_stored_first()
                    && let Some(id) = job.stored
                    && opening::send_stored(&services, &job, id, now_unix(), &tx, &cancel, &revoked)
                        .await
                {
                    return;
                }
                if cancel.is_canceled() {
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(
                        FailureKind::Canceled,
                    ))));
                    return;
                }
                if !services.can_import() {
                    // Imports cannot run on this host: not a passing
                    // condition to retry.
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(
                        FailureKind::Unavailable,
                    ))));
                    return;
                }
                let failed = tx.clone();
                // A new source with a typed guide waits for its terminal
                // result even after Cancel. Waiting for an unrelated import
                // on the browser lane should not delay that result.
                let cancel_while_queued =
                    (job.stored.is_none() && job.epg_url.is_some()).then(|| cancel.clone());
                if !services.submit_import(visible, cancel_while_queued, move |worker| async move {
                    pipeline(&worker, job, &tx, &cancel, &revoked, &authorization).await;
                }) {
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
                    && has_multiple_enabled(&owner);
                let source = imported.source;
                show_part(&owner, imported, generation, resume, held, Some(&job)).await;
                if merge {
                    merge_requested.set(true);
                    join_after_add(&owner, source, generation, previous.take());
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
                                crate::library::track_added_import(
                                    &owner,
                                    imported.source,
                                    &completion,
                                );
                                // A new playlist may have a deleted one's id:
                                // it starts from the global settings.
                                if let Err(error) = crate::settings::forget_import_switches(
                                    &owner,
                                    Some(&[imported.source]),
                                    &[],
                                ) {
                                    log::warn!(
                                        "iptv data: an old playlist's settings stay: {error}"
                                    );
                                }
                            }
                            if import_source.is_some_and(|source| {
                                !crate::library::import_is_current(&owner, source, &completion)
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
                            if crate::library::import_is_current(
                                &owner,
                                imported.source,
                                &completion,
                            ) {
                                show(imported, None).await;
                            }
                        }
                        Ok(Msg::FirstScreen(imported, held, shown)) => {
                            show(imported, Some(held)).await;
                            // Only now may the opening read the whole window.
                            drop(shown);
                        }
                        Ok(Msg::NextCheck { source, at })
                            if (shows || combined_includes(&owner, source))
                                && crate::library::import_is_current(
                                    &owner,
                                    source,
                                    &completion,
                                ) =>
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
                            if crate::library::import_is_current(&owner, source, &completion) {
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
                        let kind = if cancel.is_canceled() {
                            FailureKind::Canceled
                        } else {
                            log::warn!("iptv data: import ended without a result");
                            FailureKind::Network
                        };
                        on_update(Update::Done(Err(ImportFailure::new(kind))));
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
            // This job canceled the source's timer when it started and did
            // not answer for it: it failed, a guide action revoked its handoff
            // (its NextCheck was dropped), or it could not read the store.
            // Newer work that replaced it answers instead.
            if let Some(source) = import_source
                && !answered
                && (shows || combined_includes(&owner, source))
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
pub(crate) fn repair_source_shown(owner: &Rc<crate::Runtime>, source: SourceId) -> bool {
    owner.shown.get() == Some(source) || combined_includes(owner, source)
}

pub(crate) async fn show_repair(
    owner: &Rc<crate::Runtime>,
    imported: Box<Imported>,
    generation: u64,
    edit: u64,
) {
    if !repair_source_shown(owner, imported.source)
        || crate::stored::guide_edit_revision(owner, imported.source) != edit
    {
        return;
    }
    show(owner, imported, generation, false).await;
}

async fn show(
    owner: &std::rc::Rc<crate::Runtime>,
    imported: Box<Imported>,
    generation: u64,
    resume: bool,
) {
    show_part(owner, imported, generation, resume, None, None).await;
}

/// [`show`]; with `held`, its guide holds only those programs (a first
/// screen, `opening.rs`).
///
/// Shows of one source take turns in the order they began: each has
/// several waits, and one that began earlier (with older data) would
/// otherwise replace a later one when it happens to finish last. With
/// `job`, the import that read `imported`: when a guide action revoked it
/// meanwhile ("Delete stored guide data", a channel's guide chosen), the
/// guide it read may predate what that action deleted or matched anew.
/// Its catalog and playback are still shown, with the guide as stored
/// now (read after that action's writes) in place of the one it read.
async fn show_part(
    owner: &std::rc::Rc<crate::Runtime>,
    mut imported: Box<Imported>,
    generation: u64,
    resume: bool,
    mut held: Option<opening::Held>,
    job: Option<&crate::library::ImportCompletion>,
) {
    if owner.generation.with(Cell::get) != generation {
        return;
    }
    let source = imported.source;
    let guide_revision = crate::stored::guide_edit_revision(owner, source);
    let _turn = owner.show_turn(source).await;
    if crate::stored::guide_edit_revision(owner, source) != guide_revision {
        return;
    }
    if owner.generation.with(Cell::get) != generation {
        return;
    }
    if let Some(job) = job
        && !crate::library::import_is_current(owner, source, job)
    {
        let stored = crate::commands::stored_guide(owner, source, imported.now).await;
        // Unreadable: without programs rather than with ones that may
        // just have been deleted, until the stored guide can be read.
        imported.guide = stored.unwrap_or_else(|error| {
            log::warn!("iptv data: the stored guide cannot be read: {error}");
            restore_guide(owner, source, generation);
            GuideStore::default()
        });
        held = None;
        if owner.generation.with(Cell::get) != generation {
            return;
        }
    }
    if crate::stored::guide_edit_revision(owner, source) != guide_revision {
        return;
    }
    let first_screen = held.is_some();
    if combined::active(owner) {
        show_combined(owner, imported, held).await;
        if resume {
            resume_combined(owner);
        }
        return;
    }
    if Some(source) != owner.shown.get() {
        source_changed(owner, Some(source));
    }
    crate::stored::note_shown(owner, Some(source));
    let channels = imported.channels.len();
    let started = crate::time::Instant::now();
    let Imported {
        groups,
        channels: catalog,
        playback,
        guide,
        now,
        resume: resumes,
        ..
    } = *imported;
    let mut organization_revision = organization::revision(owner);
    let schedule = organization::build(owner, source, groups, catalog, guide, now).await;
    if owner.generation.with(Cell::get) != generation
        || combined::active(owner)
        || crate::stored::guide_edit_revision(owner, source) != guide_revision
    {
        return;
    }
    let built = started.elapsed();
    let mut schedule = match playback {
        Streams::New(context, allowed, authority) => {
            owner.emit(crate::RuntimeEvent::AccountStreams(allowed));
            schedule
                .with_playback(context)
                .with_source_authority(Some(authority))
        }
        Streams::Current => {
            let current = owner.schedule();
            let shared = current
                .as_any()
                .and_then(|s| s.downcast_ref::<EpgSchedule>())
                .and_then(EpgSchedule::shared_playback);
            let authority = current
                .as_any()
                .and_then(|s| s.downcast_ref::<EpgSchedule>())
                .and_then(EpgSchedule::source_authority);
            schedule
                .with_shared_playback(shared)
                .with_source_authority(authority)
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
    if owner.generation.with(Cell::get) != generation
        || combined::active(owner)
        || crate::stored::guide_edit_revision(owner, source) != guide_revision
    {
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
    let favorites = started.elapsed();
    let outcome = commit_schedule(
        owner,
        ScheduleUpdate::Source {
            source,
            generation,
            guide_revision,
            schedule: Rc::new(schedule),
        },
        install_favorites,
    );
    if outcome == CommitOutcome::Refused {
        return;
    }
    owner.emit(crate::RuntimeEvent::SchedulePresented(vec![
        ("schedule", built),
        ("pages", wrapped),
        ("favorites", favorites),
        ("swapped", started.elapsed()),
    ]));
    if first_screen {
        owner
            .services()
            .mark("first screen shown", format_args!("{channels} channels"));
    }
    owner.emit(crate::RuntimeEvent::SourceReady(resume && resumes));
}

/// Another stored source (or none) is about to be shown: what played and
/// the favorites belong to the old one, and the choice is remembered.
fn source_changed(owner: &std::rc::Rc<crate::Runtime>, source: Option<SourceId>) {
    log::info!("iptv data: showing source {:?}", source.map(|id| id.0));
    owner.emit(crate::RuntimeEvent::SourceChanged);
    let (has_vod, any_vod) = crate::favorites::vod_availability(owner);
    owner.emit(crate::RuntimeEvent::FavoritesChanged {
        channels: BTreeSet::new(),
        has_vod,
        any_vod,
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
    commit_schedule(
        owner,
        ScheduleUpdate::Catalog(Rc::new(EpgSchedule::new(
            Vec::new(),
            GuideStore::default(),
            now_unix(),
        ))),
        || {},
    );
}

/// Work that canceled `source`'s guide timer ended without answering for
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
/// which this work canceled, is kept at its own time. The refresh policy
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
        || crate::library::pending_import(owner, source).is_some()
        || !(owner.shown.get() == Some(source) || combined_includes(owner, source))
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
            while let Some(pending) = crate::library::pending_import(&owner, source) {
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
            let mut canceled = std::pin::pin!(cancel.canceled());
            let fired = std::future::poll_fn(|cx| {
                use std::task::Poll;
                if canceled.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(false);
                }
                elapsed.as_mut().poll(cx).map(|_| true)
            })
            .await;
            if !fired || !owner.guide_check_current(source, ticket) {
                return;
            }
            if owner.generation.with(Cell::get) != generation
                || (combined::active(&owner) && !combined_includes(&owner, source))
            {
                // No longer this timer's to check (the source left All
                // playlists): its entry must not outlive it.
                owner.end_guide_check(source);
            } else if crate::library::running(&owner, source)
                || crate::stored::updating(&owner, source)
                || crate::library::pending_import(&owner, source).is_some()
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
    handoff: Option<crate::library::ImportCompletion>,
    quiet: bool,
) {
    let requested = read;
    let read = if crate::library::running(owner, source) {
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
    if !services.can_import() {
        // Guide reads cannot run on this host: no check is owed and none
        // is retried.
        owner.end_guide_check(source);
        return;
    }
    if !services.submit_import(
        source_is_visible(owner, source),
        None,
        move |worker| async move {
            guide_pipeline(&worker, source, &tx, read, &cancel).await;
        },
    ) {
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
            .is_some_and(|job| !crate::library::import_is_current(owner, source, job))
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
            let later = now_unix() + crate::stored::RETRY_DUE;
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

#[cfg(all(test, feature = "native"))]
mod playlist_lifecycle_tests {
    use super::*;
    use iptv_providers::{SecretUrl, model::M3uSourceConfig};

    fn run(test: impl AsyncFnOnce(Rc<Runtime>)) {
        let directory = tempfile::tempdir().unwrap();
        let owner = Rc::new(Runtime::new(crate::Ports::new(
            directory.path().into(),
            |_| Ok(crate::model::DatabaseKey::from_bytes([0x73; 32])),
        )));
        owner.bind_tasks(
            |task| drop(tokio::task::spawn_local(task)),
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        owner.start_services();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&executor, async {
            tokio::time::timeout(Duration::from_secs(15), test(owner.clone()))
                .await
                .expect("playlist lifecycle scenario timed out");
        });
        assert!(owner.drain_services());
    }

    fn synthetic(name: &str) -> Source {
        Source {
            name: name.to_owned(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/synthetic.m3u"),
                user_agent: None,
                referrer: None,
            }),
        }
    }

    #[test]
    fn deleting_the_shown_source_previews_the_enabled_manual_order_fallback() {
        run(async |owner| {
            let (shown, fallback, hidden) = owner
                .services()
                .call(|store| {
                    let shown = store.add_source(&synthetic("Synthetic shown"))?;
                    let hidden = store.add_source(&synthetic("Synthetic hidden"))?;
                    let fallback = store.add_source(&synthetic("Synthetic fallback"))?;
                    let earlier = store.add_source(&synthetic("Synthetic earlier"))?;
                    store.set_active_source(shown)?;
                    store.set_source_order(&[shown, earlier, fallback, hidden])?;
                    store.set_source_policy(
                        hidden,
                        crate::model::SourcePolicy {
                            enabled: false,
                            ..Default::default()
                        },
                    )?;
                    Ok((shown, fallback, hidden))
                })
                .await
                .unwrap();
            crate::library::install(&owner, crate::library::snapshot(&owner).await.unwrap());
            assert_eq!(
                next_playlist_after_deletion(&owner, shown).as_deref(),
                Some("Synthetic fallback")
            );
            assert_eq!(
                next_playlist_after_deletion(&owner, hidden).as_deref(),
                Some("Synthetic shown")
            );
            owner
                .services()
                .call(move |store| {
                    store.delete_source(shown)?;
                    Ok(())
                })
                .await
                .unwrap();
            crate::library::install(&owner, crate::library::snapshot(&owner).await.unwrap());
            assert_eq!(
                crate::library::newest(&owner).unwrap().unwrap().id,
                fallback
            );
        });
    }

    #[test]
    fn reenabled_source_stays_shown_while_the_earlier_hide_reloads() {
        run(async |owner| {
            let shown = owner
                .services()
                .call(|store| store.add_source(&synthetic("Synthetic shown")))
                .await
                .unwrap();
            owner.shown.set(Some(shown));
            crate::library::install(&owner, crate::library::snapshot(&owner).await.unwrap());

            // Hold the reader so the checked state from both writes reaches
            // the UI before either can make a playback change.
            let (entered, held) = futures_channel::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let reader = owner.services().read(move |_| {
                let _ = entered.send(());
                released
                    .recv_timeout(Duration::from_secs(10))
                    .expect("reader was not released");
                Ok(())
            });
            held.await.unwrap();

            let (updates, mut updated) = unbounded_channel();
            let weak = Rc::downgrade(&owner);
            owner.bind(move |event| {
                if matches!(event, crate::RuntimeEvent::LibraryChanged)
                    && let Some(owner) = weak.upgrade()
                    && let Ok(policy) = library::policy(&owner, shown)
                {
                    let _ = updates.send(policy.enabled);
                }
            });

            let set_visibility = |enabled| {
                let owner = owner.clone();
                tokio::task::spawn_local(async move {
                    update_policy(&owner, shown, move |policy| policy.enabled = enabled).await
                })
            };
            let hide = set_visibility(false);
            assert_eq!(updated.recv().await, Some(false));
            let show = set_visibility(true);
            assert_eq!(updated.recv().await, Some(true));
            release.send(()).unwrap();
            reader.await.unwrap();
            hide.await.unwrap().unwrap();
            show.await.unwrap().unwrap();

            assert_eq!(owner.shown_source(), Some(shown));
            assert!(library::policy(&owner, shown).unwrap().enabled);
        });
    }
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
