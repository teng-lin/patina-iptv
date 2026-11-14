//! Application-owned state and events. This module has no UI toolkit imports.
mod events;
mod guide_checks;
pub mod state;
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
type PageWaiters = RefCell<Vec<Box<dyn FnOnce()>>>;
type Resolves = RefCell<[Option<(u64, CancelToken)>; iptv_core::multiview::MAX_SURFACES]>;
type SavedSettings = RefCell<Option<(u64, std::sync::Arc<iptv_settings::Settings>)>>;

pub struct Runtime {
    pub(crate) account_streams: Slot<Cell<Option<u32>>>,
    pub(crate) resolving: Slot<Resolves>,
    pub(crate) wizard_import: Slot<RefCell<Option<(iptv_core::wizard::ImportId, CancelToken)>>>,
    store: std::sync::Arc<store_service::Service>,
    pub(crate) settings_saved: Slot<SavedSettings>,
    pub(crate) settings_changed: Slot<Cell<u64>>,
    pub(crate) settings_read_only: Cell<Option<std::io::ErrorKind>>,
    pub(crate) link_client:
        std::sync::Arc<std::sync::Mutex<Option<(iptv_net::NetConfig, iptv_net::NetClient)>>>,
    pub(crate) backup_busy: Cell<bool>,
    pub(crate) backup_progress: Cell<backup::Progress>,
    pub(crate) search_indexes: Slot<RefCell<search::Indexes>>,
    pub(crate) saved_search: Slot<RefCell<Option<Vec<String>>>>,
    pub(crate) pending_search: Slot<Cell<u64>>,
    pub(crate) favorites: Slot<RefCell<favorites::State>>,
    pub(crate) favorite_writer: Slot<RefCell<Option<std::sync::mpsc::Sender<favorites::Write>>>>,
    pub(crate) favorite_unsaved: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) activity: RefCell<BTreeMap<SourceId, library::Activity>>,
    pub(crate) all: Cell<bool>,
    pub(crate) generation: Slot<Cell<u64>>,
    pub(crate) imports: Slot<RefCell<HashMap<SourceId, (u64, CancelToken)>>>,
    pub(crate) attempts: Slot<RefCell<HashMap<SourceId, i64>>>,
    pub(crate) tickets: Slot<Cell<u64>>,
    pub(crate) completions: Slot<RefCell<HashMap<SourceId, Weak<ImportWork>>>>,
    pub(crate) library: Slot<RefCell<Option<LibrarySnapshot>>>,
    pub(crate) parts: Slot<RefCell<BTreeMap<SourceId, Rc<EpgSchedule>>>>,
    pub(crate) limits: Slot<RefCell<BTreeMap<SourceId, Option<u32>>>>,
    pub(crate) resume: Slot<Cell<bool>>,
    pub(crate) rebuild: Slot<Cell<u64>>,
    pub(crate) epoch: Slot<Cell<i64>>,
    pub(crate) single: Slot<RefCell<Option<SingleSchedule>>>,
    pub(crate) shown: Slot<Cell<Option<SourceId>>>,
    pub(crate) updating: Slot<RefCell<Vec<SourceId>>>,
    pub(crate) edits: Slot<RefCell<BTreeMap<SourceId, u64>>>,
    pub(crate) status: Slot<RefCell<StatusSnapshot>>,
    pub(crate) page_waiters: Slot<PageWaiters>,
    pub(crate) organization_revision: Slot<Cell<u64>>,
    schedule: RefCell<Rc<dyn Schedule>>,
    events: RefCell<Option<EventSink>>,
    tasks: RefCell<Option<Rc<TaskHost>>>,
    pub(crate) foreground: Cell<bool>,
    pub(crate) scheduler_started: Cell<bool>,
    guide_checks: RefCell<guide_checks::GuideChecks>,
    organization_edits: RefCell<HashMap<SourceId, u64>>,
}

impl Runtime {
    pub fn new(ports: Ports) -> Self {
        let store = std::sync::Arc::new(store_service::Service::new(ports));
        Self {
            account_streams: Slot::default(),
            resolving: Slot::default(),
            wizard_import: Slot::default(),
            store: store.clone(),
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
            organization_edits: RefCell::default(),
        }
    }
}

impl Runtime {
    /// Private storage captured when this owner was constructed.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.store.directory
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
    pub(crate) fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        let host = self
            .tasks
            .borrow()
            .clone()
            .expect("runtime task host installed");
        (host.spawn)(Box::pin(task));
    }
    pub(crate) async fn delay(&self, duration: std::time::Duration) {
        let host = self
            .tasks
            .borrow()
            .clone()
            .expect("runtime task host installed");
        (host.delay)(duration).await;
    }
    pub fn with_context<R>(&self, run: impl FnOnce() -> R) -> R {
        store_service::with_service(self.store.clone(), run)
    }
    /// Capture native context before starting a worker, including bootstrap
    /// work accepted before the store's UI epoch exists.
    pub fn scope_worker<R: 'static, F>(
        &self,
        work: F,
    ) -> impl FnOnce() -> R + Send + 'static + use<R, F>
    where
        F: FnOnce() -> R + Send + 'static,
    {
        self.with_context(|| store_service::scoped(work))
    }
    pub fn scope_future<F: std::future::Future + 'static>(
        &self,
        future: F,
    ) -> impl std::future::Future<Output = F::Output> + 'static + use<F> {
        store_service::scope_future(self.store.clone(), future)
    }
    pub fn flush_files(&self, limit: std::time::Duration) -> bool {
        self.with_context(|| file_writer::flush(limit))
    }

    pub fn activate(&self) {
        store_service::activate(self.store.clone());
    }
    pub fn start_services(&self) {
        store_service::with_service(self.store.clone(), store_service::register_ui_thread);
    }
    pub fn drain_services(&self) -> bool {
        self.cancel_guide_checks();
        let drained =
            store_service::with_service(self.store.clone(), store_service::drain_on_shutdown);
        // Detached local tasks can retain this Runtime after the root closes.
        // Retire services now, rather than waiting for its last Rc to drop.
        self.store.retire();
        drained
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

/// Flush accepted file writes from a host callback that can run off-thread.
pub fn flush_pending_writes(limit: std::time::Duration) -> bool {
    let Some(owner) = store_service::active_service() else {
        // A host can stop before the application root has accepted any work.
        return true;
    };
    store_service::with_service(owner, || file_writer::flush(limit))
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
        let task = crate::context::with_runtime(first.clone(), || {
            crate::context::in_runtime(async move {
                let limit = answer.await.unwrap();
                crate::current().account_streams.set(Some(limit));
            })
        });
        let mut task = std::pin::pin!(task);
        let mut context = Context::from_waker(Waker::noop());
        crate::context::with_runtime(second.clone(), || {
            assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
            reply.send(3).unwrap();
            assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
        });
        assert_eq!(first.account_streams.get(), Some(3));
        assert_eq!(second.account_streams.get(), None);
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

pub mod executor;

pub(crate) mod file_writer;

pub mod commands;

pub mod imports;

pub mod stored;

pub mod opening;

pub mod paging;

pub mod playback;

pub mod guides;

pub mod workers;

pub mod wizard_map;

pub(crate) mod private_files;

pub mod library;

pub mod favorites;

pub mod search;

pub mod backup;

pub mod assign;

pub(crate) mod credential_store;
pub mod model;

pub mod settings;

#[cfg(feature = "test-support")]
pub use store_service::occupy_writer;

mod paths;

mod ports;
pub use ports::Ports;
mod context;
pub use context::{current, in_runtime, set_current, try_current, with_runtime};
pub mod storage;

#[cfg(test)]
impl Default for Runtime {
    fn default() -> Self {
        Self::new(Ports::for_tests())
    }
}

impl Runtime {
    pub fn account_streams(&self) -> &Slot<Cell<Option<u32>>> {
        &self.account_streams
    }
    pub fn resolving(&self) -> &Slot<Resolves> {
        &self.resolving
    }
    pub fn wizard_import(
        &self,
    ) -> &Slot<RefCell<Option<(iptv_core::wizard::ImportId, CancelToken)>>> {
        &self.wizard_import
    }
    pub fn search_indexes(&self) -> &Slot<RefCell<search::Indexes>> {
        &self.search_indexes
    }
    pub fn saved_search(&self) -> &Slot<RefCell<Option<Vec<String>>>> {
        &self.saved_search
    }
    pub fn pending_search(&self) -> &Slot<Cell<u64>> {
        &self.pending_search
    }
    pub fn all(&self) -> &Cell<bool> {
        &self.all
    }
    pub fn generation(&self) -> &Slot<Cell<u64>> {
        &self.generation
    }
    pub fn imports(&self) -> &Slot<RefCell<HashMap<SourceId, (u64, CancelToken)>>> {
        &self.imports
    }
    pub fn attempts(&self) -> &Slot<RefCell<HashMap<SourceId, i64>>> {
        &self.attempts
    }
    pub fn tickets(&self) -> &Slot<Cell<u64>> {
        &self.tickets
    }
    pub fn library(&self) -> &Slot<RefCell<Option<LibrarySnapshot>>> {
        &self.library
    }
    pub fn parts(&self) -> &Slot<RefCell<BTreeMap<SourceId, Rc<EpgSchedule>>>> {
        &self.parts
    }
    pub fn limits(&self) -> &Slot<RefCell<BTreeMap<SourceId, Option<u32>>>> {
        &self.limits
    }
    pub fn resume(&self) -> &Slot<Cell<bool>> {
        &self.resume
    }
    pub fn rebuild(&self) -> &Slot<Cell<u64>> {
        &self.rebuild
    }
    pub fn epoch(&self) -> &Slot<Cell<i64>> {
        &self.epoch
    }
    pub fn single(&self) -> &Slot<RefCell<Option<SingleSchedule>>> {
        &self.single
    }
    pub fn shown(&self) -> &Slot<Cell<Option<SourceId>>> {
        &self.shown
    }
    pub fn updating(&self) -> &Slot<RefCell<Vec<SourceId>>> {
        &self.updating
    }
    pub fn status(&self) -> &Slot<RefCell<StatusSnapshot>> {
        &self.status
    }
    pub fn page_waiters(&self) -> &Slot<PageWaiters> {
        &self.page_waiters
    }
    pub fn organization_revision(&self) -> &Slot<Cell<u64>> {
        &self.organization_revision
    }
    pub fn foreground(&self) -> &Cell<bool> {
        &self.foreground
    }
}
