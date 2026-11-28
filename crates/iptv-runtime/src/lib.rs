// The browser's `std::fs` is a stub whose `File` has no destructor; the
// file paths that drop one early only fail there.
#![cfg_attr(target_family = "wasm", allow(clippy::drop_non_drop))]
//! Application-owned state and events. This module has no UI toolkit imports.
//!
//! The supported query/inspection/repair facade is specified in
//! `docs/runtime-api-v1.md`; other public helpers remain experimental.
//!
//! A [`Runtime`] is one application owner. Everything it owns (its store
//! connections, file writer, network executor, refresh pool, settings and
//! platform ports) is reached through the handle the caller passes; nothing
//! in this crate looks an owner up through a thread-local or a process-wide
//! registry. The shell keeps the one handle and passes it in. A native host may
//! explicitly share recording execution across UI engine lifetimes for the same
//! locked storage root; UI services and transient playback remain owner-scoped.
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
//!
//! Runtime failures cannot be converted back into storage errors by consumers.
//!
//! ```compile_fail
//! fn convert(error: iptv_runtime::Error) -> iptv_store_sqlite::StoreError {
//!     error.into()
//! }
//! ```
//!
//! The runtime does not dereference to an internal state owner.
//!
//! ```compile_fail
//! fn requires_deref<T: std::ops::Deref>() {}
//! requires_deref::<iptv_runtime::Runtime>();
//! ```
pub mod build_config;
pub mod cast;
pub mod data;
mod error;
mod events;
pub mod guide_repair;
pub mod inspection;
mod operations;
pub mod parental;
#[cfg(feature = "native")]
pub mod recordings;
#[cfg(not(feature = "native"))]
#[path = "recordings_unavailable.rs"]
pub mod recordings;
pub mod vod;
pub use error::Error;
pub use model::StoreQueueStats;
pub use operations::ExternalPlaybackAuthorization;
#[cfg(feature = "test-support")]
mod testing;
#[cfg(feature = "test-support")]
pub use testing::TestGate;

/// Shell observations needed to prioritize reads and avoid replacing active playback.
#[derive(Default)]
pub struct BrowserState {
    pub focused_key: Option<String>,
    pub playing: bool,
}

mod catalog;
mod guide_checks;
mod import_state;
mod lifecycle;
mod organization;

pub use guide_checks::GuideWork;
pub(crate) mod state;
pub use events::{RuntimeEvent, ScheduleCause};

use iptv_core::guide::{Schedule, SyntheticSchedule};
use std::{cell::RefCell, rc::Rc};

use iptv_guide::EpgSchedule;
use iptv_import::CancelToken;
use iptv_library::SourceId;
use state::{ImportWork, LibrarySnapshot, SingleSchedule, StatusSnapshot};
use std::cell::Cell;
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

/// A source's turn on screen ([`Runtime::show_turn`]), until dropped.
pub struct ShowTurn(#[allow(dead_code)] futures_channel::oneshot::Sender<()>);

pub type LocalTask = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;
struct TaskHost {
    spawn: Box<dyn Fn(LocalTask)>,
    delay: Box<dyn Fn(std::time::Duration) -> LocalTask>,
}

#[derive(Default)]
struct TaskBinding(RefCell<Option<Rc<TaskHost>>>);
impl TaskBinding {
    fn bind(
        &self,
        spawn: impl Fn(LocalTask) + 'static,
        delay: impl Fn(std::time::Duration) -> LocalTask + 'static,
    ) {
        self.0.replace(Some(Rc::new(TaskHost {
            spawn: Box::new(spawn),
            delay: Box::new(delay),
        })));
    }

    fn host(&self) -> Option<Rc<TaskHost>> {
        self.0.borrow().clone()
    }

    fn has_tasks(&self) -> bool {
        self.0.borrow().is_some()
    }

    fn try_spawn(&self, task: LocalTask) -> bool {
        let Some(host) = self.host() else {
            return false;
        };
        (host.spawn)(task);
        true
    }

    fn delay(&self, duration: std::time::Duration) -> LocalTask {
        match self.host() {
            Some(host) => (host.delay)(duration),
            None => Box::pin(std::future::pending()),
        }
    }
}

type EventSink = Rc<dyn Fn(RuntimeEvent)>;
type BrowserObserver = Rc<dyn Fn() -> BrowserState>;
type PageWaiters = RefCell<Vec<data::pages::Waiter>>;
type Resolves = RefCell<std::collections::HashMap<playback::StreamPurpose, (u64, CancelToken)>>;
pub struct Runtime {
    cast: cast::State,
    parental: RefCell<parental::State>,
    recordings: recordings::OwnerState,
    vod: std::sync::Arc<vod::State>,
    repairs: RefCell<guide_repair::State>,
    store: std::sync::Arc<store_service::Service>,
    inspection: inspection::Vault,
    settings: settings::State,
    backup: backup::State,
    search: search::State,
    organization: organization::State,
    catalog: catalog::State,
    imports: import_state::State,
    lifecycle: lifecycle::State,
}

impl Runtime {
    /// A new owner with its own services. Constructing one starts no thread
    /// and touches no other owner: its store opens on first use, after
    /// [`Runtime::start_services`].
    pub fn new(ports: Ports) -> Self {
        let recordings = recordings::host::acquire(&ports);
        let entropy = ports.entropy;
        let store = store_service::Service::shared(ports);
        #[cfg(feature = "test-support")]
        let guide_checks = guide_checks::GuideChecks::mirrored(store.guide_checks.clone());
        #[cfg(not(feature = "test-support"))]
        let guide_checks = guide_checks::GuideChecks::default();
        let schedule = Rc::new(SyntheticSchedule);
        Self {
            cast: cast::State::default(),
            parental: RefCell::default(),
            recordings: recordings::OwnerState::new(recordings),
            vod: Default::default(),
            repairs: RefCell::default(),
            store,
            inspection: inspection::Vault::new(entropy),
            settings: settings::State::default(),
            backup: backup::State::default(),
            search: search::State::default(),
            organization: organization::State::new(favorites::State::with_catalog(&*schedule)),
            catalog: catalog::State::new(schedule),
            imports: import_state::State::new(guide_checks),
            lifecycle: lifecycle::State::default(),
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
        self.lifecycle.tasks().bind(spawn, delay);
    }
    /// Runs `task` on the host's local executor. The task holds its owner
    /// itself: it names no other one, whichever owner the host polls it under.
    pub(crate) fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        if !self.lifecycle.tasks().try_spawn(Box::pin(task)) {
            log::error!("runtime task rejected because no task host is available");
        }
    }

    pub(crate) fn has_tasks(&self) -> bool {
        self.lifecycle.tasks().has_tasks()
    }

    pub(crate) fn try_spawn(&self, task: impl std::future::Future<Output = ()> + 'static) -> bool {
        self.lifecycle.tasks().try_spawn(Box::pin(task))
    }
    pub(crate) fn delay(
        &self,
        duration: std::time::Duration,
    ) -> impl std::future::Future<Output = ()> + use<> {
        self.lifecycle.tasks().delay(duration)
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
        self.lifecycle.shutdown_requested().set(false);
        self.store.begin();
        self.inspection.renew();
    }
    pub fn drain_services(&self) -> bool {
        let deadline = crate::time::Instant::now() + self.store.drain_limit();
        self.inspection.close_workers();
        self.parental_lock();
        self.recordings.detach_ui();
        self.cancel_guide_checks();
        self.cancel_all_stream_resolutions();
        // A Recently added sort still running must not hold the analysis
        // lane's teardown barrier.
        self.vod.cancel_discovery();
        self.lifecycle.shutdown_requested().set(true);
        self.close_cast();
        let drained = self
            .store
            .drain_on_shutdown_until(deadline, || self.inspection.cancel_requests());
        if let Some((_, cancel)) = self.imports.wizard_import.borrow_mut().take() {
            cancel.cancel();
        }
        let probes_drained = self
            .inspection
            .drain_workers(deadline.saturating_duration_since(crate::time::Instant::now()));
        let cast_drained = self.drain_cast(deadline);
        // Detached local tasks can retain this Runtime after the root closes.
        // Retire services now, rather than waiting for its last Rc to drop.
        self.store.retire();
        self.inspection.clear();
        self.repairs.borrow_mut().clear();
        drained && probes_drained && cast_drained
    }

    pub fn bind_browser(&self, observe: impl Fn() -> BrowserState + 'static) {
        *self.lifecycle.browser().borrow_mut() = Some(Rc::new(observe));
    }
    pub(crate) fn browser(&self) -> BrowserState {
        self.lifecycle
            .browser()
            .borrow()
            .as_ref()
            .map(|observe| observe())
            .unwrap_or_default()
    }

    pub fn bind(&self, sink: impl Fn(RuntimeEvent) + 'static) {
        *self.lifecycle.events().borrow_mut() = Some(Rc::new(sink));
    }

    pub fn emit(&self, event: RuntimeEvent) {
        if let RuntimeEvent::AccountStreams(limit) = &event {
            self.catalog.set_account_streams(*limit);
        }
        let sink = self.lifecycle.events().borrow().clone();
        if let Some(sink) = sink {
            sink(event);
        }
    }

    pub(crate) fn begin_organization_edit(&self, source: SourceId) -> u64 {
        let mut edits = self.organization.edits().borrow_mut();
        let ticket = edits.entry(source).or_default();
        *ticket = ticket.wrapping_add(1);
        *ticket
    }
    pub(crate) fn organization_edit_current(&self, source: SourceId, ticket: u64) -> bool {
        self.organization.edits().borrow().get(&source) == Some(&ticket)
    }

    /// A turn at putting `source` on screen: shows of one source finish in
    /// the order they began, so one that began earlier, with older data,
    /// cannot replace a later one. Resolves when the earlier ones have
    /// finished (or were dropped); the turn ends when the guard is dropped.
    pub fn show_turn(&self, source: SourceId) -> impl std::future::Future<Output = ShowTurn> {
        let (done, next) = futures_channel::oneshot::channel();
        let earlier = self.catalog.enqueue_show(source, next);
        async move {
            if let Some(earlier) = earlier {
                let _ = earlier.await;
            }
            ShowTurn(done)
        }
    }

    pub fn schedule(&self) -> Rc<dyn Schedule> {
        self.catalog.schedule()
    }

    pub(crate) fn set_schedule(&self, schedule: Rc<dyn Schedule>) -> Rc<dyn Schedule> {
        self.catalog.replace_schedule(schedule)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // An abandoned reply can leave its discovery query on the analysis lane.
        // Cancel it before fallback draining waits for that lane's barrier.
        self.vod.cancel_discovery();
        self.recordings.detach_ui();
        if let Some((_, cancel)) = self.imports.wizard_import.borrow().as_ref() {
            cancel.cancel();
        }
        self.cancel_all_stream_resolutions();
        for (_, cancel) in self.imports.imports.borrow().values() {
            cancel.cancel();
        }
        for completion in self
            .imports
            .completions
            .borrow()
            .values()
            .filter_map(Weak::upgrade)
        {
            completion.cancel.cancel();
        }
        // Hosts normally drain explicitly. Unwinding or an abandoned owner must
        // also attempt to close its cipher connections before process-level
        // SQLCipher cleanup can free their private heap. These are this UI owner's lanes;
        // the independent native recording host retains its own services.
        // An explicit attempt already spent its bounded shutdown budget, even
        // if a noncooperative lane did not close. Do not spend it again in Drop.
        if !self.lifecycle.shutdown_requested().replace(true) {
            let deadline = crate::time::Instant::now() + self.store.drain_limit();
            self.inspection.close_workers();
            self.close_cast();
            if !self
                .store
                .drain_on_shutdown_until(deadline, || self.inspection.cancel_requests())
            {
                log::error!("iptv persistence: fallback store drain did not finish at disposal");
            }
            if !self
                .inspection
                .drain_workers(deadline.saturating_duration_since(crate::time::Instant::now()))
            {
                log::error!("iptv inspection: fallback worker drain did not finish at disposal");
            }
            if !self.drain_cast(deadline) {
                log::error!("iptv cast: relay shutdown exceeded the owner's drain deadline");
            }
            self.inspection.clear();
        }
        if !self.store.files().flush(std::time::Duration::from_secs(2)) {
            log::error!("iptv persistence: fallback file flush did not finish at disposal");
        }
        self.store.retire();
    }
}

/// What a host may ask of one owner from any thread: to wait for its queued
/// file writes. It names its owner explicitly and does not keep it alive:
/// once the owner is gone there is nothing to wait for.
#[derive(Clone)]
pub struct HostHandle(std::sync::Weak<store_service::Service>);
impl HostHandle {
    /// Observe the original synthetic source behind an actual native capability.
    #[cfg(feature = "test-support")]
    pub fn original_player_source(&self, url: &str) -> Option<iptv_core::playback::PlaySource> {
        self.0
            .upgrade()
            .and_then(|service| recordings::host::original_player_source(service.directory(), url))
    }

    #[cfg(feature = "test-support")]
    pub fn hold_source_preparation(&self) -> Option<TestGate> {
        self.0
            .upgrade()
            .map(|service| service.accept().hold_source_preparation())
    }
    /// Waits, at most `limit`, until every write queued so far is on disk.
    pub fn flush_files(&self, limit: std::time::Duration) -> bool {
        // A host can stop before the application root has accepted any work,
        // or after it closed.
        self.0
            .upgrade()
            .is_none_or(|service| service.files().flush(limit))
    }

    /// Reader-lane jobs accepted by this owner; synthetic diagnostics only.
    #[cfg(feature = "test-support")]
    pub fn store_reads(&self) -> u64 {
        self.0
            .upgrade()
            .expect("store_reads needs a live owner")
            .reads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(feature = "test-support")]
    pub fn guide_check_registered(&self, source: SourceId) -> bool {
        self.0.upgrade().is_some_and(|service| {
            service
                .guide_checks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&source)
        })
    }
    #[cfg(feature = "test-support")]
    pub fn refuse_work(&self, count: usize) {
        if let Some(service) = self.0.upgrade() {
            service.hooks.refuse_work(count);
        }
    }
    #[cfg(feature = "test-support")]
    pub fn hold_reports(&self, held: bool) {
        if let Some(service) = self.0.upgrade() {
            service.hooks.hold_reports(held);
        }
    }
    #[cfg(feature = "test-support")]
    pub fn reports_waiting(&self) -> usize {
        self.0
            .upgrade()
            .map_or(0, |service| service.hooks.reports_waiting())
    }

    /// Number of wizard terminal reports processed, including stale reports.
    #[cfg(feature = "test-support")]
    pub fn wizard_reports_delivered(&self) -> usize {
        self.0.upgrade().map_or(0, |service| {
            service
                .hooks
                .wizard_reports_delivered
                .load(std::sync::atomic::Ordering::Relaxed)
        })
    }

    /// Pause a FIFO writer job; the returned gate acknowledges actual entry.
    #[cfg(feature = "test-support")]
    pub fn hold_writer(&self) -> Option<TestGate> {
        self.0
            .upgrade()
            .map(|service| service.accept().hold_writer())
    }

    /// Pause the next file write immediately before filesystem publication.
    #[cfg(feature = "test-support")]
    pub fn hold_file_write(&self) -> Option<TestGate> {
        self.0.upgrade().map(|service| service.files().hold_write())
    }

    /// Pause an import after staging and before its writer request is accepted.
    #[cfg(feature = "test-support")]
    pub fn hold_import_publication(&self) -> Option<TestGate> {
        self.0
            .upgrade()
            .map(|service| service.hooks.hold_publication())
    }

    /// Acknowledges that this owner's actual store lanes have closed.
    #[cfg(feature = "test-support")]
    pub fn observe_drain(&self) -> Option<TestGate> {
        self.0
            .upgrade()
            .map(|service| service.hooks.observe_drain())
    }

    /// Acknowledges that shutdown closed admission, before its FIFO barriers.
    #[cfg(feature = "test-support")]
    pub fn observe_admission_close(&self) -> Option<TestGate> {
        self.0
            .upgrade()
            .map(|service| service.hooks.observe_admission_close())
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

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn a_failed_explicit_drain_does_not_spend_its_budget_again_in_drop() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Runtime::new(Ports::new(directory.path().into(), |_| {
            Ok(crate::model::DatabaseKey::from_bytes([0x69; 32]))
        }));
        owner.start_services();
        owner.services().call(|_| Ok(())).await.unwrap();
        let store = owner.store.clone();
        let held = owner.host_handle().hold_writer().unwrap();
        assert!(held.wait_entered(std::time::Duration::from_secs(5)));
        store.hooks.set_drain_limit(Some(std::time::Duration::ZERO));
        assert!(!owner.drain_services());
        let repeated = store.hooks.observe_admission_close();
        drop(owner);
        let retried = repeated.wait_entered(std::time::Duration::ZERO);
        // Always release and close the actual held connection before checking
        // the regression, including when the fallback has been mutated.
        held.release();
        store.hooks.set_drain_limit(None);
        assert!(store.drain_on_shutdown_with(|| {}));
        assert!(!retried, "Drop repeated an explicit shutdown attempt");
    }

    #[test]
    fn the_worker_capability_is_the_executions_and_asking_starts_nothing() {
        use iptv_library::guide_repair::CapabilityState;
        let native = Runtime::default();
        assert!(native.has_workers());
        let probe = |owner: &Runtime| {
            owner
                .foundation_capabilities()
                .into_iter()
                .find(|capability| capability.name == "epg_probe")
                .unwrap()
                .state
        };
        assert_eq!(probe(&native), CapabilityState::Implemented);
        assert!(
            native.store.tasks.get().is_none(),
            "no runtime was made to ask"
        );

        struct Unpolled;
        impl execution::CooperativeHost for Unpolled {
            fn spawn(&self, _: execution::Task) {}
            fn delay(&self, _: std::time::Duration) -> execution::Task {
                Box::pin(std::future::pending())
            }
            fn now(&self) -> std::time::Duration {
                std::time::Duration::ZERO
            }
        }
        let mut ports = Ports::for_tests();
        ports.execution =
            std::sync::Arc::new(execution::Cooperative::new(std::sync::Arc::new(Unpolled)));
        let browser = Runtime::new(ports);
        assert!(!browser.has_workers());
        assert_eq!(probe(&browser), CapabilityState::UnsupportedHost);
    }

    #[test]
    fn storage_without_files_is_neither_locked_nor_restored() {
        // A directory no process could create: the browser host's storage
        // has no file system under it.
        let mut ports = Ports::for_tests();
        ports.directory = std::path::PathBuf::from("/proc/patina-synthetic/storage");
        ports.file_storage = false;
        let owner = Runtime::new(ports);
        assert!(owner.lock_storage().is_ok());
        assert!(owner.restore_activation()().is_ok());
    }

    #[test]
    fn independent_runtimes_index_their_synthetic_schedules_without_a_sink() {
        let first = Rc::new(Runtime::default());
        let second = Runtime::default();
        let first_catalog = first.schedule();
        let second_catalog = second.schedule();
        let key = first_catalog.channel_key(0).unwrap();
        assert_eq!(first.favorites_channel_of(&key), Some(0));
        assert_eq!(second.favorites_channel_of(&key), Some(0));
        first.toggle_favorite(0);
        assert_eq!(first.favorite_channels(), [0]);
        assert!(second.favorite_channels().is_empty());
        first.reset_favorites();
        assert!(first.favorite_channels().is_empty());
        assert_eq!(first.favorites_channel_of(&key), Some(0));
        assert!(Rc::ptr_eq(&first.schedule(), &first_catalog));
        assert!(Rc::ptr_eq(&second.schedule(), &second_catalog));
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
            owner.catalog.set_account_streams(Some(limit));
        };
        let mut task = std::pin::pin!(task);
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
        reply.send(3).unwrap();
        assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
        assert_eq!(first.catalog.account_streams(), Some(3));
        assert_eq!(second.catalog.account_streams(), None);
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
            .imports
            .borrow_mut()
            .insert(SourceId(1), (1, first_cancel.clone()));
        second
            .imports
            .imports
            .borrow_mut()
            .insert(SourceId(1), (1, second_cancel.clone()));
        drop(first);
        assert!(first_cancel.is_canceled());
        assert!(!second_cancel.is_canceled());
    }
}

mod store_service;

pub mod execution;

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

pub mod file_host;
pub mod settings;

#[cfg(feature = "native")]
mod native;
#[cfg(all(
    feature = "native",
    any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    )
))]
pub use native::app_updates;
mod update_model;
#[cfg(not(all(
    feature = "native",
    any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    )
)))]
/// Portable update status values. This host has no native update installer.
pub mod app_updates {
    pub mod github {
        pub use crate::update_model::{
            CheckError, DownloadLink, ReleaseCheck, ReleaseInfo, ReleaseStatus,
        };
    }
    pub mod state {
        pub use crate::update_model::{Frequency, Snapshot, Trigger};
    }
}

#[cfg(feature = "native")]
pub use native::network::adapter_config;
pub mod network;
mod ports;
mod publish;
pub mod store_host;
pub use ports::{Entropy, Ports, RecordingFailure};
pub mod storage;
mod threading;
pub use threading::{MaybeSend, MaybeSync};
pub mod time;

#[cfg(all(test, feature = "native"))]
impl Default for Runtime {
    fn default() -> Self {
        Self::new(Ports::for_tests())
    }
}
