//! Library snapshots, refresh tickets and named persistence commands.
use crate::model::Activity;
use crate::model::StoredSource;
use crate::store_service::StoreError;
use iptv_import::CancelToken;
use iptv_library::{Playlist, SourceId, SourcePolicy};
use iptv_providers::model::Source;
#[cfg(test)]
use iptv_providers::model::SourceConfig;
#[cfg(test)]
use iptv_store_sqlite::Store;
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};
pub type ImportCompletion = Rc<ImportWork>;
pub use crate::state::ImportWork;
pub fn import_completion(cancel: CancelToken) -> ImportCompletion {
    Rc::new(ImportWork {
        finished: Cell::new(false),
        catalog_done: Cell::new(false),
        cancel,
        guide: CancelToken::new(),
        superseded: Cell::new(false),
        waiting: RefCell::default(),
    })
}
pub fn track_import(owner: &super::Runtime, source: SourceId, completion: &ImportCompletion) {
    owner.cancel_guide_check(source);
    let replaced = owner
        .imports
        .completions
        .with(|jobs| jobs.borrow_mut().insert(source, Rc::downgrade(completion)));
    if let Some(replaced) = replaced.and_then(|job| job.upgrade())
        && !Rc::ptr_eq(&replaced, completion)
    {
        replaced.superseded.set(true);
    }
}
/// An Add learns its source ID only after saving. A newer known-source job may
/// already own that ID by then, including a worker that has just completed.
pub fn track_added_import(owner: &super::Runtime, source: SourceId, completion: &ImportCompletion) {
    owner.imports.completions.with(|jobs| {
        jobs.borrow_mut()
            .entry(source)
            .or_insert_with(|| Rc::downgrade(completion));
    });
}
pub type ImportRevision = Option<Weak<ImportWork>>;
pub fn import_revision(owner: &super::Runtime, source: SourceId) -> ImportRevision {
    owner
        .imports
        .completions
        .with(|jobs| jobs.borrow().get(&source).cloned())
}
pub fn import_revision_is_current(
    owner: &super::Runtime,
    source: SourceId,
    revision: &ImportRevision,
) -> bool {
    match (import_revision(owner, source), revision) {
        (None, None) => true,
        (Some(current), Some(previous)) => Weak::ptr_eq(&current, previous),
        _ => false,
    }
}
pub fn pending_import(owner: &super::Runtime, source: SourceId) -> Option<ImportCompletion> {
    owner
        .imports
        .completions
        .with(|jobs| jobs.borrow().get(&source).and_then(Weak::upgrade))
        .filter(|job| !job.finished())
}
pub fn import_is_current(
    owner: &super::Runtime,
    source: SourceId,
    completion: &ImportCompletion,
) -> bool {
    owner.imports.completions.with(|jobs| {
        jobs.borrow()
            .get(&source)
            .and_then(Weak::upgrade)
            .is_some_and(|current| Rc::ptr_eq(&current, completion))
    })
}
pub fn revoke_import_handoff(owner: &super::Runtime, source: SourceId) {
    owner.repairs.borrow_mut().revoke(source);
    if let Some(job) = owner.imports.completions.with(|jobs| {
        jobs.borrow_mut()
            .remove(&source)
            .and_then(|job| job.upgrade())
    }) {
        // Guide edits may happen during a source's authentication/catalog
        // phase. Revoke handoff then, but let that phase establish playback.
        job.cancel_guide();
    }
}
/// A guide action replaces `source`'s pending work: revokes its handoff
/// ([`revoke_import_handoff`]) and returns that work when this canceled
/// it (its guide phase). Such work only has to unwind, which what follows
/// may wait for; an import still fetching its playlist goes on and is not
/// returned.
pub fn supersede_import(owner: &super::Runtime, source: SourceId) -> Option<ImportCompletion> {
    let pending = pending_import(owner, source);
    revoke_import_handoff(owner, source);
    pending.filter(|job| job.cancel.is_canceled())
}
/// [`supersede_import`] by newer work for `source` (an update asked for):
/// the replaced work leaves the next guide check to it
/// (`ImportWork::superseded`).
pub fn replace_import(owner: &super::Runtime, source: SourceId) -> Option<ImportCompletion> {
    if let Some(pending) = pending_import(owner, source) {
        pending.superseded.set(true);
    }
    supersede_import(owner, source)
}
pub fn reset_imports(owner: &super::Runtime) {
    owner.imports.completions.with(|jobs| {
        for job in jobs
            .borrow_mut()
            .drain()
            .filter_map(|(_, job)| job.upgrade())
        {
            job.cancel.cancel();
        }
    });
}

pub use crate::state::LibrarySnapshot as Snapshot;

pub fn install(owner: &super::Runtime, snapshot: Snapshot) -> bool {
    owner.library.with(|current| {
        let changed = current.borrow().as_ref() != Some(&snapshot);
        *current.borrow_mut() = Some(snapshot);
        changed
    })
}

/// The stored playlists in list order.
fn playlists_native(owner: &super::Runtime) -> Result<Vec<Playlist>, StoreError> {
    let mut playlists = owner.library.with(|snapshot| {
        snapshot
            .borrow()
            .as_ref()
            .map(|snapshot| snapshot.playlists.clone())
            .ok_or(StoreError::Unavailable("library is loading"))
    })?;
    let order = owner.settings().choice(
        iptv_settings::keys::PLAYLIST_SORT,
        &iptv_settings::Target::Global,
    );
    sort_playlists(&mut playlists, order);
    Ok(playlists)
}

/// The source IDs are assigned on insertion. They retain the order the
/// surviving playlists were added in, even after changes to manual positions.
fn sort_playlists(playlists: &mut [Playlist], order: &str) {
    match order {
        "added" => playlists.sort_by_key(|p| p.id),
        "name" => playlists.sort_by_cached_key(|p| p.name.to_lowercase()),
        _ => {}
    }
}

pub fn reorder_sources(
    owner: &super::Runtime,
    order: Vec<SourceId>,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner
        .services()
        .catalog()
        .set_source_order(order, owner.parental_snapshot())
}
/// The playlist to open at startup: the one shown last, else the newest.
#[cfg(test)]
pub(crate) fn startup_source(store: &Store) -> Result<Option<StoredSource>, StoreError> {
    if let Some(id) = store.active_source()?
        && store.source_policy(id)?.enabled
    {
        return store.source(id);
    }
    for summary in store.sources()?.iter().rev() {
        if store.source_policy(summary.id)?.enabled {
            return store.source(summary.id);
        }
    }
    Ok(None)
}

pub fn newest(owner: &super::Runtime) -> Result<Option<StoredSource>, StoreError> {
    newest_excluding(owner, None)
}

pub(crate) fn newest_excluding(
    owner: &super::Runtime,
    excluded: Option<SourceId>,
) -> Result<Option<StoredSource>, StoreError> {
    let eligible = |p: &Playlist| Some(p.id) != excluded && p.policy.enabled;
    owner.library.with(|snapshot| {
        let snapshot = snapshot.borrow();
        let snapshot = snapshot
            .as_ref()
            .ok_or(StoreError::Unavailable("library is loading"))?;
        let active = snapshot.active.filter(|id| {
            snapshot
                .playlists
                .iter()
                .any(|p| p.id == *id && eligible(p))
        });
        let selected = active.or_else(|| {
            snapshot
                .playlists
                .iter()
                .rev()
                .find(|p| eligible(p))
                .map(|p| p.id)
        });
        Ok(selected.and_then(|id| snapshot.details.get(&id).cloned()))
    })
}

fn policy_native(owner: &super::Runtime, id: SourceId) -> Result<SourcePolicy, StoreError> {
    playlists(owner)
        .map_err(crate::Error::into_store_error)?
        .into_iter()
        .find(|source| source.id == id)
        .map(|source| source.policy)
        .ok_or(StoreError::SourceNotFound(id))
}

pub fn load(owner: &super::Runtime, id: SourceId) -> Option<StoredSource> {
    owner.library.with(|snapshot| {
        snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.details.get(&id).cloned())
    })
}

pub fn running(owner: &super::Runtime, id: SourceId) -> bool {
    owner.imports.imports.with(|r| r.borrow().contains_key(&id))
}

/// [`cancel_running`] for a newer import of `id` that starts now: the
/// canceled work leaves the next guide check to it
/// (`ImportWork::superseded`). Not for a playlist that is being deleted or
/// switched off: when that fails the playlist stays, and its canceled
/// work still has to keep its check.
pub fn replace_running(owner: &super::Runtime, id: SourceId) {
    if let Some(pending) = pending_import(owner, id) {
        pending.superseded.set(true);
    }
    cancel_running(owner, id);
}

pub fn cancel_running(owner: &super::Runtime, id: SourceId) {
    owner.cancel_guide_check(id);
    revoke_import_handoff(owner, id);
    if let Some((_, token)) = owner.imports.imports.with(|r| r.borrow_mut().remove(&id)) {
        log::info!("iptv data: canceling the running import of source {}", id.0);
        token.cancel();
    }
}

/// Stored playlist `id` with its details (secrets included: for the
/// details form only, never shown or logged).
pub fn source_details(owner: &super::Runtime, id: SourceId) -> Option<Source> {
    load(owner, id).map(|stored| stored.source)
}

pub fn snapshot(
    owner: &super::Runtime,
) -> impl std::future::Future<Output = Result<Snapshot, StoreError>> + use<> {
    let services = owner.services();
    async move { services.catalog().snapshot().await }
}

pub fn update_policy(
    owner: &super::Runtime,
    id: SourceId,
    edit: impl FnOnce(&mut iptv_library::SourcePolicy) + Send + 'static,
) -> impl std::future::Future<Output = Result<(iptv_library::SourcePolicy, bool), StoreError>> {
    let services = owner.services();
    let access = owner.parental_snapshot();
    async move {
        services
            .catalog()
            .update_source_policy(id, access, edit)
            .await
    }
}

pub fn remember_shown(
    owner: &super::Runtime,
    id: SourceId,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner.services().catalog().set_active_source(id)
}

pub fn rename_source(
    owner: &super::Runtime,
    id: SourceId,
    name: String,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner.services().catalog().rename_source(id, name)
}

pub fn delete_source(
    owner: &super::Runtime,
    id: SourceId,
) -> impl std::future::Future<Output = Result<bool, StoreError>> + use<> {
    owner
        .services()
        .catalog()
        .delete_source(id, owner.parental_snapshot())
}

pub fn edit_guides(
    owner: &super::Runtime,
    id: SourceId,
    edit: iptv_library::guides::Edit,
) -> impl std::future::Future<Output = Result<bool, StoreError>> + use<> {
    owner.services().catalog().edit_guide_sources(id, edit)
}

pub fn set_activity(owner: &super::Runtime, id: SourceId, activity: Option<Activity>) {
    if let Some(activity) = &activity {
        owner
            .imports
            .activity
            .borrow_mut()
            .insert(id, activity.clone());
    } else {
        owner.imports.activity.borrow_mut().remove(&id);
    }
    owner.emit(super::RuntimeEvent::RefreshProgress(id, activity));
}

pub fn set_all(owner: &super::Runtime, all: bool) {
    owner.all.set(all);
    owner.emit(super::RuntimeEvent::BrowsingAll(all));
}

pub fn set_shown(owner: &super::Runtime, source: Option<SourceId>) {
    owner.shown.set(source);
    owner.emit(super::RuntimeEvent::ShownSource(source));
}

pub fn save_all(
    owner: &super::Runtime,
    all: bool,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner
        .services()
        .catalog()
        .set_all_playlists(all, owner.parental_snapshot())
}

pub fn collapsed(
    owner: &super::Runtime,
) -> impl std::future::Future<Output = Result<Vec<i64>, StoreError>> + use<> {
    let services = owner.services();
    async move { services.catalog().collapsed_playlists().await }
}

pub fn save_collapsed(
    owner: &super::Runtime,
    collapsed: Vec<i64>,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner
        .services()
        .catalog()
        .set_collapsed_playlists(collapsed)
}

/// Refresh policy belongs to the application; the host supplies timers and
/// foreground notifications. No device wakeups are requested in the background.
pub fn start_scheduler(owner: &Rc<super::Runtime>) {
    if owner.lifecycle.scheduler_started().replace(true) {
        return;
    }
    let runtime = owner.clone();
    owner.spawn(async move {
        let mut startup = true;
        loop {
            if runtime.lifecycle.foreground().get() {
                if let Ok(sources) = super::commands::source_policies(&runtime).await {
                    for (source, policy) in sources {
                        let attempted = runtime
                            .imports
                            .attempts
                            .borrow()
                            .get(&source.id)
                            .copied()
                            .or(source.import.last_attempt_at);
                        if policy.due(crate::time::now_unix(), attempted, startup)
                            && !running(&runtime, source.id)
                        {
                            crate::data::refresh(&runtime, source.id);
                        }
                    }
                }
                startup = false;
            }
            runtime.delay(std::time::Duration::from_secs(60)).await;
        }
    });
}

pub fn playlists(owner: &super::Runtime) -> Result<Vec<Playlist>, crate::Error> {
    playlists_native(owner).map_err(crate::Error::from)
}

pub fn policy(owner: &super::Runtime, id: SourceId) -> Result<SourcePolicy, crate::Error> {
    policy_native(owner, id).map_err(crate::Error::from)
}

#[cfg(test)]
mod order_tests {
    use super::*;

    fn named(id: i64, name: &str) -> Playlist {
        Playlist {
            id: SourceId(id),
            name: name.into(),
            kind: iptv_providers::model::SourceKind::M3u,
            channels: 0,
            has_movies: false,
            has_series: false,
            movies_failed: None,
            series_failed: None,
            updated_at: None,
            failed: None,
            guides: Vec::new(),
            guide_failed: false,
            user_agent: None,
            from_file: false,
            policy: Default::default(),
        }
    }

    #[test]
    fn playlist_sort_supports_insertion_name_and_manual_order() {
        let manual = [named(3, "Beta"), named(1, "Zeta"), named(2, "alpha")];
        for (mode, expected) in [
            ("added", [1, 2, 3]),
            ("name", [2, 3, 1]),
            ("manual", [3, 1, 2]),
        ] {
            let mut sorted = manual.clone();
            sort_playlists(&mut sorted, mode);
            assert_eq!(sorted.map(|p| p.id.0), expected, "{mode}");
        }
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    use iptv_providers::{SecretUrl, model::M3uSourceConfig};
    #[test]
    fn startup_ignores_disabled_active_source_and_recovers_when_reenabled() {
        let mut s = Store::open_in_memory().unwrap();
        let source = |name: &str| Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("http://example.test/list.m3u"),
                user_agent: None,
                referrer: None,
            }),
        };
        let a = s.add_source(&source("A")).unwrap();
        let b = s.add_source(&source("B")).unwrap();
        s.set_active_source(a).unwrap();
        assert_eq!(startup_source(&s).unwrap().unwrap().id, a);
        let off = iptv_store_sqlite::SourcePolicy {
            enabled: false,
            ..Default::default()
        };
        s.set_source_policy(a, off).unwrap();
        assert_eq!(startup_source(&s).unwrap().unwrap().id, b);
        s.set_source_policy(b, off).unwrap();
        assert!(startup_source(&s).unwrap().is_none());
        s.set_source_policy(a, Default::default()).unwrap();
        assert_eq!(startup_source(&s).unwrap().unwrap().id, a);
    }
}

#[cfg(all(test, feature = "native"))]
mod guide_work_supersession;
