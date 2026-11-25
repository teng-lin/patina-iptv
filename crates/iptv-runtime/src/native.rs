//! The native implementations of the runtime's ports, and the services only
//! native hosts have. Everything here is behind the default `native`
//! feature: a browser build supplies its ports from the host instead.
pub(crate) mod execution;
pub(crate) mod network;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::Ports;
use crate::model::{DatabaseKey, StoreError};

impl Ports {
    /// The ports of a native host: `iptv-net` for networking, threads and
    /// Tokio for execution, and the operating system's random source.
    pub fn new(
        directory: PathBuf,
        database_key: impl Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync + 'static,
    ) -> Self {
        Self::with_adapters(
            directory,
            database_key,
            Arc::new(network::NativeNetwork),
            Arc::new(execution::NativeExecution),
            entropy,
        )
    }
}

/// The operating system's secure random source ([`crate::Ports::entropy`]).
pub(crate) fn entropy(bytes: &mut [u8]) -> std::io::Result<()> {
    getrandom::fill(bytes).map_err(|error| std::io::Error::other(error.to_string()))
}
