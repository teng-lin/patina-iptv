//! Application-owned state and events. This module has no UI toolkit imports.
//!
//! A [`Runtime`] is one application owner. Everything it owns (its store
//! connections, file writer, network executor, refresh pool, settings and
//! platform ports) is reached through the handle the caller passes; nothing
//! in this crate looks an owner up through a thread-local or a process-wide
//! registry. The shell keeps the one handle and passes it in.
//!
//! Native library snapshots and store jobs are implementation details. External
//! callers use named commands and portable values instead:
//!
//! ```compile_fail
//! use iptv_runtime::library::Snapshot;
//! ```
//!
//! Stored feed configurations likewise stay behind the runtime boundary:
//!
//! ```compile_fail
//! use iptv_runtime::stored::plan_with;
//! ```
//!
//! ```compile_fail
//! use iptv_runtime::stored::configs_from_sources;
//! ```
pub mod data;
mod error;
mod events;
mod operations;
pub use error::Error;

/// Shell observations needed to prioritize reads and avoid replacing active playback.
#[derive(Default)]
pub struct BrowserState {
    pub focused_key: Option<String>,
    pub playing: bool,
    pub has_vod: bool,
}

mod guide_checks;
pub(crate) mod state;
pub use events::RuntimeEvent;

use iptv_core::guide::{Schedule, SyntheticSchedule};
use std::{cell::RefCell, rc::Rc};

use iptv_guide::EpgSchedule;
use iptv_import::CancelToken;
use iptv_library::SourceId;
use state::{ImportWork, LibrarySnapshot, SingleSchedule, StatusSnapshot};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Weak;

#[derive(Default)]
pub struct Slot<T>(T);
impl<T> Slot<T> {
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0)
    }
}
impl<T> std::ops::Deref for Slot<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

pub type LocalTask = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;
struct TaskHost {
    spawn: Box<dyn Fn(LocalTask)>,
    delay: Box<dyn Fn(std::time::Duration) -> LocalTask>,
}

type EventSink = Rc<dyn Fn(RuntimeEvent)>;
type BrowserObserver = Rc<dyn Fn() -> BrowserState>;
type PageWaiters = RefCell<Vec<Box<dyn FnOnce()>>>;
type Resolves = RefCell<[Option<(u64, CancelToken)>; iptv_core::multiview::MAX_SURFACES]>;
type SavedSettings = RefCell<Option<(u64, std::sync::Arc<iptv_settings::Settings>)>>;

pub struct Runtime {
    account_streams: Slot<Cell<Option<u32>>>,
    resolving: Slot<Resolves>,
    wizard_import: Slot<RefCell<Option<(iptv_core::wizard::ImportId, CancelToken)>>>,
    store: std::sync::Arc<store_service::Service>,
    settings_saved: Slot<SavedSettings>,
    settings_changed: Slot<Cell<u64>>,
    settings_read_only: Cell<Option<std::io::ErrorKind>>,
    link_client:
        std::sync::Arc<std::sync::Mutex<Option<(iptv_net::NetConfig, iptv_net::NetClient)>>>,
    backup_busy: Cell<bool>,
    backup_progress: Cell<backup::Progress>,
    search_indexes: Slot<RefCell<search::Indexes>>,
    saved_search: Slot<RefCell<Option<Vec<String>>>>,
    pending_search: Slot<Cell<u64>>,
    favorites: Slot<RefCell<favorites::State>>,
    favorite_writer: Slot<RefCell<Option<std::sync::mpsc::Sender<favorites::Write>>>>,
    favorite_unsaved: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    activity: RefCell<BTreeMap<SourceId, library::Activity>>,
    all: Cell<bool>,
    generation: Slot<Cell<u64>>,
    imports: Slot<RefCell<HashMap<SourceId, (u64, CancelToken)>>>,
    attempts: Slot<RefCell<HashMap<SourceId, i64>>>,
    tickets: Slot<Cell<u64>>,
    completions: Slot<RefCell<HashMap<SourceId, Weak<ImportWork>>>>,
    library: Slot<RefCell<Option<LibrarySnapshot>>>,
    parts: Slot<RefCell<BTreeMap<SourceId, Rc<EpgSchedule>>>>,
    limits: Slot<RefCell<BTreeMap<SourceId, Option<u32>>>>,
    resume: Slot<Cell<bool>>,
    rebuild: Slot<Cell<u64>>,
    epoch: Slot<Cell<i64>>,
    single: Slot<RefCell<Option<SingleSchedule>>>,
    shown: Slot<Cell<Option<SourceId>>>,
    updating: Slot<RefCell<Vec<SourceId>>>,
    edits: Slot<RefCell<BTreeMap<SourceId, u64>>>,
    status: Slot<RefCell<StatusSnapshot>>,
    page_waiters: Slot<PageWaiters>,
    organization_revision: Slot<Cell<u64>>,
    schedule: RefCell<Rc<dyn Schedule>>,
    events: RefCell<Option<EventSink>>,
    tasks: RefCell<Option<Rc<TaskHost>>>,
    foreground: Cell<bool>,
    scheduler_started: Cell<bool>,
    guide_checks: RefCell<guide_checks::GuideChecks>,
    status_revision: Cell<u64>,
    browser: RefCell<Option<BrowserObserver>>,
    organization_edits: RefCell<HashMap<SourceId, u64>>,
}

impl Runtime {
    /// A new owner with its own services. Constructing one starts no thread
    /// and touches no other owner: its store opens on first use, after
    /// [`Runtime::start_services`].
    pub fn new(ports: Ports) -> Self {
        let store = std::sync::Arc::new(store_service::Service::new(ports));
        Self {
            account_streams: Slot::default(),
            resolving: Slot::default(),
            wizard_import: Slot::default(),
            store,
            settings_saved: Slot::default(),
            settings_changed: Slot::default(),
            settings_read_only: Cell::new(None),
            link_client: Default::default(),
            backup_busy: Cell::new(false),
            backup_progress: Cell::new(backup::Progress::Idle),
            search_indexes: Slot::default(),
            saved_search: Slot::default(),
            pending_search: Slot::default(),
            favorites: Slot::default(),
            favorite_writer: Slot::default(),
            favorite_unsaved: Default::default(),
            activity: RefCell::default(),
            all: Cell::new(false),
            generation: Slot::default(),
            imports: Slot::default(),
            attempts: Slot::default(),
            tickets: Slot::default(),
            completions: Slot::default(),
            library: Slot::default(),
            parts: Slot::default(),
            limits: Slot::default(),
            resume: Slot::default(),
            rebuild: Slot::default(),
            epoch: Slot::default(),
            single: Slot::default(),
            shown: Slot::default(),
            updating: Slot::default(),
            edits: Slot::default(),
            status: Slot::default(),
            page_waiters: Slot::default(),
            organization_revision: Slot::default(),
            schedule: RefCell::new(Rc::new(SyntheticSchedule)),
            events: RefCell::new(None),
            tasks: RefCell::new(None),
            foreground: Cell::new(true),
            scheduler_started: Cell::new(false),
            guide_checks: RefCell::default(),
            status_revision: Cell::new(0),
            browser: RefCell::default(),
            organization_edits: RefCell::default(),
        }
    }
}

impl Runtime {
    /// Private storage captured when this owner was constructed.
    pub fn data_dir(&self) -> &std::path::Path {
        self.store.directory()
    }

    /// This owner's services for work accepted now (see
    /// [`store_service::Services`]): what a worker carries instead of
    /// looking an owner up.
    pub(crate) fn services(&self) -> store_service::Services {
        self.store.accept()
    }

    pub fn bind_tasks(
        &self,
        spawn: impl Fn(LocalTask) + 'static,
        delay: impl Fn(std::time::Duration) -> LocalTask + 'static,
    ) {
        *self.tasks.borrow_mut() = Some(Rc::new(TaskHost {
            spawn: Box::new(spawn),
            delay: Box::new(delay),
        }));
    }
    /// Runs `task` on the host's local executor. The task holds its owner
    /// itself: it names no other one, whichever owner the host polls it under.
    pub(crate) fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        let host = self
            .tasks
            .borrow()
            .clone()
            .expect("runtime task host installed");
        (host.spawn)(Box::pin(task));
    }
    pub(crate) fn delay(
        &self,
        duration: std::time::Duration,
    ) -> impl std::future::Future<Output = ()> + use<> {
        let host = self
            .tasks
            .borrow()
            .clone()
            .expect("runtime task host installed");
        (host.delay)(duration)
    }
    pub fn flush_files(&self, limit: std::time::Duration) -> bool {
        self.store.files().flush(limit)
    }
    /// A handle a host callback may use off the application thread (Android
    /// `onStop`) to wait for this owner's queued file writes.
    pub fn host_handle(&self) -> HostHandle {
        HostHandle(std::sync::Arc::downgrade(&self.store))
    }

    /// The calling thread becomes this owner's application thread: store
    /// requests are accepted from now until [`Runtime::drain_services`].
    pub fn start_services(&self) {
        self.store.begin();
    }
    pub fn drain_services(&self) -> bool {
        self.cancel_guide_checks();
        let drained = self.store.drain_on_shutdown();
        // Detached local tasks can retain this Runtime after the root closes.
        // Retire services now, rather than waiting for its last Rc to drop.
        self.store.retire();
        drained
    }

    pub fn bind_browser(&self, observe: impl Fn() -> BrowserState + 'static) {
        *self.browser.borrow_mut() = Some(Rc::new(observe));
    }
    pub(crate) fn browser(&self) -> BrowserState {
        self.browser
            .borrow()
            .as_ref()
            .map(|observe| observe())
            .unwrap_or_default()
    }

    pub fn bind(&self, sink: impl Fn(RuntimeEvent) + 'static) {
        *self.events.borrow_mut() = Some(Rc::new(sink));
    }

    pub fn emit(&self, event: RuntimeEvent) {
        if let RuntimeEvent::AccountStreams(limit) = &event {
            self.account_streams.set(*limit);
        }
        let sink = self.events.borrow().clone();
        if let Some(sink) = sink {
            sink(event);
        }
    }

    pub fn begin_organization_edit(&self, source: SourceId) -> u64 {
        let mut edits = self.organization_edits.borrow_mut();
        let ticket = edits.entry(source).or_default();
        *ticket = ticket.wrapping_add(1);
        *ticket
    }
    pub fn organization_edit_current(&self, source: SourceId, ticket: u64) -> bool {
        self.organization_edits.borrow().get(&source) == Some(&ticket)
    }

    pub fn schedule(&self) -> Rc<dyn Schedule> {
        self.schedule.borrow().clone()
    }

    pub fn set_schedule(&self, schedule: Rc<dyn Schedule>) {
        *self.schedule.borrow_mut() = schedule;
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.store.retire();
        if let Some((_, cancel)) = self.wizard_import.borrow().as_ref() {
            cancel.cancel();
        }
        for (_, cancel) in self.resolving.borrow().iter().flatten() {
            cancel.cancel();
        }
        for (_, cancel) in self.imports.borrow().values() {
            cancel.cancel();
        }
        for completion in self.completions.borrow().values().filter_map(Weak::upgrade) {
            completion.cancel.cancel();
        }
    }
}

/// What a host may ask of one owner from any thread: to wait for its queued
/// file writes. It names its owner explicitly and does not keep it alive:
/// once the owner is gone there is nothing to wait for.
#[derive(Clone)]
pub struct HostHandle(std::sync::Weak<store_service::Service>);
impl HostHandle {
    /// Waits, at most `limit`, until every write queued so far is on disk.
    pub fn flush_files(&self, limit: std::time::Duration) -> bool {
        // A host can stop before the application root has accepted any work,
        // or after it closed.
        self.0
            .upgrade()
            .is_none_or(|service| service.files().flush(limit))
    }

    /// Occupies this owner's store writer for `duration` after the work
    /// queued before it (the headless tests of what must not wait for it
    /// drive the app from another thread).
    #[cfg(feature = "test-support")]
    pub fn occupy_writer(&self, duration: std::time::Duration) {
        if let Some(service) = self.0.upgrade() {
            service.accept().occupy_writer(duration);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_runtimes_can_apply_events_during_delivery() {
        let first = Rc::new(Runtime::default());
        let second = Runtime::default();
        let original_second = second.schedule();
        let target: Rc<dyn Schedule> = Rc::new(SyntheticSchedule);
        let weak = Rc::downgrade(&first);
        first.bind(move |event| {
            if let RuntimeEvent::ScheduleReady(schedule) = event {
                weak.upgrade().unwrap().set_schedule(schedule);
            }
        });
        first.emit(RuntimeEvent::ScheduleReady(target.clone()));
        assert!(Rc::ptr_eq(&first.schedule(), &target));
        assert!(Rc::ptr_eq(&second.schedule(), &original_second));
        assert!(!Rc::ptr_eq(&second.schedule(), &target));
    }

    #[test]
    fn a_late_local_reply_keeps_its_accepting_runtime() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let first = Rc::new(Runtime::default());
        let second = Rc::new(Runtime::default());
        let (reply, answer) = futures_channel::oneshot::channel();
        // The task holds the owner that accepted it; no ambient owner exists
        // for a later poll to pick up instead.
        let owner = first.clone();
        let task = async move {
            let limit = answer.await.unwrap();
            owner.account_streams.set(Some(limit));
        };
        let mut task = std::pin::pin!(task);
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
        reply.send(3).unwrap();
        assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
        assert_eq!(first.account_streams.get(), Some(3));
        assert_eq!(second.account_streams.get(), None);
    }

    #[test]
    fn constructing_another_runtime_leaves_an_open_owner_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let make = |name: &str| {
            let mut ports = Ports::for_tests();
            ports.directory = directory.path().join(name);
            Rc::new(Runtime::new(ports))
        };
        let first = make("alpha");
        first.start_services();
        let accepted = first.services();
        assert!(accepted.is_current());
        // Constructing, starting and closing another owner changes nothing
        // the first owner or its accepted work can observe.
        let second = make("beta");
        assert!(accepted.is_current());
        assert_eq!(first.services().directory(), directory.path().join("alpha"));
        second.start_services();
        assert!(accepted.is_current());
        assert_eq!(accepted.directory(), directory.path().join("alpha"));
        assert_eq!(second.services().directory(), directory.path().join("beta"));
        assert!(second.drain_services());
        drop(second);
        assert!(accepted.is_current());
        assert!(first.services().is_current());
        // Each owner has its own settings.
        let third = make("gamma");
        *first.services().shared_settings().write().unwrap() = Some(Default::default());
        assert!(third.services().shared_settings().read().unwrap().is_none());
        assert!(first.drain_services());
        assert!(!accepted.is_current());
    }

    #[test]
    fn closing_one_library_cancels_only_its_own_imports() {
        let first = Runtime::default();
        let second = Runtime::default();
        let first_cancel = CancelToken::new();
        let second_cancel = CancelToken::new();
        first
            .imports
            .borrow_mut()
            .insert(SourceId(1), (1, first_cancel.clone()));
        second
            .imports
            .borrow_mut()
            .insert(SourceId(1), (1, second_cancel.clone()));
        drop(first);
        assert!(first_cancel.is_cancelled());
        assert!(!second_cancel.is_cancelled());
    }
}

mod store_service;

mod executor;

pub(crate) mod file_writer;

pub mod commands;

pub(crate) mod imports;

pub mod stored;

pub(crate) mod opening;

pub(crate) mod paging;

pub mod playback;

pub mod guides;

mod workers;

pub mod wizard_map;

pub(crate) mod private_files;

mod library;

pub(crate) mod favorites;

pub(crate) mod search;

pub mod backup;

pub(crate) mod assign;

pub(crate) mod credential_store;
pub mod model;

pub mod settings;

mod ports;
pub use ports::Ports;
pub mod storage;

#[cfg(test)]
impl Default for Runtime {
    fn default() -> Self {
        Self::new(Ports::for_tests())
    }
}
