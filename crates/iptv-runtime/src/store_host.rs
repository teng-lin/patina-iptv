//! A host that keeps the store itself (a browser: SQLite on memory files
//! it restores at launch and persists in chunks), in place of the
//! encrypted file natively opened with the platform's key.
//!
//! Only opening, persistence and publishing are the host's: the store is
//! the one `iptv-store-sqlite` implementation on both targets, and every
//! query runs as natively. The runtime calls the host on its writer lane:
//! [`StoreHost::open`] once (and again after a failure), [`StoreHost::committed`]
//! after each writer job, and the three publish calls from a sliced
//! publish, which runs catalog and guide replacements on a copy while
//! reads continue on the live store (`publish.rs`).

use std::sync::Arc;

use iptv_store_sqlite::{Store, StoreError};

/// Asks the runtime to call [`StoreHost::committed`] again soon, on the
/// writer lane: for changes the host could not take while its last save
/// was still being written.
pub type Persist = Arc<dyn Fn() + Send + Sync>;

/// Opening, persisting and publishing a store the host keeps.
pub trait StoreHost: Send + Sync {
    /// Opens the app's store, restored from the host's storage. It cannot
    /// wait: what it restores must already be at hand.
    fn open(&self) -> Result<Store, StoreError>;

    /// A writer job ended on `store`, between transactions: persist what
    /// it committed, if anything. A host that is still writing an earlier
    /// save calls `again` once that save ended.
    fn committed(&self, store: &Store, again: Persist);

    /// Starts a publish: a copy of `store` that a replacement writes in
    /// slices while `store` serves reads and grants no write.
    fn begin_publish(&self, store: &mut Store) -> Result<Store, StoreError>;

    /// Replaces `store` with `copy`, whose replacement committed.
    fn finish_publish(&self, store: &mut Store, copy: Store) -> Result<(), StoreError>;

    /// Drops `copy`, whose replacement failed or was canceled; `store` is
    /// unchanged and takes writes again.
    fn abandon_publish(&self, store: &mut Store, copy: Store);
}
