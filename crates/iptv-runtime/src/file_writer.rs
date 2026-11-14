//! The app's small files (settings, each playlist's last channel) are
//! written by one background thread, never on the UI thread: a zap or a
//! settings change only queues the new content.
//!
//! - **Latest wins.** A file with a write still queued takes the newer
//!   content in its place; the older one is never written. Every caller
//!   waiting on either hears the result of the write that happened.
//! - **Atomic and private.** Content goes to a temporary sibling
//!   (owner-only on Unix), is synced, renamed over the file, and the
//!   directory is synced, so a crash leaves the old file or the new one.
//! - **Read your writes.** [`read`] answers queued content before the disk.
//! - **Flushed on the way out.** The app's teardown, its move to the
//!   background and the Android activity's `onStop` wait (bounded) for
//!   queued writes ([`flush`]).
//!
//! Logs name neither paths nor content.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use futures_channel::oneshot;

/// Why a write did not complete as asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// Nothing was published; the previous file is still in place.
    Storage(io::ErrorKind),
    /// Published, but the directory could not be synced: the new file is
    /// what a restart reads unless the device loses power first.
    Durability(io::ErrorKind),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(kind) => write!(f, "could not write ({kind})"),
            Self::Durability(kind) => write!(f, "written, but not synced ({kind})"),
        }
    }
}

type Reply = oneshot::Sender<Result<(), WriteError>>;

/// What to leave at a path: bytes, or no file.
type Content = Option<Vec<u8>>;

struct Entry {
    content: Content,
    waiters: Vec<Reply>,
}

#[derive(Default)]
struct Queue {
    closed: bool,
    entries: HashMap<PathBuf, Entry>,
    order: VecDeque<PathBuf>,
    /// The write in progress, still answered by [`read`].
    writing: Option<(PathBuf, Content)>,
}

struct Writer {
    queue: Mutex<Queue>,
    /// Work was queued.
    ready: Condvar,
    /// The queue ran empty.
    idle: Condvar,
}

#[derive(Default)]
pub(crate) struct Files {
    closed: std::sync::atomic::AtomicBool,
    writer: OnceLock<Arc<Writer>>,
}
impl Files {
    fn writer(&self) -> Arc<Writer> {
        let writer = self
            .writer
            .get_or_init(|| {
                let writer = Arc::new(Writer {
                    queue: Mutex::default(),
                    ready: Condvar::new(),
                    idle: Condvar::new(),
                });
                let work = writer.clone();
                std::thread::Builder::new()
                    .name("iptv-files".into())
                    .spawn(move || run(&work))
                    .expect("start the file writer");
                writer
            })
            .clone();
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            lock(&writer).closed = true;
            writer.ready.notify_all();
        }
        writer
    }
    pub(crate) fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(writer) = self.writer.get() {
            lock(writer).closed = true;
            writer.ready.notify_all();
        }
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        self.close();
    }
}
fn writer() -> Arc<Writer> {
    super::store_service::service().files.writer()
}

fn lock(writer: &Writer) -> MutexGuard<'_, Queue> {
    // Only this module's bookkeeping runs under the lock (no IO), so a
    // panic elsewhere cannot leave it half-updated.
    writer.queue.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Queues `bytes` for `path`; the future answers when they (or newer
/// content for the same path) are on disk. Dropping it does not cancel
/// the write.
pub(crate) fn write(
    path: PathBuf,
    bytes: Vec<u8>,
) -> impl Future<Output = Result<(), WriteError>> + 'static {
    submit(path, Some(bytes))
}

/// Queues the removal of `path` behind any write queued for it.
pub(crate) fn remove(path: PathBuf) -> impl Future<Output = Result<(), WriteError>> + 'static {
    submit(path, None)
}

fn submit(path: PathBuf, content: Content) -> impl Future<Output = Result<(), WriteError>> {
    let (reply, answer) = oneshot::channel();
    let writer = writer();
    {
        let mut queue = lock(&writer);
        if queue.closed {
            let _ = reply.send(Err(WriteError::Storage(io::ErrorKind::Interrupted)));
        } else if let Some(entry) = queue.entries.get_mut(&path) {
            entry.content = content;
            entry.waiters.push(reply);
        } else {
            queue.order.push_back(path.clone());
            queue.entries.insert(
                path,
                Entry {
                    content,
                    waiters: vec![reply],
                },
            );
        }
    }
    writer.ready.notify_one();
    async move {
        answer
            .await
            .unwrap_or(Err(WriteError::Storage(io::ErrorKind::Interrupted)))
    }
}

/// The content `path` will have: queued content first, else the file.
/// `None` for no file (or one that cannot be read).
pub(crate) fn read(path: &Path) -> Option<Vec<u8>> {
    let queued = {
        let writer = writer();
        let queue = lock(&writer);
        queue
            .entries
            .get(path)
            .map(|entry| entry.content.clone())
            .or_else(|| {
                queue
                    .writing
                    .as_ref()
                    .filter(|(writing, _)| writing == path)
                    .map(|(_, content)| content.clone())
            })
    };
    match queued {
        Some(content) => content,
        None => std::fs::read(path).ok(),
    }
}

/// Waits, at most `limit`, until every write queued so far is done.
pub(crate) fn flush(limit: Duration) -> bool {
    let writer = writer();
    let deadline = Instant::now() + limit;
    let mut queue = lock(&writer);
    while !queue.order.is_empty() || queue.writing.is_some() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        queue = writer
            .idle
            .wait_timeout(queue, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    true
}

fn run(writer: &Writer) {
    loop {
        let (path, content, waiters) = {
            let mut queue = lock(writer);
            loop {
                if let Some(path) = queue.order.pop_front()
                    && let Some(entry) = queue.entries.remove(&path)
                {
                    queue.writing = Some((path.clone(), entry.content.clone()));
                    break (path, entry.content, entry.waiters);
                }
                writer.idle.notify_all();
                if queue.closed {
                    return;
                }
                queue = writer
                    .ready
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let result = std::panic::catch_unwind(|| match &content {
            Some(bytes) => publish(&path, bytes, sync_directory),
            None => unlink(&path),
        })
        .unwrap_or(Err(WriteError::Storage(io::ErrorKind::Other)));
        if let Err(error) = result {
            log::warn!("iptv files: a small file {error}");
        }
        lock(writer).writing = None;
        for waiter in waiters {
            let _ = waiter.send(result);
        }
    }
}

fn sync_directory(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Writes `bytes` to `path` through a synced temporary sibling, then syncs
/// the directory with `sync_directory`.
pub(crate) fn publish(
    path: &Path,
    bytes: &[u8],
    sync_directory: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), WriteError> {
    let publish = || -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = temporary(path);
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    };
    publish().map_err(|e| WriteError::Storage(e.kind()))?;
    // The rename has published the new file. Failure here means uncertain
    // crash durability, not that the previous content remains on disk.
    if let Some(dir) = path.parent() {
        sync_directory(dir).map_err(|e| WriteError::Durability(e.kind()))?;
    }
    Ok(())
}

fn unlink(path: &Path) -> Result<(), WriteError> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(WriteError::Storage(e.kind())),
    }
    if let Some(dir) = path.parent() {
        sync_directory(dir).map_err(|e| WriteError::Durability(e.kind()))?;
    }
    Ok(())
}

/// `settings.json` → `settings.json.tmp`.
fn temporary(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn the_latest_content_wins_and_reads_see_it_before_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("last_channel-1");
        let first = write(path.clone(), b"synthetic:1".to_vec());
        let second = write(path.clone(), b"synthetic:2".to_vec());
        assert_eq!(read(&path).as_deref(), Some(&b"synthetic:2"[..]));
        assert_eq!(block_on(first), Ok(()));
        assert_eq!(block_on(second), Ok(()));
        assert!(flush(Duration::from_secs(5)));
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic:2");
        assert!(!temporary(&path).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let removed = remove(path.clone());
        assert_eq!(read(&path), None, "a queued removal reads as no file");
        assert_eq!(block_on(removed), Ok(()));
        assert!(!path.exists());
        assert_eq!(block_on(remove(path.clone())), Ok(()), "already gone");
    }

    #[test]
    fn a_failed_write_keeps_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(block_on(write(path.clone(), b"{}".to_vec())), Ok(()));
        // A directory where the temporary file goes: nothing is published.
        std::fs::create_dir(temporary(&path)).unwrap();
        let failed = block_on(write(path.clone(), b"{\"next\":1}".to_vec()));
        assert!(matches!(failed, Err(WriteError::Storage(_))), "{failed:?}");
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        assert_eq!(read(&path).as_deref(), Some(&b"{}"[..]));
    }

    #[test]
    fn a_directory_sync_failure_still_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let result = publish(&path, b"{}", |_| {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "synthetic directory sync failure",
            ))
        });
        assert_eq!(
            result,
            Err(WriteError::Durability(io::ErrorKind::Unsupported))
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }

    #[test]
    fn many_queued_writes_flush_in_order_per_file() {
        let dir = tempfile::tempdir().unwrap();
        for round in 0..50 {
            for file in 0..4 {
                drop(write(
                    dir.path().join(format!("file-{file}")),
                    format!("{round}").into_bytes(),
                ));
            }
        }
        assert!(flush(Duration::from_secs(10)));
        for file in 0..4 {
            assert_eq!(
                std::fs::read_to_string(dir.path().join(format!("file-{file}"))).unwrap(),
                "49"
            );
        }
    }
}
