//! The store's side of an import: a stored source's catalog and guide,
//! shown before any network, and keeping the guide in the store under the
//! refresh policy (`iptv_import::decide`).
//!
//! A guide download streams into a stage file next to the store
//! (`super::download_guide`, `crate::model::EpgStage`) and replaces
//! the stored guide only once it loaded ([`replace`]); it is read back from
//! the store without descriptions (a few MiB for thousands of channels
//! instead of the parsed feed's hundreds); the guide's details read a
//! description on demand ([`description_lookup`]).
//!
//! The `epg.*` settings drive it (global values, `crate::settings`): the
//! refresh policy, how many days of finished programmes are kept (the
//! last of them held in memory, older ones paged in: `pages.rs`), and
//! whether descriptions are stored. The storage settings
//! travel with each guide's config, so changing them downloads the guide
//! again at the next check (the stored feeds no longer match).
//!
//! The guides to download are the playlist's guide sources (`guides.rs`,
//! kept in the store); the channels whose guide the user chose by hand
//! (`assign.rs`) are matched with those choices ([`overrides`]).
//!
//! The settings screen's guide commands act on the source on screen
//! ([`note_shown`]): [`update_now`] downloads its guide (every source
//! switched on), [`clear`] deletes the stored one, [`guide_status`] says
//! how the last update went and [`source_lines`] how each source did.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use tokio::sync::mpsc::unbounded_channel;

use crate::GuideWork;
use crate::model::{SourceId, StoredGuide};
use iptv_epg::{ChannelKey, UnixTime};
use iptv_import::CancelToken;

use super::Msg;
pub(super) use crate::stored::RETRY_DUE;

pub fn refresh_status(owner: &std::rc::Rc<crate::Runtime>) {
    owner
        .status_revision
        .set(owner.status_revision.get().wrapping_add(1));
    let expected = owner.status_revision.get();
    let loaded = crate::commands::guide_status_snapshot(owner);
    owner.spawn({
        let owner = owner.clone();
        async move {
            if let Ok(mut snapshot) = loaded.await
                && owner.status_revision.get() == expected
            {
                let changed = snapshot
                    .library
                    .take()
                    .is_some_and(|snapshot| super::library::install(&owner, snapshot));
                *owner.status.borrow_mut() = snapshot;
                if changed && super::combined::active(&owner) {
                    super::combined::rebuild(&owner);
                }
                owner.emit(crate::RuntimeEvent::SettingsChanged);
            }
        }
    });
}

/// How long a guide check waits after the guide sources changed (more
/// edits may follow).
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

pub use crate::stored::{
    begin_update, configs, end_update, guide_edit_is_current, keep_past_days, next_guide_edit,
    note_shown, shown_sources, unwinding_updates, update_current, updating, with_storage,
};

/// Fetch descriptions on the shared worker; a bounded cache makes later renders synchronous.
pub fn description_lookup(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
) -> Option<impl Fn(&ChannelKey, UnixTime) -> Option<String> + 'static> {
    let cache = Rc::new(RefCell::new(
        BTreeMap::<(String, UnixTime), Option<String>>::new(),
    ));
    let owner = Rc::downgrade(owner);
    Some(move |key: &ChannelKey, start| {
        let identity = (key.to_string(), start);
        if let Some(description) = cache.borrow().get(&identity) {
            return description.clone();
        }
        let owner = owner.upgrade()?;
        if cache.borrow().len() >= 128 {
            cache.borrow_mut().pop_first();
        }
        cache.borrow_mut().insert(identity.clone(), None);
        let key = key.clone();
        let read = crate::commands::description(&owner, source, key, start);
        let cache = cache.clone();
        owner.spawn({
            let owner = owner.clone();
            async move {
                match read.await {
                    Ok(description) => {
                        let mut cache = cache.borrow_mut();
                        let Some(entry) = cache.get_mut(&identity) else {
                            return;
                        };
                        *entry = description;
                    }
                    Err(error) => {
                        log::warn!("iptv data: description: {error}");
                        // A transient read failure is not a cached absence. Retry
                        // on the next user-driven lookup without causing a render
                        // loop while the database remains unavailable.
                        cache.borrow_mut().remove(&identity);
                        return;
                    }
                }
                owner.emit(crate::RuntimeEvent::GuideChanged);
            }
        });
        None
    })
}

/// "Update the guide now" of the source on screen: see [`update_source`].
pub fn update_now(owner: &Rc<crate::Runtime>, done: impl FnOnce() + 'static) -> bool {
    let sources = shown_sources(owner);
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
        if update_current(owner, source) {
            remaining.set(remaining.get() - 1);
            started = true;
            continue;
        }
        if update_source(owner, source, move |_| finish()) {
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

/// Work cancelled by a guide action, until its writer guards unwind.
pub(super) struct Superseded {
    pub cancelled: Option<super::library::ImportCompletion>,
    pub read: Option<GuideWork>,
}
impl Superseded {
    pub(super) fn keep_check(
        &self,
        owner: &Rc<crate::Runtime>,
        source: SourceId,
        generation: u64,
        at: i64,
    ) {
        match self.read {
            Some(GuideWork::Check) => super::keep_guide_check(owner, source, generation, Some(at)),
            Some(_) => super::keep_guide_check(owner, source, generation, None),
            None => {}
        }
    }
}
pub(super) fn supersede(owner: &Rc<crate::Runtime>, source: SourceId) -> Superseded {
    Superseded {
        cancelled: super::library::supersede_import(owner, source),
        read: owner.interrupt_guide_read(source),
    }
}

/// Downloads `source`'s guide now (every guide source switched on, with
/// the storage settings and guide choices of now); the guide on screen
/// changes when it is `source`'s. `done` runs on the UI thread when it
/// finished. False when there is no guide to download.
///
/// A request joins an update that runs already, unless a guide action
/// superseded that one: it then downloads nothing any more, and this
/// request starts an update of its own once that one has unwound.
pub fn update_source(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    if update_current(owner, source) {
        return true;
    }
    start_update(owner, source, None, done)
}

/// [`update_source`] for a change the download has to include (a
/// channel's guide choice just saved): an update that runs already read
/// the store before it, and is replaced instead of joined. `after`: work
/// that change cancelled, which the download starts after.
pub(super) fn restart_update(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    after: Option<super::library::ImportCompletion>,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    start_update(owner, source, after, done)
}

fn start_update(
    owner: &std::rc::Rc<crate::Runtime>,
    source: SourceId,
    after: Option<super::library::ImportCompletion>,
    done: impl FnOnce(bool) + 'static,
) -> bool {
    let has_guides = owner.status.with(|status| {
        let status = status.borrow();
        match status.lines.get(&source) {
            Some(lines) if !lines.is_empty() => lines.iter().any(|line| line.source.enabled),
            _ => status
                .guides
                .get(&source)
                .is_some_and(|guide| !guide.configs().is_empty()),
        }
    });
    if !has_guides {
        return false;
    }
    // This update owns the source's guide from now on: work it replaces
    // is revoked. What that cancelled, and an update a guide action
    // superseded before, only unwinds: this one starts after it.
    let mut unwinding = unwinding_updates(owner, source);
    unwinding.extend(super::library::replace_import(owner, source));
    unwinding.extend(after);
    let cancel = CancelToken::new();
    let completion = super::library::import_completion(cancel.clone());
    completion.playlist_done(); // This worker owns guide work only.
    super::library::track_import(owner, source, &completion);
    begin_update(owner, source, &completion);
    let generation = owner.generation.with(Cell::get);
    owner.spawn({
        let owner = owner.clone();
        async move {
            for job in unwinding {
                job.until_finished().await;
            }
            let guides = match crate::commands::guide_sources(&owner, source).await {
                Ok((stored, sources)) if sources.is_empty() => Ok(with_storage(
                    stored
                        .as_ref()
                        .map(StoredGuide::configs)
                        .unwrap_or_default(),
                    &owner.settings(),
                )),
                Ok((_, sources)) => Ok(configs(&owner.settings(), &sources)),
                Err(error) => Err(error),
            };
            let (tx, mut rx) = unbounded_channel();
            // Without a download (nothing to download, the store unreadable,
            // the workers busy) the update ends as soon as it began.
            let accepted = match guides {
                Ok(guides) if guides.is_empty() => {
                    owner.emit(crate::RuntimeEvent::Notify(
                        "No guide sources are configured",
                    ));
                    // No worker holds the sender: the wait below ends at once.
                    drop(tx);
                    false
                }
                Ok(guides) => {
                    let visible = super::source_is_visible(&owner, source);
                    let services = owner.services();
                    let worker = services.clone();
                    let accepted = services.submit(visible, move || {
                        worker.block_on(download_now(&worker, source, guides, &tx, &cancel));
                    });
                    if !accepted {
                        owner.emit(crate::RuntimeEvent::Notify(
                            "Too many playlists are updating. Try again shortly.",
                        ));
                    }
                    accepted
                }
                Err(error) => {
                    log::warn!("iptv data: cannot load guide sources: {error}");
                    drop(tx);
                    false
                }
            };
            // Whether this update set the next check, or settled that the
            // policy asks for none.
            let mut answered = false;
            // The next check this update asked for after it was superseded.
            let mut unscheduled = None;
            loop {
                let shown = || {
                    owner.shown.with(Cell::get) == Some(source)
                        || super::combined::includes(&owner, source)
                };
                let current = || super::library::import_is_current(&owner, source, &completion);
                match rx.recv().await {
                    Some(Msg::Guide(imported)) if shown() && current() => {
                        super::show(&owner, imported, generation, false).await
                    }
                    Some(Msg::NextCheck { source, at }) if shown() && current() => {
                        // Not when a newer generation began meanwhile: should
                        // that fail and leave this playlist on screen, this
                        // is what it is owed.
                        if !super::schedule_check(&owner, source, at, generation) {
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
            end_update(&owner, source, &completion);
            // This update cancelled the source's timer and did not answer for
            // it: a guide action superseded it (its NextCheck was dropped),
            // the worker pool refused it, or the store could not be read.
            // Newer work that replaced it answers instead.
            if !answered && !completion.superseded.get() {
                super::keep_guide_check(&owner, source, generation, unscheduled);
            }
            owner.emit(crate::RuntimeEvent::LibraryChanged);
            done(accepted);
        }
    });
    true
}

/// The download of [`update_source`], on its own thread.
use crate::stored::download_now;

/// `source`'s guide sources changed: its guide is checked once the edits
/// settle (the refresh policy downloads changed feeds). Only for the
/// source on screen; another one is checked when it is shown.
pub fn guides_changed(owner: &std::rc::Rc<crate::Runtime>, source: SourceId) {
    let previous = super::library::pending_import(owner, source);
    super::library::revoke_import_handoff(owner, source);
    if owner.shown.with(Cell::get) != Some(source) && !super::combined::includes(owner, source) {
        return;
    }
    // A check under way would download and store the feeds as they were.
    // The check below answers for it, or the update that runs instead.
    owner.interrupt_guide_read(source);
    let edit = next_guide_edit(owner, source);
    let generation = owner.generation.with(Cell::get);
    owner.spawn({
        let owner = owner.clone();
        async move {
            owner.delay(SETTLE).await;
            // A refresh may still be installing its old guide configuration. Wait
            // for that handoff before checking the edited feeds, rather than turning
            // this check into a cache-only read and losing the edit's refresh.
            if let Some(previous) = previous {
                previous.until_finished().await;
            }
            while let Some(pending) = super::library::pending_import(&owner, source) {
                pending.until_finished().await;
            }
            let current = owner.generation.with(Cell::get) == generation
                && guide_edit_is_current(&owner, source, edit);
            if current && !updating(&owner, source) {
                log::info!("iptv data: guide sources changed; checking the guide");
                super::check_guide(&owner, source, generation);
            }
        }
    });
}

/// "Delete stored guide data": the source on screen keeps its channels
/// without programmes until its guide is downloaded again (at the next
/// start, refresh, or "Update the guide now"). The store works off the UI
/// thread; `done` hears whether the guide was deleted. False when no
/// playlist is on screen.
pub fn clear(owner: &std::rc::Rc<crate::Runtime>, done: impl FnOnce(bool) + 'static) -> bool {
    let sources = shown_sources(owner);
    if sources.is_empty() {
        return false;
    }
    // Work under way would show the guide again or store it anew.
    let superseded: Vec<_> = sources
        .iter()
        .map(|source| (*source, supersede(owner, *source)))
        .collect();
    let generation = owner.generation.with(Cell::get);
    let saved = crate::commands::clear_guides(owner, sources);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let outcome = saved.await;
            // A check stopped for it stays owed, as after a download that
            // was cancelled: still due, later, not at once.
            let later = super::now_unix() + RETRY_DUE;
            for (source, superseded) in superseded {
                superseded.keep_check(&owner, source, generation, later);
            }
            let Ok((deleted, imports)) = outcome else {
                return done(false);
            };
            for imported in imports {
                super::show(&owner, Box::new(imported), generation, false).await;
            }
            done(deleted);
        }
    });
    true
}
