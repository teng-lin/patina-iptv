//! Library snapshots, refresh tickets and named persistence commands.
use iptv_core::wizard::{ImportCounts, ImportFailure, ImportProgress};
use iptv_import::CancelToken;
use iptv_library::Playlist;
use iptv_providers::model::{Source, SourceConfig};
use iptv_store_sqlite::{SourceId, Store, StoreError, StoredSource};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
};
/// A stored playlist's refresh (or check of new details) this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    Running(Option<ImportProgress>),
    /// Finished; on failure the stored channels stay. New details that
    /// imported are saved.
    Done(Result<ImportCounts, ImportFailure>),
    /// New details are being tried.
    Checking(Option<ImportProgress>),
    /// New details did not import: the stored ones and their data stay.
    Rejected(ImportFailure),
}

pub type ImportCompletion = Rc<ImportWork>;
pub use crate::state::ImportWork;
pub fn import_completion(cancel: CancelToken) -> ImportCompletion {
    Rc::new(ImportWork {
        finished: Cell::new(false),
        catalog_done: Cell::new(false),
        cancel,
        waiting: RefCell::default(),
    })
}
pub fn track_import(owner: &super::Runtime, source: SourceId, completion: &ImportCompletion) {
    owner.cancel_guide_check(source);
    owner
        .completions
        .with(|jobs| jobs.borrow_mut().insert(source, Rc::downgrade(completion)));
}
/// An Add learns its source ID only after saving. A newer known-source job may
/// already own that ID by then, including a worker that has just completed.
pub fn track_added_import(owner: &super::Runtime, source: SourceId, completion: &ImportCompletion) {
    owner.completions.with(|jobs| {
        jobs.borrow_mut()
            .entry(source)
            .or_insert_with(|| Rc::downgrade(completion));
    });
}
pub type ImportRevision = Option<Weak<ImportWork>>;
pub fn import_revision(owner: &super::Runtime, source: SourceId) -> ImportRevision {
    owner
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
        .completions
        .with(|jobs| jobs.borrow().get(&source).and_then(Weak::upgrade))
        .filter(|job| !job.finished())
}
pub fn import_is_current(
    owner: &super::Runtime,
    source: SourceId,
    completion: &ImportCompletion,
) -> bool {
    owner.completions.with(|jobs| {
        jobs.borrow()
            .get(&source)
            .and_then(Weak::upgrade)
            .is_some_and(|current| Rc::ptr_eq(&current, completion))
    })
}
pub fn revoke_import_handoff(owner: &super::Runtime, source: SourceId) {
    if let Some(job) = owner.completions.with(|jobs| {
        jobs.borrow_mut()
            .remove(&source)
            .and_then(|job| job.upgrade())
    }) {
        // Guide edits may happen during a source's authentication/catalog
        // phase. Revoke handoff then, but let that phase establish playback.
        job.cancel_guide();
    }
}
pub fn reset_imports(owner: &super::Runtime) {
    owner.completions.with(|jobs| {
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

pub(crate) fn snapshot_in(store: &Store) -> Result<Snapshot, StoreError> {
    let playlists = playlists_in(store)?;
    let mut details = BTreeMap::new();
    let mut imports = BTreeMap::new();
    for playlist in &playlists {
        imports.insert(playlist.id, store.import_status(playlist.id)?);
        if let Some(source) = store.source(playlist.id)? {
            details.insert(playlist.id, source);
        }
    }
    Ok(Snapshot {
        playlists,
        details,
        imports,
        active: store.active_source()?,
    })
}
pub fn install(owner: &super::Runtime, snapshot: Snapshot) -> bool {
    owner.library.with(|current| {
        let changed = current.borrow().as_ref() != Some(&snapshot);
        *current.borrow_mut() = Some(snapshot);
        changed
    })
}

/// The stored playlists in list order.
fn playlists_native(owner: &super::Runtime) -> Result<Vec<Playlist>, StoreError> {
    owner.library.with(|snapshot| {
        snapshot
            .borrow()
            .as_ref()
            .map(|snapshot| snapshot.playlists.clone())
            .ok_or(StoreError::Unavailable("library is loading"))
    })
}
pub(crate) fn playlists_in(store: &Store) -> Result<Vec<Playlist>, StoreError> {
    store
        .sources()?
        .into_iter()
        .map(|s| {
            let guide = store.epg_guide(s.id)?;
            let guides = store.guide_sources(s.id)?;
            let config = store.source(s.id)?.map(|stored| stored.source.config);
            let (user_agent, from_file) = match &config {
                Some(SourceConfig::M3u(c)) => {
                    (c.user_agent.clone(), !c.location.expose().contains("://"))
                }
                Some(SourceConfig::Xtream(c)) => (c.user_agent.clone(), false),
                _ => (None, false),
            };
            Ok(Playlist {
                policy: store.source_policy(s.id)?,
                id: s.id,
                name: s.name,
                kind: s.kind,
                channels: s.import.counts.channels,
                updated_at: s.import.last_success_at,
                failed: s.import.last_error.map(|k| (k, s.import.last_error_at)),
                guide_failed: guide.as_ref().is_some_and(|g| g.last_error.is_some()),
                guides,
                user_agent,
                from_file,
            })
        })
        .collect()
}

/// The playlist to open at startup: the one shown last, else the newest.
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
    owner.library.with(|snapshot| {
        let snapshot = snapshot.borrow();
        let snapshot = snapshot
            .as_ref()
            .ok_or(StoreError::Unavailable("library is loading"))?;
        let active = snapshot.active.filter(|id| {
            snapshot
                .playlists
                .iter()
                .any(|p| p.id == *id && p.policy.enabled)
        });
        let selected = active.or_else(|| {
            snapshot
                .playlists
                .iter()
                .rev()
                .find(|p| p.policy.enabled)
                .map(|p| p.id)
        });
        Ok(selected.and_then(|id| snapshot.details.get(&id).cloned()))
    })
}

fn policy_native(
    owner: &super::Runtime,
    id: SourceId,
) -> Result<iptv_store_sqlite::SourcePolicy, StoreError> {
    playlists(owner)?
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
    owner.imports.with(|r| r.borrow().contains_key(&id))
}

pub fn cancel_running(owner: &super::Runtime, id: SourceId) {
    owner.cancel_guide_check(id);
    revoke_import_handoff(owner, id);
    if let Some((_, token)) = owner.imports.with(|r| r.borrow_mut().remove(&id)) {
        log::info!(
            "iptv data: cancelling the running import of source {}",
            id.0
        );
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
    owner.services().read(snapshot_in)
}

pub fn save_policy(
    owner: &super::Runtime,
    id: SourceId,
    policy: iptv_library::SourcePolicy,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner
        .services()
        .call(move |store| store.set_source_policy(id, policy))
}

pub fn remember_shown(
    owner: &super::Runtime,
    id: SourceId,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner
        .services()
        .call(move |store| store.set_active_source(id))
}

pub fn rename_source(
    owner: &super::Runtime,
    id: SourceId,
    name: String,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner
        .services()
        .call(move |store| store.rename_source(id, &name))
}

pub fn delete_source(
    owner: &super::Runtime,
    id: SourceId,
) -> impl std::future::Future<Output = Result<bool, StoreError>> + use<> {
    owner.services().call(move |store| store.delete_source(id))
}

pub fn edit_guides(
    owner: &super::Runtime,
    id: SourceId,
    edit: iptv_library::guides::Edit,
) -> impl std::future::Future<Output = Result<bool, StoreError>> + use<> {
    owner.services().call(move |store| {
        let Some(sources) = iptv_library::guides::apply(&store.guide_sources(id)?, &edit) else {
            return Ok(false);
        };
        store.set_guide_sources(id, &sources)?;
        Ok(true)
    })
}

pub fn set_activity(owner: &super::Runtime, id: SourceId, activity: Option<Activity>) {
    if let Some(activity) = &activity {
        owner.activity.borrow_mut().insert(id, activity.clone());
    } else {
        owner.activity.borrow_mut().remove(&id);
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
        .call(move |store| store.set_all_playlists(all))
}

pub fn collapsed(
    owner: &super::Runtime,
) -> impl std::future::Future<Output = Result<Vec<i64>, StoreError>> + use<> {
    owner.services().call(|store| store.collapsed_playlists())
}

pub fn save_collapsed(
    owner: &super::Runtime,
    collapsed: Vec<i64>,
) -> impl std::future::Future<Output = Result<(), StoreError>> + use<> {
    owner
        .services()
        .call(move |store| store.set_collapsed_playlists(&collapsed))
}

/// Refresh policy belongs to the application; the host supplies timers and
/// foreground notifications. No device wakeups are requested in the background.
pub fn start_scheduler(owner: &Rc<super::Runtime>) {
    if owner.scheduler_started.replace(true) {
        return;
    }
    let runtime = owner.clone();
    owner.spawn(async move {
        let mut startup = true;
        loop {
            if runtime.foreground.get() {
                if let Ok(sources) = super::commands::source_policies(&runtime).await {
                    for (source, policy) in sources {
                        let attempted = runtime
                            .attempts
                            .borrow()
                            .get(&source.id)
                            .copied()
                            .or(source.import.last_attempt_at);
                        if policy.due(super::imports::now_unix(), attempted, startup)
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

pub fn policy(
    owner: &super::Runtime,
    id: SourceId,
) -> Result<iptv_store_sqlite::SourcePolicy, crate::Error> {
    policy_native(owner, id).map_err(crate::Error::from)
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
