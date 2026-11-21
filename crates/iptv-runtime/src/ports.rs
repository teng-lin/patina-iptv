//! Platform inputs captured by each runtime and its native workers.
use crate::model::{DatabaseKey, StoreError};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

type KeyBroker = dyn Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync;

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

#[derive(Clone)]
pub struct Ports {
    pub directory: PathBuf,
    /// Native host keeps recording independently of UI engine ownership.
    pub background_recordings: bool,
    /// Register the next wake (UTC seconds, zero when none) and capture activity.
    /// Runs on a native worker; return only after OS registration completes and
    /// report the capability actually granted.
    pub recording_schedule: fn(i64, bool) -> iptv_core::recording::WakeCapability,
    pub database_key: Arc<KeyBroker>,
    pub exclude_from_backup: fn(&Path) -> io::Result<()>,
    pub startup_mark: fn(&'static str, String),
    /// Makes the network clients of imports, browsing and link resolves.
    pub network: Arc<dyn crate::network::Network>,
}

impl Ports {
    /// The ports of a native host: `iptv-net` for the network.
    #[cfg(feature = "native")]
    pub fn new(
        directory: PathBuf,
        database_key: impl Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync + 'static,
    ) -> Self {
        Self::with_network(
            directory,
            database_key,
            Arc::new(crate::native::network::NativeNetwork),
        )
    }

    /// The ports of a host that supplies its own network adapter.
    pub fn with_network(
        directory: PathBuf,
        database_key: impl Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync + 'static,
        network: Arc<dyn crate::network::Network>,
    ) -> Self {
        Self {
            directory,
            background_recordings: false,
            recording_schedule: |_, _| iptv_core::recording::WakeCapability::AwakeOnly,
            database_key: Arc::new(database_key),
            exclude_from_backup: |_| Ok(()),
            startup_mark: |_, _| {},
            network,
        }
    }

    #[cfg(test)]
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
