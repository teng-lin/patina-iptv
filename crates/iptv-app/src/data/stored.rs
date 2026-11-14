//! The store's side of an import: a stored source's catalog and guide,
//! shown before any network, and keeping the guide in the store under the
//! refresh policy (`iptv_import::decide`).
//!
//! A guide download streams into a stage file next to the store
//! (`super::download_guide`, `crate::runtime::model::EpgStage`) and replaces
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
use tokio::sync::mpsc::unbounded_channel;

use crate::runtime::model::{SourceId, StoredGuide};
use iptv_epg::{ChannelKey, UnixTime};
use iptv_import::CancelToken;

use nsh_core::prelude::timer;

use super::Msg;
use crate::Ui;

/// Refresh small UI metadata after imports/edits without querying SQLite in views.
pub fn bind(ui: Ui) {
    let revision = std::rc::Rc::new(Cell::new(0_u64));
    nsh_core::prelude::effect(move || {
        ui.library.version.get();
        ui.data_version.get();
        revision.set(revision.get().wrapping_add(1));
        let expected = revision.get();
        let revision = revision.clone();
        let loaded = crate::runtime::commands::guide_status_snapshot();
        super::spawn_in_app(async move {
            if let Ok(mut snapshot) = loaded.await
                && revision.get() == expected
            {
                let library_changed = snapshot.library.take().is_some_and(super::library::install);
                crate::runtime()
                    .status()
                    .with(|status| *status.borrow_mut() = snapshot);
                if library_changed && super::combined::active(ui) {
                    super::combined::rebuild(ui);
                }
                crate::runtime().emit(crate::runtime::RuntimeEvent::SettingsChanged);
            }
        });
    });
}

/// How long a guide check waits after the guide sources changed (more
/// edits may follow).
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

pub use crate::runtime::stored::*;

/// Fetch descriptions on the shared worker; a bounded cache makes later renders synchronous.
pub fn description_lookup(
    _ui: Ui,
    source: SourceId,
) -> Option<impl Fn(&ChannelKey, UnixTime) -> Option<String> + 'static> {
    let cache = std::rc::Rc::new(RefCell::new(
        BTreeMap::<(String, UnixTime), Option<String>>::new(),
    ));
    Some(move |key: &ChannelKey, start| {
        let identity = (key.to_string(), start);
        if let Some(description) = cache.borrow().get(&identity) {
            return description.clone();
        }
        if cache.borrow().len() >= 128 {
            cache.borrow_mut().pop_first();
        }
        cache.borrow_mut().insert(identity.clone(), None);
        let key = key.clone();
        let read = crate::runtime::commands::description(source, key, start);
        let cache = cache.clone();
        super::spawn_in_app(async move {
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
            crate::runtime().emit(crate::runtime::RuntimeEvent::GuideChanged);
        });
        None
    })
}

/// "Update the guide now" of the source on screen: see [`update_source`].
pub fn update_now(ui: Ui, done: impl FnOnce() + 'static) -> bool {
    let sources = shown_sources();
    if sources.is_empty() {
        return false;
    }
    let remaining = std::rc::Rc::new(Cell::new(sources.len()));
    let done = std::rc::Rc::new(RefCell::new(Some(done)));
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
        if updating(source) {
            remaining.set(remaining.get() - 1);
            started = true;
            continue;
        }
        if update_source(ui, source, move |_| finish()) {
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
pub fn update_source(ui: Ui, source: SourceId, done: impl FnOnce(bool) + 'static) -> bool {
    if updating(source) {
        return true;
    }
    let has_guides = crate::runtime().status().with(|status| {
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
    let loaded = crate::runtime::commands::guide_sources(source);
    crate::runtime()
        .updating()
        .with(|updating| updating.borrow_mut().push(source));
    super::spawn_in_app(async move {
        let (stored, sources) = match loaded.await {
            Ok(loaded) => loaded,
            Err(error) => {
                log::warn!("iptv data: cannot load guide sources: {error}");
                crate::runtime()
                    .updating()
                    .with(|updating| updating.borrow_mut().retain(|id| *id != source));
                done(false);
                return;
            }
        };
        let guides = if sources.is_empty() {
            with_storage(
                stored
                    .as_ref()
                    .map(StoredGuide::configs)
                    .unwrap_or_default(),
                &crate::settings::get(),
            )
        } else {
            configs(&sources)
        };
        if guides.is_empty() {
            crate::runtime()
                .updating()
                .with(|updating| updating.borrow_mut().retain(|id| *id != source));
            crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
                "No guide sources are configured",
            ));
            done(false);
            return;
        }
        super::library::revoke_import_handoff(source);
        let cancel = CancelToken::new();
        let completion = super::library::import_completion(cancel.clone());
        completion.playlist_done(); // This worker owns guide work only.
        let had_check = crate::runtime().has_guide_check(source);
        super::library::track_import(source, &completion);

        let generation = crate::runtime().generation().with(Cell::get);
        let (tx, mut rx) = unbounded_channel();
        let accepted = super::workers::submit(super::source_is_visible(ui, source), move || {
            super::with_runtime(|| download_now(source, guides, &tx, &cancel));
        });
        if !accepted {
            if had_check {
                super::schedule_check(
                    ui,
                    source,
                    super::now_unix() + super::MIN_WAIT.as_secs() as i64,
                    generation,
                );
            }
            crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
                "Too many playlists are updating. Try again shortly.",
            ));
        }
        super::spawn_in_app(async move {
            loop {
                match rx.recv().await {
                    Some(Msg::Guide(imported))
                        if (crate::runtime().shown().with(Cell::get) == Some(source)
                            || super::combined::includes(ui, source))
                            && super::library::import_is_current(source, &completion) =>
                    {
                        super::show(ui, imported, generation, false).await
                    }
                    Some(Msg::NextCheck { source, at })
                        if (crate::runtime().shown().with(Cell::get) == Some(source)
                            || super::combined::includes(ui, source))
                            && super::library::import_is_current(source, &completion) =>
                    {
                        super::schedule_check(ui, source, at, generation);
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            completion.finish();
            crate::runtime()
                .updating()
                .with(|u| u.borrow_mut().retain(|id| *id != source));
            crate::runtime().emit(crate::runtime::RuntimeEvent::LibraryChanged);
            done(accepted);
        });
    });
    true
}

/// The download of [`update_source`], on its own thread.
use crate::runtime::stored::download_now;

/// `source`'s guide sources changed: its guide is checked once the edits
/// settle (the refresh policy downloads changed feeds). Only for the
/// source on screen; another one is checked when it is shown.
pub fn guides_changed(ui: Ui, source: SourceId) {
    let previous = super::library::pending_import(source);
    super::library::revoke_import_handoff(source);
    if crate::runtime().shown().with(Cell::get) != Some(source)
        && !super::combined::includes(ui, source)
    {
        return;
    }
    let edit = next_guide_edit(source);
    let generation = crate::runtime().generation().with(Cell::get);
    super::spawn_in_app(async move {
        timer(SETTLE).await;
        // A refresh may still be installing its old guide configuration. Wait
        // for that handoff before checking the edited feeds, rather than turning
        // this check into a cache-only read and losing the edit's refresh.
        if let Some(previous) = previous {
            previous.until_finished().await;
        }
        while let Some(pending) = super::library::pending_import(source) {
            pending.until_finished().await;
        }
        let current = crate::runtime().generation().with(Cell::get) == generation
            && guide_edit_is_current(source, edit);
        if current && !updating(source) {
            log::info!("iptv data: guide sources changed; checking the guide");
            super::check_guide(ui, source, generation);
        }
    });
}

/// "Delete stored guide data": the source on screen keeps its channels
/// without programmes until its guide is downloaded again (at the next
/// start, refresh, or "Update the guide now"). The store works off the UI
/// thread; `done` hears whether the guide was deleted. False when no
/// playlist is on screen.
pub fn clear(ui: Ui, done: impl FnOnce(bool) + 'static) -> bool {
    let sources = shown_sources();
    if sources.is_empty() {
        return false;
    }
    for source in &sources {
        super::library::revoke_import_handoff(*source);
    }
    let generation = crate::runtime().generation().with(Cell::get);
    let saved = crate::runtime::commands::clear_guides(sources);
    super::spawn_in_app(async move {
        let Ok((deleted, imports)) = saved.await else {
            return done(false);
        };
        for imported in imports {
            super::show(ui, Box::new(imported), generation, false).await;
        }
        done(deleted);
    });
    true
}
