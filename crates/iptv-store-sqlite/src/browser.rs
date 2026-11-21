//! The browser's storage: SQLite files held in memory and persisted by the
//! host in 64 KiB chunks ([`CHUNK_BYTES`]).
//!
//! A browser TV has no file system SQLite can use, and writing the whole
//! database to IndexedDB after every change takes seconds. [`BrowserFiles`]
//! is a memory VFS (on `rsqlite-vfs`, the layer under `sqlite-wasm-rs`)
//! that records which chunks of the database file each write changed. After
//! a commit the host takes them ([`Store::take_changed_chunks`]): a take is
//! a snapshot of those chunks and the file's size at that commit, which
//! later commits cannot change. The host writes them in one IndexedDB
//! transaction and then acknowledges the take ([`ChangedChunks::saved`]);
//! until then no other take is given out, and a take dropped unacknowledged
//! (its write failed) returns its chunks to the next one. So storage only
//! ever holds the state of one commit: a crash can lose the last changes,
//! never mix two commits. At launch the host restores the file from its
//! chunks before opening ([`BrowserFiles::restore_size`],
//! [`BrowserFiles::restore_chunk`]); the store refuses to open while a
//! chunk is missing.
//!
//! Three kinds of file share one namespace:
//!
//! - **The database** ([`Store::open_browser`]), the only tracked file. Only
//!   its store opens it: another connection (a stage's, an `ATTACH`) is
//!   refused, so nothing writes it behind the store's back.
//! - **The publish copy** ([`Store::begin_publish`]): a long catalog or guide
//!   replacement runs on it in slices while reads continue on the database,
//!   and it replaces the database when the replacement committed
//!   ([`Store::finish_publish`]). Chunks are shared until written
//!   (copy-on-write), so the copy costs only what the publish changes, and
//!   the swap marks only chunks that differ as changed. While a copy exists
//!   the database refuses write locks; dropping the copy (or a job that owns
//!   it) abandons the publish and lifts that.
//! - **Scratch files**: staging databases ([`crate::CatalogStage`],
//!   [`crate::EpgStage`]) opened through [`Store::open_staging_connection`],
//!   and SQLite's temporary files. A scratch file disappears when its last
//!   connection closes, as an unlinked file does natively.
//!
//! The store holds no JavaScript: the host ABI that moves chunks to and from
//! IndexedDB belongs to the app's browser adapter (`iptv-web`). Storage is
//! not encrypted (`docs/security.md`).
//!
//! Single-threaded by construction: `wasm32-unknown-unknown` without the
//! atomics feature has one thread, and `sqlite-wasm-rs` builds SQLite with
//! `SQLITE_THREADSAFE=0`. Locks still matter between connections on that
//! thread (a stage's own connection and its attachment to a publish), so
//! files keep SQLite's lock levels.
#![allow(unsafe_code)]

#[cfg(target_feature = "atomics")]
compile_error!(
    "the browser VFS assumes one thread: wasm32 with atomics (threads) is not supported"
);

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rsqlite_vfs::{
    AccessMode, LockLevel, OpenOptions as VfsOpenOptions, OpenRequest, OpenedFile, SQLiteIoMethods,
    SQLiteVfs, SyncOptions, VfsError, VfsErrorCode, VfsFile, VfsResult, VfsStore,
};
use rusqlite::{Connection, OpenFlags};

use crate::{OpenOptions, Result, Store, StoreError};

/// The unit the host persists: the database file is written and restored
/// in chunks of this many bytes (the last one may be shorter).
pub const CHUNK_BYTES: usize = 64 * 1024;

/// The database's name in the namespace.
const DATABASE: &str = "patina.sqlite";
/// The publish copy's name.
const PUBLISH_COPY: &str = "patina-publish.sqlite";
/// The largest database a restore accepts: far above the TV's storage
/// quota (about 92 MB), far below what a corrupt size would ask for.
pub const MAX_DATABASE_BYTES: u64 = 1 << 30;
/// SQLite's smallest page: a database file is a whole number of pages.
const MIN_PAGE_BYTES: u64 = 512;

type Chunk = Arc<[u8; CHUNK_BYTES]>;

fn zero_chunk() -> Chunk {
    Arc::new([0; CHUNK_BYTES])
}

/// Whether `name` is a file only its own store may open: the database, the
/// publish copy, or a name SQLite would take for one of their journals.
fn reserved(name: &str) -> bool {
    [DATABASE, PUBLISH_COPY].into_iter().any(|base| {
        name.strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    })
}

/// The one VFS every browser store's files live under, registered once
/// for the life of the process (SQLite's VFS list is process-wide too):
/// a connection that outlives its store, even a `:memory:` one that never
/// opened a file, can still call it. Each [`BrowserFiles`] is a namespace
/// of it, `/patina-<id>/...`, dropped with its last clone.
const VFS: &str = "patina-chunks";
const PREFIX: &str = "/patina-";

fn chunk_count(len: u64) -> usize {
    usize::try_from(len.div_ceil(CHUNK_BYTES as u64)).unwrap_or(usize::MAX)
}

/// A file's bytes as shared chunks: copying a file clones pointers, and a
/// write copies only the chunk it changes.
#[derive(Clone, Default)]
struct Data {
    chunks: Vec<Chunk>,
    len: u64,
}

impl Data {
    fn read(&self, buf: &mut [u8], offset: u64) -> usize {
        if offset >= self.len {
            return 0;
        }
        let end = (offset + buf.len() as u64).min(self.len);
        let mut at = offset;
        while at < end {
            let index = (at / CHUNK_BYTES as u64) as usize;
            let within = (at % CHUNK_BYTES as u64) as usize;
            let n = (CHUNK_BYTES - within).min((end - at) as usize);
            let out = (at - offset) as usize;
            buf[out..out + n].copy_from_slice(&self.chunks[index][within..within + n]);
            at += n as u64;
        }
        (end - offset) as usize
    }

    /// Writes `buf` at `offset`; returns the chunks whose stored copy it
    /// changed: those it wrote, and when it extends the file, also the old
    /// last chunk (now longer) and the zeroed gap.
    fn write(&mut self, buf: &[u8], offset: u64) -> Option<std::ops::RangeInclusive<u32>> {
        if buf.is_empty() {
            return None;
        }
        let end = offset + buf.len() as u64;
        let old_len = self.len;
        let needed = chunk_count(end);
        if needed > self.chunks.len() {
            self.chunks.resize_with(needed, zero_chunk);
        }
        let mut at = offset;
        while at < end {
            let index = (at / CHUNK_BYTES as u64) as usize;
            let within = (at % CHUNK_BYTES as u64) as usize;
            let n = (CHUNK_BYTES - within).min((end - at) as usize);
            let from = (at - offset) as usize;
            Arc::make_mut(&mut self.chunks[index])[within..within + n]
                .copy_from_slice(&buf[from..from + n]);
            at += n as u64;
        }
        self.len = self.len.max(end);
        let mut first = offset / CHUNK_BYTES as u64;
        if end > old_len {
            first = first.min(old_len / CHUNK_BYTES as u64);
        }
        let last = ((end - 1) / CHUNK_BYTES as u64) as u32;
        Some(first as u32..=last)
    }

    /// Cuts the file to `size` bytes; returns the chunk that now ends it,
    /// whose stored copy holds bytes past the end, if any.
    fn truncate(&mut self, size: u64) -> Option<u32> {
        if size >= self.len {
            return None;
        }
        self.chunks.truncate(chunk_count(size));
        let within = (size % CHUNK_BYTES as u64) as usize;
        self.len = size;
        if within == 0 {
            return None;
        }
        let last = self.chunks.len() - 1;
        Arc::make_mut(&mut self.chunks[last])[within..].fill(0);
        Some(last as u32)
    }

    /// The bytes of chunk `index` (shorter for the last chunk).
    fn chunk(&self, index: usize) -> Option<&[u8]> {
        let chunk = self.chunks.get(index)?;
        let start = index as u64 * CHUNK_BYTES as u64;
        let len = (self.len - start).min(CHUNK_BYTES as u64) as usize;
        Some(&chunk[..len])
    }
}

/// SQLite's locks on one file, by handle.
#[derive(Default)]
struct Locks {
    /// Handles holding at least a shared lock.
    shared: u32,
    /// The handle holding the reserved, pending or exclusive lock.
    writer: Option<u64>,
    /// The writer is waiting for, or holds, exclusive access.
    exclusive: bool,
}

/// One file of the namespace.
#[derive(Default)]
struct FileState {
    data: Data,
    /// Chunks written since the host last took them; `Some` only for the
    /// database.
    changed: Option<BTreeSet<u32>>,
    /// A take of the database's chunks the host has not acknowledged.
    take_outstanding: bool,
    /// Chunks `restore_size` expects that `restore_chunk` has not given.
    missing: BTreeSet<u32>,
    /// A publish copy exists: the database grants no write lock.
    frozen: bool,
    locks: Locks,
    /// Removed from the namespace when its last connection closes.
    scratch: bool,
    /// SQLite handles open on it (a take's snapshot holds the file too, so
    /// the `Arc`'s count says nothing about connections).
    handles: u32,
}

type SharedFile = Arc<Mutex<FileState>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // One thread: a poisoned lock can only follow a panic, which aborts.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The files of one [`BrowserFiles`].
#[derive(Default)]
struct Namespace {
    files: Mutex<HashMap<String, SharedFile>>,
    /// The reserved file the store is opening its own connection on; any
    /// other open of a reserved file is refused.
    admitted: Mutex<Option<&'static str>>,
}

impl Namespace {
    fn file(&self, name: &str) -> Option<SharedFile> {
        lock(&self.files).get(name).cloned()
    }
}

/// What the VFS callbacks share: the live namespaces and SQLite's last
/// error.
#[derive(Default)]
struct Registry {
    namespaces: Mutex<HashMap<u32, std::sync::Weak<Namespace>>>,
    next_namespace: AtomicU32,
    /// Handle ids, for lock ownership.
    next_handle: AtomicU64,
    error: Mutex<Option<VfsError>>,
}

impl Registry {
    /// The namespace and local name of a VFS path `/patina-<id>/<name>`.
    fn resolve<'n>(&self, path: &'n str) -> VfsResult<(Arc<Namespace>, &'n str)> {
        let gone = || VfsError::new(VfsErrorCode::CantOpen, "no such browser store".into());
        let (id, name) = path
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.split_once('/'))
            .ok_or_else(gone)?;
        let id: u32 = id.parse().map_err(|_| gone())?;
        let namespace = lock(&self.namespaces)
            .get(&id)
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(gone)?;
        Ok((namespace, name))
    }
}

type AppData = Arc<Registry>;

/// An open file: SQLite reads and writes through it.
struct Handle {
    file: SharedFile,
    read_only: bool,
    id: u64,
    level: LockLevel,
}

fn busy(reason: &'static str) -> VfsError {
    VfsError::new(VfsErrorCode::Busy, reason.into())
}

fn read_only_error() -> VfsError {
    VfsError::new(VfsErrorCode::ReadOnly, "file opened read-only".into())
}

impl VfsFile for Handle {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        Ok(lock(&self.file).data.read(buf, offset))
    }

    fn write(&mut self, buf: &[u8], offset: u64) -> VfsResult<()> {
        if self.read_only {
            return Err(read_only_error());
        }
        let mut file = lock(&self.file);
        let touched = file.data.write(buf, offset);
        if let (Some(changed), Some(touched)) = (&mut file.changed, touched) {
            changed.extend(touched);
        }
        Ok(())
    }

    fn truncate(&mut self, size: u64) -> VfsResult<()> {
        if self.read_only {
            return Err(read_only_error());
        }
        let mut file = lock(&self.file);
        let last = file.data.truncate(size);
        if let Some(changed) = &mut file.changed {
            let count = chunk_count(size) as u32;
            changed.retain(|&index| index < count);
            changed.extend(last);
        }
        Ok(())
    }

    fn sync(&mut self, _options: SyncOptions) -> VfsResult<()> {
        Ok(())
    }

    fn size(&self) -> VfsResult<u64> {
        Ok(lock(&self.file).data.len)
    }

    // SQLite's lock levels, kept per file between the connections of the one
    // thread: a stage written by its own connection while a publish has it
    // attached, or the database while a publish copy exists, gets BUSY
    // instead of changing under a reader or being lost in the swap.
    fn lock(&mut self, level: LockLevel) -> VfsResult<()> {
        if level <= self.level {
            return Ok(());
        }
        let mut file = lock(&self.file);
        let id = self.id;
        let other_writer = file.locks.writer.is_some_and(|w| w != id);
        match level {
            LockLevel::None => {}
            LockLevel::Shared => {
                if other_writer && file.locks.exclusive {
                    return Err(busy("another connection is writing the file"));
                }
                file.locks.shared += 1;
            }
            LockLevel::Reserved => {
                if file.frozen {
                    return Err(busy("a publish copy of the database exists"));
                }
                if other_writer {
                    return Err(busy("another connection is writing the file"));
                }
                file.locks.writer = Some(id);
            }
            LockLevel::Pending | LockLevel::Exclusive => {
                // BEGIN EXCLUSIVE, VACUUM and exclusive locking mode go
                // straight here, past the reserved lock.
                if file.frozen {
                    return Err(busy("a publish copy of the database exists"));
                }
                if other_writer {
                    return Err(busy("another connection is writing the file"));
                }
                file.locks.writer = Some(id);
                file.locks.exclusive = true;
                if level == LockLevel::Exclusive && file.locks.shared > 1 {
                    // Pending: no new readers; this writer retries.
                    self.level = LockLevel::Pending;
                    return Err(busy("another connection is reading the file"));
                }
            }
        }
        self.level = level;
        Ok(())
    }

    fn unlock(&mut self, level: LockLevel) -> VfsResult<()> {
        if level >= self.level {
            return Ok(());
        }
        let mut file = lock(&self.file);
        if self.level >= LockLevel::Reserved && file.locks.writer == Some(self.id) {
            file.locks.writer = None;
            file.locks.exclusive = false;
        }
        if level == LockLevel::None && self.level >= LockLevel::Shared {
            file.locks.shared = file.locks.shared.saturating_sub(1);
        }
        self.level = level;
        Ok(())
    }

    fn check_reserved_lock(&self) -> VfsResult<bool> {
        Ok(lock(&self.file).locks.writer.is_some())
    }
}

struct ChunkStore;

impl VfsStore for ChunkStore {
    type File = Handle;
    type AppData = AppData;

    fn record_error(data: &AppData, error: VfsError) {
        *lock(&data.error) = Some(error);
    }

    fn last_error(data: &AppData) -> Option<VfsError> {
        lock(&data.error).clone()
    }

    fn open_file(data: &AppData, request: OpenRequest<'_>) -> VfsResult<OpenedFile<Handle>> {
        let options = request.options;
        let file = match request.filename {
            None => Arc::new(Mutex::new(FileState {
                scratch: true,
                ..FileState::default()
            })),
            Some(path) => {
                let (namespace, name) = data.resolve(path.path())?;
                if reserved(name) {
                    let mut admitted = lock(&namespace.admitted);
                    if *admitted != Some(name) {
                        return Err(VfsError::new(
                            VfsErrorCode::CantOpen,
                            "only its store opens the browser database".into(),
                        ));
                    }
                    *admitted = None;
                }
                let mut files = lock(&namespace.files);
                let file = match files.get(name) {
                    Some(_) if options.create() && options.exclusive() => {
                        return Err(VfsError::new(
                            VfsErrorCode::CantOpen,
                            "file already exists".into(),
                        ));
                    }
                    Some(file) => file.clone(),
                    None if options.create() => {
                        let file = Arc::new(Mutex::new(FileState {
                            changed: (name == DATABASE).then(BTreeSet::new),
                            scratch: !reserved(name),
                            ..FileState::default()
                        }));
                        files.insert(name.to_owned(), file.clone());
                        file
                    }
                    None => {
                        return Err(VfsError::new(VfsErrorCode::CantOpen, "no such file".into()));
                    }
                };
                if options.delete_on_close() {
                    // Unlinked at open: the handle keeps the data until it closes.
                    files.remove(name);
                }
                file
            }
        };
        lock(&file).handles += 1;
        Ok(OpenedFile {
            file: Handle {
                file,
                read_only: options.access() == rsqlite_vfs::OpenAccess::ReadOnly,
                id: data.next_handle.fetch_add(1, Ordering::Relaxed),
                level: LockLevel::None,
            },
            access: options.access(),
        })
    }

    fn close_file(
        data: &AppData,
        path: Option<&str>,
        mut handle: Handle,
        _options: VfsOpenOptions,
    ) -> VfsResult<()> {
        handle.unlock(LockLevel::None)?;
        let (scratch, last) = {
            let mut file = lock(&handle.file);
            file.handles = file.handles.saturating_sub(1);
            (file.scratch, file.handles == 0)
        };
        if scratch
            && last
            && let Some(path) = path
            && let Ok((namespace, name)) = data.resolve(path)
        {
            let mut files = lock(&namespace.files);
            if files
                .get(name)
                .is_some_and(|file| Arc::ptr_eq(file, &handle.file))
            {
                files.remove(name);
            }
        }
        Ok(())
    }

    fn access(data: &AppData, path: &str, _mode: AccessMode) -> VfsResult<bool> {
        Ok(match data.resolve(path) {
            // The database's journals never exist (its journal is in memory).
            Ok((namespace, name)) => lock(&namespace.files).contains_key(name),
            Err(_) => false,
        })
    }

    fn full_pathname(_data: &AppData, name: &str) -> VfsResult<String> {
        Ok(name.to_owned())
    }

    fn delete_file(data: &AppData, path: &str, _sync_dir: bool) -> VfsResult<()> {
        let (namespace, name) = data.resolve(path)?;
        if reserved(name) {
            return Err(VfsError::new(
                VfsErrorCode::IoDelete,
                "the browser database is not deleted through SQLite".into(),
            ));
        }
        lock(&namespace.files).remove(name);
        Ok(())
    }
}

struct ChunkIo;

impl SQLiteIoMethods for ChunkIo {
    type Store = ChunkStore;
}

struct ChunkVfs;

impl SQLiteVfs<ChunkIo> for ChunkVfs {
    type Os = sqlite_wasm_rs::WasmOsCallback;

    fn os(_data: &AppData) -> &Self::Os {
        &sqlite_wasm_rs::WasmOsCallback
    }
}

/// The registry of the process's one browser VFS, registered on first use.
fn registry() -> Result<&'static AppData> {
    static REGISTRY: std::sync::OnceLock<Option<AppData>> = std::sync::OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let registry = AppData::default();
            // SAFETY: the method table, file type and app data are this
            // module's own and agree; the name is this module's; the one
            // thread registers it once. `into_raw` keeps it for the life of
            // the process, so no connection can outlive it.
            let registered = unsafe {
                rsqlite_vfs::register_vfs::<ChunkIo, ChunkVfs>(VFS, registry.clone(), false)
            };
            registered.ok().map(|registration| {
                let _ = registration.into_raw();
                registry
            })
        })
        .as_ref()
        .ok_or_else(|| unavailable("could not register the browser storage"))
}

struct Inner {
    id: u32,
    namespace: Arc<Namespace>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Free the files a closed store left. A staging connection that
        // outlives every store keeps its own file until it closes.
        lock(&self.namespace.files).clear();
        if let Ok(registry) = registry() {
            lock(&registry.namespaces).remove(&self.id);
        }
    }
}

/// The in-memory files of one browser store: its database, a publish copy
/// and staging databases. Cheap to clone; every clone is the same files.
#[derive(Clone)]
pub struct BrowserFiles(Arc<Inner>);

impl std::fmt::Debug for BrowserFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserFiles")
            .field("namespace", &self.0.id)
            .finish_non_exhaustive()
    }
}

fn unavailable(reason: &'static str) -> StoreError {
    StoreError::Unavailable(reason)
}

impl BrowserFiles {
    /// New, empty files: a namespace of the browser VFS.
    pub fn new() -> Result<Self> {
        let registry = registry()?;
        let id = registry.next_namespace.fetch_add(1, Ordering::Relaxed);
        let namespace = Arc::new(Namespace::default());
        lock(&registry.namespaces).insert(id, Arc::downgrade(&namespace));
        Ok(Self(Arc::new(Inner { id, namespace })))
    }

    /// The VFS path of file `name` of these files.
    pub(crate) fn path(&self, name: &str) -> String {
        format!("{PREFIX}{}/{}", self.0.id, name.trim_start_matches('/'))
    }

    /// The local name of a caller's path (a stage's).
    fn local(path: &std::path::Path) -> Option<&str> {
        path.to_str().map(|name| name.trim_start_matches('/'))
    }

    /// How many scratch files (stages) these files hold: none once every
    /// stage is dropped.
    pub fn scratch_files(&self) -> usize {
        lock(&self.0.namespace.files)
            .values()
            .filter(|file| lock(file).scratch)
            .count()
    }

    fn file(&self, name: &str) -> Option<SharedFile> {
        self.0.namespace.file(name)
    }

    fn has_publish_copy(&self) -> bool {
        lock(&self.0.namespace.files).contains_key(PUBLISH_COPY)
    }

    fn database(&self) -> Option<SharedFile> {
        self.file(DATABASE)
    }

    /// Whether `file` is open on some connection.
    fn is_open(file: &SharedFile) -> bool {
        lock(file).handles > 0
    }

    fn database_is_open(&self) -> bool {
        lock(&self.0.namespace.files)
            .get(DATABASE)
            .is_some_and(Self::is_open)
    }

    /// Prepares the database file to be restored at launch: `size` bytes,
    /// every chunk of which [`Self::restore_chunk`] must then give before
    /// the store opens. Only before the store opens; replaces what the
    /// files held. `size` must be a whole number of SQLite pages (512
    /// bytes or more each) and at most [`MAX_DATABASE_BYTES`].
    pub fn restore_size(&self, size: u64) -> Result<()> {
        if self.database_is_open() {
            return Err(unavailable("the browser database is open"));
        }
        if self.has_publish_copy() {
            return Err(unavailable("a publish copy of the database exists"));
        }
        if size > MAX_DATABASE_BYTES || !size.is_multiple_of(MIN_PAGE_BYTES) {
            return Err(StoreError::InvalidData(format!(
                "a stored database of {size} bytes is not a SQLite database this store restores"
            )));
        }
        let count = chunk_count(size);
        // One shared zero chunk: each is copied when restored.
        let zero = zero_chunk();
        let file = FileState {
            data: Data {
                chunks: vec![zero; count],
                len: size,
            },
            changed: Some(BTreeSet::new()),
            missing: (0..count as u32).collect(),
            ..FileState::default()
        };
        lock(&self.0.namespace.files).insert(DATABASE.to_owned(), Arc::new(Mutex::new(file)));
        Ok(())
    }

    /// Copies chunk `index` of the stored database in (after
    /// [`Self::restore_size`], before the store opens). `bytes` is the whole
    /// chunk: [`CHUNK_BYTES`], or the rest of the file for the last one.
    pub fn restore_chunk(&self, index: u32, bytes: &[u8]) -> Result<()> {
        if self.database_is_open() {
            return Err(unavailable("the browser database is open"));
        }
        if self.has_publish_copy() {
            return Err(unavailable("a publish copy of the database exists"));
        }
        let file = self
            .database()
            .ok_or_else(|| unavailable("restore_size comes before restore_chunk"))?;
        let mut file = lock(&file);
        let start = u64::from(index) * CHUNK_BYTES as u64;
        let expected = file.data.chunk(index as usize).map(<[u8]>::len);
        if expected != Some(bytes.len()) {
            return Err(StoreError::InvalidData(format!(
                "stored chunk {index} has {} bytes, the database {}",
                bytes.len(),
                expected.map_or_else(|| "ends before it".to_owned(), |n| format!("expects {n}"))
            )));
        }
        file.data.write(bytes, start);
        file.missing.remove(&index);
        Ok(())
    }

    /// Opens a connection on `name`. A reserved file (the database, the
    /// publish copy) admits this one open only.
    fn open(&self, name: &str) -> Result<Connection> {
        crate::initialize_sqlite()?;
        registry()?;
        let reserved_name = [DATABASE, PUBLISH_COPY].into_iter().find(|&n| n == name);
        *lock(&self.0.namespace.admitted) = reserved_name;
        let opened = Connection::open_with_flags_and_vfs(
            self.path(name),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            VFS,
        );
        *lock(&self.0.namespace.admitted) = None;
        Ok(opened?)
    }

    /// Opens a scratch database (a stage) at a caller's `path` in these
    /// files. It never waits for a lock: on one thread nothing could release
    /// one meanwhile (rusqlite's default would block the app for 5 s).
    pub(crate) fn open_scratch(&self, path: &std::path::Path) -> Result<Connection> {
        let name = Self::local(path)
            .filter(|name| !name.is_empty() && !reserved(name))
            .ok_or_else(|| unavailable("a stage cannot use the database's names"))?;
        let conn = self.open(name)?;
        conn.busy_timeout(std::time::Duration::ZERO)?;
        // No journal files: a stage that outlives its store's files can
        // still write (and the stages set their own journal anyway).
        let _mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "MEMORY", |r| r.get(0))?;
        Ok(conn)
    }

    /// The VFS path to attach a caller's scratch `path` by, when it exists
    /// in these files.
    pub(crate) fn attachable(&self, path: &std::path::Path) -> Option<String> {
        let name = Self::local(path)?;
        (!reserved(name) && lock(&self.0.namespace.files).contains_key(name))
            .then(|| self.path(name))
    }

    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Which file of its [`BrowserFiles`] a store has open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Database,
    PublishCopy,
}

/// A publish copy's claim on its files: dropping it unfinished abandons the
/// publish (removes the copy, lets the database take writes again).
struct PublishGuard {
    files: BrowserFiles,
    copy: SharedFile,
    /// The database this publish copied: the one it unfreezes, and the only
    /// one it may replace (a restore since gives the files another).
    database: SharedFile,
    /// A replacement on the copy committed some of its steps but not its
    /// last: the copy holds half a publish and must not be swapped in.
    partial: bool,
    armed: bool,
}

impl PublishGuard {
    /// Ends the publish: removes the copy (if it is still this one) and
    /// unfreezes the database.
    fn release(&mut self) {
        if !std::mem::take(&mut self.armed) {
            return;
        }
        {
            let mut names = lock(&self.files.0.namespace.files);
            if names
                .get(PUBLISH_COPY)
                .is_some_and(|file| Arc::ptr_eq(file, &self.copy))
            {
                names.remove(PUBLISH_COPY);
            }
        }
        lock(&self.database).frozen = false;
    }
}

impl Drop for PublishGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// A browser store's link to its files.
pub(crate) struct Link {
    files: BrowserFiles,
    role: Role,
    /// The publish this copy belongs to (publish copies only).
    publish: Option<PublishGuard>,
}

impl Link {
    pub(crate) fn files(&self) -> &BrowserFiles {
        &self.files
    }
}

/// One take of the database's changed chunks: a snapshot, as of the commit
/// it was taken after, of those chunks and of the file's size. Later
/// commits do not change it.
///
/// The host writes [`Self::chunks`] in one IndexedDB transaction, deletes
/// stored chunks at or past `ceil(size / CHUNK_BYTES)`, then calls
/// [`Self::saved`]. Until then the store gives out no other take. Dropping
/// a take without `saved` (its write failed) returns its chunks: the next
/// take includes them again.
#[must_use = "acknowledge a written take with `saved`; dropping it returns its chunks"]
pub struct ChangedChunks {
    database: SharedFile,
    size: u64,
    chunks: Vec<(u32, Chunk)>,
    settled: bool,
}

impl std::fmt::Debug for ChangedChunks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangedChunks")
            .field("size", &self.size)
            .field("indices", &self.indices().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl ChangedChunks {
    /// The database file's size in bytes at the take.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Changed chunk indices, ascending.
    pub fn indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.chunks.iter().map(|(index, _)| *index)
    }

    /// Each changed chunk's index and bytes: [`CHUNK_BYTES`], fewer for the
    /// last chunk of the file.
    pub fn chunks(&self) -> impl Iterator<Item = (u32, &[u8])> + '_ {
        self.chunks.iter().map(|(index, chunk)| {
            let start = u64::from(*index) * CHUNK_BYTES as u64;
            let len = (self.size - start).min(CHUNK_BYTES as u64) as usize;
            (*index, &chunk[..len])
        })
    }

    /// The host wrote this take: the next take starts from here.
    pub fn saved(mut self) {
        self.settled = true;
        lock(&self.database).take_outstanding = false;
    }
}

impl Drop for ChangedChunks {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let mut file = lock(&self.database);
        file.take_outstanding = false;
        let count = chunk_count(file.data.len) as u32;
        let returned = self
            .chunks
            .iter()
            .map(|(index, _)| *index)
            .filter(|&i| i < count);
        if let Some(changed) = &mut file.changed {
            changed.extend(returned);
        }
    }
}

/// The browser connection's settings: the rollback journal stays in memory
/// (persistence is the host's chunk write after a commit, so there is no
/// file to recover from), no fsync, temporary data in memory, freed pages
/// zeroed (a deleted credential must not stay in a saved chunk), and no
/// busy waiting (on one thread, nothing can release a lock meanwhile).
pub(crate) fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(std::time::Duration::ZERO)?;
    let _mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "MEMORY", |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "secure_delete", true)?;
    // The size a restore accepts is also the size the database may grow to.
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
    let pages = MAX_DATABASE_BYTES as i64 / page_size.max(1);
    conn.pragma_update(None, "max_page_count", pages)?;
    Ok(())
}

impl Store {
    /// Opens (creating if empty) the browser store in `files` and migrates
    /// it. Restore its chunks first: opening refuses a restore with a chunk
    /// missing. One store per files: a second open while one is open fails.
    pub fn open_browser(files: &BrowserFiles) -> Result<Self> {
        Self::open_browser_with(files, &OpenOptions::default())
    }

    pub fn open_browser_with(files: &BrowserFiles, options: &OpenOptions) -> Result<Self> {
        if files.database_is_open() {
            return Err(unavailable("the browser database is already open"));
        }
        if let Some(database) = files.database()
            && let Some(&index) = lock(&database).missing.first()
        {
            return Err(StoreError::InvalidData(format!(
                "chunk {index} of the stored database was not restored"
            )));
        }
        Self::open_role(files, Role::Database, options)
    }

    fn open_role(files: &BrowserFiles, role: Role, options: &OpenOptions) -> Result<Self> {
        let name = match role {
            Role::Database => DATABASE,
            Role::PublishCopy => PUBLISH_COPY,
        };
        let conn = files.open(name)?;
        let mut store = Self::init_with(conn, options, crate::Journal::Browser)?;
        store.browser = Some(Link {
            files: files.clone(),
            role,
            publish: None,
        });
        Ok(store)
    }

    fn link(&self) -> Result<&Link> {
        self.browser
            .as_ref()
            .ok_or_else(|| unavailable("not a browser store"))
    }

    fn database_link(&self) -> Result<&Link> {
        let link = self.link()?;
        if link.role != Role::Database {
            return Err(unavailable("a publish copy has no persisted chunks"));
        }
        Ok(link)
    }

    fn database_file(&self) -> Result<SharedFile> {
        self.database_link()?
            .files
            .database()
            .ok_or_else(|| unavailable("the browser database is missing"))
    }

    /// Takes the database chunks changed since the last saved take, as a
    /// snapshot for the host to write ([`ChangedChunks`]). Only between
    /// transactions, when the file holds the last commit and nothing
    /// uncommitted, and only once the previous take was saved or dropped.
    pub fn take_changed_chunks(&self) -> Result<ChangedChunks> {
        let database = self.database_file()?;
        if !self.conn.is_autocommit() {
            return Err(unavailable("a transaction is open"));
        }
        let mut file = lock(&database);
        if file.take_outstanding {
            return Err(unavailable("the previous take of chunks is not saved yet"));
        }
        let indices = file
            .changed
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        let chunks = indices
            .into_iter()
            .filter_map(|index| Some((index, file.data.chunks.get(index as usize)?.clone())))
            .collect();
        file.take_outstanding = true;
        let size = file.data.len;
        drop(file);
        Ok(ChangedChunks {
            database,
            size,
            chunks,
            settled: false,
        })
    }

    /// The memory files of a browser store.
    pub fn browser_files(&self) -> Option<&BrowserFiles> {
        self.browser.as_ref().map(Link::files)
    }

    /// Whether this is a publish copy: a replacement on it commits each step
    /// (`replacement.rs`), as rolling back is dropping the copy.
    pub(crate) fn is_publish_copy(&self) -> bool {
        self.browser
            .as_ref()
            .is_some_and(|link| link.role == Role::PublishCopy)
    }

    /// Marks this publish copy as holding part of a replacement (or no
    /// longer): `finish_publish` refuses a partial copy.
    pub(crate) fn set_partial(&mut self, partial: bool) {
        if let Some(guard) = self.browser.as_mut().and_then(|link| link.publish.as_mut()) {
            guard.partial = partial;
        }
    }

    /// Whether a publish copy exists ([`Self::begin_publish`]).
    pub fn is_publishing(&self) -> bool {
        self.database_file()
            .is_ok_and(|database| lock(&database).frozen)
    }

    /// Starts a publish on a copy of the database: returns a store on the
    /// copy, where a long replacement runs in slices
    /// ([`crate::CatalogReplacement`], [`crate::EpgReplacement`]) while this
    /// store keeps serving reads of the last commit.
    ///
    /// Until the copy is swapped in ([`Self::finish_publish`]) or abandoned
    /// ([`Self::abandon_publish`], or dropping the copy or a job that owns
    /// it), this database grants no write lock: a write here (SQLite reports
    /// it busy) would be lost in the swap. The caller holds writes back
    /// until the publish ends and runs them after it, in order, as the
    /// native writer queue does.
    pub fn begin_publish(&mut self) -> Result<Self> {
        let database = self.database_file()?;
        if !self.conn.is_autocommit() {
            return Err(unavailable("a transaction is open"));
        }
        let files = self.database_link()?.files.clone();
        let copy = {
            let database = lock(&database);
            if database.frozen {
                return Err(unavailable("a publish is already running"));
            }
            FileState {
                data: database.data.clone(),
                ..FileState::default()
            }
        };
        let copy = Arc::new(Mutex::new(copy));
        {
            let mut names = lock(&files.0.namespace.files);
            if names.get(PUBLISH_COPY).is_some_and(BrowserFiles::is_open) {
                return Err(unavailable("a publish copy is still open"));
            }
            names.insert(PUBLISH_COPY.to_owned(), copy.clone());
        }
        lock(&database).frozen = true;
        // From here the guard undoes the publish if anything fails.
        let guard = PublishGuard {
            files: files.clone(),
            copy,
            database,
            partial: false,
            armed: true,
        };
        let mut store = Self::open_role(
            &files,
            Role::PublishCopy,
            &OpenOptions {
                prefer_fts: self.fts,
                ..OpenOptions::default()
            },
        )?;
        store.clock = self.clock;
        if let Some(link) = &mut store.browser {
            link.publish = Some(guard);
        }
        Ok(store)
    }

    fn check_copy(&self, copy: &Self) -> Result<BrowserFiles> {
        let link = self.database_link()?;
        let theirs = copy.link()?;
        let database = link.files.database();
        let ours = theirs.publish.as_ref().is_some_and(|guard| {
            database
                .as_ref()
                .is_some_and(|database| Arc::ptr_eq(database, &guard.database))
        });
        if theirs.role != Role::PublishCopy || !link.files.same(&theirs.files) || !ours {
            return Err(unavailable("not this store's publish copy"));
        }
        Ok(link.files.clone())
    }

    /// Replaces the database with the publish copy, whose replacement has
    /// committed, and reopens this store on it. Marks the chunks that differ
    /// from the database's as changed. Writes may resume afterwards.
    ///
    /// On an error before the swap the publish is abandoned and the
    /// database is unchanged; if reopening on the swapped file fails, the
    /// swap is undone, so the database is unchanged then too.
    pub fn finish_publish(&mut self, mut copy: Self) -> Result<()> {
        let files = self.check_copy(&copy)?;
        if !copy.conn.is_autocommit() {
            return Err(unavailable("the publish has not committed"));
        }
        let mut guard = copy
            .browser
            .as_mut()
            .and_then(|link| link.publish.take())
            .ok_or_else(|| unavailable("the publish copy has ended"))?;
        // Closes the copy's connection; the guard still owns the publish.
        drop(copy);
        if guard.partial {
            return Err(unavailable("the replacement on the copy did not finish"));
        }
        let database = guard.database.clone();
        let new = std::mem::take(&mut lock(&guard.copy).data);
        let (old, changed_before) = {
            let mut database = lock(&database);
            let old = std::mem::replace(&mut database.data, new);
            (old, database.changed.clone())
        };
        let options = OpenOptions {
            prefer_fts: self.fts,
            ..OpenOptions::default()
        };
        // The copy was migrated by this build: opening only configures. This
        // store's old connection is idle (no transaction, no lock), so it can
        // stay open until the new one is.
        lock(&database).frozen = false;
        match Self::open_role(&files, Role::Database, &options) {
            Ok(reopened) => {
                self.conn = reopened.conn;
                let mut database = lock(&database);
                let differing = differing_chunks(&old, &database.data);
                let count = database.data.chunks.len() as u32;
                if let Some(changed) = &mut database.changed {
                    changed.retain(|&i| i < count);
                    changed.extend(differing);
                }
                drop(database);
                guard.armed = false;
                lock(&files.0.namespace.files).remove(PUBLISH_COPY);
                Ok(())
            }
            Err(e) => {
                let mut database = lock(&database);
                database.data = old;
                database.changed = changed_before;
                drop(database);
                // Removes the copy; the database already takes writes.
                guard.release();
                Err(e)
            }
        }
    }

    /// Drops a publish copy whose replacement failed or was canceled; the
    /// database is unchanged and accepts writes again. Dropping the copy
    /// does the same.
    pub fn abandon_publish(&mut self, copy: Self) -> Result<()> {
        self.check_copy(&copy)?;
        drop(copy);
        Ok(())
    }
}

/// The chunks of `new` whose stored copy (`old`'s) differs: other length,
/// or other bytes (shared chunks are equal).
fn differing_chunks(old: &Data, new: &Data) -> Vec<u32> {
    (0..new.chunks.len())
        .filter(|&i| match (old.chunk(i), new.chunk(i)) {
            (Some(before), Some(after)) => {
                before.len() != after.len()
                    || (!Arc::ptr_eq(&old.chunks[i], &new.chunks[i]) && before != after)
            }
            _ => true,
        })
        .map(|i| i as u32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host's storage in these tests: chunk index to bytes, and size.
    #[derive(Default)]
    struct Saved {
        chunks: std::collections::BTreeMap<u32, Vec<u8>>,
        size: u64,
    }

    impl Saved {
        fn save(&mut self, store: &Store) {
            let take = store.take_changed_chunks().unwrap();
            for (index, bytes) in take.chunks() {
                self.chunks.insert(index, bytes.to_vec());
            }
            self.size = take.size();
            let count = chunk_count(self.size) as u32;
            self.chunks.retain(|&index, _| index < count);
            take.saved();
        }

        fn restore(&self) -> BrowserFiles {
            let files = BrowserFiles::new().unwrap();
            files.restore_size(self.size).unwrap();
            for (&index, bytes) in &self.chunks {
                files.restore_chunk(index, bytes).unwrap();
            }
            files
        }
    }

    #[test]
    fn an_empty_write_touches_nothing() {
        let mut data = Data::default();
        assert_eq!(data.write(&[], 0), None);
        assert_eq!(data.write(&[], 5 * CHUNK_BYTES as u64), None);
        assert_eq!(data.len, 0);
    }

    /// Growing the file re-saves the old partial last chunk (its stored copy
    /// is shorter) and the zeroed gap, not only the chunk written.
    #[test]
    fn extending_the_file_marks_the_old_tail_and_the_gap() {
        let mut data = Data::default();
        assert_eq!(data.write(&[1; 100], 0), Some(0..=0));
        let at = 3 * CHUNK_BYTES as u64 + 10;
        assert_eq!(data.write(&[2; 10], at), Some(0..=3));
        // Within the file: only what is written.
        assert_eq!(data.write(&[3; 10], 2 * CHUNK_BYTES as u64), Some(2..=2));
    }

    #[test]
    fn a_longer_shared_last_chunk_differs() {
        let mut old = Data::default();
        old.write(&[1; 100], 0);
        let mut new = old.clone();
        // Same Arc, but the copy's file grew past its end.
        new.len = 200;
        assert_eq!(differing_chunks(&old, &new), [0]);
        assert!(differing_chunks(&old, &old.clone()).is_empty());
    }

    /// VACUUM shrinks the file across chunk boundaries; the save records the
    /// new size and the restored database is whole.
    #[test]
    fn a_shrunken_database_restores() {
        let files = BrowserFiles::new().unwrap();
        let store = Store::open_browser(&files).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TABLE filler(value BLOB);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
                 INSERT INTO filler SELECT randomblob(1000) FROM n;",
            )
            .unwrap();
        let mut saved = Saved::default();
        saved.save(&store);
        let big = saved.size;
        store
            .conn
            .execute_batch("DELETE FROM filler WHERE rowid > 10; VACUUM;")
            .unwrap();
        saved.save(&store);
        assert!(
            chunk_count(saved.size) + 10 < chunk_count(big),
            "{} against {big}",
            saved.size
        );
        drop(store);
        let files = saved.restore();
        let store = Store::open_browser(&files).unwrap();
        let (rows, integrity): (i64, String) = store
            .conn
            .query_row(
                "SELECT (SELECT count(*) FROM filler), (SELECT * FROM pragma_integrity_check)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((rows, integrity.as_str()), (10, "ok"));
    }

    /// If reopening on the swapped database fails, the swap is undone: the
    /// database is unchanged, the publish is over and writes work.
    #[test]
    fn a_failed_swap_leaves_the_database_unchanged() {
        let files = BrowserFiles::new().unwrap();
        let mut store = Store::open_browser(&files).unwrap();
        store
            .conn
            .execute_batch("CREATE TABLE marks(value TEXT); INSERT INTO marks VALUES('old');")
            .unwrap();
        let copy = store.begin_publish().unwrap();
        copy.conn
            .execute("UPDATE marks SET value = 'new'", [])
            .unwrap();
        // The swapped-in file is no database: reopening on it fails.
        {
            let copy_file = files.file(PUBLISH_COPY).unwrap();
            let mut file = lock(&copy_file);
            let len = file.data.len as usize;
            file.data.write(&vec![0xff; len.min(4096)], 0);
        }
        assert!(store.finish_publish(copy).is_err());
        assert!(!store.is_publishing());
        assert!(files.file(PUBLISH_COPY).is_none());
        let value: String = store
            .conn
            .query_row("SELECT value FROM marks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "old");
        store
            .conn
            .execute("UPDATE marks SET value = 'later'", [])
            .unwrap();
    }

    /// Locks hold between the connections of the one thread: a stage
    /// cannot be written while a publish reads it inside its transaction.
    #[test]
    fn a_stage_attached_in_a_transaction_refuses_its_writer() {
        let files = BrowserFiles::new().unwrap();
        let store = Store::open_browser(&files).unwrap();
        let path = std::path::Path::new("/stages/locked.sqlite");
        let stage = store.open_staging_connection(path).unwrap();
        stage
            .execute_batch("CREATE TABLE rows(value); INSERT INTO rows VALUES(1);")
            .unwrap();
        store.attach_staging_database(path, "staged").unwrap();
        store.conn.execute_batch("BEGIN").unwrap();
        let n: i64 = store
            .conn
            .query_row("SELECT count(*) FROM staged.rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        assert!(is_busy(stage.execute("INSERT INTO rows VALUES(2)", [])));
        store.conn.execute_batch("COMMIT").unwrap();
        stage.execute("INSERT INTO rows VALUES(2)", []).unwrap();
        store.conn.execute_batch("DETACH DATABASE staged").unwrap();
    }

    fn is_busy<T: std::fmt::Debug>(result: rusqlite::Result<T>) -> bool {
        matches!(
            result,
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::DatabaseBusy
        )
    }

    fn store_with_marks(files: &BrowserFiles) -> Store {
        let store = Store::open_browser(files).unwrap();
        store
            .conn
            .execute_batch("CREATE TABLE marks(value TEXT); INSERT INTO marks VALUES('old');")
            .unwrap();
        store
    }

    /// The VFS lives as long as the process: a connection that outlives its
    /// store and files (a `:memory:` one opens no file; a stage's keeps its
    /// own) still works.
    #[test]
    fn connections_outlive_their_store_and_files() {
        let files = BrowserFiles::new().unwrap();
        let store = Store::open_browser(&files).unwrap();
        let memory = Connection::open_with_flags_and_vfs(
            ":memory:",
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            VFS,
        )
        .unwrap();
        let stage = store
            .open_staging_connection(std::path::Path::new("/stages/outliving.sqlite"))
            .unwrap();
        stage
            .execute_batch("CREATE TABLE rows(value); INSERT INTO rows VALUES(1);")
            .unwrap();
        drop(store);
        drop(files);
        let now: i64 = memory
            .query_row("SELECT unixepoch('now')", [], |r| r.get(0))
            .unwrap();
        assert!(now > 1_700_000_000);
        stage.execute("INSERT INTO rows VALUES(2)", []).unwrap();
        let n: i64 = stage
            .query_row("SELECT count(*) FROM rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        // A file of gone files cannot be opened again.
        assert!(BrowserFiles::new().unwrap().path("x").starts_with(PREFIX));
    }

    /// A restore is refused while a publish copy exists, so an old publish
    /// can never replace a restored database.
    #[test]
    fn a_restore_is_refused_while_a_publish_copy_exists() {
        let files = BrowserFiles::new().unwrap();
        let mut store = store_with_marks(&files);
        let copy = store.begin_publish().unwrap();
        drop(store);
        assert!(files.restore_size(8192).is_err());
        assert!(files.restore_chunk(0, &[0; 8192]).is_err());
        drop(copy);
        files.restore_size(0).unwrap();
    }

    /// Every write lock is refused while a publish copy exists, also the
    /// ones that skip the reserved lock (BEGIN EXCLUSIVE, VACUUM).
    #[test]
    fn exclusive_writes_are_refused_during_a_publish() {
        let files = BrowserFiles::new().unwrap();
        let mut store = store_with_marks(&files);
        let copy = store.begin_publish().unwrap();
        assert!(is_busy(store.conn.execute_batch("BEGIN EXCLUSIVE")));
        let _ = store.conn.execute_batch("ROLLBACK");
        assert!(is_busy(store.conn.execute_batch("VACUUM")));
        assert!(is_busy(
            store.conn.execute("UPDATE marks SET value = 'lost'", [])
        ));
        store.finish_publish(copy).unwrap();
        let value: String = store
            .conn
            .query_row("SELECT value FROM marks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "old");
    }

    /// Names SQLite would take for the database's journals are reserved: a
    /// stage cannot pose as a hot journal of the database.
    #[test]
    fn a_stage_cannot_take_a_journal_name_of_the_database() {
        let files = BrowserFiles::new().unwrap();
        let store = store_with_marks(&files);
        for name in [
            "patina.sqlite",
            "patina.sqlite-journal",
            "patina.sqlite-wal",
            "patina-publish.sqlite-journal",
        ] {
            assert!(
                store
                    .open_staging_connection(std::path::Path::new(name))
                    .is_err(),
                "{name}"
            );
        }
        let stage = store
            .open_staging_connection(std::path::Path::new("patina.sqlite.stage"))
            .unwrap();
        drop(stage);
    }

    /// The size a restore accepts is the size the database may grow to.
    #[test]
    fn the_database_is_capped_at_the_restorable_size() {
        let files = BrowserFiles::new().unwrap();
        let store = Store::open_browser(&files).unwrap();
        let (pages, size): (i64, i64) = store
            .conn
            .query_row(
                "SELECT (SELECT * FROM pragma_max_page_count), (SELECT * FROM pragma_page_size)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(pages * size, MAX_DATABASE_BYTES as i64);
    }

    /// An unsaved take holds the database's chunks, not its connection: a
    /// closed store opens again.
    #[test]
    fn an_unsaved_take_does_not_keep_the_database_open() {
        let files = BrowserFiles::new().unwrap();
        let store = store_with_marks(&files);
        let take = store.take_changed_chunks().unwrap();
        drop(store);
        let store = Store::open_browser(&files).unwrap();
        drop(take);
        assert!(!store.take_changed_chunks().unwrap().is_empty());
    }

    /// Stages vanish with their last connection.
    #[test]
    fn dropped_stages_leave_no_scratch_files() {
        let files = BrowserFiles::new().unwrap();
        let store = Store::open_browser(&files).unwrap();
        let stage = crate::EpgStage::create_in_store(&store, "/stages").unwrap();
        let catalog = crate::CatalogStage::create_in_with("/stages", |path| {
            store.open_staging_connection(path)
        })
        .unwrap();
        assert_eq!(files.scratch_files(), 2);
        drop((stage, catalog));
        assert_eq!(files.scratch_files(), 0);
    }
}
