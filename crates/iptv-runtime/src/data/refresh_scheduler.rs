use super::*;
#[allow(unused_imports)]
use crate::imports::{GuideRead, Imported, Msg, Streams, guide_pipeline, pipeline};
#[allow(unused_imports)]
use crate::model::{StoreError, StoredSource};
#[allow(unused_imports)]
use crate::state::SingleSchedule;
#[allow(unused_imports)]
use crate::time::now_unix;
#[allow(unused_imports)]
use crate::{GuideWork, Runtime, RuntimeEvent, ScheduleCause};
#[allow(unused_imports)]
use iptv_core::add_playlist::{FailureKind, ImportFailure};
#[allow(unused_imports)]
use iptv_core::guide::Schedule;
#[allow(unused_imports)]
use iptv_epg::GuideStore;
#[allow(unused_imports)]
use iptv_guide::EpgSchedule;
#[allow(unused_imports)]
use iptv_guide::combined::{CombinedSchedule, SourceSchedule};
#[allow(unused_imports)]
use iptv_import::CancelToken;
#[allow(unused_imports)]
use iptv_library::SourceId;
#[allow(unused_imports)]
use iptv_providers::model::{Source, SourceConfig};
#[allow(unused_imports)]
use std::cell::{Cell, RefCell};
#[allow(unused_imports)]
use std::collections::BTreeSet;
#[allow(unused_imports)]
use std::rc::Rc;
#[allow(unused_imports)]
use std::time::Duration;
#[allow(unused_imports)]
use tokio::sync::mpsc::error::TryRecvError;
#[allow(unused_imports)]
use tokio::sync::mpsc::unbounded_channel;

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
/// finished, with true only if the worker accepted it and it still owns
/// the source's guide. False when there is no guide to download.
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
pub(super) fn restart_update(
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

pub(super) fn start_update(
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
    let generation = owner.imports.generation.with(Cell::get);
    owner.spawn({
        let owner = owner.clone();
        async move {
            for job in unwinding {
                job.until_finished().await;
            }
            let guides = stored::configured_guides(&owner, source).await;
            let current = crate::library::import_is_current(&owner, source, &completion);
            let (accepted, mut rx) =
                stored::download(&owner, source, guides, cancel, current, || {
                    source_is_visible(&owner, source)
                });
            // Whether this update set the next check, or settled that the
            // policy asks for none.
            let mut answered = false;
            // The next check this update asked for after it was superseded.
            let mut unscheduled = None;
            loop {
                let shown = || {
                    owner.catalog.shown_source() == Some(source)
                        || combined_includes(&owner, source)
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
            // A guide action can finish before the canceled worker unwinds.
            // Its old completion must not report an update over that result.
            done(accepted && crate::library::import_is_current(&owner, source, &completion));
        }
    });
    true
}

/// `source`'s guide sources changed: its guide is checked once the edits
/// settle (the refresh policy downloads changed feeds). Only for the
/// source on screen; another one is checked when it is shown.
pub(super) fn guides_changed(owner: &std::rc::Rc<crate::Runtime>, source: SourceId) {
    let previous = crate::library::pending_import(owner, source);
    crate::library::revoke_import_handoff(owner, source);
    if owner.catalog.shown_source() != Some(source) && !combined_includes(owner, source) {
        return;
    }
    // A check under way would download and store the feeds as they were.
    // The check below answers for it, or the update that runs instead.
    owner.interrupt_guide_read(source);
    let edit = crate::stored::next_guide_edit(owner, source);
    let generation = owner.imports.generation.with(Cell::get);
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
            let current = owner.imports.generation.with(Cell::get) == generation
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

pub(super) fn clear_sources(
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
    let generation = owner.imports.generation.with(Cell::get);
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

pub(super) fn keep_guide_check(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    asked: Option<i64>,
) {
    if owner.has_guide_check(source)
        || crate::library::pending_import(owner, source).is_some()
        || !(owner.catalog.shown_source() == Some(source) || combined_includes(owner, source))
    {
        return;
    }
    if owner.imports.generation.with(Cell::get) != generation {
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
pub(super) fn keep_shown_check(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
) {
    owner.spawn({
        let owner = owner.clone();
        async move {
            while let Some(pending) = crate::library::pending_import(&owner, source) {
                pending.until_finished().await;
            }
            if owner.imports.generation.with(Cell::get) == generation {
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
pub(super) fn defer_guide_check(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
) {
    schedule_check(
        owner,
        source,
        now_unix() + MIN_WAIT.as_secs() as i64,
        generation,
    );
}

/// Looks at `source`'s guide again at `at` (unix time), unless a newer
/// import started meanwhile (false: no timer was set).
pub(super) fn schedule_check(
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
            if owner.imports.generation.with(Cell::get) != generation
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
pub(super) fn check_guide(owner: &Rc<crate::Runtime>, source: SourceId, generation: u64) {
    let task = owner.clone();
    owner.spawn(async move {
        read_guide(&task, source, generation, GuideRead::Check, None, false).await;
    });
}

/// Restore the retained source's complete cache and then its guide policy.
pub(super) fn restore_guide(owner: &Rc<crate::Runtime>, source: SourceId, generation: u64) {
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
pub(super) fn source_is_visible(owner: &std::rc::Rc<crate::Runtime>, source: SourceId) -> bool {
    if owner.catalog.shown_source() == Some(source) {
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
pub(super) async fn read_guide(
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
    let failed_tx = tx.clone();
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
        move || {
            let _ = failed_tx.send(Msg::NoCheck { source });
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
