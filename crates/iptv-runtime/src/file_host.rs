//! A host that keeps the app's small files itself (a browser: entries in
//! its IndexedDB, read before the app starts), in place of the private
//! directory's files that the file writer replaces natively
//! (`file_writer.rs`).
//!
//! The runtime names a file by its path under [`crate::Ports::directory`];
//! the host keeps it under that name. Reads cannot wait: what was stored
//! must be at hand when the app starts, and a read sees every write handed
//! over before it. Writes are stored in the order they are handed over.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// A write's answer: true once the host stored it, false when it failed
/// (what was stored before is kept).
#[cfg(feature = "native")]
pub type Stored = Pin<Box<dyn Future<Output = bool> + Send>>;
/// A write's answer: true once the host stored it, false when it failed
/// (what was stored before is kept).
#[cfg(not(feature = "native"))]
pub type Stored = Pin<Box<dyn Future<Output = bool>>>;

/// Reading and replacing the app's small files a host keeps.
pub trait FileHost: Send + Sync {
    /// What `path` holds: the last write's content, or what was stored at
    /// launch. `None` for no file.
    fn read(&self, path: &Path) -> Option<Vec<u8>>;

    /// Stores `content` at `path`; `None` removes the file. Reads answer
    /// `content` from now on; once the write failed, what was stored
    /// before again, unless a newer write followed.
    fn write(&self, path: &Path, content: Option<Vec<u8>>) -> Stored;
}
