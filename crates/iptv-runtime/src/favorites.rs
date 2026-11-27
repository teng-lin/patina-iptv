//! Favorite and recently watched channels, kept by channel key per source
//! in the store (its `favorites` and `watch_history` tables), so they
//! survive refreshes and restarts. `Ui::favorites` holds their catalog
//! channels on screen (stars, search ranking); the Favorites and Recently
//! watched lists are built from here (`lists.rs`).
//!
//! The in-memory lists are the truth while a source is shown; writes go to
//! one writer thread in order, so a toggle never waits for the store (on a
//! host without workers, a browser, to the store's writer lane in order,
//! [`queue_on_writer`]). They are read when a source is shown, on the
//! store's reader unless writes of this module are still queued
//! ([`load`]). The synthetic guide keeps them in memory only.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};

use iptv_core::favorites::{Favorites, KeyIndex, RECENT_LIMIT, Recent, diff};
use iptv_core::guide::Schedule;
use iptv_providers::ContentId;
use iptv_providers::model::ContentKind;
use iptv_store_sqlite::{SourceId, Store, StoreError};

#[derive(Default)]
pub(crate) struct State {
    source: Option<SourceId>,
    combined_sources: Vec<SourceId>,
    favorites: Favorites,
    recent: Recent,
    history_revision: u64,
    pending_order: Weak<PendingOrder>,
    /// The catalog on screen, by key.
    index: KeyIndex,
    /// The source has movies or shows.
    has_vod: bool,
    /// Some enabled source has movies or shows (the cross-source shelves).
    any_vod: bool,
    /// Reset identity and request order. Only a successful application advances
    /// authority: a newer failed read must not suppress an older valid result.
    vod_epoch: Rc<()>,
    vod_requested: u64,
    vod_applied: u64,
    writer: Option<mpsc::Sender<Write>>,
    unsaved: Arc<AtomicUsize>,
    /// A write queued on the store's writer lane failed since the last barrier
    /// (hosts without workers).
    failed: Arc<AtomicBool>,
}

impl State {
    fn reset(&mut self) {
        // A previous root's epoch-bound writer must not accept more writes.
        // Keep tracking its outstanding writes until they finish.
        *self = Self {
            index: std::mem::take(&mut self.index),
            unsaved: self.unsaved.clone(),
            ..Self::default()
        };
    }

    fn send(&mut self, services: crate::store_service::Services, write: Write) {
        if self.writer.is_none() {
            let (tx, rx) = mpsc::channel();
            let unsaved = self.unsaved.clone();
            // The writer belongs to this owner's application epoch.
            let execution = services.ports().execution.clone();
            if let Err(error) = execution.worker(
                "iptv-favorites".into(),
                Box::new(move || write_all(&services, rx, unsaved)),
            ) {
                // Nothing is cached or counted: the next change tries again,
                // and reads do not wait for a write no writer has.
                log::error!("iptv favorites: cannot start the writer: {error}");
                return;
            }
            self.writer = Some(tx);
        }
        let tx = self.writer.as_ref().expect("the writer was started");
        if !matches!(write, Write::Barrier(_) | Write::Shutdown(_)) {
            self.unsaved.fetch_add(1, Ordering::AcqRel);
        }
        if tx.send(write).is_err() {
            log::warn!("iptv favorites: the writer stopped");
        }
        // A write that never reached the thread is never saved; it stays
        // counted, so reads keep going behind the writer's queue.
    }
}

impl State {
    pub(crate) fn with_catalog(catalog: &dyn Schedule) -> Self {
        Self {
            index: KeyIndex::build(catalog),
            ..Self::default()
        }
    }
}

/// Whether `source` has movies or shows by its last import's counts.
fn source_has_vod(store: &Store, source: SourceId) -> Result<bool, StoreError> {
    let counts = store.import_status(source)?.counts;
    Ok(counts.vod > 0 || counts.series > 0)
}

/// Whether any enabled source has movies or shows: one status read per
/// configured playlist, never a catalog scan.
fn any_enabled_vod(store: &Store) -> Result<bool, StoreError> {
    for source in store.sources()? {
        if store.source_policy(source.id)?.enabled && source_has_vod(store, source.id)? {
            return Ok(true);
        }
    }
    Ok(false)
}

struct VodRequest {
    epoch: Rc<()>,
    revision: u64,
}

/// Reserve before queuing work, independently from the newest applied result.
fn begin_vod_request(owner: &super::Runtime) -> VodRequest {
    owner.organization.favorites().with(|state| {
        let mut state = state.borrow_mut();
        state.vod_requested = match state.vod_requested.checked_add(1) {
            Some(next) => next,
            None => {
                state.vod_epoch = Rc::new(());
                state.vod_applied = 0;
                1
            }
        };
        VodRequest {
            epoch: state.vod_epoch.clone(),
            revision: state.vod_requested,
        }
    })
}

/// Even an unchanged successful result fences older reads. An error/unknown
/// result never reaches here and cannot discard valid work still in flight.
fn apply_vod_availability(state: &mut State, request: &VodRequest, any_vod: bool) -> bool {
    if !Rc::ptr_eq(&state.vod_epoch, &request.epoch) || request.revision <= state.vod_applied {
        return false;
    }
    state.vod_applied = request.revision;
    let changed = state.any_vod != any_vod;
    state.any_vod = any_vod;
    changed
}

/// Refresh just the cross-source availability, leaving the guide and its
/// favorites intact. Policy/deletion callers await this nonblocking read;
/// completed background imports schedule it. Errors retain the last good value.
pub(crate) fn refresh_vod_availability(
    owner: &Rc<super::Runtime>,
) -> impl std::future::Future<Output = ()> + use<> {
    let request = begin_vod_request(owner);
    let services = owner.services();
    let read = services.read(any_enabled_vod);
    let owner = owner.clone();
    async move {
        let Ok(any_vod) = read.await else { return };
        if !services.is_current() {
            return;
        }
        let changed = owner
            .organization
            .favorites()
            .with(|state| apply_vod_availability(&mut state.borrow_mut(), &request, any_vod));
        if changed {
            publish(&owner);
        }
    }
}

/// A store write, in the order made.
enum Write {
    ClearHistory,
    SaveOrder(Box<dyn FnOnce(bool) + Send>),
    Barrier(futures_channel::oneshot::Sender<bool>),
    Shutdown(mpsc::Sender<bool>),
    MoveFavorite {
        source: SourceId,
        key: String,
        index: usize,
    },
    Favorite {
        source: SourceId,
        key: String,
        on: bool,
    },
    Watched {
        source: SourceId,
        key: String,
        at: i64,
    },
    WatchTime {
        source: SourceId,
        key: String,
        elapsed_ms: u64,
    },
}

#[derive(Clone)]
enum PendingWrite {
    ClearHistory,
    MoveFavorite {
        source: SourceId,
        key: String,
        index: usize,
    },
    Favorite {
        source: SourceId,
        key: String,
        on: bool,
    },
    Watched {
        source: SourceId,
        key: String,
        at: i64,
    },
    WatchTime {
        source: SourceId,
        key: String,
        elapsed_ms: u64,
    },
}

/// Writes sent to the writer thread and not yet committed (or dropped with
/// their deleted playlist).
fn unsaved(owner: &super::Runtime) -> Arc<AtomicUsize> {
    owner.organization.favorites().borrow().unsaved.clone()
}

fn cooperative_failure(owner: &super::Runtime) -> Arc<AtomicBool> {
    owner.organization.favorites().borrow().failed.clone()
}

/// Reads saved favorites. The store's reader does not wait for a long
/// publish, but it cannot see a write still queued: while any of ours is,
/// a favorites-inbox barrier must finish before queuing the read.
async fn load<R: Send + 'static>(
    owner: &super::Runtime,
    read: impl FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
) -> Result<R, StoreError> {
    if unsaved(owner).load(Ordering::Acquire) > 0 {
        if !flush(owner).await {
            return Err(StoreError::Unavailable(
                "recent favorite edits are not saved",
            ));
        }
        owner.services().call(move |store| read(store)).await
    } else {
        owner.services().read(read).await
    }
}

/// A new app root must reload persisted favorites, including an applied restore.
pub fn reset(owner: &super::Runtime) {
    owner.organization.favorites().borrow_mut().reset();
}

/// The source whose catalog is about to be shown: its saved favorites
/// and recent channels. The same
/// source keeps its in-memory lists (writes may still be on their way).
pub async fn prepare_source(
    owner: &std::rc::Rc<super::Runtime>,
    source: Option<SourceId>,
) -> Option<impl FnOnce()> {
    let generation = owner.generation.get();
    let id = source?;
    let services = owner.services();
    let history_revision = owner
        .organization
        .favorites()
        .with(|state| state.borrow().history_revision);
    let request = begin_vod_request(owner);
    let loaded = load(owner, move |store| {
        let has_vod = source_has_vod(store, id).unwrap_or(false);
        // Another enabled playlist's films keep the cross-source shelves
        // reachable while a live-only playlist is on screen.
        // An unrelated playlist's metadata error must not block this guide.
        // None preserves the last known cross-source availability.
        let any_vod = if has_vod {
            Some(true)
        } else {
            any_enabled_vod(store).ok()
        };
        let keys = store.favorite_keys(id)?;
        let recent = store.recent(Some(id), Some(ContentKind::Live), RECENT_LIMIT as u32)?;
        Ok(((has_vod, any_vod), keys, recent))
    })
    .await;
    if !services.is_current() || owner.generation.get() != generation {
        return None;
    }
    let ((has_vod, any_vod), keys, recent) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            log::warn!("iptv favorites: store unavailable ({error})");
            return None;
        }
    };
    let owner = owner.clone();
    Some(move || {
        owner.organization.favorites().with(|state| {
            let mut state = state.borrow_mut();
            state.has_vod = has_vod;
            if services.is_current()
                && let Some(any_vod) = any_vod
            {
                apply_vod_availability(&mut state, &request, any_vod);
            }
            if state.source == source {
                return;
            }
            state.source = source;
            state.pending_order = Weak::new();
            state.combined_sources.clear();
            state.favorites = Favorites::from_keys(keys);
            state.recent = Recent::from_keys(
                recent
                    .into_iter()
                    .filter(|_| state.history_revision == history_revision)
                    .map(|row| row.item_id.as_str().to_owned()),
            );
        })
    })
}

/// Load enabled-source favorites while preserving edits already held in memory.
pub async fn prepare_combined(owner: &std::rc::Rc<super::Runtime>) -> Option<impl FnOnce()> {
    let history_revision = owner
        .organization
        .favorites()
        .with(|state| state.borrow().history_revision);
    let generation = owner.generation.get();
    let services = owner.services();
    let request = begin_vod_request(owner);
    use iptv_guide::combined::{qualified_key, split_key};
    let loaded = load(owner, |store| {
        let mut ids = Vec::new();
        let mut favorites = Vec::new();
        let mut has_vod = false;
        let mut availability_known = true;
        for source in store.sources()? {
            if store.source_policy(source.id)?.enabled {
                ids.push(source.id);
                match source_has_vod(store, source.id) {
                    Ok(available) => has_vod |= available,
                    Err(_) => availability_known = false,
                }
                favorites.push((source.id, store.favorite_keys(source.id)?));
            }
        }
        Ok((
            ids,
            favorites,
            store.recent(None, Some(ContentKind::Live), RECENT_LIMIT as u32)?,
            (has_vod || availability_known).then_some(has_vod),
        ))
    })
    .await;
    if !services.is_current() || owner.generation.get() != generation {
        return None;
    }
    let Ok((ids, favorites, recent, has_vod)) = loaded else {
        return None;
    };
    let owner = owner.clone();
    Some(move || {
        owner.organization.favorites().with(|state| {
            let mut state = state.borrow_mut();
            let previous = state.combined_sources.clone();
            let mut keys: Vec<_> = state
                .favorites
                .keys()
                .iter()
                .filter(|key| {
                    split_key(key).is_some_and(|(source, _)| {
                        previous.contains(&SourceId(source)) && ids.contains(&SourceId(source))
                    })
                })
                .cloned()
                .collect();
            for (source, saved) in favorites {
                if !previous.contains(&source) {
                    let local = if state.source == Some(source) {
                        state.favorites.keys()
                    } else {
                        &saved
                    };
                    keys.extend(local.iter().map(|key| qualified_key(source.0, key)));
                }
            }
            state.source = None;
            state.combined_sources = ids.clone();
            state.favorites = Favorites::from_keys(keys);
            if previous != ids && state.history_revision == history_revision {
                state.recent = Recent::from_keys(
                    recent
                        .into_iter()
                        .filter(|row| ids.contains(&row.source_id))
                        .map(|row| qualified_key(row.source_id.0, row.item_id.as_str())),
                );
            }
            // Every enabled playlist is on screen.
            if let Some(has_vod) = has_vod {
                state.has_vod = has_vod;
                if services.is_current() {
                    apply_vod_availability(&mut state, &request, has_vod);
                }
            }
        })
    })
}

fn write_owner(state: &State, key: &str) -> Option<(SourceId, String)> {
    if let Some(source) = state.source {
        Some((source, key.to_owned()))
    } else {
        let (s, k) = iptv_guide::combined::split_key(key)?;
        state
            .combined_sources
            .contains(&SourceId(s))
            .then(|| (SourceId(s), k.to_owned()))
    }
}

/// Index the installed catalog before the shell restores its lists.
pub(crate) fn rebuild_index(owner: &super::Runtime, catalog: &dyn Schedule) {
    let index = KeyIndex::build(catalog);
    owner
        .organization
        .favorites()
        .with(|s| s.borrow_mut().index = index);
}

pub(crate) fn vod_availability(owner: &super::Runtime) -> (bool, bool) {
    owner.organization.favorites().with(|s| {
        let state = s.borrow();
        (state.has_vod, state.any_vod)
    })
}

pub fn publish(owner: &super::Runtime) {
    let (set, has_vod, any_vod) = owner.organization.favorites().with(|s| {
        let s = s.borrow();
        (s.favorites.set_of(&s.index), s.has_vod, s.any_vod)
    });
    owner.emit(super::RuntimeEvent::FavoritesChanged {
        channels: set,
        has_vod,
        any_vod,
    });
}

/// The favorites on screen as catalog channels, in their order.
pub fn favorite_channels(owner: &super::Runtime) -> Vec<usize> {
    owner.organization.favorites().with(|s| {
        let s = s.borrow();
        s.favorites.channels(&s.index)
    })
}

/// The recently watched channels on screen, newest first.
pub fn recent_channels(owner: &super::Runtime) -> Vec<usize> {
    owner.organization.favorites().with(|s| {
        let s = s.borrow();
        s.recent.channels(&s.index)
    })
}

/// The catalog channel with `key`.
pub fn channel_of(owner: &super::Runtime, key: &str) -> Option<usize> {
    owner
        .organization
        .favorites()
        .with(|s| s.borrow().index.get(key))
}

/// Changes favorites through their catalog channels (the editor works on
/// sets of them): what `f` adds or removes is saved by key.
pub fn edit(owner: &super::Runtime, f: impl FnOnce(&mut BTreeSet<usize>)) {
    let before = owner.organization.favorites().with(|state| {
        let state = state.borrow();
        state.favorites.set_of(&state.index)
    });
    let mut after = before.clone();
    f(&mut after);
    let (added, removed) = diff(&before, &after);
    if added.is_empty() && removed.is_empty() {
        return;
    }
    let catalog = owner.schedule();
    for (channels, on) in [(added, true), (removed, false)] {
        for key in channels.into_iter().filter_map(|c| catalog.channel_key(c)) {
            set_key(owner, &key, on);
        }
    }
    publish(owner);
    owner.emit(super::RuntimeEvent::FavoriteOrderChanged);
}

/// Adds `channel` (catalog) to the favorites or removes it.
pub fn toggle(owner: &super::Runtime, channel: usize) {
    edit(owner, |set| {
        if !set.remove(&channel) {
            set.insert(channel);
        }
    });
}

struct PendingOrder {
    source: SourceId,
    changes: RefCell<Vec<FavoriteChange>>,
}

enum FavoriteChange {
    Set { key: String, on: bool },
    Move { key: String, index: usize },
}

impl FavoriteChange {
    fn apply(self, favorites: &mut Favorites, present: impl Fn(&str) -> bool) {
        match self {
            Self::Set { key, on } => {
                favorites.set(&key, on);
            }
            Self::Move { key, index } => {
                favorites.move_key_among(&key, index, present);
            }
        }
    }
}

fn set_key(owner: &super::Runtime, key: &str, on: bool) {
    let source = owner.organization.favorites().with(|s| {
        let mut s = s.borrow_mut();
        if s.favorites.set(key, on) {
            let owned = write_owner(&s, key);
            if let Some((source, local)) = &owned
                && let Some(pending) = s
                    .pending_order
                    .upgrade()
                    .filter(|pending| pending.source == *source)
            {
                pending.changes.borrow_mut().push(FavoriteChange::Set {
                    key: local.clone(),
                    on,
                });
            }
            owned
        } else {
            None
        }
    });
    if let Some((source, key)) = source {
        send(owner, Write::Favorite { source, key, on });
    }
}

/// A live channel started playing (by key).
pub fn played(owner: &super::Runtime, key: &str) {
    let source = owner.organization.favorites().with(|s| {
        let mut s = s.borrow_mut();
        s.recent.played(key);
        write_owner(&s, key)
    });
    if let Some((source, key)) = source {
        owner
            .organization
            .watch_sort_dirty()
            .borrow_mut()
            .insert(source);
        send(
            owner,
            Write::Watched {
                source,
                key,
                at: crate::time::now_unix(),
            },
        );
    }
}

pub(crate) fn watch_time(owner: &super::Runtime, source: SourceId, key: String, elapsed_ms: u64) {
    owner
        .organization
        .watch_sort_dirty()
        .borrow_mut()
        .insert(source);
    send(
        owner,
        Write::WatchTime {
            source,
            key,
            elapsed_ms,
        },
    );
}

fn send(owner: &super::Runtime, write: Write) {
    let services = owner.services();
    if !services.has_workers() {
        queue_on_writer(owner, write);
        return;
    }
    owner
        .organization
        .favorites()
        .borrow_mut()
        .send(services, write);
}

/// Keeps a write counted in [`unsaved`] until it ran or was dropped.
struct Counted(std::sync::Arc<AtomicUsize>);

impl Counted {
    fn new(owner: &super::Runtime) -> Self {
        let unsaved = unsaved(owner);
        unsaved.fetch_add(1, Ordering::AcqRel);
        Self(unsaved)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A store open or an expired epoch can discard the job before its
/// closure runs. Count that as a failure as well as a failed SQLite edit.
struct ReportFailure(Option<std::sync::Arc<std::sync::atomic::AtomicBool>>);

impl Drop for ReportFailure {
    fn drop(&mut self) {
        if let Some(failed) = &self.0 {
            failed.store(true, Ordering::Release);
        }
    }
}

/// A host without workers (a browser) has no writer thread: each write is
/// a job on the store's writer lane, which runs them in the order queued,
/// after the writes of other modules queued before them, and persists each
/// commit (`store_host`). A write that fails is logged and lost, unlike
/// the thread's, which keeps it at the head and retries: the next barrier
/// answers false, as the thread's would while it retries.
fn queue_on_writer(owner: &super::Runtime, write: Write) {
    let services = owner.services();
    let failed = cooperative_failure(owner);
    let barrier = |done: Box<dyn FnOnce(bool) + Send>| {
        let failed = failed.clone();
        // A barrier that cannot be queued is dropped, and its waiter hears
        // that nothing was saved.
        drop(services.call(move |_| {
            done(!failed.swap(false, Ordering::AcqRel));
            Ok(())
        }));
    };
    let operation = match write {
        Write::Barrier(done) => {
            return barrier(Box::new(move |saved| {
                let _ = done.send(saved);
            }));
        }
        Write::Shutdown(done) => {
            return barrier(Box::new(move |saved| {
                let _ = done.send(saved);
            }));
        }
        Write::SaveOrder(_) => {
            unreachable!("without workers a reorder is saved by `save_order` itself")
        }
        Write::ClearHistory => PendingWrite::ClearHistory,
        Write::MoveFavorite { source, key, index } => {
            PendingWrite::MoveFavorite { source, key, index }
        }
        Write::Favorite { source, key, on } => PendingWrite::Favorite { source, key, on },
        Write::Watched { source, key, at } => PendingWrite::Watched { source, key, at },
        Write::WatchTime {
            source,
            key,
            elapsed_ms,
        } => PendingWrite::WatchTime {
            source,
            key,
            elapsed_ms,
        },
    };
    let counted = Counted::new(owner);
    let failure = ReportFailure(Some(failed));
    drop(services.call(move |store| {
        let _counted = counted;
        let mut failure = failure;
        match persist(store, operation) {
            Ok(()) | Err(StoreError::SourceNotFound(_)) => {
                failure.0 = None;
                Ok(())
            }
            Err(error) => {
                log::warn!("iptv favorites: could not save ({error})");
                Err(error)
            }
        }
    }));
}

/// One write, on the store.
fn persist(store: &mut Store, write: PendingWrite) -> Result<(), StoreError> {
    match write {
        PendingWrite::ClearHistory => store.clear_history(None).map(|_| ()),
        PendingWrite::MoveFavorite { source, key, index } => store
            .move_favorite(source, &ContentId::from_stored(key), index)
            .map(|_| ()),
        PendingWrite::Favorite { source, key, on } => {
            store.set_favorite(source, &ContentId::from_stored(key), on)
        }
        PendingWrite::WatchTime {
            source,
            key,
            elapsed_ms,
        } => store.add_channel_watch_time(source, &ContentId::from_stored(key), elapsed_ms),
        PendingWrite::Watched { source, key, at } => store.record_watch(
            source,
            &ContentId::from_stored(key),
            ContentKind::Live,
            at,
            None,
        ),
    }
}

/// Clear in writer order so older queued plays cannot resurrect deleted history.
pub fn clear_history(owner: &super::Runtime) -> impl std::future::Future<Output = bool> + use<> {
    owner
        .organization
        .watch_sort_dirty()
        .borrow_mut()
        .extend(owner.shown_sources());
    send(owner, Write::ClearHistory);
    let (tx, rx) = futures_channel::oneshot::channel();
    send(owner, Write::Barrier(tx));
    owner.organization.favorites().with(|state| {
        let mut state = state.borrow_mut();
        state.history_revision += 1;
        state.recent = Recent::default();
    });
    publish(owner);
    async move { rx.await.unwrap_or(false) }
}

/// The writer thread: one store connection, writes in order.
pub async fn flush(owner: &super::Runtime) -> bool {
    let (tx, rx) = futures_channel::oneshot::channel();
    send(owner, Write::Barrier(tx));
    rx.await.unwrap_or(false)
}

/// Root teardown only: queued edits outlive their UI owner and drain first.
pub fn flush_on_shutdown(owner: &super::Runtime) -> bool {
    let writer = owner.organization.favorites().borrow_mut().writer.take();
    let Some(writer) = writer else { return true };
    let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (tx, rx) = mpsc::channel();
        if writer.send(Write::Shutdown(tx)).is_err() {
            return false;
        }
        match rx.recv_timeout(deadline.saturating_duration_since(crate::time::Instant::now())) {
            Ok(true) => return true,
            Ok(false) if crate::time::Instant::now() < deadline => {
                // A busy database retains the head edit. Keep the engine alive
                // for bounded retries instead of treating a failed attempt as
                // a completed shutdown barrier.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => return false,
        }
    }
}

fn write_all(
    services: &super::store_service::Services,
    rx: Receiver<Write>,
    unsaved: std::sync::Arc<AtomicUsize>,
) {
    write_all_with(rx, |write| {
        let operation = write.clone();
        let result = services.blocking(move |store| persist(store, operation));
        // Deleting a playlist also deletes its customization/history. A queued
        // change whose owner was explicitly removed must not block other sources.
        if matches!(
            &result,
            Err(iptv_store_sqlite::StoreError::SourceNotFound(_))
        ) {
            unsaved.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        if let Err(e) = result {
            log::warn!("iptv favorites: could not save ({e})");
            return false;
        }
        unsaved.fetch_sub(1, Ordering::AcqRel);
        true
    });
}

/// A failed operation stays at the head of the queue. Neither later writes nor
/// barriers may skip it; barriers and an idle retry timer retry changes in order.
fn write_all_with(rx: Receiver<Write>, mut persist: impl FnMut(&PendingWrite) -> bool) {
    let mut pending = VecDeque::new();
    loop {
        let received = if pending.is_empty() {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(std::time::Duration::from_millis(250))
        };
        let barrier: Option<Box<dyn FnOnce(bool)>> = match received {
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Ok(write) => match write {
                Write::ClearHistory => {
                    pending.push_back(PendingWrite::ClearHistory);
                    None
                }
                Write::SaveOrder(save) => Some(save),
                Write::Barrier(done) => Some(Box::new(move |saved| {
                    let _ = done.send(saved);
                })),
                Write::Shutdown(done) => Some(Box::new(move |saved| {
                    let _ = done.send(saved);
                })),
                Write::MoveFavorite { source, key, index } => {
                    pending.push_back(PendingWrite::MoveFavorite { source, key, index });
                    None
                }
                Write::Favorite { source, key, on } => {
                    pending.push_back(PendingWrite::Favorite { source, key, on });
                    None
                }
                Write::WatchTime {
                    source,
                    key,
                    elapsed_ms,
                } => {
                    pending.push_back(PendingWrite::WatchTime {
                        source,
                        key,
                        elapsed_ms,
                    });
                    None
                }
                Write::Watched { source, key, at } => {
                    pending.push_back(PendingWrite::Watched { source, key, at });
                    None
                }
            },
        };
        while let Some(write) = pending.front() {
            if !persist(write) {
                break;
            }
            pending.pop_front();
        }
        if let Some(done) = barrier {
            done(pending.is_empty());
        }
    }
    if !pending.is_empty() {
        log::warn!(
            "iptv favorites: writer stopped with {} unsaved changes",
            pending.len()
        );
    }
}

/// Ordered keys of the displayed source. Reorders use the same writer queue
/// as membership changes, so rapid add/move/remove cannot race persistence.
pub fn ordered_keys(owner: &super::Runtime) -> Vec<String> {
    owner.organization.favorites().with(|s| {
        let s = s.borrow();
        s.favorites
            .keys()
            .iter()
            .filter(|key| s.index.get(key).is_some())
            .cloned()
            .collect()
    })
}
pub fn reorder(owner: &super::Runtime, key: &str, index: usize) {
    let write = owner.organization.favorites().with(|s| {
        let mut s = s.borrow_mut();
        let State {
            favorites,
            index: keys,
            ..
        } = &mut *s;
        if keys.get(key).is_none()
            || !favorites.move_key_among(key, index, |key| keys.get(key).is_some())
        {
            return None;
        }
        let write = move_write(&s, key, index, |key| s.index.get(key).is_some());
        if let Some(Write::MoveFavorite { source, key, index }) = &write
            && let Some(pending) = s
                .pending_order
                .upgrade()
                .filter(|pending| pending.source == *source)
        {
            pending.changes.borrow_mut().push(FavoriteChange::Move {
                key: key.clone(),
                index: *index,
            });
        }
        write
    });
    if let Some(write) = write {
        send(owner, write);
    }
    publish(owner);
    owner.emit(super::RuntimeEvent::FavoriteOrderChanged);
}

pub(crate) async fn save_order(
    owner: &super::Runtime,
    source: SourceId,
    original: Vec<String>,
    order: Vec<String>,
) -> Result<(), StoreError> {
    let changes = Rc::new(PendingOrder {
        source,
        changes: RefCell::new(Vec::new()),
    });
    let mut updated = owner.organization.favorites().with(|state| {
        let mut state = state.borrow_mut();
        if state.source != Some(source) || state.pending_order.upgrade().is_some() {
            return Err(StoreError::Unavailable(
                "the favorites list changed during editing",
            ));
        }
        state.pending_order = Rc::downgrade(&changes);
        Ok(state.favorites.clone())
    })?;
    updated.apply_order(&order);

    if owner.services().has_workers() {
        let (done, result) = futures_channel::oneshot::channel();
        let services = owner.services();
        let pending = unsaved(owner);
        send(
            owner,
            Write::SaveOrder(Box::new(move |prior_saved| {
                let result = if prior_saved {
                    services
                        .blocking(move |store| store.save_favorite_order(source, &original, &order))
                } else {
                    Err(StoreError::Unavailable(
                        "recent favorite edits are not saved",
                    ))
                };
                pending.fetch_sub(1, Ordering::AcqRel);
                let _ = done.send(result);
            })),
        );
        result
            .await
            .map_err(|_| StoreError::Unavailable("the favorites order was not saved"))??;
    } else {
        // No writer thread: the order is saved on the store's writer lane,
        // after the writes queued before it (`queue_on_writer`), unless
        // one of them failed.
        let failed = cooperative_failure(owner);
        let counted = Counted::new(owner);
        owner
            .services()
            .call(move |store| {
                let _counted = counted;
                if failed.swap(false, Ordering::AcqRel) {
                    return Err(StoreError::Unavailable(
                        "recent favorite edits are not saved",
                    ));
                }
                store.save_favorite_order(source, &original, &order)
            })
            .await?;
    }
    let changed = owner.organization.favorites().with(|state| {
        let mut state = state.borrow_mut();
        if !state.pending_order.ptr_eq(&Rc::downgrade(&changes)) {
            return false;
        }
        state.pending_order = Weak::new();
        let combined = state.source.is_none() && state.combined_sources.contains(&source);
        if !combined && state.source != Some(source) {
            return false;
        }
        for change in changes.changes.take() {
            change.apply(&mut updated, |key| {
                if combined {
                    state
                        .index
                        .get(&iptv_guide::combined::qualified_key(source.0, key))
                        .is_some()
                } else {
                    state.index.get(key).is_some()
                }
            });
        }
        if combined {
            let order: Vec<_> = updated
                .keys()
                .iter()
                .map(|key| iptv_guide::combined::qualified_key(source.0, key))
                .collect();
            state.favorites.apply_order(&order);
        } else {
            state.favorites = updated;
        }
        true
    });
    if changed {
        publish(owner);
        owner.emit(super::RuntimeEvent::FavoriteOrderChanged);
    }
    Ok(())
}

/// The store write for a favorite that the list in memory just moved to
/// `index`, by the owner lookup the other writes use ([`write_owner`]).
///
/// One playlist on screen: `index` is its place in that playlist's list,
/// which is what the store keeps. "All playlists": the list is every
/// playlist's favorites together, so `index` is not a place in any one
/// of them. The store keeps an order per playlist only, so the write
/// carries the favorite's place among its own playlist's favorites,
/// read from the moved list. How the playlists' favorites interleave is
/// held in memory, as for the rest of the combined list
/// ([`prepare_combined`]). (No screen reorders the combined list today:
/// the menu item is disabled there. This keeps the function right
/// should one start to.)
fn move_write(
    state: &State,
    key: &str,
    index: usize,
    present: impl Fn(&str) -> bool,
) -> Option<Write> {
    let (source, local) = write_owner(state, key)?;
    let index = if state.source.is_some() {
        index
    } else {
        state
            .favorites
            .keys()
            .iter()
            .take_while(|other| other.as_str() != key)
            .filter(|other| {
                present(other)
                    && iptv_guide::combined::split_key(other).is_some_and(|(id, _)| id == source.0)
            })
            .count()
    };
    Some(Write::MoveFavorite {
        source,
        key: local,
        index,
    })
}

#[cfg(all(test, feature = "native"))]
mod reorder_tests {
    use super::*;
    use iptv_guide::combined::qualified_key;

    fn moved(state: &mut State, key: &str, index: usize) -> Option<(SourceId, String, usize)> {
        assert!(state.favorites.move_key(key, index));
        match move_write(state, key, index, |_| true)? {
            Write::MoveFavorite { source, key, index } => Some((source, key, index)),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_load_waits_for_the_inbox_barrier_and_rejects_failed_edits() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let runtime = std::rc::Rc::new(super::super::Runtime::default());
        let (writer, inbox) = mpsc::channel();
        runtime.organization.favorites().borrow_mut().writer = Some(writer);
        unsaved(&runtime).store(1, Ordering::Release);
        let task = {
            let runtime = runtime.clone();
            async move {
                let result = load::<()>(&runtime, |_| {
                    panic!("failed edits must not produce a stale read")
                })
                .await;
                assert!(matches!(result, Err(StoreError::Unavailable(_))));
            }
        };
        let mut task = std::pin::pin!(task);
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
        let Write::Barrier(reply) = inbox
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
        else {
            panic!("the read must wait behind earlier inbox edits")
        };
        reply.send(false).unwrap();
        assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn one_playlist_saves_the_place_in_its_own_list() {
        let mut state = State {
            source: Some(SourceId(7)),
            favorites: Favorites::from_keys(["a", "b", "c"].map(String::from)),
            ..State::default()
        };
        assert_eq!(
            moved(&mut state, "c", 0),
            Some((SourceId(7), "c".to_owned(), 0))
        );
        assert_eq!(state.favorites.keys(), ["c", "a", "b"]);
    }

    #[test]
    fn all_playlists_save_the_place_among_the_owning_playlists_favorites() {
        let key = |source, key: &str| qualified_key(source, key);
        let mut state = State {
            source: None,
            combined_sources: vec![SourceId(1), SourceId(2)],
            favorites: Favorites::from_keys([
                key(1, "a"),
                key(1, "b"),
                key(2, "x"),
                key(2, "y"),
                key(2, "z"),
            ]),
            ..State::default()
        };
        // Last of the list to its front: first of playlist 2's own.
        assert_eq!(
            moved(&mut state, &key(2, "z"), 0),
            Some((SourceId(2), "z".to_owned(), 0))
        );
        // To the fourth place of the combined list: z, a and b lie before
        // it, of which only z is playlist 2's.
        assert_eq!(
            moved(&mut state, &key(2, "y"), 3),
            Some((SourceId(2), "y".to_owned(), 1))
        );
        assert_eq!(
            state.favorites.keys(),
            [
                key(2, "z"),
                key(1, "a"),
                key(1, "b"),
                key(2, "y"),
                key(2, "x")
            ]
        );
        // Playlist 1's own order is untouched by those: b to the end is
        // its second place.
        assert_eq!(
            moved(&mut state, &key(1, "b"), 4),
            Some((SourceId(1), "b".to_owned(), 1))
        );
    }

    #[test]
    fn a_favorite_of_no_shown_playlist_is_not_written() {
        let stray = qualified_key(9, "gone");
        let mut state = State {
            source: None,
            combined_sources: vec![SourceId(1)],
            favorites: Favorites::from_keys([qualified_key(1, "a"), stray.clone()]),
            ..State::default()
        };
        assert_eq!(moved(&mut state, &stray, 0), None);
        // A key that is not a combined one has no owner either.
        state.favorites = Favorites::from_keys(["bare".to_owned()]);
        assert_eq!(moved(&mut state, "bare", 0), None);
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    #[test]
    fn failed_changes_retry_in_order_before_a_barrier_can_succeed() {
        let (tx, rx) = mpsc::channel();
        tx.send(Write::Favorite {
            source: SourceId(1),
            key: "generated-a".into(),
            on: true,
        })
        .unwrap();
        tx.send(Write::MoveFavorite {
            source: SourceId(1),
            key: "generated-a".into(),
            index: 0,
        })
        .unwrap();
        let (first, mut first_result) = futures_channel::oneshot::channel();
        tx.send(Write::Barrier(first)).unwrap();
        let (retry, mut retry_result) = futures_channel::oneshot::channel();
        tx.send(Write::Barrier(retry)).unwrap();
        tx.send(Write::Watched {
            source: SourceId(2),
            key: "generated-b".into(),
            at: 123,
        })
        .unwrap();
        let (last, mut last_result) = futures_channel::oneshot::channel();
        tx.send(Write::Barrier(last)).unwrap();
        drop(tx);
        let mut attempts = 0;
        let mut persisted = Vec::new();
        write_all_with(rx, |write| {
            attempts += 1;
            if attempts <= 3 {
                return false;
            }
            persisted.push(match write {
                PendingWrite::WatchTime { .. } => "duration",
                PendingWrite::ClearHistory => "clear",
                PendingWrite::Favorite { on, .. } => {
                    assert!(*on);
                    "favorite"
                }
                PendingWrite::MoveFavorite { index, .. } => {
                    assert_eq!(*index, 0);
                    "order"
                }
                PendingWrite::Watched { source, at, .. } => {
                    assert_eq!((*source, *at), (SourceId(2), 123));
                    "history"
                }
            });
            true
        });
        assert_eq!(first_result.try_recv().unwrap(), Some(false));
        assert_eq!(retry_result.try_recv().unwrap(), Some(true));
        assert_eq!(last_result.try_recv().unwrap(), Some(true));
        assert_eq!(persisted, ["favorite", "order", "history"]);
    }

    #[test]
    fn history_clear_is_ordered_between_older_and_newer_plays() {
        let (tx, rx) = mpsc::channel();
        let watched = |key: &str| Write::Watched {
            source: SourceId(1),
            key: key.into(),
            at: 1,
        };
        tx.send(watched("old")).unwrap();
        tx.send(Write::ClearHistory).unwrap();
        tx.send(watched("new")).unwrap();
        drop(tx);
        let mut retained = Vec::new();
        write_all_with(rx, |write| {
            match write {
                PendingWrite::Watched { key, .. } => retained.push(key.clone()),
                PendingWrite::ClearHistory => retained.clear(),
                _ => unreachable!(),
            }
            true
        });
        assert_eq!(retained, ["new"]);
    }

    #[test]
    fn repeated_flushes_cannot_clear_a_persistent_failure() {
        let (tx, rx) = mpsc::channel();
        tx.send(Write::Favorite {
            source: SourceId(1),
            key: "generated".into(),
            on: false,
        })
        .unwrap();
        let mut answers = Vec::new();
        for _ in 0..3 {
            let (done, result) = futures_channel::oneshot::channel();
            tx.send(Write::Barrier(done)).unwrap();
            answers.push(result);
        }
        drop(tx);
        let mut attempts = 0;
        write_all_with(rx, |_| {
            attempts += 1;
            false
        });
        assert_eq!(attempts, 4);
        for mut result in answers {
            assert_eq!(result.try_recv().unwrap(), Some(false));
        }
    }
}

#[cfg(all(test, feature = "native"))]
mod vod_availability_tests {
    use super::*;
    use crate::{Ports, Runtime};
    use std::cell::Cell;

    #[test]
    fn reordered_availability_reads_and_old_service_epochs_cannot_publish() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
            Ok(crate::model::DatabaseKey::from_bytes([0x72; 32]))
        })));
        owner.start_services();
        owner
            .organization
            .favorites()
            .with(|state| state.borrow_mut().any_vod = true);
        let events = Rc::new(Cell::new(0));
        let reported = events.clone();
        owner.bind(move |event| {
            if matches!(event, super::super::RuntimeEvent::FavoritesChanged { .. }) {
                reported.set(reported.get() + 1);
            }
        });
        // Poll the newer success first; the older reply cannot replace it.
        let earlier = refresh_vod_availability(&owner);
        let newer = refresh_vod_availability(&owner);
        owner.services().block_on(newer);
        assert!(!owner.organization.favorites().with(|s| s.borrow().any_vod));
        assert_eq!(events.get(), 1);
        owner.services().block_on(earlier);
        assert!(!owner.organization.favorites().with(|s| s.borrow().any_vod));
        assert_eq!(events.get(), 1);
        owner
            .organization
            .favorites()
            .with(|state| state.borrow_mut().any_vod = true);
        let retired = refresh_vod_availability(&owner);
        assert!(owner.drain_services());
        owner.start_services();
        owner.services().block_on(retired);
        assert!(owner.organization.favorites().with(|s| s.borrow().any_vod));
        assert_eq!(events.get(), 1);
        let reset_request = refresh_vod_availability(&owner);
        reset(&owner);
        owner
            .organization
            .favorites()
            .with(|state| state.borrow_mut().any_vod = true);
        owner.services().block_on(reset_request);
        assert!(owner.organization.favorites().with(|s| s.borrow().any_vod));
        assert_eq!(events.get(), 1);
        assert!(owner.drain_services());
    }

    #[test]
    fn a_failed_newer_guide_read_does_not_discard_a_valid_availability_reply() {
        use std::{
            future::Future,
            task::{Context, Waker},
        };
        let directory = tempfile::tempdir().unwrap();
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
            Ok(crate::model::DatabaseKey::from_bytes([0x74; 32]))
        })));
        owner.start_services();
        owner
            .organization
            .favorites()
            .with(|s| s.borrow_mut().any_vod = true);
        let valid = refresh_vod_availability(&owner); // reads the empty library
        let (writer, inbox) = mpsc::channel();
        owner.organization.favorites().borrow_mut().writer = Some(writer);
        unsaved(&owner).store(1, Ordering::Release);
        // A newer guide preparation reserves, but a failed persistence barrier
        // prevents its read/install. Its reservation carries no new authority.
        let mut failed = std::pin::pin!(prepare_source(&owner, Some(SourceId(74))));
        let mut context = Context::from_waker(Waker::noop());
        assert!(failed.as_mut().poll(&mut context).is_pending());
        let Write::Barrier(reply) = inbox
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
        else {
            panic!("guide preparation must await its favorite-write barrier")
        };
        reply.send(false).unwrap();
        assert!(owner.services().block_on(failed).is_none());
        owner.services().block_on(valid);
        assert!(!owner.organization.favorites().with(|s| s.borrow().any_vod));
        owner.organization.favorites().borrow_mut().writer.take();
        unsaved(&owner).store(0, Ordering::Release);
        assert!(owner.drain_services());
    }

    #[test]
    fn a_newer_unchanged_success_still_rejects_an_older_different_value() {
        let owner = Runtime::default();
        let older = begin_vod_request(&owner);
        let newer = begin_vod_request(&owner);
        owner.organization.favorites().with(|s| {
            let mut state = s.borrow_mut();
            assert!(!apply_vod_availability(&mut state, &newer, false));
            assert!(!apply_vod_availability(&mut state, &older, true));
            assert!(!state.any_vod);
        });
    }

    /// A newer shelf read can finish while an already-read guide waits to
    /// install. It reports the still-mounted guide's index and does not cancel
    /// that preparation; only its older global availability value is fenced.
    #[test]
    fn dynamic_availability_keeps_the_mounted_index_and_allows_pending_guide_install() {
        use iptv_providers::{
            SecretUrl,
            model::{M3uSourceConfig, Source, SourceConfig},
        };
        let directory = tempfile::tempdir().unwrap();
        let key = crate::model::DatabaseKey::from_bytes([0x73; 32]);
        let mut store = Store::open_encrypted(directory.path().join("iptv.sqlite"), &key).unwrap();
        let source = |name: &str| Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated-guide.m3u"),
                user_agent: None,
                referrer: None,
            }),
        };
        let old = store
            .add_source(&source("Generated mounted source"))
            .unwrap();
        let next = store.add_source(&source("Generated next source")).unwrap();
        drop(store);
        let owner = Rc::new(Runtime::new(Ports::new(
            directory.path().into(),
            move |_| Ok(key.clone()),
        )));
        owner.start_services();
        let mounted = owner.schedule();
        let channel = mounted.channel_key(0).unwrap();
        owner.organization.favorites().with(|state| {
            let mut state = state.borrow_mut();
            state.source = Some(old);
            state.index = KeyIndex::build(mounted.as_ref());
            state.favorites = Favorites::from_keys([channel]);
            state.has_vod = true;
            state.any_vod = true;
        });
        let events = Rc::new(std::cell::RefCell::new(Vec::new()));
        let reported = events.clone();
        owner.bind(move |event| {
            if let super::super::RuntimeEvent::FavoritesChanged {
                channels,
                has_vod,
                any_vod,
            } = event
            {
                reported.borrow_mut().push((channels, has_vod, any_vod));
            }
        });
        // The next guide's data has loaded, but its actual installation is held.
        let install = owner
            .services()
            .block_on(prepare_source(&owner, Some(next)))
            .unwrap();
        owner.services().block_on(refresh_vod_availability(&owner));
        assert!(Rc::ptr_eq(&mounted, &owner.schedule()));
        assert_eq!(
            owner.organization.favorites().with(|s| s.borrow().source),
            Some(old)
        );
        assert_eq!(*events.borrow(), vec![(BTreeSet::from([0]), true, false)]);
        // A superseded availability ticket must not discard the prepared guide.
        install();
        owner.organization.favorites().with(|state| {
            let state = state.borrow();
            assert_eq!(state.source, Some(next));
            assert!(state.favorites.keys().is_empty());
            assert!(!state.has_vod);
            assert!(!state.any_vod);
        });
        assert!(owner.drain_services());
    }

    #[test]
    fn an_unavailable_read_keeps_the_last_good_shelf_availability() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
            Err(StoreError::Unavailable("generated availability read failure").into())
        })));
        owner.start_services();
        owner
            .organization
            .favorites()
            .with(|state| state.borrow_mut().any_vod = true);
        owner.services().block_on(refresh_vod_availability(&owner));
        assert!(owner.organization.favorites().with(|s| s.borrow().any_vod));
        assert!(owner.drain_services());
    }
}

#[cfg(all(test, feature = "native"))]
mod start_tests {
    use super::*;
    use crate::execution::{Execution, Job, Lane, Tasks};

    /// Native lanes and tasks whose workers cannot start (a thread-spawn
    /// failure).
    struct NoThreads;
    impl Execution for NoThreads {
        fn lane(&self, name: &'static str) -> std::io::Result<Box<dyn Lane>> {
            crate::native::execution::NativeExecution.lane(name)
        }
        fn worker(&self, _: String, _: Job) -> std::io::Result<()> {
            Err(std::io::Error::other("synthetic: no threads left"))
        }
        fn tasks(&self) -> Box<dyn Tasks> {
            crate::native::execution::NativeExecution.tasks()
        }
    }

    struct Unpolled;
    impl crate::execution::CooperativeHost for Unpolled {
        fn spawn(&self, _: crate::execution::Task) {}
        fn delay(&self, _: std::time::Duration) -> crate::execution::Task {
            Box::pin(std::future::pending())
        }
        fn now(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
    }

    /// A runtime on a cooperative executor (a browser's), over a store
    /// file in `directory`, with one synthetic playlist on screen.
    fn cooperative(directory: &std::path::Path) -> (crate::Runtime, SourceId) {
        let ports = crate::Ports::with_adapters(
            directory.to_owned(),
            |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x5c; 32])),
            std::sync::Arc::new(crate::native::network::NativeNetwork),
            std::sync::Arc::new(crate::execution::Cooperative::new(std::sync::Arc::new(
                Unpolled,
            ))),
            crate::native::entropy,
        );
        let owner = crate::Runtime::new(ports);
        owner.start_services();
        let source = owner
            .services()
            .blocking(|store| {
                store.add_source(&iptv_providers::model::Source {
                    name: "Synthetic favorites".into(),
                    config: iptv_providers::model::SourceConfig::M3u(
                        iptv_providers::model::M3uSourceConfig {
                            location: iptv_providers::SecretUrl::new(
                                "https://example.invalid/favorites.m3u",
                            ),
                            user_agent: None,
                            referrer: None,
                        },
                    ),
                })
            })
            .unwrap();
        owner
            .organization
            .favorites()
            .with(|state| state.borrow_mut().source = Some(source));
        (owner, source)
    }

    fn poll<T>(future: std::pin::Pin<&mut impl std::future::Future<Output = T>>) -> Option<T> {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        match future.poll(&mut context) {
            std::task::Poll::Ready(value) => Some(value),
            std::task::Poll::Pending => None,
        }
    }

    #[test]
    fn without_workers_changes_are_saved_on_the_stores_writer_in_order() {
        let directory = tempfile::tempdir().unwrap();
        let (owner, source) = cooperative(directory.path());
        let notices = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = notices.clone();
        owner.bind(move |event| {
            if matches!(event, crate::RuntimeEvent::Notify(_)) {
                seen.set(seen.get() + 1);
            }
        });
        set_key(&owner, "synthetic-a", true);
        set_key(&owner, "synthetic-b", true);
        set_key(&owner, "synthetic-a", false);
        set_key(&owner, "synthetic-c", true);
        played(&owner, "synthetic-b");
        assert_eq!(
            unsaved(&owner).load(Ordering::Acquire),
            5,
            "queued, not yet run"
        );
        {
            let flushed = flush(&owner);
            let mut flushed = std::pin::pin!(flushed);
            assert_eq!(
                poll(flushed.as_mut()),
                None,
                "the barrier waits for the writes"
            );
            // A blocking request runs the writer's queue up to it, as a turn
            // of the host's loop would.
            let (keys, recent) = owner
                .services()
                .blocking(move |store| {
                    Ok((
                        store.favorite_keys(source)?,
                        store.recent(Some(source), Some(ContentKind::Live), 10)?,
                    ))
                })
                .unwrap();
            assert_eq!(keys, ["synthetic-b", "synthetic-c"]);
            assert_eq!(recent.len(), 1);
            assert_eq!(poll(flushed.as_mut()), Some(true), "everything was saved");
        }
        assert_eq!(unsaved(&owner).load(Ordering::Acquire), 0);
        assert_eq!(notices.get(), 0, "nothing tells the user changes are lost");

        // What a relaunch reads.
        assert!(owner.drain_services());
        drop(owner);
        let (owner, _) = cooperative(directory.path());
        let keys = owner
            .services()
            .blocking(move |store| store.favorite_keys(source))
            .unwrap();
        assert_eq!(keys, ["synthetic-b", "synthetic-c"]);
    }

    #[test]
    fn without_workers_a_failed_write_is_reported_by_the_next_barrier() {
        let directory = tempfile::tempdir().unwrap();
        let (owner, _) = cooperative(directory.path());
        // A playlist the store does not have is dropped, as natively; a
        // store error is not.
        owner
            .organization
            .favorites()
            .with(|state| state.borrow_mut().source = Some(SourceId(999)));
        set_key(&owner, "synthetic-gone", true);
        let first = flush(&owner);
        let mut first = std::pin::pin!(first);
        assert_eq!(poll(first.as_mut()), None, "queued");
        owner.services().blocking(|_| Ok(())).unwrap();
        assert_eq!(poll(first.as_mut()), Some(true));
        cooperative_failure(&owner).store(true, Ordering::Release);
        let failed = flush(&owner);
        let mut failed = std::pin::pin!(failed);
        assert_eq!(poll(failed.as_mut()), None, "queued");
        let next = flush(&owner);
        let mut next = std::pin::pin!(next);
        assert_eq!(poll(next.as_mut()), None, "queued");
        owner.services().blocking(|_| Ok(())).unwrap();
        assert_eq!(poll(failed.as_mut()), Some(false));
        assert_eq!(poll(next.as_mut()), Some(true), "reported once");
    }

    #[test]
    fn without_workers_a_reorder_is_saved_on_the_stores_writer() {
        let directory = tempfile::tempdir().unwrap();
        let (owner, source) = cooperative(directory.path());
        // The order is kept among channels the catalog has.
        let parsed = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Synthetic A\nhttps://example.invalid/a.ts\n\
             #EXTINF:-1,Synthetic B\nhttps://example.invalid/b.ts\n",
        );
        let keys: Vec<String> = parsed
            .channels
            .items
            .iter()
            .map(|channel| channel.id.as_str().to_owned())
            .collect();
        owner
            .services()
            .blocking(move |store| {
                store.replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
            })
            .unwrap();
        set_key(&owner, &keys[0], true);
        set_key(&owner, &keys[1], true);
        let order = vec![keys[1].clone(), keys[0].clone()];
        let saved = save_order(&owner, source, keys.clone(), order.clone());
        let mut saved = std::pin::pin!(saved);
        assert!(
            poll(saved.as_mut()).is_none(),
            "queued behind the additions"
        );
        let stored = owner
            .services()
            .blocking(move |store| store.favorite_keys(source))
            .unwrap();
        assert_eq!(stored, order);
        assert!(matches!(poll(saved.as_mut()), Some(Ok(()))));
        assert_eq!(unsaved(&owner).load(Ordering::Acquire), 0);
    }

    #[test]
    fn reset_retires_the_writer_and_keeps_counting_previous_writes() {
        let owner = crate::Runtime::default();
        let (writer, inbox) = mpsc::channel();
        owner.organization.favorites().borrow_mut().writer = Some(writer);
        let pending = Counted::new(&owner);
        let previous_failure = cooperative_failure(&owner);

        reset(&owner);

        assert!(matches!(
            inbox.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(unsaved(&owner).load(Ordering::Acquire), 1);
        previous_failure.store(true, Ordering::Release);
        assert!(!cooperative_failure(&owner).load(Ordering::Acquire));
        drop(pending);
        assert_eq!(unsaved(&owner).load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_writer_that_cannot_start_is_not_kept_and_its_writes_are_not_owed() {
        let mut ports = crate::Ports::for_tests();
        ports.execution = std::sync::Arc::new(NoThreads);
        let owner = crate::Runtime::new(ports);
        owner.start_services();
        watch_time(&owner, SourceId(1), "synthetic-channel".into(), 1_000);
        watch_time(&owner, SourceId(1), "synthetic-channel".into(), 1_000);
        assert_eq!(unsaved(&owner).load(Ordering::Acquire), 0);
        assert!(owner.organization.favorites().borrow().writer.is_none());
        assert!(owner.drain_services());
    }
}
