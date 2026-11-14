//! Platform inputs captured by each runtime and its native workers.
use crate::model::{DatabaseKey, StoreError};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

type KeyBroker = dyn Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync;

#[derive(Clone)]
pub struct Ports {
    pub directory: PathBuf,
    pub database_key: Arc<KeyBroker>,
    pub exclude_from_backup: fn(&Path) -> io::Result<()>,
    pub startup_mark: fn(&'static str, String),
}

impl Ports {
    pub fn new(
        directory: PathBuf,
        database_key: impl Fn(&Path) -> Result<DatabaseKey, StoreError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            directory,
            database_key: Arc::new(database_key),
            exclude_from_backup: |_| Ok(()),
            startup_mark: |_, _| {},
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
