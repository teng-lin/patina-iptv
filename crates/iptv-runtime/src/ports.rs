//! Platform inputs captured by each runtime and its native workers.
use crate::model::{DatabaseKey, StoreError};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

type KeyBroker = dyn Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync;

/// Fills a buffer with random bytes from the platform's secure source.
pub type Entropy = fn(&mut [u8]) -> io::Result<()>;

/// `Send` when the runtime runs work on threads (the `native` feature); no
/// bound in a single-threaded browser build, whose adapters hold the page's
/// values.
#[cfg(feature = "native")]
pub trait MaybeSend: Send {}
#[cfg(feature = "native")]
impl<T: Send + ?Sized> MaybeSend for T {}
/// `Send` when the runtime runs work on threads (the `native` feature); no
/// bound in a single-threaded browser build, whose adapters hold the page's
/// values.
#[cfg(not(feature = "native"))]
pub trait MaybeSend {}
#[cfg(not(feature = "native"))]
impl<T: ?Sized> MaybeSend for T {}

/// `Sync` exactly when [`MaybeSend`] is `Send`.
#[cfg(feature = "native")]
pub trait MaybeSync: Sync {}
#[cfg(feature = "native")]
impl<T: Sync + ?Sized> MaybeSync for T {}
/// `Sync` exactly when [`MaybeSend`] is `Send`.
#[cfg(not(feature = "native"))]
pub trait MaybeSync {}
#[cfg(not(feature = "native"))]
impl<T: ?Sized> MaybeSync for T {}

/// Why the recording host should post an incomplete-capture alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingFailure {
    /// The capture failed or ended with incomplete media.
    Unexpected,
    /// The TV was already in standby when the capture worker took the job.
    TvStandby,
}

#[derive(Clone)]
pub struct Ports {
    pub directory: PathBuf,
    /// Native host keeps recording independently of UI engine ownership.
    pub background_recordings: bool,
    /// Whether a capture is running or due, so the host keeps running for
    /// it (Android's foreground service and its watchdog alarm, a desktop's
    /// idle-sleep hold) or lets go. Runs on a native worker; returns once
    /// the host has applied it.
    pub recording_schedule: fn(bool),
    /// An active capture failed or ended with incomplete media. The host
    /// may notify a viewer in the background. No recording metadata crosses.
    pub recording_failed: fn(RecordingFailure),
    /// Read whether TV standby currently prevents a capture from opening
    /// or resuming. Runs on a native blocking worker.
    pub recording_standby: Option<fn() -> bool>,
    pub database_key: Arc<KeyBroker>,
    pub exclude_from_backup: fn(&Path) -> io::Result<()>,
    pub startup_mark: fn(&'static str, String),
    /// Makes the network clients of imports, browsing and link resolves.
    pub network: Arc<dyn crate::network::Network>,
    /// Runs store lanes, workers and each owner's tasks.
    pub execution: Arc<dyn crate::execution::Execution>,
    /// Random bytes for salts, recovery codes and private identities. The
    /// operating system's natively; a browser host supplies its own, and
    /// without one every operation that needs randomness fails.
    pub entropy: Entropy,
    /// The private directory is a file-system directory this process must
    /// hold alone: it is locked against other instances and a scheduled
    /// restore is activated in it at startup. A browser host keeps its
    /// storage in one Worker and has neither, so it sets this false.
    pub file_storage: bool,
    /// Opens and persists the store in place of the encrypted file in
    /// `directory` (a browser's memory files), and publishes on a copy
    /// in slices. `None` natively.
    pub store_host: Option<Arc<dyn crate::store_host::StoreHost>>,
    /// Keeps the app's small files (settings, last channels) in place of
    /// files in `directory` (a browser's IndexedDB). `None` natively.
    pub file_host: Option<Arc<dyn crate::file_host::FileHost>>,
}

impl Ports {
    /// The ports of a native host: `iptv-net` for the network, threads and
    /// Tokio for execution.
    #[cfg(feature = "native")]
    pub fn new(
        directory: PathBuf,
        database_key: impl Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync + 'static,
    ) -> Self {
        Self::with_adapters(
            directory,
            database_key,
            Arc::new(crate::native::network::NativeNetwork),
            Arc::new(crate::native::execution::NativeExecution),
        )
    }

    /// The ports of a host that supplies its own network and execution
    /// (a browser host: its `fetch` adapter and [`crate::execution::Cooperative`]).
    pub fn with_adapters(
        directory: PathBuf,
        database_key: impl Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync + 'static,
        network: Arc<dyn crate::network::Network>,
        execution: Arc<dyn crate::execution::Execution>,
    ) -> Self {
        Self {
            directory,
            background_recordings: false,
            recording_schedule: |_| {},
            recording_failed: |_| {},
            recording_standby: None,
            database_key: Arc::new(database_key),
            exclude_from_backup: |_| Ok(()),
            startup_mark: |_, _| {},
            network,
            execution,
            #[cfg(feature = "native")]
            entropy: crate::native::entropy,
            #[cfg(not(feature = "native"))]
            entropy: |_| {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "the host supplied no entropy",
                ))
            },
            file_storage: true,
            store_host: None,
            file_host: None,
        }
    }

    #[cfg(all(test, feature = "native"))]
    pub(crate) fn for_tests() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self::new(
            std::env::temp_dir().join(format!(
                "patina-runtime-unopened-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )),
            |_| {
                Err(StoreError::unavailable(
                    "synthetic database key was not installed",
                ))
            },
        )
    }
}
