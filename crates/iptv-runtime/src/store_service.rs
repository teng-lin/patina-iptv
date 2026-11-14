//! The app's two store connections, each on its own long-lived worker
//! thread. UI callers await replies; import workers may wait, but cannot
//! open competing app connections.
//!
//! - **The writer** ([`call`], [`blocking`]) runs every write, all staging,
//!   and the reads that must see writes queued before them, in FIFO order.
//! - **The reader** ([`read`]) runs UI reads (guide pages, descriptions,
//!   search, favourites, organisation) on a second, `query_only`
//!   connection to the same file. The store uses WAL, so these read the
//!   last commit while the writer holds a long publish transaction instead
//!   of queueing behind it. A read closure gets `&Store`, so it cannot call
//!   a writing method, and SQLite refuses a write through the connection
//!   anyway. A read sees what a write committed once that write's reply
//!   arrived; it may not see a write still queued, so a read that must
//!   follow an unawaited write goes to the writer with [`call`].
//!
//! The reader's connection is made by the writer ([`Store::open_reader`]):
//! the platform key is fetched, and the file migrated, only there.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

use futures_channel::oneshot;
use iptv_store_sqlite::{Store, StoreError};

type Job = Box<dyn FnOnce(&mut Option<Store>) + Send>;

thread_local! {
    static APP_EPOCH: Cell<Option<u64>> = const { Cell::new(None) };
    static UI_THREAD: Cell<bool> = const { Cell::new(false) };
    /// The worker this thread is, if it is one: a job may not queue on its
    /// own worker and wait for itself.
    static WORKER_THREAD: Cell<Option<Role>> = const { Cell::new(None) };
    static RETRY_OPEN_AT: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Writer,
    Reader,
}

/// One connection's worker: a thread that owns the connection and runs
/// jobs in order.
struct Lane {
    role: Role,
    name: &'static str,
    jobs: OnceLock<mpsc::Sender<Job>>,
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
pub(crate) struct Service {
    pub(crate) ports: crate::Ports,
    pub(crate) directory: std::path::PathBuf,
    writer: Lane,
    reader: Lane,
    epochs: Epochs,
    pub(crate) executor: Arc<super::executor::Executor>,
    pub(crate) files: Arc<super::file_writer::Files>,
    pub(crate) pool: OnceLock<super::workers::Pool>,
    pub(crate) settings: super::settings::Current,
    /// This owner's registered guide checks, for a test's thread.
    #[cfg(feature = "test-support")]
    pub(crate) guide_checks: super::guide_checks::Mirror,
    /// This owner's synthetic worker conditions.
    #[cfg(feature = "test-support")]
    pub(crate) hooks: super::workers::Hooks,
}
impl Service {
    pub(crate) fn new(ports: crate::Ports) -> Self {
        Self {
            directory: ports.directory.clone(),
            ports,
            writer: Lane {
                role: Role::Writer,
                name: "iptv-store",
                jobs: OnceLock::new(),
            },
            reader: Lane {
                role: Role::Reader,
                name: "iptv-store-read",
                jobs: OnceLock::new(),
            },
            epochs: Epochs::default(),
            executor: Arc::default(),
            files: Arc::default(),
            pool: OnceLock::new(),
            settings: Arc::default(),
            #[cfg(feature = "test-support")]
            guide_checks: Default::default(),
            #[cfg(feature = "test-support")]
            hooks: Default::default(),
        }
    }
}
impl Service {
    pub(crate) fn retire(&self) {
        self.epochs.active.store(0, Ordering::SeqCst);
        #[cfg(feature = "test-support")]
        self.hooks.release();
        self.files.close();
        if let Some(pool) = self.pool.get() {
            pool.close();
        }
    }
}

thread_local! { static CONTEXT: RefCell<Option<Weak<Service>>> = const { RefCell::new(None) }; }
// A platform callback (Android onStop) can arrive off the application thread.
// This weak registry identifies the active owner without owning its state.
static ACTIVE: Mutex<Weak<Service>> = Mutex::new(Weak::new());

pub(crate) fn service() -> Arc<Service> {
    try_service().unwrap_or_else(|| crate::current().store.clone())
}

/// The owner of this context, else the active one; `None` without either.
pub(crate) fn try_service() -> Option<Arc<Service>> {
    CONTEXT
        .with(|context| context.borrow().as_ref().and_then(Weak::upgrade))
        .or_else(active_service)
}

pub(crate) fn active_service() -> Option<Arc<Service>> {
    ACTIVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .upgrade()
}

pub(crate) fn activate(owner: Arc<Service>) {
    *ACTIVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&owner);
    CONTEXT.set(Some(Arc::downgrade(&owner)));
}

pub(crate) fn with_service<R>(owner: Arc<Service>, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<Weak<Service>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CONTEXT.set(self.0.take());
        }
    }
    let _restore = Restore(CONTEXT.replace(Some(Arc::downgrade(&owner))));
    super::settings::with_current(owner.settings.clone(), run)
}

pub(crate) fn scope_future<F: Future>(
    owner: Arc<Service>,
    future: F,
) -> impl Future<Output = F::Output> {
    let epoch = APP_EPOCH.get();
    let mut future = Box::pin(future);
    std::future::poll_fn(move |context| {
        struct Epoch(Option<u64>);
        impl Drop for Epoch {
            fn drop(&mut self) {
                APP_EPOCH.set(self.0);
            }
        }
        let _epoch = Epoch(APP_EPOCH.replace(epoch));
        with_service(owner.clone(), || future.as_mut().poll(context))
    })
}

fn stale() -> StoreError {
    StoreError::Unavailable("request belongs to a closed application")
}
fn affinity() -> Result<u64, StoreError> {
    APP_EPOCH
        .get()
        .filter(|epoch| service().epochs.permits(*epoch))
        .ok_or_else(stale)
}
pub(crate) fn is_current() -> bool {
    affinity().is_ok()
}

pub fn register_ui_thread() {
    UI_THREAD.set(true);
    APP_EPOCH.set(Some(service().epochs.begin()));
}

/// Capture ownership before spawning work. A pooled worker never adopts the
/// epoch of a newer app just because an older network request finishes late.
pub(crate) fn scoped<R: 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> impl FnOnce() -> R + Send + 'static {
    let epoch = APP_EPOCH.get();
    let owner = service();
    move || {
        struct Restore(Option<u64>);
        impl Drop for Restore {
            fn drop(&mut self) {
                APP_EPOCH.set(self.0);
            }
        }
        let _restore = Restore(APP_EPOCH.replace(epoch));
        with_service(owner, work)
    }
}

fn with_epoch<R>(
    epochs: &Epochs,
    epoch: u64,
    store: &mut Option<Store>,
    open: impl FnOnce() -> Result<Store, StoreError>,
    operation: impl FnOnce(&mut Store) -> Result<R, StoreError>,
) -> Result<R, StoreError> {
    if !epochs.permits(epoch) {
        return Err(stale());
    }
    with_store(store, open, |store| {
        // Opening the OS broker can wait; a newer app may have started while
        // an old open was waiting, so recheck before touching catalog rows.
        if !epochs.permits(epoch) {
            return Err(stale());
        }
        operation(store)
    })
}

fn open_store() -> Result<Store, StoreError> {
    let directory = super::paths::data_dir();
    std::fs::create_dir_all(&directory)
        .map_err(|_| StoreError::Unavailable("cannot create data directory"))?;
    super::credential_store::open_database(&directory.join("iptv.sqlite"))
}

/// The reader's connection, made by the writer from its own open store
/// (the key and the migration stay there). Runs on the reader's thread,
/// which may wait for the writer; the writer never waits for the reader.
fn open_reader(epoch: u64) -> Result<Store, StoreError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    service().writer.queue(Box::new(move |store| {
        let _ = sender.send(with_epoch(
            &service().epochs,
            epoch,
            store,
            open_store,
            |store| store.open_reader(),
        ));
    }))?;
    receiver
        .recv()
        .map_err(|_| StoreError::Unavailable("store reply lost"))?
}

impl Lane {
    fn sender(&self) -> Result<mpsc::Sender<Job>, StoreError> {
        if WORKER_THREAD.get() == Some(self.role) {
            return Err(StoreError::Unavailable("recursive store request"));
        }
        Ok(self
            .jobs
            .get_or_init(|| {
                let (sender, receiver) = mpsc::channel::<Job>();
                let role = self.role;
                std::thread::Builder::new()
                    .name(self.name.into())
                    .spawn(move || {
                        WORKER_THREAD.set(Some(role));
                        let mut store = None;
                        while let Ok(job) = receiver.recv() {
                            run_job(&mut store, job);
                        }
                    })
                    .expect("start store service");
                sender
            })
            .clone())
    }

    fn queue(&self, job: Job) -> Result<(), StoreError> {
        let owner = service();
        self.sender()?
            .send(Box::new(move |store| with_service(owner, || job(store))))
            .map_err(|_| StoreError::Unavailable("store worker stopped"))
    }

    /// Runs `job` after everything queued before it and waits, at most
    /// `limit`, until it has run. False when it could not be queued or did
    /// not finish in time.
    fn barrier(
        &self,
        limit: std::time::Duration,
        job: impl FnOnce(&mut Option<Store>) + Send + 'static,
    ) -> bool {
        if self.jobs.get().is_none() {
            return true;
        }
        let (done, reply) = mpsc::channel();
        if self
            .queue(Box::new(move |store| {
                job(store);
                let _ = done.send(());
            }))
            .is_err()
        {
            return false;
        }
        reply.recv_timeout(limit).is_ok()
    }
}

fn run_job(store: &mut Option<Store>, job: Job) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(store))).is_err() {
        // Discard the connection so an interrupted operation cannot leak its
        // transaction or statement state into the next request.
        *store = None;
        log::error!("iptv store: request panicked; connection discarded");
    }
}

fn with_store<R>(
    store: &mut Option<Store>,
    open: impl FnOnce() -> Result<Store, StoreError>,
    operation: impl FnOnce(&mut Store) -> Result<R, StoreError>,
) -> Result<R, StoreError> {
    if store.is_none() {
        if RETRY_OPEN_AT
            .get()
            .is_some_and(|at| std::time::Instant::now() < at)
        {
            return Err(StoreError::Unavailable(
                "credential storage is temporarily unavailable",
            ));
        }
        match open() {
            Ok(opened) => {
                *store = Some(opened);
                RETRY_OPEN_AT.set(None);
            }
            Err(error) => {
                RETRY_OPEN_AT.set(Some(
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                ));
                return Err(error);
            }
        }
    }
    operation(store.as_mut().expect("opened store"))
}

/// Queues `operation` on the writer and returns its reply. Transfers
/// ownership immediately in FIFO order, even if the caller drops its reply
/// future. Bulk producers are bounded by the refresh pool; no SQLite work
/// or enqueue wait runs on the UI thread. Use it for every write, and for
/// a read that must see a write the caller has queued but not awaited.
pub fn call<R: Send + 'static>(
    operation: impl FnOnce(&mut Store) -> Result<R, StoreError> + Send + 'static,
) -> impl Future<Output = Result<R, StoreError>> {
    let (sender, receiver) = oneshot::channel();
    let queued = affinity().and_then(|epoch| {
        service().writer.queue(Box::new(move |store| {
            let _ = sender.send(with_epoch(
                &service().epochs,
                epoch,
                store,
                open_store,
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

/// Queues `operation` on the reader and returns its reply: a read of the
/// last commit that does not wait for the writer (see the module docs).
/// The first read opens the reader's connection through the writer.
pub fn read<R: Send + 'static>(
    operation: impl FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
) -> impl Future<Output = Result<R, StoreError>> {
    let (sender, receiver) = oneshot::channel();
    let queued = affinity().and_then(|epoch| {
        service().reader.queue(Box::new(move |store| {
            let opened = with_epoch(
                &service().epochs,
                epoch,
                store,
                || open_reader(epoch),
                |store| operation(store),
            );
            let _ = sender.send(opened);
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
pub fn blocking<R: Send + 'static>(
    operation: impl FnOnce(&mut Store) -> Result<R, StoreError> + Send + 'static,
) -> Result<R, StoreError> {
    assert!(!UI_THREAD.get(), "blocking store request on the UI thread");
    let epoch = affinity()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    service().writer.queue(Box::new(move |store| {
        let _ = sender.send(with_epoch(
            &service().epochs,
            epoch,
            store,
            open_store,
            operation,
        ));
    }))?;
    receiver
        .recv()
        .map_err(|_| StoreError::Unavailable("store reply lost"))?
}

/// For import/background workers only: a read of the last commit that runs on
/// the reader lane and does not wait for the writer.
pub fn blocking_read<R: Send + 'static>(
    operation: impl FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
) -> Result<R, StoreError> {
    assert!(!UI_THREAD.get(), "blocking store read on the UI thread");
    let epoch = affinity()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    service().reader.queue(Box::new(move |store| {
        let opened = with_epoch(
            &service().epochs,
            epoch,
            store,
            || open_reader(epoch),
            |store| operation(store),
        );
        let _ = sender.send(opened);
    }))?;
    receiver
        .recv()
        .map_err(|_| StoreError::Unavailable("store reply lost"))?
}

/// Occupies the writer for `duration` after the work queued before it.
#[cfg(feature = "test-support")]
pub fn occupy_writer(duration: std::time::Duration) {
    let _ = service()
        .writer
        .queue(Box::new(move |_| std::thread::sleep(duration)));
}

/// Root teardown only, after the favourites writer has submitted its last work.
/// Accepted requests already belong to their workers, independent of UI futures.
pub(crate) fn drain_on_shutdown() -> bool {
    let Some(epoch) = APP_EPOCH.get() else {
        return true;
    };
    if service().writer.jobs.get().is_none() {
        service().epochs.close(epoch);
        return true;
    }
    let limit = std::time::Duration::from_secs(10);
    // The writer first: once it closed the epoch, a late read cannot open
    // the reader's connection again.
    let writer = service().writer.barrier(limit, move |store| {
        // Release SQLite handles before a new app instance opens or restores
        // its library in the same process.
        service().epochs.close(epoch);
        // Even if a timed-out teardown was followed by a newer start, this
        // FIFO barrier still retires the connection held by preceding work.
        *store = None;
        RETRY_OPEN_AT.set(None);
    });
    let reader = service().reader.barrier(limit, |store| {
        *store = None;
        RETRY_OPEN_AT.set(None);
    });
    writer && reader
}

#[cfg(test)]
impl Default for Service {
    fn default() -> Self {
        Self::new(crate::Ports::for_tests())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_scoped_worker_keeps_its_original_settings_and_directory() {
        use iptv_settings::{Settings, Target, Value, keys};
        use std::sync::Arc;
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("alpha");
        let first = Arc::new(super::Service {
            directory: first_path.clone(),
            ..Default::default()
        });
        let second = Arc::new(super::Service {
            directory: directory.path().join("beta"),
            ..Default::default()
        });
        let values = |agent: &str| {
            let mut settings = Settings::new();
            settings
                .set("general.user_agent", Value::Text(agent.into()))
                .unwrap();
            Some(Arc::new(settings))
        };
        *first.settings.write().unwrap() = values("SyntheticAlpha/1");
        *second.settings.write().unwrap() = values("SyntheticBeta/1");
        let worker = super::with_service(first.clone(), || {
            super::scoped(|| {
                (
                    super::super::settings::get().text(keys::USER_AGENT, &Target::Global),
                    super::super::paths::data_dir(),
                )
            })
        });
        first.retire();
        let result = super::with_service(second, || std::thread::spawn(worker).join().unwrap());
        assert_eq!(result, ("SyntheticAlpha/1".into(), first_path));
    }

    #[test]
    fn a_retired_owner_cannot_start_a_file_writer_late() {
        let owner = std::rc::Rc::new(super::super::Runtime::default());
        let retained = owner.clone();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic-settings.json");
        assert!(owner.drain_services());
        drop(owner);
        retained.with_context(|| {
            drop(super::super::file_writer::write(
                path.clone(),
                b"{}".to_vec(),
            ));
            assert!(super::super::file_writer::flush(
                std::time::Duration::from_secs(1)
            ));
        });
        assert!(!path.exists());
    }

    fn unreachable_open() -> Result<iptv_store_sqlite::Store, iptv_store_sqlite::StoreError> {
        unreachable!("the test supplies an open store")
    }
    #[test]
    fn delayed_old_worker_cannot_write_a_replacement_catalog() {
        use std::sync::{Arc, mpsc};
        let epochs = Arc::new(super::Epochs::default());
        let old = epochs.begin();
        let mut store = Some(iptv_store_sqlite::Store::open_in_memory().unwrap());
        super::with_epoch(&epochs, old, &mut store, unreachable_open, |store| {
            store.set_combined_last_channel("accepted-before-close")
        })
        .unwrap();
        assert_eq!(
            store
                .as_ref()
                .unwrap()
                .combined_last_channel()
                .unwrap()
                .as_deref(),
            Some("accepted-before-close")
        );
        let previous = super::APP_EPOCH.replace(Some(old));
        let (release, wait) = mpsc::channel();
        let (done, result) = mpsc::channel();
        let worker_epochs = epochs.clone();
        let delayed = super::scoped(move || {
            wait.recv().unwrap();
            let captured = super::APP_EPOCH.get().unwrap();
            let mut replacement = Some(iptv_store_sqlite::Store::open_in_memory().unwrap());
            replacement
                .as_mut()
                .unwrap()
                .set_combined_last_channel("new-app")
                .unwrap();
            let write = super::with_epoch(
                &worker_epochs,
                captured,
                &mut replacement,
                unreachable_open,
                |store| store.set_combined_last_channel("late-old-worker"),
            );
            done.send((
                write.is_err(),
                replacement.unwrap().combined_last_channel().unwrap(),
            ))
            .unwrap();
        });
        super::APP_EPOCH.set(previous);
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
        let mut store = None;
        super::run_job(&mut store, Box::new(|_| panic!("synthetic store failure")));
        let (tx, rx) = std::sync::mpsc::channel();
        super::run_job(
            &mut store,
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
