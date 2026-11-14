//! The stored playlists (sources) as the playlists screen sees them, and
//! what it does to them: which one is shown (remembered in the store, so
//! the next start opens it), refresh, rename, its guide sources, delete.
//!
//! One refresh per playlist runs at a time; each has a ticket, so a
//! cancelled one never overwrites what a newer one reports.

use std::collections::BTreeMap;

use crate::runtime::model::{SourceId, StoreError, StoredSource};
use iptv_import::CancelToken;
use iptv_providers::model::{Source, SourceConfig};
use nsh_core::prelude::*;

use super::{Job, Purpose, Update, guides, start_import_with, stored};
use crate::Ui;

/// What the data layer tells the screens about the stored playlists.
#[derive(Clone, Copy)]
pub struct Library {
    /// The stored playlist on screen; `None` before one is (first run).
    pub shown: Signal<Option<SourceId>>,
    pub all: Signal<bool>,
    /// Refreshes of stored playlists since the app started.
    pub activity: Signal<BTreeMap<SourceId, Activity>>,
    /// Bumped when what is stored about the playlists changed.
    pub version: Signal<u64>,
}

impl Library {
    pub fn new() -> Self {
        Self {
            shown: signal(None),
            all: signal(false),
            activity: signal(BTreeMap::new()),
            version: signal(0),
        }
    }
}

pub use crate::runtime::library::Activity;

pub use iptv_library::Playlist;

pub(super) use crate::runtime::library::*;
pub(super) async fn refresh_cache(ui: Ui) -> Result<(), StoreError> {
    let snapshot = crate::runtime::library::snapshot().await?;
    install(snapshot);
    changed(ui);
    Ok(())
}

/// A committed mutation must retain its cleanup even if a subsequent read fails.
async fn refresh_after_commit(ui: Ui, patch: impl FnOnce(&mut Snapshot)) {
    crate::runtime().library().with(|snapshot| {
        if let Some(snapshot) = snapshot.borrow_mut().as_mut() {
            patch(snapshot);
        }
    });
    changed(ui);
    if let Err(error) = refresh_cache(ui).await {
        log::warn!("iptv library: change saved but metadata refresh failed: {error}");
        crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
            "Change saved; library details could not be refreshed.",
        ));
    }
}

pub use crate::runtime::library::{playlists, policy};

pub async fn set_policy(
    ui: Ui,
    id: SourceId,
    policy: crate::runtime::model::SourcePolicy,
) -> Result<(), StoreError> {
    crate::runtime::library::save_policy(id, policy).await?;
    refresh_after_commit(ui, |snapshot| {
        if let Some(playlist) = snapshot.playlists.iter_mut().find(|p| p.id == id) {
            playlist.policy = policy;
        }
    })
    .await;
    if super::combined::active(ui) {
        if !policy.enabled {
            cancel_running(id);
        }
        super::combined::rebuild(ui);
        if policy.enabled
            && let Some(source) = load(id)
        {
            run(ui, source, Purpose::Combined, false);
        }
        return Ok(());
    }
    if !policy.enabled {
        cancel_running(id);
        if ui.library.shown.get_untracked() == Some(id) {
            let next = newest()?;
            super::show_nothing(ui);
            match next {
                Some(next) => run(ui, next, Purpose::Switch, false),
                None => crate::runtime().emit(crate::runtime::RuntimeEvent::OpenPlaylists),
            }
        }
    }
    Ok(())
}
/// Persists the shown identity without delaying navigation.
pub(super) fn remember_shown(id: SourceId) {
    let saved = crate::runtime::library::remember_shown(id);
    super::spawn_in_app(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv data: could not remember the shown playlist: {error}");
        }
    });
}

fn changed(_ui: Ui) {
    crate::runtime().emit(crate::runtime::RuntimeEvent::LibraryChanged);
}

/// Imports stored playlist `stored` for `purpose`, reporting into the
/// library's activity. With `edit`, `stored.source` holds new details to
/// try (saved only if they import).
pub(super) fn run(ui: Ui, stored: StoredSource, purpose: Purpose, edit: bool) {
    let id = stored.id;
    // A mode switch can replace a still-running job for this source.
    cancel_running(id);
    let due = !matches!(
        purpose,
        Purpose::Startup | Purpose::Switch | Purpose::Combined
    ) || crate::runtime().library().with(|snapshot| {
        let snapshot = snapshot.borrow();
        let Some(snapshot) = snapshot.as_ref() else {
            return true;
        };
        let Some(playlist) = snapshot.playlists.iter().find(|playlist| playlist.id == id) else {
            return true;
        };
        let Some(status) = snapshot.imports.get(&id) else {
            return true;
        };
        status.last_success_at.is_none()
            || playlist.policy.due(
                super::now_unix(),
                status.last_attempt_at,
                matches!(purpose, Purpose::Startup | Purpose::Combined),
            )
    });
    if due {
        crate::runtime()
            .attempts()
            .with(|a| a.borrow_mut().insert(id, super::now_unix()));
    }
    log::info!(
        "iptv data: {purpose:?} of source {} ({:?})",
        id.0,
        stored.source.kind()
    );
    let ticket = crate::runtime().tickets().with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    let current = move || {
        crate::runtime()
            .imports()
            .with(|r| r.borrow().get(&id).is_some_and(|(at, _)| *at == ticket))
    };
    let job = Job {
        source: stored.source,
        epg_url: None,
        stored: Some(id),
        purpose,
        edit,
    };
    let token = CancelToken::new();
    crate::runtime()
        .imports()
        .with(|r| r.borrow_mut().insert(id, (ticket, token.clone())));
    crate::runtime::library::set_activity(
        id,
        Some(if edit {
            Activity::Checking(None)
        } else {
            Activity::Running(None)
        }),
    );
    start_import_with(ui, job, token, move |update| {
        if !current() {
            // Cancelled or replaced by a newer import of it, which reports.
            if matches!(update, Update::Done(_)) {
                changed(ui);
            }
            return;
        }
        let activity = match update {
            Update::Progress(p) if edit => Activity::Checking(Some(p)),
            Update::Progress(p) => Activity::Running(Some(p)),
            Update::Done(Err(f)) if edit => {
                log::warn!("iptv data: the new details did not import ({f:?}); the old ones stay");
                crate::runtime()
                    .imports()
                    .with(|r| r.borrow_mut().remove(&id));
                changed(ui);
                Activity::Rejected(f)
            }
            Update::Done(result) => {
                match &result {
                    Ok(counts) => log::info!("iptv data: refreshed {counts:?}"),
                    Err(f) => {
                        log::warn!("iptv data: refresh failed ({f:?}); the stored data stays")
                    }
                }
                crate::runtime()
                    .imports()
                    .with(|r| r.borrow_mut().remove(&id));
                changed(ui);
                Activity::Done(result)
            }
        };
        crate::runtime::library::set_activity(id, Some(activity));
    });
}

/// Shows stored playlist `id`: its stored channels and guide at once,
/// refreshed in the background as at startup.
pub fn switch_to(ui: Ui, id: SourceId) {
    super::spawn_in_app(async move {
        if crate::favorites::flush().await {
            switch_to_ready(ui, id);
        } else {
            crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
                "Recent changes could not be saved; try again",
            ));
        }
    });
}
fn switch_to_ready(ui: Ui, id: SourceId) {
    if policy(id).is_ok_and(|p| !p.enabled) {
        return;
    }
    if !super::combined::active(ui) && ui.library.shown.get_untracked() == Some(id) {
        return;
    }
    let Some(stored) = load(id) else {
        return;
    };
    super::combined::leave(ui);
    // A refresh of it in the background gives way to the shown one.
    cancel_running(id);
    run(ui, stored, Purpose::Switch, false);
}

/// Downloads stored playlist `id` again (unless that already runs): the
/// shown one in place, another one into the store only.
pub fn refresh(ui: Ui, id: SourceId) {
    if running(id) {
        return;
    }
    let Some(stored) = load(id) else {
        return;
    };
    run(ui, stored, refresh_purpose(ui, id), false);
}

/// The shown playlist refreshes in place, another one into the store only.
fn refresh_purpose(ui: Ui, id: SourceId) -> Purpose {
    if super::combined::active(ui) {
        Purpose::Background
    } else if ui.library.shown.get_untracked() == Some(id) {
        Purpose::Refresh
    } else {
        Purpose::Background
    }
}

/// Refreshes every stored playlist (each one not refreshing already).
pub fn refresh_all(ui: Ui) {
    match playlists() {
        Ok(all) => all
            .iter()
            .filter(|p| p.policy.enabled)
            .for_each(|p| refresh(ui, p.id)),
        Err(e) => log::warn!("iptv data: the store is unavailable: {e}"),
    }
}

/// Stored playlist `id` with its details (secrets included: for the
/// details form only, never shown or logged).
pub use crate::runtime::library::source_details;

pub fn try_details(ui: Ui, id: SourceId, source: Source) {
    let Some(mut stored) = load(id) else {
        return;
    };
    // Only the details change; the name is the stored one.
    stored.source.config = source.config;
    cancel_running(id);
    run(ui, stored, refresh_purpose(ui, id), true);
}

/// Sets the User-Agent of playlist `id` (`None`: the app's own), tried
/// like other new details.
pub fn set_user_agent(ui: Ui, id: SourceId, user_agent: Option<String>) {
    let Some(mut source) = source_details(id) else {
        return;
    };
    match &mut source.config {
        SourceConfig::M3u(c) => c.user_agent = user_agent,
        SourceConfig::Xtream(c) => c.user_agent = user_agent,
        SourceConfig::Stalker(_) => return,
    }
    try_details(ui, id, source);
}

pub async fn rename(ui: Ui, id: SourceId, name: &str) -> Result<(), StoreError> {
    let name = name.to_owned();
    let saved_name = name.clone();
    crate::runtime::library::rename_source(id, name).await?;
    refresh_after_commit(ui, |snapshot| {
        if let Some(playlist) = snapshot.playlists.iter_mut().find(|p| p.id == id) {
            playlist.name = saved_name.clone();
        }
        if let Some(source) = snapshot.details.get_mut(&id) {
            source.source.name = saved_name;
        }
    })
    .await;
    if super::combined::active(ui) {
        super::combined::rebuild(ui);
    }
    Ok(())
}

/// Changes the guide sources of playlist `id`; false when `edit` changes
/// nothing. The guide on screen is checked again once the edits settle
/// (changed feeds are downloaded); another playlist's when it is shown.
pub async fn edit_guides(ui: Ui, id: SourceId, edit: &guides::Edit) -> Result<bool, StoreError> {
    let edit = edit.clone();
    let edited = crate::runtime::library::edit_guides(id, edit).await?;
    if !edited {
        return Ok(false);
    }
    refresh_after_commit(ui, |_| {}).await;
    stored::guides_changed(ui, id);
    Ok(true)
}

/// Deletes stored playlist `id` with its channels, guide and favourites.
/// The shown one gives way to the newest other playlist, or to the
/// first-run state when none is left.
pub async fn delete(ui: Ui, id: SourceId) -> Result<(), StoreError> {
    cancel_running(id);
    crate::runtime::library::delete_source(id).await?;
    refresh_after_commit(ui, |snapshot| {
        snapshot.playlists.retain(|p| p.id != id);
        snapshot.details.remove(&id);
        snapshot.imports.remove(&id);
        if snapshot.active == Some(id) {
            snapshot.active = None;
        }
    })
    .await;
    log::info!("iptv data: deleted source {}", id.0);
    crate::runtime().emit(crate::runtime::RuntimeEvent::ForgetSource(id));
    crate::runtime::library::set_activity(id, None);
    changed(ui);
    if super::combined::active(ui) {
        // Keep the explicitly selected aggregate usable when its last source
        // is deleted; the playlist screen still offers Add playlist.
        super::combined::rebuild(ui);
        return Ok(());
    }
    if playlists()?.is_empty() {
        super::show_nothing(ui);
        crate::runtime().emit(crate::runtime::RuntimeEvent::ClosePlaylists);
        crate::runtime().emit(crate::runtime::RuntimeEvent::FirstRun);
        return Ok(());
    }
    if ui.library.shown.get_untracked() != Some(id) {
        return Ok(());
    }
    match newest()? {
        Some(next) => run(ui, next, Purpose::Switch, false),
        None => {
            super::show_nothing(ui);
            crate::runtime().emit(crate::runtime::RuntimeEvent::ClosePlaylists);
            if playlists()?.is_empty() {
                crate::runtime().emit(crate::runtime::RuntimeEvent::FirstRun);
            } else {
                crate::runtime().emit(crate::runtime::RuntimeEvent::OpenPlaylists);
            }
        }
    }
    Ok(())
}
