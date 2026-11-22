//! The app's small files (settings, each playlist's last channel) are
//! written by one background worker of the execution port, never on the UI
//! thread: a zap or a settings change only queues the new content. A host
//! that keeps the files itself (a browser, [`crate::file_host`]) is handed
//! each write instead, and answers reads; a host with neither has no file
//! writer: writes answer that nothing was published.
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

use crate::time::Instant;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

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
    #[cfg(feature = "test-support")]
    pause: Mutex<Option<Arc<crate::testing::GateState>>>,
}

pub(crate) struct Files {
    closed: std::sync::atomic::AtomicBool,
    writer: OnceLock<Arc<Writer>>,
    execution: Arc<dyn crate::execution::Execution>,
    /// Keeps the files in place of the writer and the disk.
    host: Option<Arc<dyn crate::file_host::FileHost>>,
}
impl Files {
    pub(crate) fn new(
        execution: Arc<dyn crate::execution::Execution>,
        host: Option<Arc<dyn crate::file_host::FileHost>>,
    ) -> Self {
        Self {
            closed: Default::default(),
            writer: OnceLock::new(),
            execution,
            host,
        }
    }

    /// Writes can be saved: by the host that keeps the files, or by the
    /// file writer, which needs a worker.
    pub(crate) fn can_write(&self) -> bool {
        self.host.is_some() || self.execution.has_workers()
    }

    fn writer(&self) -> Arc<Writer> {
        let writer = self
            .writer
            .get_or_init(|| {
                let writer = Arc::new(Writer {
                    queue: Mutex::default(),
                    ready: Condvar::new(),
                    idle: Condvar::new(),
                    #[cfg(feature = "test-support")]
                    pause: Mutex::default(),
                });
                let work = writer.clone();
                if let Err(error) = self
                    .execution
                    .worker("iptv-files".into(), Box::new(move || run(&work)))
                {
                    // Every write answers that nothing was published.
                    log::error!("iptv files: cannot start the file writer: {error}");
                    lock(&writer).closed = true;
                }
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
fn lock(writer: &Writer) -> MutexGuard<'_, Queue> {
    // Only this module's bookkeeping runs under the lock (no IO), so a
    // panic elsewhere cannot leave it half-updated.
    writer.queue.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Files {
    #[cfg(feature = "test-support")]
    pub(crate) fn hold_write(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        *self.writer().pause.lock().unwrap() = Some(gate.0.clone());
        gate
    }
    /// Queues `bytes` for `path`; the future answers when they (or newer
    /// content for the same path) are on disk. Dropping it does not cancel
    /// the write.
    pub(crate) fn write(
        &self,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<(), WriteError>> + 'static {
        self.submit(path, Some(bytes))
    }

    /// Queues the removal of `path` behind any write queued for it.
    pub(crate) fn remove(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<(), WriteError>> + 'static {
        self.submit(path, None)
    }

    fn submit(
        &self,
        path: PathBuf,
        content: Content,
    ) -> impl Future<Output = Result<(), WriteError>> + 'static {
        enum Answer {
            Host(crate::file_host::Stored),
            Writer(oneshot::Receiver<Result<(), WriteError>>),
            Closed,
        }
        let answer = if let Some(host) = &self.host {
            if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                Answer::Closed
            } else {
                Answer::Host(host.write(&path, content))
            }
        } else {
            Answer::Writer(self.queue(path, content))
        };
        async move {
            match answer {
                Answer::Host(stored) => match stored.await {
                    true => Ok(()),
                    false => Err(WriteError::Storage(io::ErrorKind::Other)),
                },
                Answer::Writer(answer) => answer
                    .await
                    .unwrap_or(Err(WriteError::Storage(io::ErrorKind::Interrupted))),
                Answer::Closed => Err(WriteError::Storage(io::ErrorKind::Interrupted)),
            }
        }
    }

    /// Queues `content` for `path` on the file writer.
    fn queue(&self, path: PathBuf, content: Content) -> oneshot::Receiver<Result<(), WriteError>> {
        let (reply, answer) = oneshot::channel();
        let writer = self.writer();
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
        answer
    }

    /// The content `path` will have: queued content first, else the file.
    /// `None` for no file (or one that cannot be read).
    pub(crate) fn read(&self, path: &Path) -> Option<Vec<u8>> {
        if let Some(host) = &self.host {
            return host.read(path);
        }
        // Nothing was ever queued: no writer thread is started for a read.
        let queued = self.writer.get().and_then(|writer| {
            let queue = lock(writer);
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
        });
        match queued {
            Some(content) => content,
            None => std::fs::read(path).ok(),
        }
    }

    /// Waits, at most `limit`, until every write queued so far is done.
    /// With a host that keeps the files there is nothing to wait for: it
    /// was handed every write, in order, and one thread cannot wait for
    /// its answers.
    pub(crate) fn flush(&self, limit: Duration) -> bool {
        let Some(writer) = self.writer.get() else {
            return true;
        };
        let deadline = Instant::now() + limit;
        let mut queue = lock(writer);
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
        #[cfg(feature = "test-support")]
        {
            let pause = writer.pause.lock().unwrap().take();
            if let Some(pause) = pause {
                pause.pause();
            }
        }
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

#[cfg(all(test, feature = "native"))]
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
        let files = Files::new(Arc::new(crate::native::execution::NativeExecution), None);
        let path = dir.path().join("nested").join("last_channel-1");
        let first = files.write(path.clone(), b"synthetic:1".to_vec());
        let second = files.write(path.clone(), b"synthetic:2".to_vec());
        assert_eq!(files.read(&path).as_deref(), Some(&b"synthetic:2"[..]));
        assert_eq!(block_on(first), Ok(()));
        assert_eq!(block_on(second), Ok(()));
        assert!(files.flush(Duration::from_secs(5)));
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
        let removed = files.remove(path.clone());
        assert_eq!(files.read(&path), None, "a queued removal reads as no file");
        assert_eq!(block_on(removed), Ok(()));
        assert!(!path.exists());
        assert_eq!(block_on(files.remove(path.clone())), Ok(()), "already gone");
    }

    #[test]
    fn a_failed_write_keeps_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(Arc::new(crate::native::execution::NativeExecution), None);
        let path = dir.path().join("settings.json");
        assert_eq!(block_on(files.write(path.clone(), b"{}".to_vec())), Ok(()));
        // A directory where the temporary file goes: nothing is published.
        std::fs::create_dir(temporary(&path)).unwrap();
        let failed = block_on(files.write(path.clone(), b"{\"next\":1}".to_vec()));
        assert!(matches!(failed, Err(WriteError::Storage(_))), "{failed:?}");
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        assert_eq!(files.read(&path).as_deref(), Some(&b"{}"[..]));
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
        let files = Files::new(Arc::new(crate::native::execution::NativeExecution), None);
        for round in 0..50 {
            for file in 0..4 {
                drop(files.write(
                    dir.path().join(format!("file-{file}")),
                    format!("{round}").into_bytes(),
                ));
            }
        }
        assert!(files.flush(Duration::from_secs(10)));
        for file in 0..4 {
            assert_eq!(
                std::fs::read_to_string(dir.path().join(format!("file-{file}"))).unwrap(),
                "49"
            );
        }
    }
}
