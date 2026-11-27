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
pub(super) fn switch_to_ready(owner: &Rc<crate::Runtime>, id: SourceId) {
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
pub(super) fn refresh_purpose(owner: &Rc<crate::Runtime>, id: SourceId) -> Purpose {
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

pub(super) fn run_import(owner: &Rc<Runtime>, source: StoredSource, purpose: Purpose, edit: bool) {
    let (job, cancel, report) = library::prepare_import(owner, source, purpose, edit);
    let _ = start_import_with(owner, job, cancel, |_| report);
}

pub fn start(owner: &std::rc::Rc<crate::Runtime>) {
    start_library(owner, false);
}

/// Retry the startup library read after a user has addressed a storage failure.
pub fn retry_startup(owner: &std::rc::Rc<crate::Runtime>) {
    start_library(owner, true);
}

pub(super) fn start_library(owner: &std::rc::Rc<crate::Runtime>, recovering: bool) {
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
pub(super) fn next_generation(owner: &std::rc::Rc<crate::Runtime>, shown: Option<SourceId>) -> u64 {
    owner.cancel_guide_checks_keeping(shown);
    owner.imports.generation.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}
