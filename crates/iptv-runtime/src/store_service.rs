//! The app's two store connections, each on its own long-lived worker
//! thread. UI callers await replies; import workers may wait, but cannot
//! open competing app connections.
//!
//! Every owner ([`Service`]) has its own pair; work reaches them through the
//! [`Services`] handle its owner gave out, never through a process-wide or
//! thread-local lookup.
//!
//! - **The writer** ([`Services::call`], [`Services::blocking`]) runs every write, all staging,
//!   and the reads that must see writes queued before them, in FIFO order.
//! - **The reader** ([`Services::read`]) runs UI reads (guide pages, descriptions,
//!   search, favourites, organisation) on a second, `query_only`
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

use std::future::Future;
use std::path::Path;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread::ThreadId;

use futures_channel::oneshot;
use iptv_store_sqlite::{Store, StoreError};

/// What a lane's thread keeps between jobs: its connection, and when a
/// failed open may be tried again.
#[derive(Default)]
struct Connection {
    store: Option<Store>,
    retry_open_at: Option<std::time::Instant>,
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// One connection's worker: a thread that owns the connection and runs
/// jobs in order.
struct Lane {
    name: &'static str,
    /// The job queue and its thread: a job may not queue on its own lane
    /// and wait for itself.
    jobs: OnceLock<(mpsc::Sender<Job>, ThreadId)>,
}

#[derive(Default)]
struct Epochs {
    next: AtomicU64,
    active: AtomicU64,
}
impl Epochs {
    fn begin(&self) -> u64 {
        let epoch = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        self.active.store(epoch, Ordering::SeqCst);
        epoch
    }
    fn permits(&self, epoch: u64) -> bool {
        epoch != 0 && self.active.load(Ordering::SeqCst) == epoch
    }
    fn close(&self, epoch: u64) -> bool {
        self.active
            .compare_exchange(epoch, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

/// The native services of one application owner: its two store lanes, file
/// writer, network executor, refresh pool, settings and platform ports.
/// Nothing here is process-wide: a second owner has its own.
pub(crate) struct Service {
    ports: crate::Ports,
    writer: Lane,
    reader: Lane,
    epochs: Epochs,
    executor: super::executor::Executor,
    files: super::file_writer::Files,
    pool: OnceLock<super::workers::Pool>,
    settings: super::settings::Shared,
    /// The thread that started the services; it may not block on the store.
    ui_thread: OnceLock<ThreadId>,
}
impl Service {
    pub(crate) fn new(ports: crate::Ports) -> Self {
        Self {
            ports,
            writer: Lane {
                name: "iptv-store",
                jobs: OnceLock::new(),
            },
            reader: Lane {
                name: "iptv-store-read",
                jobs: OnceLock::new(),
            },
            epochs: Epochs::default(),
            executor: Default::default(),
            files: Default::default(),
            pool: OnceLock::new(),
            settings: Default::default(),
            ui_thread: OnceLock::new(),
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
        Services {
            service: self.clone(),
            epoch: self.epochs.active.load(Ordering::SeqCst),
        }
    }

    pub(crate) fn retire(&self) {
        self.epochs.active.store(0, Ordering::SeqCst);
        self.files.close();
        if let Some(pool) = self.pool.get() {
            pool.close();
        }
    }

    pub(crate) fn files(&self) -> &super::file_writer::Files {
        &self.files
    }

    fn open_store(&self) -> Result<Store, StoreError> {
        let directory = self.directory();
        std::fs::create_dir_all(directory)
            .map_err(|_| StoreError::Unavailable("cannot create data directory"))?;
        super::credential_store::open_database(&self.ports, &directory.join("iptv.sqlite"))
    }

    fn assert_off_ui_thread(&self, what: &str) {
        assert!(
            self.ui_thread.get() != Some(&std::thread::current().id()),
            "{what} on the UI thread"
        );
    }

    /// Root teardown only, after the favourites writer has submitted its last
    /// work. Accepted requests already belong to their workers, independent
    /// of UI futures.
    pub(crate) fn drain_on_shutdown(self: &Arc<Self>) -> bool {
        let epoch = self.epochs.active.load(Ordering::SeqCst);
        if self.writer.jobs.get().is_none() {
            self.epochs.close(epoch);
            return true;
        }
        let limit = std::time::Duration::from_secs(10);
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
        let reader = self.reader.barrier(limit, |connection| {
            *connection = Connection::default();
        });
        writer && reader
    }
}

/// One owner's services as the work that was accepted sees them. `Send`:
/// workers carry it instead of looking an owner up. A handle taken before
/// the owner closed (or restarted) cannot reach the store afterwards.
#[derive(Clone)]
pub(crate) struct Services {
    service: Arc<Service>,
    epoch: u64,
}

fn stale() -> StoreError {
    StoreError::Unavailable("request belongs to a closed application")
}

impl Services {
    fn affinity(&self) -> Result<u64, StoreError> {
        if self.service.epochs.permits(self.epoch) {
            Ok(self.epoch)
        } else {
            Err(stale())
        }
    }

    /// The application that accepted this work is still the open one.
    pub(crate) fn is_current(&self) -> bool {
        self.affinity().is_ok()
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

    /// Runs `future` on the owner's network runtime; await the handle for
    /// its output. Dropping the handle detaches the task (cancel it with its
    /// `CancelToken`).
    pub(crate) fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.service.executor.spawn(future)
    }

    /// Runs CPU-bound `work` on the network runtime's blocking threads
    /// (building a large schedule); await the handle for its output.
    pub(crate) fn spawn_blocking<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> tokio::task::JoinHandle<R> {
        self.service.executor.spawn_blocking(work)
    }

    /// Drive a non-Send import on its bounded blocking worker, sharing the
    /// IO executor.
    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.service.executor.block_on(future)
    }

    /// Queues refresh `work` on the owner's bounded pool; false when it is
    /// full. Work for the visible source precedes background work.
    pub(crate) fn submit(&self, visible: bool, work: impl FnOnce() + Send + 'static) -> bool {
        let accepted = self.clone();
        self.service
            .pool
            .get_or_init(super::workers::Pool::standard)
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
    /// The first read opens the reader's connection through the writer.
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
                let _ = sender.send(owner.read(epoch, connection, operation));
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
        let (sender, receiver) = mpsc::sync_channel(1);
        let owner = self.service.clone();
        self.service.writer.queue(Box::new(move |connection| {
            let _ = sender.send(owner.write(epoch, connection, operation));
        }))?;
        receiver
            .recv()
            .map_err(|_| StoreError::Unavailable("store reply lost"))?
    }

    /// For import/background workers only: a read of the last commit that runs
    /// on the reader lane and does not wait for the writer.
    pub(crate) fn blocking_read<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
    ) -> Result<R, StoreError> {
        self.service.assert_off_ui_thread("blocking store read");
        let epoch = self.affinity()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let owner = self.service.clone();
        self.service.reader.queue(Box::new(move |connection| {
            let _ = sender.send(owner.read(epoch, connection, operation));
        }))?;
        receiver
            .recv()
            .map_err(|_| StoreError::Unavailable("store reply lost"))?
    }

    /// Occupies the writer for `duration` after the work queued before it.
    #[cfg(feature = "test-support")]
    pub(crate) fn occupy_writer(&self, duration: std::time::Duration) {
        let _ = self
            .service
            .writer
            .queue(Box::new(move |_| std::thread::sleep(duration)));
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
            || self.open_reader(epoch),
            |store| operation(store),
        )
    }

    /// The reader's connection, made by the writer from its own open store
    /// (the key and the migration stay there). Runs on the reader's thread,
    /// which may wait for the writer; the writer never waits for the reader.
    fn open_reader(self: &Arc<Self>, epoch: u64) -> Result<Store, StoreError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let owner = self.clone();
        self.writer.queue(Box::new(move |connection| {
            let _ = sender.send(owner.write(epoch, connection, |store| store.open_reader()));
        }))?;
        receiver
            .recv()
            .map_err(|_| StoreError::Unavailable("store reply lost"))?
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
    fn sender(&self) -> Result<mpsc::Sender<Job>, StoreError> {
        let (sender, thread) = self.jobs.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            let thread = std::thread::Builder::new()
                .name(self.name.into())
                .spawn(move || {
                    let mut connection = Connection::default();
                    while let Ok(job) = receiver.recv() {
                        run_job(&mut connection, job);
                    }
                })
                .expect("start store service");
            (sender, thread.thread().id())
        });
        if *thread == std::thread::current().id() {
            return Err(StoreError::Unavailable("recursive store request"));
        }
        Ok(sender.clone())
    }

    fn queue(&self, job: Job) -> Result<(), StoreError> {
        self.sender()?
            .send(job)
            .map_err(|_| StoreError::Unavailable("store worker stopped"))
    }

    /// Runs `job` after everything queued before it and waits, at most
    /// `limit`, until it has run. False when it could not be queued or did
    /// not finish in time.
    fn barrier(
        &self,
        limit: std::time::Duration,
        job: impl FnOnce(&mut Connection) + Send + 'static,
    ) -> bool {
        if self.jobs.get().is_none() {
            return true;
        }
        let (done, reply) = mpsc::channel();
        if self
            .queue(Box::new(move |connection| {
                job(connection);
                let _ = done.send(());
            }))
            .is_err()
        {
            return false;
        }
        reply.recv_timeout(limit).is_ok()
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
            .is_some_and(|at| std::time::Instant::now() < at)
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
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
                return Err(error);
            }
        }
    }
    operation(connection.store.as_mut().expect("opened store"))
}

#[cfg(test)]
impl Default for Service {
    fn default() -> Self {
        Self::new(crate::Ports::for_tests())
    }
}

#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::{Connection, Service};
    use std::sync::Arc;

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
