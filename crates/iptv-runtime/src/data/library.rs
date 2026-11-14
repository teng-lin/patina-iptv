//! The stored playlists (sources) as the playlists screen sees them, and
//! what it does to them: which one is shown (remembered in the store, so
//! the next start opens it), refresh, rename, its guide sources, delete.
//!
//! One refresh per playlist runs at a time; each has a ticket, so a
//! cancelled one never overwrites what a newer one reports.

use crate::model::{SourceId, StoreError, StoredSource};
use iptv_import::CancelToken;
use iptv_providers::model::{Source, SourceConfig};

use super::{Job, Purpose, Update, guides, start_import_with, stored};

pub use crate::library::Activity;

pub use iptv_library::Playlist;

pub(super) use crate::library::{
    ImportCompletion, Snapshot, cancel_running, import_completion, import_is_current,
    import_revision, import_revision_is_current, install, load, newest, pending_import,
    replace_import, replace_running, reset_imports, revoke_import_handoff, running,
    supersede_import, track_added_import, track_import,
};
pub(super) async fn refresh_cache() -> Result<(), StoreError> {
    let snapshot = crate::library::snapshot().await?;
    install(snapshot);
    changed();
    Ok(())
}

/// A committed mutation must retain its cleanup even if a subsequent read fails.
async fn refresh_after_commit(patch: impl FnOnce(&mut Snapshot)) {
    crate::current().library().with(|snapshot| {
        if let Some(snapshot) = snapshot.borrow_mut().as_mut() {
            patch(snapshot);
        }
    });
    changed();
    if let Err(error) = refresh_cache().await {
        log::warn!("iptv library: change saved but metadata refresh failed: {error}");
        crate::current().emit(crate::RuntimeEvent::Notify(
            "Change saved; library details could not be refreshed.",
        ));
    }
}

pub use crate::library::{playlists, policy};

pub async fn set_policy(
    id: SourceId,
    policy: crate::model::SourcePolicy,
) -> Result<(), StoreError> {
    crate::library::save_policy(id, policy).await?;
    refresh_after_commit(|snapshot| {
        if let Some(playlist) = snapshot.playlists.iter_mut().find(|p| p.id == id) {
            playlist.policy = policy;
        }
    })
    .await;
    if super::combined::active() {
        if !policy.enabled {
            cancel_running(id);
        }
        super::combined::rebuild();
        if policy.enabled
            && let Some(source) = load(id)
        {
            run(source, Purpose::Combined, false);
        }
        return Ok(());
    }
    if !policy.enabled {
        cancel_running(id);
        if crate::current().shown.get() == Some(id) {
            let next = newest()?;
            super::show_nothing();
            match next {
                Some(next) => run(next, Purpose::Switch, false),
                None => crate::current().emit(crate::RuntimeEvent::OpenPlaylists),
            }
        }
    }
    Ok(())
}
/// Persists the shown identity without delaying navigation.
pub(super) fn remember_shown(id: SourceId) {
    let saved = crate::library::remember_shown(id);
    super::spawn_in_app(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv data: could not remember the shown playlist: {error}");
        }
    });
}

fn changed() {
    crate::current().emit(crate::RuntimeEvent::LibraryChanged);
}

/// Imports stored playlist `stored` for `purpose`, reporting into the
/// library's activity. With `edit`, `stored.source` holds new details to
/// try (saved only if they import).
pub(super) fn run(stored: StoredSource, purpose: Purpose, edit: bool) {
    let id = stored.id;
    // A mode switch can replace a still-running job for this source.
    replace_running(id);
    let due = !matches!(
        purpose,
        Purpose::Startup | Purpose::Switch | Purpose::Combined
    ) || crate::current().library().with(|snapshot| {
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
        crate::current()
            .attempts()
            .with(|a| a.borrow_mut().insert(id, super::now_unix()));
    }
    log::info!(
        "iptv data: {purpose:?} of source {} ({:?})",
        id.0,
        stored.source.kind()
    );
    let ticket = crate::current().tickets().with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    let current = move || {
        crate::current()
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
    crate::current()
        .imports()
        .with(|r| r.borrow_mut().insert(id, (ticket, token.clone())));
    crate::library::set_activity(
        id,
        Some(if edit {
            Activity::Checking(None)
        } else {
            Activity::Running(None)
        }),
    );
    start_import_with(job, token, move |update| {
        if !current() {
            // Cancelled or replaced by a newer import of it, which reports.
            if matches!(update, Update::Done(_)) {
                changed();
            }
            return;
        }
        let activity = match update {
            Update::Progress(p) if edit => Activity::Checking(Some(p)),
            Update::Progress(p) => Activity::Running(Some(p)),
            Update::Done(Err(f)) if edit => {
                log::warn!("iptv data: the new details did not import ({f:?}); the old ones stay");
                crate::current()
                    .imports()
                    .with(|r| r.borrow_mut().remove(&id));
                changed();
                Activity::Rejected(f)
            }
            Update::Done(result) => {
                match &result {
                    Ok(counts) => log::info!("iptv data: refreshed {counts:?}"),
                    Err(f) => {
                        log::warn!("iptv data: refresh failed ({f:?}); the stored data stays")
                    }
                }
                crate::current()
                    .imports()
                    .with(|r| r.borrow_mut().remove(&id));
                changed();
                Activity::Done(result)
            }
        };
        crate::library::set_activity(id, Some(activity));
    });
}

/// Shows stored playlist `id`: its stored channels and guide at once,
/// refreshed in the background as at startup.
pub fn switch_to(id: SourceId) {
    super::spawn_in_app(async move {
        if crate::favorites::flush().await {
            switch_to_ready(id);
        } else {
            crate::current().emit(crate::RuntimeEvent::Notify(
                "Recent changes could not be saved; try again",
            ));
        }
    });
}
fn switch_to_ready(id: SourceId) {
    if policy(id).is_ok_and(|p| !p.enabled) {
        return;
    }
    if !super::combined::active() && crate::current().shown.get() == Some(id) {
        return;
    }
    let Some(stored) = load(id) else {
        return;
    };
    super::combined::leave();
    // A refresh of it in the background gives way to the shown one.
    replace_running(id);
    run(stored, Purpose::Switch, false);
}

/// Downloads stored playlist `id` again (unless that already runs): the
/// shown one in place, another one into the store only.
pub fn refresh(id: SourceId) {
    if running(id) {
        return;
    }
    let Some(stored) = load(id) else {
        return;
    };
    run(stored, refresh_purpose(id), false);
}

/// The shown playlist refreshes in place, another one into the store only.
fn refresh_purpose(id: SourceId) -> Purpose {
    if super::combined::active() {
        Purpose::Background
    } else if crate::current().shown.get() == Some(id) {
        Purpose::Refresh
    } else {
        Purpose::Background
    }
}

/// Refreshes every stored playlist (each one not refreshing already).
pub fn refresh_all() {
    match playlists() {
        Ok(all) => all
            .iter()
            .filter(|p| p.policy.enabled)
            .for_each(|p| refresh(p.id)),
        Err(e) => log::warn!("iptv data: the store is unavailable: {e}"),
    }
}

/// Stored playlist `id` with its details (secrets included: for the
/// details form only, never shown or logged).
pub use crate::library::source_details;

pub fn try_details(id: SourceId, source: Source) {
    let Some(mut stored) = load(id) else {
        return;
    };
    // Only the details change; the name is the stored one.
    stored.source.config = source.config;
    replace_running(id);
    run(stored, refresh_purpose(id), true);
}

/// Sets the User-Agent of playlist `id` (`None`: the app's own), tried
/// like other new details.
pub fn set_user_agent(id: SourceId, user_agent: Option<String>) {
    let Some(mut source) = source_details(id) else {
        return;
    };
    match &mut source.config {
        SourceConfig::M3u(c) => c.user_agent = user_agent,
        SourceConfig::Xtream(c) => c.user_agent = user_agent,
        SourceConfig::Stalker(_) => return,
    }
    try_details(id, source);
}

pub async fn rename(id: SourceId, name: &str) -> Result<(), StoreError> {
    let name = name.to_owned();
    let saved_name = name.clone();
    crate::library::rename_source(id, name).await?;
    refresh_after_commit(|snapshot| {
        if let Some(playlist) = snapshot.playlists.iter_mut().find(|p| p.id == id) {
            playlist.name = saved_name.clone();
        }
        if let Some(source) = snapshot.details.get_mut(&id) {
            source.source.name = saved_name;
        }
    })
    .await;
    if super::combined::active() {
        super::combined::rebuild();
    }
    Ok(())
}

/// Changes the guide sources of playlist `id`; false when `edit` changes
/// nothing. The guide on screen is checked again once the edits settle
/// (changed feeds are downloaded); another playlist's when it is shown.
pub async fn edit_guides(id: SourceId, edit: &guides::Edit) -> Result<bool, StoreError> {
    let edit = edit.clone();
    let edited = crate::library::edit_guides(id, edit).await?;
    if !edited {
        return Ok(false);
    }
    refresh_after_commit(|_| {}).await;
    stored::guides_changed(id);
    Ok(true)
}

/// Deletes stored playlist `id` with its channels, guide and favourites.
/// The shown one gives way to the newest other playlist, or to the
/// first-run state when none is left.
pub async fn delete(id: SourceId) -> Result<(), StoreError> {
    let generation = super::generation();
    cancel_running(id);
    if let Err(error) = crate::library::delete_source(id).await {
        // The playlist stays, and so does its guide check, which the
        // cancelling ended (work that was cancelled keeps one itself).
        super::keep_guide_check(id, generation, None);
        return Err(error);
    }
    refresh_after_commit(|snapshot| {
        snapshot.playlists.retain(|p| p.id != id);
        snapshot.details.remove(&id);
        snapshot.imports.remove(&id);
        if snapshot.active == Some(id) {
            snapshot.active = None;
        }
    })
    .await;
    log::info!("iptv data: deleted source {}", id.0);
    crate::current().emit(crate::RuntimeEvent::ForgetSource(id));
    crate::library::set_activity(id, None);
    changed();
    if super::combined::active() {
        // Keep the explicitly selected aggregate usable when its last source
        // is deleted; the playlist screen still offers Add playlist.
        super::combined::rebuild();
        return Ok(());
    }
    if playlists()?.is_empty() {
        super::show_nothing();
        crate::current().emit(crate::RuntimeEvent::ClosePlaylists);
        crate::current().emit(crate::RuntimeEvent::FirstRun);
        return Ok(());
    }
    if crate::current().shown.get() != Some(id) {
        return Ok(());
    }
    match newest()? {
        Some(next) => run(next, Purpose::Switch, false),
        None => {
            super::show_nothing();
            crate::current().emit(crate::RuntimeEvent::ClosePlaylists);
            if playlists()?.is_empty() {
                crate::current().emit(crate::RuntimeEvent::FirstRun);
            } else {
                crate::current().emit(crate::RuntimeEvent::OpenPlaylists);
            }
        }
    }
    Ok(())
}
