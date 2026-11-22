//! The app's two store connections, each on its own lane of the owner's
//! execution port (natively a long-lived thread; see
//! [`crate::execution`]). UI callers await replies; import workers may
//! wait, but cannot open competing app connections.
//!
//! Every owner ([`Service`]) has its own pair; work reaches them through the
//! [`Services`] handle its owner gave out, never through a process-wide or
//! thread-local lookup.
//!
//! - **The writer** ([`Services::call`], [`Services::blocking`]) runs every write, all staging,
//!   and the reads that must see writes queued before them, in FIFO order.
//! - **The reader** ([`Services::read`]) runs UI reads (guide pages, descriptions,
//!   search, favorites, organization) on a second, `query_only`
//!   connection to the same file. The store uses WAL, so these read the
//!   last commit while the writer holds a long publish transaction instead
//!   of queueing behind it. A read closure gets `&Store`, so it cannot call
//!   a writing method, and SQLite refuses a write through the connection
//!   anyway. A read sees what a write committed once that write's reply
//!   arrived; it may not see a write still queued, so a read that must
//!   follow an unawaited write goes to the writer with [`Services::call`].
//!
//! The reader's connection is made by the writer ([`Store::open_reader`]):
//! the platform key is fetched, and the file migrated, only there.

use crate::time::Instant;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
#[cfg(feature = "test-support")]
use std::sync::mpsc;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::thread::ThreadId;
use std::time::Duration;

use futures_channel::oneshot;
use iptv_import::CancelToken;
use iptv_store_sqlite::{Store, StoreError};

/// What a lane's thread keeps between jobs: its connection, and when a
/// failed open may be tried again.
#[derive(Default)]
pub(crate) struct Connection {
    store: Option<Store>,
    retry_open_at: Option<crate::time::Instant>,
}

/// Numeric-only observations for one store lane; no query, path or provider data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreQueueStats {
    /// Fixed lane name: writer, reader or analysis.
    pub lane: &'static str,
    /// Requests waiting to start, excluding the request currently running.
    pub pending: usize,
    /// Time since the oldest still-pending request was admitted; zero when empty.
    pub oldest_age: Duration,
    /// Longest observed wait before a request started during this owner's lifetime.
    pub max_wait: Duration,
    /// Queued reader requests skipped because their reply futures were dropped.
    pub skipped_reads: u64,
}

#[derive(Default)]
struct QueueMetrics {
    // One timestamp per pending request, reclaimed when started or discarded.
    // Equal clock ticks share a counter; completed requests retain no entries.
    waiting: BTreeMap<Instant, usize>,
    pending: usize,
    max_wait: Duration,
    skipped_reads: u64,
}

struct QueueTicket {
    metrics: Arc<Mutex<QueueMetrics>>,
    admitted: Instant,
    waiting: bool,
}
impl QueueTicket {
    fn new(metrics: Arc<Mutex<QueueMetrics>>) -> Self {
        let admitted = Instant::now();
        {
            let mut state = metrics.lock().unwrap_or_else(|e| e.into_inner());
            *state.waiting.entry(admitted).or_default() += 1;
            state.pending += 1;
        }
        Self {
            metrics,
            admitted,
            waiting: true,
        }
    }
    fn start(mut self) {
        self.remove(true);
    }
    fn remove(&mut self, started: bool) {
        if !self.waiting {
            return;
        }
        let mut state = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = state.waiting.get_mut(&self.admitted) {
            *count -= 1;
            if *count == 0 {
                state.waiting.remove(&self.admitted);
            }
            state.pending -= 1;
        }
        if started {
            state.max_wait = state.max_wait.max(self.admitted.elapsed());
        }
        self.waiting = false;
    }
}
impl Drop for QueueTicket {
    fn drop(&mut self) {
        // Failed sends and discarded jobs must not leave phantom queue entries.
        self.remove(false);
    }
}

impl crate::Runtime {
    /// Observe store contention without opening connections or starting workers.
    /// Counts are instantaneous per lane; this does not acquire a database lock.
    pub fn store_queue_stats(&self) -> [StoreQueueStats; 3] {
        [
            self.store.writer.stats("writer"),
            self.store.reader.stats("reader"),
            self.store.analysis.stats("analysis"),
        ]
    }
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// One connection's worker: a lane of the owner's execution port (natively
/// a thread) that runs jobs in order, each with the connection.
struct Lane {
    name: &'static str,
    /// The execution lane, started on first use: a job may not queue on
    /// its own lane and wait for itself.
    jobs: OnceLock<Box<dyn crate::execution::Lane>>,
    execution: Arc<dyn crate::execution::Execution>,
    /// The lane's connection; only its jobs, one at a time, lock it.
    connection: Arc<Mutex<Connection>>,
    metrics: Arc<Mutex<QueueMetrics>>,
}
impl Lane {
    fn new(name: &'static str, execution: Arc<dyn crate::execution::Execution>) -> Self {
        Self {
            name,
            jobs: OnceLock::new(),
            execution,
            connection: Arc::default(),
            metrics: Arc::default(),
        }
    }

    /// A lane on `connection`, another lane's (a browser store has one).
    fn sharing(
        name: &'static str,
        execution: Arc<dyn crate::execution::Execution>,
        connection: Arc<Mutex<Connection>>,
    ) -> Self {
        Self {
            connection,
            ..Self::new(name, execution)
        }
    }
}

#[derive(Default)]
struct Epochs {
    next: AtomicU64,
    active: AtomicU64,
    /// Closing admission must wake queries before accepted FIFO writes drain.
    closed: Mutex<CancelToken>,
}
impl Epochs {
    fn begin(&self) -> u64 {
        let mut closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        closed.cancel();
        *closed = CancelToken::new();
        let epoch = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        self.active.store(epoch, Ordering::SeqCst);
        epoch
    }
    fn permits(&self, epoch: u64) -> bool {
        epoch != 0 && self.active.load(Ordering::SeqCst) == epoch
    }
    fn close(&self, epoch: u64) -> bool {
        let closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        let retired = self
            .active
            .compare_exchange(epoch, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if retired {
            closed.cancel();
        }
        retired
    }
    fn close_admission(&self, epoch: u64) {
        let closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        if self.permits(epoch) {
            closed.cancel();
        }
    }
}

/// The services of one application owner: its two store lanes, file
/// writer, task executor, refresh pool, settings and platform ports.
/// Nothing here is process-wide: a second owner has its own.
pub(crate) struct Service {
    ports: crate::Ports,
    /// The lanes share the writer's connection (`Execution::shares_connection`):
    /// reads open the store itself, not a reader connection.
    shared: bool,
    writer: Lane,
    reader: Lane,
    analysis: Lane,
    epochs: Epochs,
    /// The owner's task executor from its execution port, made on first use.
    pub(crate) tasks: OnceLock<Box<dyn crate::execution::Tasks>>,
    files: super::file_writer::Files,
    pool: OnceLock<super::workers::Pool>,
    settings: super::settings::Shared,
    /// The thread that started the services; it may not block on the store.
    ui_thread: OnceLock<ThreadId>,
    /// This service, for work it queues on itself (a host's persistence).
    me: OnceLock<std::sync::Weak<Service>>,
    /// Imports and guide work where there are no workers (a cooperative
    /// host) run one at a time: a blocking store request made by one
    /// would otherwise step another's publish to its end inline.
    #[cfg(not(feature = "native"))]
    cooperative_imports: tokio::sync::Mutex<()>,
    #[cfg(feature = "test-support")]
    pub(crate) guide_checks: super::guide_checks::Mirror,
    #[cfg(feature = "test-support")]
    pub(crate) hooks: super::workers::Hooks,
    #[cfg(feature = "test-support")]
    pub(crate) reads: AtomicU64,
}
impl Service {
    pub(crate) fn new(ports: crate::Ports) -> Self {
        let execution = ports.execution.clone();
        let shared = execution.shares_connection();
        let writer = Lane::new("iptv-store", execution.clone());
        let lane = |name| {
            if shared {
                Lane::sharing(name, execution.clone(), writer.connection.clone())
            } else {
                Lane::new(name, execution.clone())
            }
        };
        let (reader, analysis) = (lane("iptv-store-read"), lane("iptv-store-analysis"));
        Self {
            shared,
            writer,
            reader,
            analysis,
            files: super::file_writer::Files::new(execution),
            ports,
            epochs: Epochs::default(),
            tasks: OnceLock::new(),
            pool: OnceLock::new(),
            settings: Default::default(),
            ui_thread: OnceLock::new(),
            me: OnceLock::new(),
            #[cfg(not(feature = "native"))]
            cooperative_imports: tokio::sync::Mutex::new(()),
            #[cfg(feature = "test-support")]
            guide_checks: Default::default(),
            #[cfg(feature = "test-support")]
            hooks: Default::default(),
            #[cfg(feature = "test-support")]
            reads: AtomicU64::new(0),
        }
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.ports.directory
    }

    /// The calling (UI) thread starts accepting store work for this owner.
    pub(crate) fn begin(&self) {
        let _ = self.ui_thread.set(std::thread::current().id());
        self.epochs.begin();
    }

    /// A handle for work accepted now. It stays bound to this owner and to
    /// the application epoch of this moment, on whichever thread it runs.
    pub(crate) fn accept(self: &Arc<Self>) -> Services {
        let _ = self.me.set(Arc::downgrade(self));
        let closed = self.epochs.closed.lock().unwrap_or_else(|e| e.into_inner());
        Services {
            service: self.clone(),
            epoch: self.epochs.active.load(Ordering::SeqCst),
            closed: closed.clone(),
        }
    }

    pub(crate) fn retire(&self) {
        self.epochs.close(self.epochs.active.load(Ordering::SeqCst));
        #[cfg(feature = "test-support")]
        self.hooks.release();
        self.files.close();
        if let Some(pool) = self.pool.get() {
            pool.close();
        }
    }

    pub(crate) fn files(&self) -> &super::file_writer::Files {
        &self.files
    }

    fn tasks(&self) -> &dyn crate::execution::Tasks {
        &**self.tasks.get_or_init(|| self.ports.execution.tasks())
    }

    fn open_store(&self) -> Result<Store, StoreError> {
        if let Some(host) = &self.ports.store_host {
            return host.open();
        }
        let directory = self.directory();
        std::fs::create_dir_all(directory)
            .map_err(|_| StoreError::Unavailable("cannot create data directory"))?;
        super::credential_store::open_database(&self.ports, &directory.join("iptv.sqlite"))
    }

    fn assert_off_ui_thread(&self, what: &str) {
        // A cooperative host has one thread: its blocking requests run the
        // lane's queue inline (`execution::Cooperative`).
        if !self.ports.execution.has_workers() {
            return;
        }
        assert!(
            self.ui_thread.get() != Some(&std::thread::current().id()),
            "{what} on the UI thread"
        );
    }

    /// Root teardown only, after the favorites writer has submitted its last
    /// work. Accepted requests already belong to their workers, independent
    /// of UI futures.
    #[cfg(all(test, feature = "native"))]
    pub(crate) fn drain_on_shutdown(self: &Arc<Self>) -> bool {
        self.drain_on_shutdown_with(|| {})
    }

    pub(crate) fn drain_on_shutdown_with(self: &Arc<Self>, after_writer: impl FnOnce()) -> bool {
        self.drain_on_shutdown_with_limit(self.drain_limit(), after_writer)
    }

    pub(crate) fn drain_limit(&self) -> std::time::Duration {
        let limit = std::time::Duration::from_secs(10);
        #[cfg(feature = "test-support")]
        let limit = self.hooks.drain_limit().unwrap_or(limit);
        limit
    }

    fn drain_on_shutdown_with_limit(
        self: &Arc<Self>,
        limit: std::time::Duration,
        after_writer: impl FnOnce(),
    ) -> bool {
        let epoch = self.epochs.active.load(Ordering::SeqCst);
        // No new work may enter now. Queries observe owner closure immediately,
        // while queued writer jobs retain their epoch until the FIFO barrier.
        self.epochs.close_admission(epoch);
        #[cfg(feature = "test-support")]
        self.hooks.admission_closed();
        // Natively a reader opens only through the writer; where lanes share
        // the writer's connection, a read alone may have opened it.
        if [&self.writer, &self.reader, &self.analysis]
            .iter()
            .all(|lane| lane.jobs.get().is_none())
        {
            self.epochs.close(epoch);
            after_writer();
            #[cfg(feature = "test-support")]
            self.hooks.drained();
            return true;
        }
        // A publish still in slices is abandoned, not finished: the owner is
        // closing (natively a publish runs as one job and is never pending).
        if let Some(lane) = self.writer.jobs.get() {
            lane.cancel_sliced();
        }
        // The writer first: once it closed the epoch, a late read cannot open
        // the reader's connection again.
        let owner = self.clone();
        let writer = self.writer.barrier(limit, move |connection| {
            // Release SQLite handles before a new app instance opens or restores
            // its library in the same process.
            owner.epochs.close(epoch);
            // Even if a timed-out teardown was followed by a newer start, this
            // FIFO barrier still retires the connection held by preceding work.
            *connection = Connection::default();
        });
        if self.writer.jobs.get().is_none() {
            // Shared lanes that only read: no writer job closes the epoch.
            self.epochs.close(epoch);
        }
        // Queued writes have acknowledged before transient read/probe cancellation.
        after_writer();
        let reader = self.reader.barrier(limit, |connection| {
            *connection = Connection::default();
        });
        let analysis = self.analysis.barrier(limit, |connection| {
            *connection = Connection::default();
        });
        let drained = writer && reader && analysis;
        #[cfg(feature = "test-support")]
        if drained {
            self.hooks.drained();
        }
        drained
    }
}

/// One owner's services as the work that was accepted sees them. `Send`:
/// workers carry it instead of looking an owner up. A handle taken before
/// the owner closed (or restarted) cannot reach the store afterwards.
#[derive(Clone)]
pub(crate) struct Services {
    service: Arc<Service>,
    epoch: u64,
    closed: CancelToken,
}

fn stale() -> StoreError {
    StoreError::Unavailable("request belongs to a closed application")
}

impl Services {
    pub(crate) fn service(&self) -> &Arc<Service> {
        &self.service
    }

    #[cfg(feature = "native")]
    pub(crate) fn service_retire(&self) -> bool {
        let drained = self.service.drain_on_shutdown_with(|| {});
        self.service.retire();
        drained
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    fn affinity(&self) -> Result<u64, StoreError> {
        if !self.closed.is_canceled() && self.service.epochs.permits(self.epoch) {
            Ok(self.epoch)
        } else {
            Err(stale())
        }
    }

    /// The application that accepted this work is still the open one.
    pub(crate) fn is_current(&self) -> bool {
        self.affinity().is_ok()
    }

    pub(crate) fn closed(&self) -> impl Future<Output = ()> + '_ {
        self.closed.canceled()
    }

    pub(crate) fn directory(&self) -> &Path {
        self.service.directory()
    }

    pub(crate) fn ports(&self) -> &crate::Ports {
        &self.service.ports
    }

    pub(crate) fn mark(&self, phase: &'static str, detail: impl std::fmt::Display) {
        (self.service.ports.startup_mark)(phase, detail.to_string());
    }

    /// The effective settings (defaults until they are loaded).
    pub(crate) fn settings(&self) -> Arc<iptv_settings::Settings> {
        super::settings::get(&self.service.settings)
    }

    pub(crate) fn shared_settings(&self) -> &super::settings::Shared {
        &self.service.settings
    }

    pub(crate) fn files(&self) -> &super::file_writer::Files {
        &self.service.files
    }

    /// A network client with this owner's settings of now.
    pub(crate) fn network_client(&self) -> Result<crate::network::Client, iptv_import::FetchError> {
        self.service
            .ports
            .network
            .client(&super::settings::network_config(&self.settings()))
    }

    /// Runs `future` on the owner's task executor; await the handle for
    /// its output. Dropping the handle detaches the task (cancel it with its
    /// `CancelToken`).
    pub(crate) fn spawn<F>(&self, future: F) -> crate::execution::Join<F::Output>
    where
        F: Future + crate::MaybeSend + 'static,
        F::Output: crate::MaybeSend + 'static,
    {
        crate::execution::spawn(self.service.tasks(), future)
    }

    /// Runs CPU-bound `work` on the task executor's blocking side (building
    /// a large schedule); await the handle for its output.
    pub(crate) fn spawn_blocking<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> crate::execution::Join<R> {
        crate::execution::spawn_blocking(self.service.tasks(), work)
    }

    /// The host has blocking workers ([`crate::execution::Execution::has_workers`]).
    pub(crate) fn has_workers(&self) -> bool {
        self.service.ports.execution.has_workers()
    }

    /// Drive a non-Send import on its bounded blocking worker, sharing the
    /// IO executor.
    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        crate::execution::block_on(self.service.tasks(), future)
    }

    /// `future`, unless `deadline` passes first on the owner's timer.
    pub(crate) async fn timeout_at<F: Future>(
        &self,
        deadline: Instant,
        future: F,
    ) -> Result<F::Output, crate::execution::Elapsed> {
        let limit = deadline.saturating_duration_since(Instant::now());
        crate::execution::timeout(self.service.tasks(), limit, future).await
    }

    /// Queues refresh `work` on the owner's bounded pool; false when it is
    /// full. Work for the visible source precedes background work.
    pub(crate) fn submit(&self, visible: bool, work: impl FnOnce() + Send + 'static) -> bool {
        #[cfg(feature = "test-support")]
        if self.service.hooks.refuse() {
            return false;
        }
        let accepted = self.clone();
        self.service
            .pool
            .get_or_init(|| super::workers::Pool::standard(&*self.service.ports.execution))
            .submit(
                visible,
                Box::new(move || {
                    // Retired queued refreshes must not spend a new app's network
                    // budget. Running work remains guarded again at every store
                    // operation.
                    if accepted.is_current() {
                        work();
                    }
                }),
            )
    }

    /// Imports and guide downloads can run here: on the refresh pool's
    /// workers, or, on a host without workers whose store is its own
    /// (a browser), as tasks one at a time whose publishes run in slices
    /// (`publish.rs`).
    pub(crate) fn can_import(&self) -> bool {
        self.has_workers()
            || (cfg!(not(feature = "native")) && self.service.ports.store_host.is_some())
    }

    /// Queues import or guide `work` ([`Self::can_import`]): natively on
    /// the bounded pool, driven there to its end (false when the pool is
    /// full); on a host without workers as a task, after the one running.
    pub(crate) fn submit_import<F>(
        &self,
        visible: bool,
        work: impl FnOnce(Services) -> F + Send + 'static,
    ) -> bool
    where
        F: Future<Output = ()> + 'static,
    {
        if self.has_workers() {
            let worker = self.clone();
            return self.submit(visible, move || {
                let services = worker.clone();
                worker.block_on(work(services))
            });
        }
        #[cfg(not(feature = "native"))]
        if self.can_import() {
            let accepted = self.clone();
            drop(self.spawn(async move {
                let service = accepted.service.clone();
                let _turn = service.cooperative_imports.lock().await;
                if accepted.is_current() {
                    work(accepted).await;
                }
            }));
            return true;
        }
        let _ = (visible, work);
        false
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn before_report(&self) {
        self.service.hooks.before_report();
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn before_publication(&self) {
        self.service.hooks.before_publication();
    }

    /// Queues `operation` on the writer and returns its reply. Transfers
    /// ownership immediately in FIFO order, even if the caller drops its reply
    /// future. Bulk producers are bounded by the refresh pool; no SQLite work
    /// or enqueue wait runs on the UI thread. Use it for every write, and for
    /// a read that must see a write the caller has queued but not awaited.
    pub(crate) fn call<R, F>(
        &self,
        operation: F,
    ) -> impl Future<Output = Result<R, StoreError>> + use<R, F>
    where
        R: Send + 'static,
        F: FnOnce(&mut Store) -> Result<R, StoreError> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let queued = self.affinity().and_then(|epoch| {
            let owner = self.service.clone();
            self.service.writer.queue(Box::new(move |connection| {
                let _ = sender.send(owner.write(epoch, connection, operation));
            }))
        });
        async move {
            queued?;
            receiver
                .await
                .map_err(|_| StoreError::Unavailable("store reply lost"))?
        }
    }

    /// Queues `operation` on the reader and returns its reply: a read of the
    /// last commit that does not wait for the writer (see the module docs).
    /// The first read opens the reader's connection through the writer. Dropping
    /// its reply skips a still-queued read; an already-started read may finish.
    pub(crate) fn read<R, F>(
        &self,
        operation: F,
    ) -> impl Future<Output = Result<R, StoreError>> + use<R, F>
    where
        R: Send + 'static,
        F: FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let queued = self.affinity().and_then(|epoch| {
            let owner = self.service.clone();
            self.service.reader.queue(Box::new(move |connection| {
                // Superseded/dropped UI reads have no durable effect. Skip them
                // before opening SQLite or scanning data; writer jobs always run.
                if sender.is_canceled() {
                    owner.reader.skipped_read();
                    return;
                }
                let _ = sender.send(owner.read(epoch, connection, operation));
            }))?;
            #[cfg(feature = "test-support")]
            self.service.reads.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        async move {
            queued?;
            receiver
                .await
                .map_err(|_| StoreError::Unavailable("store reply lost"))?
        }
    }

    /// Bounded inspection uses its own lazy query-only lane so a scan cannot
    /// queue guide-page reads behind itself. Dropping its reply skips queued
    /// work; already-running inspections retain their own cancellation contract.
    pub(crate) fn analyze<R, F>(
        &self,
        operation: F,
    ) -> impl Future<Output = Result<R, StoreError>> + use<R, F>
    where
        R: Send + 'static,
        F: FnOnce(&mut Store) -> Result<R, StoreError> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let queued = self.affinity().and_then(|epoch| {
            let owner = self.service.clone();
            self.service.analysis.queue(Box::new(move |connection| {
                if sender.is_canceled() {
                    owner.analysis.skipped_read();
                    return;
                }
                let _ = sender.send(with_epoch(
                    &owner.epochs,
                    epoch,
                    connection,
                    || owner.open_lane(epoch),
                    operation,
                ));
            }))
        });
        async move {
            queued?;
            receiver
                .await
                .map_err(|_| StoreError::Unavailable("store reply lost"))?
        }
    }

    /// For import/background workers only. Calling this from the UI is a bug.
    pub(crate) fn blocking<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Store) -> Result<R, StoreError> + Send + 'static,
    ) -> Result<R, StoreError> {
        self.service.assert_off_ui_thread("blocking store request");
        let epoch = self.affinity()?;
        let owner = self.service.clone();
        self.service
            .writer
            .wait(move |connection| owner.write(epoch, connection, operation))?
    }

    /// For import/background workers only: a read of the last commit that runs
    /// on the reader lane and does not wait for the writer.
    pub(crate) fn blocking_read<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
    ) -> Result<R, StoreError> {
        self.service.assert_off_ui_thread("blocking store read");
        let epoch = self.affinity()?;
        let owner = self.service.clone();
        #[cfg(feature = "test-support")]
        let reads = self.service.clone();
        self.service.reader.wait_counted(
            move |connection| owner.read(epoch, connection, operation),
            move || {
                #[cfg(feature = "test-support")]
                reads.reads.fetch_add(1, Ordering::Relaxed);
            },
        )?
    }

    /// Occupies the writer for `duration` after the work queued before it.
    #[cfg(feature = "test-support")]
    pub(crate) fn occupy_writer(&self, duration: std::time::Duration) {
        let _ = self
            .service
            .writer
            .queue(Box::new(move |_| std::thread::sleep(duration)));
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn hold_source_preparation(&self) -> crate::TestGate {
        self.service.hooks.hold_source_preparation()
    }
    #[cfg(all(feature = "test-support", feature = "native"))]
    pub(crate) fn take_source_preparation(&self) -> Option<Arc<crate::testing::GateState>> {
        self.service.hooks.take_source_preparation()
    }

    /// Explicit synthetic accepted work, independent of read-reply lifetime.
    #[cfg(feature = "test-support")]
    pub(crate) fn hold_analysis(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, observed) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        self.service
            .analysis
            .queue(Box::new(move |_| {
                let _ = entered.send(());
                let _ = resume.recv_timeout(Duration::from_secs(10));
            }))
            .expect("queue synthetic analysis blocker");
        (observed, release)
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn hold_writer(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        let pause = gate.0.clone();
        self.service
            .writer
            .queue(Box::new(move |_| pause.pause()))
            .unwrap();
        gate
    }
}

impl Service {
    /// A writer job: on the writer's thread, with its connection.
    fn write<R>(
        &self,
        epoch: u64,
        connection: &mut Connection,
        operation: impl FnOnce(&mut Store) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        let result = with_epoch(
            &self.epochs,
            epoch,
            connection,
            || self.open_store(),
            operation,
        );
        if let Some(store) = &connection.store {
            self.persist(store);
        }
        result
    }

    /// A host that keeps the store persists what the last writer job
    /// committed (natively the file is the persistence: nothing to do).
    pub(crate) fn persist(&self, store: &Store) {
        let Some(host) = &self.ports.store_host else {
            return;
        };
        let me = self.me.get().cloned().unwrap_or_default();
        host.committed(
            store,
            Arc::new(move || {
                if let Some(service) = me.upgrade() {
                    service.queue_persist();
                }
            }),
        );
    }

    /// Persists again on the writer, after what is queued there.
    fn queue_persist(self: &Arc<Self>) {
        let owner = self.clone();
        let _ = self.writer.queue(Box::new(move |connection| {
            if let Some(store) = &connection.store {
                owner.persist(store);
            }
        }));
    }

    /// Queues a sliced job on the writer lane (a publish, `publish.rs`).
    pub(crate) fn push_sliced(
        &self,
        job: Box<dyn crate::execution::Sliced>,
    ) -> Result<(), StoreError> {
        if self.writer.lane()?.push_sliced(job) {
            Ok(())
        } else {
            Err(StoreError::Unavailable("store worker stopped"))
        }
    }

    /// The writer's connection, which a sliced job takes for each step.
    pub(crate) fn writer_connection(&self) -> Arc<Mutex<Connection>> {
        self.writer.connection.clone()
    }

    /// Runs `operation` on `connection`'s store, opening it if needed, for
    /// work accepted in `epoch` (a step of a sliced job).
    pub(crate) fn with_writer_store<R>(
        &self,
        epoch: u64,
        connection: &mut Connection,
        operation: impl FnOnce(&mut Store) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        with_epoch(
            &self.epochs,
            epoch,
            connection,
            || self.open_store(),
            operation,
        )
    }

    /// A reader job: on the reader's thread, with its connection.
    fn read<R>(
        self: &Arc<Self>,
        epoch: u64,
        connection: &mut Connection,
        operation: impl FnOnce(&Store) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        with_epoch(
            &self.epochs,
            epoch,
            connection,
            || self.open_lane(epoch),
            |store| operation(store),
        )
    }

    /// A reading lane's connection: the writer's own store where the lanes
    /// share one, else a reader connection ([`Service::open_reader`]).
    fn open_lane(self: &Arc<Self>, epoch: u64) -> Result<Store, StoreError> {
        if self.shared {
            self.open_store()
        } else {
            self.open_reader(epoch)
        }
    }

    /// The reader's connection, made by the writer from its own open store
    /// (the key and the migration stay there). Runs on the reader's thread,
    /// which may wait for the writer; the writer never waits for the reader.
    fn open_reader(self: &Arc<Self>, epoch: u64) -> Result<Store, StoreError> {
        let owner = self.clone();
        self.writer
            .wait(move |connection| owner.write(epoch, connection, |store| store.open_reader()))?
    }
}

fn with_epoch<R>(
    epochs: &Epochs,
    epoch: u64,
    connection: &mut Connection,
    open: impl FnOnce() -> Result<Store, StoreError>,
    operation: impl FnOnce(&mut Store) -> Result<R, StoreError>,
) -> Result<R, StoreError> {
    if !epochs.permits(epoch) {
        return Err(stale());
    }
    with_store(connection, open, |store| {
        // Opening the OS broker can wait; a newer app may have started while
        // an old open was waiting, so recheck before touching catalog rows.
        if !epochs.permits(epoch) {
            return Err(stale());
        }
        operation(store)
    })
}

impl Lane {
    fn lane(&self) -> Result<&dyn crate::execution::Lane, StoreError> {
        let lane = self
            .jobs
            .get_or_init(|| self.execution.lane(self.name).expect("start store service"));
        if lane.is_current() {
            return Err(StoreError::Unavailable("recursive store request"));
        }
        Ok(&**lane)
    }

    /// Wraps `job` with its queue ticket and the lane's connection.
    fn job(&self, job: Job) -> crate::execution::Job {
        let ticket = QueueTicket::new(self.metrics.clone());
        let connection = self.connection.clone();
        Box::new(move || {
            ticket.start();
            let mut connection = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            run_job(&mut connection, job);
        })
    }

    fn queue(&self, job: Job) -> Result<(), StoreError> {
        let lane = self.lane()?;
        if lane.push(self.job(job)) {
            Ok(())
        } else {
            Err(StoreError::Unavailable("store worker stopped"))
        }
    }

    /// Runs `operation` after everything queued before it and waits for its
    /// reply (workers only; a cooperative lane runs its queue inline).
    fn wait<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> R + Send + 'static,
    ) -> Result<R, StoreError> {
        self.wait_counted(operation, || {})
    }

    /// [`Lane::wait`], calling `queued` once the request is queued.
    fn wait_counted<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> R + Send + 'static,
        queued: impl FnOnce(),
    ) -> Result<R, StoreError> {
        let lane = self.lane()?;
        let reply = Arc::new(Mutex::new(None));
        let slot = reply.clone();
        let job = self.job(Box::new(move |connection| {
            let result = operation(connection);
            *slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
        }));
        queued();
        if !lane.run(job, None) {
            return Err(StoreError::Unavailable("store reply lost"));
        }
        reply
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or(StoreError::Unavailable("store reply lost"))
    }

    fn skipped_read(&self) {
        let mut metrics = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
        metrics.skipped_reads = metrics.skipped_reads.saturating_add(1);
    }

    fn stats(&self, lane: &'static str) -> StoreQueueStats {
        let state = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
        StoreQueueStats {
            lane,
            pending: state.pending,
            oldest_age: state
                .waiting
                .first_key_value()
                .map_or(Duration::ZERO, |(at, _)| at.elapsed()),
            max_wait: state.max_wait,
            skipped_reads: state.skipped_reads,
        }
    }

    /// Runs `job` after everything queued before it and waits, at most
    /// `limit`, until it has run. False when it could not be queued or did
    /// not finish in time.
    fn barrier(
        &self,
        limit: std::time::Duration,
        job: impl FnOnce(&mut Connection) + Send + 'static,
    ) -> bool {
        let Some(lane) = self.jobs.get() else {
            return true;
        };
        if lane.is_current() {
            return false;
        }
        lane.run(self.job(Box::new(job)), Some(limit))
    }
}

fn run_job(connection: &mut Connection, job: Job) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(connection))).is_err() {
        // Discard the connection so an interrupted operation cannot leak its
        // transaction or statement state into the next request.
        connection.store = None;
        log::error!("iptv store: request panicked; connection discarded");
    }
}

fn with_store<R>(
    connection: &mut Connection,
    open: impl FnOnce() -> Result<Store, StoreError>,
    operation: impl FnOnce(&mut Store) -> Result<R, StoreError>,
) -> Result<R, StoreError> {
    if connection.store.is_none() {
        if connection
            .retry_open_at
            .is_some_and(|at| crate::time::Instant::now() < at)
        {
            return Err(StoreError::Unavailable(
                "credential storage is temporarily unavailable",
            ));
        }
        match open() {
            Ok(opened) => {
                connection.store = Some(opened);
                connection.retry_open_at = None;
            }
            Err(error) => {
                connection.retry_open_at =
                    Some(crate::time::Instant::now() + std::time::Duration::from_secs(1));
                return Err(error);
            }
        }
    }
    operation(connection.store.as_mut().expect("opened store"))
}

#[cfg(all(test, feature = "native"))]
impl Default for Service {
    fn default() -> Self {
        Self::new(crate::Ports::for_tests())
    }
}

#[cfg(all(test, feature = "native"))]
impl Service {
    /// A started owner on `ports` (tests of other modules).
    pub(crate) fn started_for_tests(ports: crate::Ports) -> Arc<Self> {
        let service = Arc::new(Self::new(ports));
        service.epochs.begin();
        service
    }
}

#[cfg(all(test, feature = "native"))]
impl Services {
    /// An owner of its own in `directory`, started on a thread that never
    /// blocks (tests call the blocking requests from their own thread).
    pub(crate) fn for_tests(directory: &Path) -> Self {
        let mut ports = crate::Ports::for_tests();
        ports.directory = directory.to_owned();
        let service = Arc::new(Service::new(ports));
        service.epochs.begin();
        service.accept()
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::{Connection, Service};
    use std::sync::Arc;

    fn hold_lane(lane: &super::Lane) -> std::sync::mpsc::Sender<()> {
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        lane.queue(Box::new(move |_| {
            entered.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        }))
        .unwrap();
        observed
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        release
    }

    #[tokio::test]
    async fn dropped_reads_leave_observable_contention_but_do_not_open_or_query_sqlite() {
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let owner = service(directory.path(), 0x71);
        owner.epochs.begin();
        let services = owner.accept();
        let release = hold_lane(&owner.reader);
        let release_analysis = hold_lane(&owner.analysis);
        for _ in 0..64 {
            drop(
                services.read(|_| -> Result<(), iptv_store_sqlite::StoreError> {
                    panic!("a superseded read must not execute");
                }),
            );
            drop(
                services.analyze(|_| -> Result<(), iptv_store_sqlite::StoreError> {
                    panic!("a superseded analysis must not execute");
                }),
            );
        }
        assert_eq!(owner.analysis.stats("analysis").pending, 64);
        let queued = owner.reader.stats("reader");
        assert_eq!(queued.pending, 64);
        assert!(queued.oldest_age > Duration::ZERO);
        assert_eq!(owner.writer.stats("writer").pending, 0);
        release.send(()).unwrap();
        release_analysis.send(()).unwrap();
        assert!(owner.analysis.barrier(Duration::from_secs(5), |_| {}));
        assert_eq!(owner.analysis.stats("analysis").skipped_reads, 64);
        assert_eq!(owner.analysis.stats("analysis").pending, 0);
        assert!(owner.reader.barrier(Duration::from_secs(5), |_| {}));
        let drained = owner.reader.stats("reader");
        assert_eq!(drained.pending, 0);
        assert_eq!(drained.oldest_age, Duration::ZERO);
        assert_eq!(drained.skipped_reads, 64);
        assert!(drained.max_wait >= queued.oldest_age);
        assert!(owner.reader.metrics.lock().unwrap().waiting.is_empty());
        assert!(!directory.path().join("iptv.sqlite").exists());
        // A still-wanted read uses the same lane normally after the burst.
        assert_eq!(
            services
                .read(|store| store.combined_last_channel())
                .await
                .unwrap(),
            None
        );
        assert!(directory.path().join("iptv.sqlite").is_file());
        assert!(owner.drain_on_shutdown());
    }

    #[tokio::test]
    async fn dropped_write_replies_preserve_durable_fifo_order() {
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let owner = service(directory.path(), 0x72);
        owner.epochs.begin();
        let services = owner.accept();
        let release = hold_lane(&owner.writer);
        drop(services.call(|store| store.set_combined_last_channel("synthetic-first")));
        drop(services.call(|store| {
            assert_eq!(
                store.combined_last_channel()?.as_deref(),
                Some("synthetic-first")
            );
            store.set_combined_last_channel("synthetic-second")
        }));
        let latest = services.call(|store| store.combined_last_channel());
        assert_eq!(owner.writer.stats("writer").pending, 3);
        release.send(()).unwrap();
        assert_eq!(latest.await.unwrap().as_deref(), Some("synthetic-second"));
        assert!(owner.writer.barrier(Duration::from_secs(5), |_| {}));
        let drained = owner.writer.stats("writer");
        assert_eq!(drained.pending, 0);
        assert_eq!(drained.skipped_reads, 0);
        assert!(owner.drain_on_shutdown());
    }

    /// A lane whose worker has stopped.
    struct Stopped;
    impl crate::execution::Lane for Stopped {
        fn push(&self, _: crate::execution::Job) -> bool {
            false
        }
        fn run(&self, _: crate::execution::Job, _: Option<std::time::Duration>) -> bool {
            false
        }
        fn is_current(&self) -> bool {
            false
        }
    }

    /// A host loop that is never polled: blocking requests run inline.
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

    #[test]
    fn cooperative_lanes_read_on_the_writers_store() {
        let directory = tempfile::tempdir().unwrap();
        let ports = crate::Ports::with_adapters(
            directory.path().to_owned(),
            |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x73; 32])),
            Arc::new(crate::native::network::NativeNetwork),
            Arc::new(crate::execution::Cooperative::new(Arc::new(Unpolled))),
        );
        let owner = Arc::new(Service::new(ports));
        assert!(Arc::ptr_eq(
            &owner.reader.connection,
            &owner.writer.connection
        ));
        assert!(Arc::ptr_eq(
            &owner.analysis.connection,
            &owner.writer.connection
        ));
        owner.epochs.begin();
        let services = owner.accept();
        services
            .blocking(|store| store.set_combined_last_channel("synthetic-shared"))
            .unwrap();
        assert_eq!(
            services
                .blocking_read(|store| store.combined_last_channel())
                .unwrap()
                .as_deref(),
            Some("synthetic-shared")
        );
        assert!(owner.drain_on_shutdown());
    }

    #[test]
    fn closing_shared_lanes_closes_a_connection_only_a_read_opened() {
        let directory = tempfile::tempdir().unwrap();
        let ports = crate::Ports::with_adapters(
            directory.path().to_owned(),
            |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x74; 32])),
            Arc::new(crate::native::network::NativeNetwork),
            Arc::new(crate::execution::Cooperative::new(Arc::new(Unpolled))),
        );
        let owner = Arc::new(Service::new(ports));
        owner.epochs.begin();
        owner
            .accept()
            .blocking_read(|store| store.combined_last_channel())
            .unwrap();
        assert!(owner.writer.jobs.get().is_none());
        assert!(owner.drain_on_shutdown());
        assert!(owner.writer.connection.lock().unwrap().store.is_none());
        assert!(
            !owner.accept().is_current(),
            "the closed owner accepts nothing"
        );
    }

    #[test]
    fn a_thread_lane_whose_worker_stopped_refuses_work() {
        let (sender, receiver) = std::sync::mpsc::channel::<crate::execution::Job>();
        drop(receiver);
        let lane = crate::native::execution::ThreadLane::for_tests(sender);
        assert!(!crate::execution::Lane::push(&lane, Box::new(|| {})));
        assert!(!crate::execution::Lane::run(&lane, Box::new(|| {}), None));
    }

    #[test]
    fn failed_lane_send_reclaims_its_queue_ticket() {
        let lane = super::Lane::new(
            "synthetic-disconnected",
            Arc::new(crate::native::execution::NativeExecution),
        );
        assert!(lane.jobs.set(Box::new(Stopped)).is_ok());
        assert!(
            lane.queue(Box::new(|_| panic!("disconnected work")))
                .is_err()
        );
        let snapshot = lane.stats("writer");
        assert_eq!(snapshot.pending, 0);
        assert_eq!(snapshot.oldest_age, std::time::Duration::ZERO);
        assert!(lane.metrics.lock().unwrap().waiting.is_empty());
    }

    #[test]
    fn observing_idle_lanes_does_not_start_workers() {
        let owner = super::super::Runtime::default();
        let snapshots = owner.store_queue_stats();
        assert_eq!(snapshots.map(|s| s.lane), ["writer", "reader", "analysis"]);
        for snapshot in snapshots {
            assert_eq!(snapshot.pending, 0);
            assert_eq!(snapshot.oldest_age, std::time::Duration::ZERO);
        }
        assert!(owner.store.writer.jobs.get().is_none());
        assert!(owner.store.reader.jobs.get().is_none());
        assert!(owner.store.analysis.jobs.get().is_none());
    }

    #[test]
    #[cfg(feature = "test-support")]
    fn a_timed_out_drain_does_not_acknowledge_connection_closure() {
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let owner = service(directory.path(), 0x46);
        owner.epochs.begin();
        owner.accept().blocking(|_| Ok(())).unwrap();
        let held = owner.accept().hold_writer();
        assert!(held.wait_entered(Duration::from_secs(5)));
        let observed = owner.hooks.observe_drain();
        assert!(!owner.drain_on_shutdown_with_limit(Duration::ZERO, || {}));
        assert!(!observed.wait_entered(Duration::ZERO));
        held.release();
        assert!(owner.drain_on_shutdown());
        assert!(observed.wait_entered(Duration::ZERO));
        owner.retire();
    }

    fn service(directory: &std::path::Path, key: u8) -> Arc<Service> {
        Arc::new(Service::new(crate::Ports::new(
            directory.to_owned(),
            move |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([key; 32])),
        )))
    }

    #[test]
    fn a_worker_keeps_its_accepting_settings_and_directory() {
        use iptv_settings::{Settings, Target, Value, keys};
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("alpha");
        let first = service(&first_path, 1);
        let second = service(&directory.path().join("beta"), 2);
        let values = |agent: &str| {
            let mut settings = Settings::new();
            settings
                .set("general.user_agent", Value::Text(agent.into()))
                .unwrap();
            Some(Arc::new(settings))
        };
        *first.settings.write().unwrap() = values("SyntheticAlpha/1");
        *second.settings.write().unwrap() = values("SyntheticBeta/1");
        let accepted = first.accept();
        let worker = move || {
            (
                accepted.settings().text(keys::USER_AGENT, &Target::Global),
                accepted.directory().to_owned(),
            )
        };
        first.retire();
        // Another owner being alive, started or even constructed later does
        // not change what the accepted work reads.
        second.begin();
        let third = service(&directory.path().join("gamma"), 3);
        let result = std::thread::spawn(worker).join().unwrap();
        assert_eq!(result, ("SyntheticAlpha/1".into(), first_path));
        drop(third);
    }

    #[test]
    fn constructing_or_starting_another_owner_never_redirects_an_open_store() {
        let directory = tempfile::tempdir().unwrap();
        let first = service(&directory.path().join("alpha"), 0x41);
        first.epochs.begin();
        let accepted = first.accept();
        accepted
            .blocking(|store| store.set_combined_last_channel("alpha-before"))
            .unwrap();
        // A second owner is constructed, started and used while the first is
        // open; then a third one is only constructed.
        let second = service(&directory.path().join("beta"), 0x42);
        second.epochs.begin();
        second
            .accept()
            .blocking(|store| store.set_combined_last_channel("beta"))
            .unwrap();
        let third = service(&directory.path().join("gamma"), 0x43);
        // Both a handle taken before and one taken after still reach alpha.
        for handle in [accepted, first.accept()] {
            assert_eq!(
                handle
                    .blocking_read(|store| store.combined_last_channel())
                    .unwrap()
                    .as_deref(),
                Some("alpha-before")
            );
        }
        assert_eq!(
            second
                .accept()
                .blocking_read(|store| store.combined_last_channel())
                .unwrap()
                .as_deref(),
            Some("beta")
        );
        assert!(directory.path().join("alpha/iptv.sqlite").is_file());
        assert!(directory.path().join("beta/iptv.sqlite").is_file());
        assert!(!directory.path().join("gamma").exists());
        // Closing the second owner leaves the first one open.
        assert!(second.drain_on_shutdown());
        second.retire();
        assert!(second.accept().blocking(|_| Ok(())).is_err());
        first
            .accept()
            .blocking(|store| store.set_combined_last_channel("alpha-after"))
            .unwrap();
        assert!(first.drain_on_shutdown());
        drop(third);
        let reopened = iptv_store_sqlite::Store::open_encrypted(
            directory.path().join("alpha/iptv.sqlite"),
            &iptv_store_sqlite::DatabaseKey::from_bytes([0x41; 32]),
        )
        .unwrap();
        assert_eq!(
            reopened.combined_last_channel().unwrap().as_deref(),
            Some("alpha-after")
        );
    }

    #[test]
    fn work_accepted_before_a_restart_cannot_reach_the_restarted_store() {
        let directory = tempfile::tempdir().unwrap();
        let owner = service(directory.path(), 0x44);
        // Not started yet: nothing is accepted.
        assert!(!owner.accept().is_current());
        owner.epochs.begin();
        let old = owner.accept();
        old.blocking(|store| store.set_combined_last_channel("old"))
            .unwrap();
        assert!(owner.drain_on_shutdown());
        assert!(!old.is_current());
        owner.epochs.begin();
        let new = owner.accept();
        assert!(old.blocking(|_| Ok(())).is_err());
        // The pool takes the retired work, and skips it when its turn comes.
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ran.clone();
        assert!(old.submit(true, move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst)
        }));
        // Both workers meet here only after the queue before them is done.
        let meet = Arc::new(std::sync::Barrier::new(3));
        for _ in 0..2 {
            let meet = meet.clone();
            assert!(new.submit(true, move || {
                meet.wait();
            }));
        }
        meet.wait();
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        new.blocking(|store| store.set_combined_last_channel("new"))
            .unwrap();
        assert!(owner.drain_on_shutdown());
        owner.retire();
    }

    #[test]
    fn a_lane_refuses_a_request_from_its_own_thread_only() {
        let directory = tempfile::tempdir().unwrap();
        let first = service(&directory.path().join("alpha"), 0x45);
        let second = service(&directory.path().join("beta"), 0x46);
        first.epochs.begin();
        second.epochs.begin();
        let own = first.accept();
        let other = second.accept();
        let (recursive, across) = first
            .accept()
            .blocking(move |_| {
                Ok((
                    own.blocking(|_| Ok(())).is_err(),
                    other
                        .blocking(|store| store.set_combined_last_channel("across"))
                        .is_ok(),
                ))
            })
            .unwrap();
        assert!(recursive, "a writer job cannot wait for its own lane");
        assert!(across, "another owner's lane is a different lane");
        assert!(first.drain_on_shutdown());
        assert!(second.drain_on_shutdown());
    }

    #[test]
    fn a_retired_owner_cannot_start_a_file_writer_late() {
        let owner = std::rc::Rc::new(super::super::Runtime::default());
        let retained = owner.clone();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic-settings.json");
        assert!(owner.drain_services());
        drop(owner);
        let services = retained.services();
        drop(services.files().write(path.clone(), b"{}".to_vec()));
        assert!(services.files().flush(std::time::Duration::from_secs(1)));
        assert!(!path.exists());
    }

    fn unreachable_open() -> Result<iptv_store_sqlite::Store, iptv_store_sqlite::StoreError> {
        unreachable!("the test supplies an open store")
    }
    fn open() -> Connection {
        Connection {
            store: Some(iptv_store_sqlite::Store::open_in_memory().unwrap()),
            retry_open_at: None,
        }
    }
    #[test]
    fn delayed_old_worker_cannot_write_a_replacement_catalog() {
        use std::sync::mpsc;
        let epochs = Arc::new(super::Epochs::default());
        let old = epochs.begin();
        let mut connection = open();
        super::with_epoch(&epochs, old, &mut connection, unreachable_open, |store| {
            store.set_combined_last_channel("accepted-before-close")
        })
        .unwrap();
        assert_eq!(
            connection
                .store
                .as_ref()
                .unwrap()
                .combined_last_channel()
                .unwrap()
                .as_deref(),
            Some("accepted-before-close")
        );
        let (release, wait) = mpsc::channel();
        let (done, result) = mpsc::channel();
        let worker_epochs = epochs.clone();
        // The worker carries the epoch that accepted it.
        let delayed = move || {
            wait.recv().unwrap();
            let mut replacement = open();
            replacement
                .store
                .as_mut()
                .unwrap()
                .set_combined_last_channel("new-app")
                .unwrap();
            let write = super::with_epoch(
                &worker_epochs,
                old,
                &mut replacement,
                unreachable_open,
                |store| store.set_combined_last_channel("late-old-worker"),
            );
            done.send((
                write.is_err(),
                replacement.store.unwrap().combined_last_channel().unwrap(),
            ))
            .unwrap();
        };
        let thread = std::thread::spawn(delayed);
        assert!(epochs.close(old));
        let current = epochs.begin();
        assert_ne!(current, old);
        release.send(()).unwrap();
        let (rejected, value) = result.recv().unwrap();
        assert!(rejected);
        assert_eq!(value.as_deref(), Some("new-app"));
        thread.join().unwrap();
    }
    #[test]
    fn a_panicking_request_does_not_prevent_the_next_reply() {
        let mut connection = Connection::default();
        super::run_job(
            &mut connection,
            Box::new(|_| panic!("synthetic store failure")),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        super::run_job(
            &mut connection,
            Box::new(move |_| {
                tx.send(()).unwrap();
            }),
        );
        rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn application_database_opening_is_confined_to_the_service() {
        fn inspect(directory: &std::path::Path) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    inspect(&path);
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    && path
                        .file_name()
                        .is_none_or(|name| name != "store_service.rs")
                {
                    let source = std::fs::read_to_string(&path).unwrap();
                    assert!(
                        !source.contains("open_store("),
                        "database opened outside service: {}",
                        path.display()
                    );
                }
            }
        }
        inspect(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    }
}
