//! Stored playlist mutations, the library snapshot, and import preparation.
//!
//! One refresh per playlist runs at a time; each has a ticket, so a
//! canceled one never overwrites what a newer one reports.
use std::rc::Rc;

use crate::guides;
use crate::imports::{Job, Purpose, Update};
use crate::model::{SourceId, StoreError, StoredSource};
use iptv_core::add_playlist::FailureKind;
use iptv_import::CancelToken;

pub use crate::model::Activity;

pub use iptv_library::Playlist;

pub(super) use crate::library::{
    ImportCompletion, Snapshot, cancel_running, import_completion, import_is_current,
    import_revision, import_revision_is_current, install, load, newest, pending_import,
    replace_import, replace_running, reset_imports, revoke_import_handoff, running,
    supersede_import, track_added_import, track_import,
};
pub(super) async fn refresh_cache(owner: &Rc<crate::Runtime>) -> Result<(), StoreError> {
    let snapshot = crate::library::snapshot(owner).await?;
    install(owner, snapshot);
    changed(owner);
    Ok(())
}

/// A committed mutation must retain its cleanup even if a subsequent read fails.
async fn refresh_after_commit(owner: &Rc<crate::Runtime>, patch: impl FnOnce(&mut Snapshot)) {
    owner.library.with(|snapshot| {
        if let Some(snapshot) = snapshot.borrow_mut().as_mut() {
            patch(snapshot);
        }
    });
    changed(owner);
    if let Err(error) = refresh_cache(owner).await {
        log::warn!("iptv library: change saved but metadata refresh failed: {error}");
        owner.emit(crate::RuntimeEvent::Notify(
            "Change saved; library details could not be refreshed.",
        ));
    }
}

pub fn playlists(owner: &Rc<crate::Runtime>) -> Result<Vec<Playlist>, crate::Error> {
    crate::library::playlists(owner)
}

pub fn policy(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
) -> Result<crate::model::SourcePolicy, crate::Error> {
    crate::library::policy(owner, id)
}

pub(super) async fn set_policy(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
    policy: crate::model::SourcePolicy,
) -> Result<(), StoreError> {
    crate::library::save_policy(owner, id, policy).await?;
    refresh_after_commit(owner, |snapshot| {
        if let Some(playlist) = snapshot.playlists.iter_mut().find(|p| p.id == id) {
            playlist.policy = policy;
        }
    })
    .await;
    Ok(())
}

/// Persists the shown identity without delaying navigation.
pub(super) fn remember_shown(owner: &Rc<crate::Runtime>, id: SourceId) {
    let saved = crate::library::remember_shown(owner, id);
    owner.spawn(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv data: could not remember the shown playlist: {error}");
        }
    });
}

fn changed(owner: &Rc<crate::Runtime>) {
    owner.emit(crate::RuntimeEvent::LibraryChanged);
}

/// Prepares a stored import, its cancellation ticket, and its activity reporter.
/// With `edit`, `stored.source` holds new details to try and only saves
/// them if they import.
pub(super) fn prepare_import(
    owner: &Rc<crate::Runtime>,
    stored: StoredSource,
    purpose: Purpose,
    edit: bool,
) -> (Job, CancelToken, impl Fn(Update) + 'static) {
    let id = stored.id;
    // A mode switch can replace a still-running job for this source.
    replace_running(owner, id);
    let due = !matches!(
        purpose,
        Purpose::Startup | Purpose::Switch | Purpose::Combined
    ) || owner.library.with(|snapshot| {
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
                crate::time::now_unix(),
                status.last_attempt_at,
                matches!(purpose, Purpose::Startup | Purpose::Combined),
            )
    });
    if due {
        owner
            .attempts
            .with(|a| a.borrow_mut().insert(id, crate::time::now_unix()));
    }
    log::info!(
        "iptv data: {purpose:?} of source {} ({:?})",
        id.0,
        stored.source.kind()
    );
    let ticket = owner.tickets.with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    let current = {
        let owner = owner.clone();
        move || {
            owner
                .imports
                .with(|r| r.borrow().get(&id).is_some_and(|(at, _)| *at == ticket))
        }
    };
    let job = Job {
        source: stored.source,
        epg_url: None,
        stored: Some(id),
        purpose,
        edit,
    };
    let token = CancelToken::new();
    owner
        .imports
        .with(|r| r.borrow_mut().insert(id, (ticket, token.clone())));
    crate::library::set_activity(
        owner,
        id,
        Some(if edit {
            Activity::Checking(None)
        } else {
            Activity::Running(None)
        }),
    );
    let reporting = owner.clone();
    (job, token, move |update| {
        let owner = &reporting;
        if matches!(update, Update::Done(_)) {
            owner.spawn(crate::favorites::refresh_vod_availability(owner));
        }
        if !current() {
            // Canceled or replaced by a newer import of it, which reports.
            if matches!(update, Update::Done(_)) {
                changed(owner);
            }
            return;
        }
        let activity = match update {
            Update::Progress(p) if edit => Activity::Checking(Some(p)),
            Update::Progress(p) => Activity::Running(Some(p)),
            Update::Done(Err(f)) if edit => {
                log::warn!("iptv data: the new details did not import ({f:?}); the old ones stay");
                owner.imports.with(|r| r.borrow_mut().remove(&id));
                changed(owner);
                Activity::Rejected(f)
            }
            Update::Done(result) => {
                match &result {
                    Ok(counts) => log::info!("iptv data: refreshed {counts:?}"),
                    Err(f) => {
                        log::warn!("iptv data: refresh failed ({f:?}); the stored data stays")
                    }
                }
                owner.imports.with(|r| r.borrow_mut().remove(&id));
                changed(owner);
                if let Err(failure) = &result {
                    // The import records the precise kind of a failed
                    // download; one refused before it ran (a full queue)
                    // or one saving it is recorded here, so the store never
                    // keeps an older failure's kind for this attempt.
                    let unrecorded =
                        matches!(failure.kind, FailureKind::Busy | FailureKind::Storage);
                    let owner = owner.clone();
                    owner.clone().spawn(async move {
                        if unrecorded {
                            let recorded = owner
                                .services()
                                .call(move |store| {
                                    store.record_import_failure(
                                        id,
                                        crate::model::ImportErrorKind::Other,
                                    )
                                })
                                .await;
                            if let Err(error) = recorded {
                                log::warn!("iptv library: failure not recorded: {error}");
                            }
                        }
                        // The snapshot must not keep showing an older
                        // failure's kind.
                        if let Err(error) = refresh_cache(&owner).await {
                            log::warn!("iptv library: failure recorded, details stale: {error}");
                        }
                    });
                }
                Activity::Done(result)
            }
        };
        crate::library::set_activity(owner, id, Some(activity));
    })
}

/// Stored playlist `id` with its details (secrets included: for the
/// details form only, never shown or logged).
pub use crate::library::source_details;

pub(super) async fn rename(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
    name: &str,
) -> Result<(), StoreError> {
    let name = name.to_owned();
    let saved_name = name.clone();
    crate::library::rename_source(owner, id, name).await?;
    refresh_after_commit(owner, |snapshot| {
        if let Some(playlist) = snapshot.playlists.iter_mut().find(|p| p.id == id) {
            playlist.name = saved_name.clone();
        }
        if let Some(source) = snapshot.details.get_mut(&id) {
            source.source.name = saved_name;
        }
    })
    .await;
    Ok(())
}

/// Saves a changed guide configuration and refreshes the library metadata.
pub(super) async fn edit_guides(
    owner: &Rc<crate::Runtime>,
    id: SourceId,
    edit: &guides::Edit,
) -> Result<bool, StoreError> {
    let edit = edit.clone();
    let edited = crate::library::edit_guides(owner, id, edit).await?;
    if !edited {
        return Ok(false);
    }
    refresh_after_commit(owner, |_| {}).await;
    Ok(true)
}

pub(super) async fn delete(owner: &Rc<crate::Runtime>, id: SourceId) -> Result<(), StoreError> {
    cancel_running(owner, id);
    crate::library::delete_source(owner, id).await?;
    refresh_after_commit(owner, |snapshot| {
        snapshot.playlists.retain(|p| p.id != id);
        snapshot.details.remove(&id);
        snapshot.imports.remove(&id);
        if snapshot.active == Some(id) {
            snapshot.active = None;
        }
    })
    .await;
    // Source ids can be given again: a later playlist must not inherit
    // this one's own settings (its movie and series import, for one).
    if let Err(error) = crate::settings::forget_playlist(owner, id) {
        log::warn!("iptv data: the deleted playlist's settings stay: {error}");
    }
    crate::favorites::refresh_vod_availability(owner).await;
    log::info!("iptv data: deleted source {}", id.0);
    owner.emit(crate::RuntimeEvent::ForgetSource(id));
    crate::library::set_activity(owner, id, None);
    changed(owner);
    Ok(())
}
