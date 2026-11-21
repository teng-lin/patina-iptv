//! The browser's storage: SQLite files held in memory and persisted by the
//! host in 64 KiB chunks ([`CHUNK_BYTES`]).
//!
//! A browser TV has no file system SQLite can use, and writing the whole
//! database to IndexedDB after every change takes seconds. [`BrowserFiles`]
//! is a memory VFS (on `rsqlite-vfs`, the layer under `sqlite-wasm-rs`)
//! that records which chunks of the database file each write changed. After
//! a commit the host takes them ([`Store::take_changed_chunks`]), copies them
//! out ([`Store::read_chunk`]) and writes only those; at launch it restores
//! the file from its chunks before opening ([`BrowserFiles::restore_size`],
//! [`BrowserFiles::restore_chunk`]). Committed work reaches storage when the
//! host has written it: a crash can lose the last changes, never mix two
//! commits, as long as the host writes all chunks of one take in one
//! IndexedDB transaction.
//!
//! Three kinds of file share one namespace:
//!
//! - **The database** ([`Store::open_browser`]), the only tracked file.
//! - **The publish copy** ([`Store::begin_publish`]): a long catalog or guide
//!   replacement runs on it in slices while reads continue on the database,
//!   and it replaces the database when the replacement committed
//!   ([`Store::finish_publish`]). Chunks are shared until written
//!   (copy-on-write), so the copy costs only what the publish changes, and
//!   the swap marks only chunks that differ as changed.
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
//! `SQLITE_THREADSAFE=0`.
#![allow(unsafe_code)]

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rsqlite_vfs::{
    AccessMode, LockLevel, OpenOptions as VfsOpenOptions, OpenRequest, OpenedFile, SQLiteIoMethods,
    SQLiteVfs, SyncOptions, VfsError, VfsErrorCode, VfsFile, VfsRegistration, VfsResult, VfsStore,
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

type Chunk = Arc<[u8; CHUNK_BYTES]>;

fn zero_chunk() -> Chunk {
    Arc::new([0; CHUNK_BYTES])
}

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

    /// Writes `buf` at `offset`; returns the chunks it touched.
    fn write(&mut self, buf: &[u8], offset: u64) -> std::ops::RangeInclusive<u32> {
        let end = offset + buf.len() as u64;
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
        let first = (offset / CHUNK_BYTES as u64) as u32;
        let last = ((end - 1) / CHUNK_BYTES as u64) as u32;
        first..=last
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

/// One file of the namespace.
struct FileState {
    data: Data,
    /// Chunks written since the host last took them; `Some` only for the
    /// database.
    changed: Option<BTreeSet<u32>>,
    /// Removed from the namespace when its last connection closes.
    scratch: bool,
}

type SharedFile = Arc<Mutex<FileState>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // One thread: a poisoned lock can only follow a panic, which aborts.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the VFS callbacks share: the files and SQLite's last error.
#[derive(Default)]
struct Namespace {
    files: Mutex<HashMap<String, SharedFile>>,
    /// Open file handles, including anonymous temporary files.
    handles: AtomicU32,
    error: Mutex<Option<VfsError>>,
}

impl Namespace {
    fn file(&self, name: &str) -> Option<SharedFile> {
        lock(&self.files).get(name).cloned()
    }
}

type AppData = Arc<Namespace>;

/// An open file: SQLite reads and writes through it.
struct Handle {
    file: SharedFile,
    read_only: bool,
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
        if let Some(changed) = &mut file.changed {
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

    // One connection per file and one thread: there is nothing to lock
    // against (the store opens no second connection in the browser).
    fn lock(&mut self, _level: LockLevel) -> VfsResult<()> {
        Ok(())
    }

    fn unlock(&mut self, _level: LockLevel) -> VfsResult<()> {
        Ok(())
    }

    fn check_reserved_lock(&self) -> VfsResult<bool> {
        Ok(false)
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
                data: Data::default(),
                changed: None,
                scratch: true,
            })),
            Some(name) => {
                let name = name.path();
                let mut files = lock(&data.files);
                match files.get(name) {
                    Some(_) if options.create() && options.exclusive() => {
                        return Err(VfsError::new(
                            VfsErrorCode::CantOpen,
                            "file already exists".into(),
                        ));
                    }
                    Some(file) => file.clone(),
                    None if options.create() => {
                        let file = Arc::new(Mutex::new(FileState {
                            data: Data::default(),
                            changed: (name == DATABASE).then(BTreeSet::new),
                            scratch: name != DATABASE && name != PUBLISH_COPY,
                        }));
                        files.insert(name.to_owned(), file.clone());
                        file
                    }
                    None => {
                        return Err(VfsError::new(VfsErrorCode::CantOpen, "no such file".into()));
                    }
                }
            }
        };
        if options.delete_on_close()
            && let Some(name) = request.filename
        {
            // Unlinked at open: the handle keeps the data until it closes.
            lock(&data.files).remove(name.path());
        }
        data.handles.fetch_add(1, Ordering::Relaxed);
        Ok(OpenedFile {
            file: Handle {
                file,
                read_only: options.access() == rsqlite_vfs::OpenAccess::ReadOnly,
            },
            access: options.access(),
        })
    }

    fn close_file(
        data: &AppData,
        name: Option<&str>,
        handle: Handle,
        _options: VfsOpenOptions,
    ) -> VfsResult<()> {
        data.handles.fetch_sub(1, Ordering::Relaxed);
        let scratch = lock(&handle.file).scratch;
        if let Some(name) = name.filter(|_| scratch) {
            let mut files = lock(&data.files);
            // The namespace's reference and this handle's: the last close.
            if files
                .get(name)
                .is_some_and(|file| Arc::ptr_eq(file, &handle.file) && Arc::strong_count(file) == 2)
            {
                files.remove(name);
            }
        }
        Ok(())
    }

    fn access(data: &AppData, name: &str, _mode: AccessMode) -> VfsResult<bool> {
        Ok(lock(&data.files).contains_key(name))
    }

    fn full_pathname(_data: &AppData, name: &str) -> VfsResult<String> {
        Ok(name.to_owned())
    }

    fn delete_file(data: &AppData, name: &str, _sync_dir: bool) -> VfsResult<()> {
        lock(&data.files).remove(name);
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

/// The VFS registration, freed with the last [`BrowserFiles`] clone when no
/// file is open any more.
struct Registration(Option<VfsRegistration<AppData>>);

// SAFETY: `wasm32-unknown-unknown` without atomics runs one thread, so the
// registration's raw pointers are never shared between threads; the store
// is `Send` only so that code generic over threads compiles for both targets.
unsafe impl Send for Registration {}
// SAFETY: as above, there is no second thread to share it with.
unsafe impl Sync for Registration {}

struct Inner {
    name: String,
    namespace: AppData,
    registration: Mutex<Registration>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Free the files a closed store left. A staging connection that
        // outlives every store keeps its own file until it closes.
        lock(&self.namespace.files).clear();
        let Some(registration) = lock(&self.registration).0.take() else {
            return;
        };
        if self.namespace.handles.load(Ordering::Relaxed) == 0 {
            // SAFETY: no file of this VFS is open, every store holding it
            // is gone, and the one thread is here.
            if let Err((registration, _)) = unsafe { registration.unregister() } {
                let _ = registration.into_raw();
            }
        } else {
            // An open file still calls into the VFS: keep it registered
            // (a few hundred bytes) for the rest of the process.
            let _ = registration.into_raw();
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
            .field("vfs", &self.0.name)
            .finish_non_exhaustive()
    }
}

fn unavailable(reason: &'static str) -> StoreError {
    StoreError::Unavailable(reason)
}

impl BrowserFiles {
    /// New, empty files under a VFS of their own.
    pub fn new() -> Result<Self> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!("patina-chunks-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let namespace = AppData::default();
        // SAFETY: the method table, file type and app data are this module's
        // own and agree; the name is fresh; one thread registers and uses it.
        let registration = unsafe {
            rsqlite_vfs::register_vfs::<ChunkIo, ChunkVfs>(&name, namespace.clone(), false)
        }
        .map_err(|_| unavailable("could not register the browser storage"))?;
        Ok(Self(Arc::new(Inner {
            name,
            namespace,
            registration: Mutex::new(Registration(Some(registration))),
        })))
    }

    fn database(&self) -> Option<SharedFile> {
        self.0.namespace.file(DATABASE)
    }

    /// Whether `file`, as the namespace holds it, is open on some
    /// connection (each open handle holds another reference).
    fn is_open(file: &SharedFile) -> bool {
        Arc::strong_count(file) > 1
    }

    fn database_is_open(&self) -> bool {
        lock(&self.0.namespace.files)
            .get(DATABASE)
            .is_some_and(Self::is_open)
    }

    /// Prepares the database file to be restored at launch: `size` bytes,
    /// zero until [`Self::restore_chunk`] fills them. Only before the store
    /// opens; replaces what the files held.
    pub fn restore_size(&self, size: u64) -> Result<()> {
        if self.database_is_open() {
            return Err(unavailable("the browser database is open"));
        }
        let chunks = (0..chunk_count(size)).map(|_| zero_chunk()).collect();
        let file = FileState {
            data: Data { chunks, len: size },
            changed: Some(BTreeSet::new()),
            scratch: false,
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
        Ok(())
    }

    fn open(&self, name: &str) -> Result<Connection> {
        crate::initialize_sqlite()?;
        Ok(Connection::open_with_flags_and_vfs(
            name,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            self.0.name.as_str(),
        )?)
    }

    /// Opens a scratch database (a stage) in these files.
    pub(crate) fn open_scratch(&self, name: &str) -> Result<Connection> {
        if name == DATABASE || name == PUBLISH_COPY {
            return Err(unavailable("a stage cannot use the database's name"));
        }
        self.open(name)
    }

    /// Whether a file named `path` exists in these files.
    pub(crate) fn contains(&self, path: &std::path::Path) -> bool {
        path.to_str()
            .is_some_and(|name| lock(&self.0.namespace.files).contains_key(name))
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

/// A browser store's link to its files.
#[derive(Clone, Debug)]
pub(crate) struct Link {
    files: BrowserFiles,
    role: Role,
    /// A publish copy exists: the database refuses writes until it is
    /// swapped in or abandoned, so none can be lost in the swap.
    publishing: bool,
}

impl Link {
    pub(crate) fn files(&self) -> &BrowserFiles {
        &self.files
    }
}

/// The database chunks written since the host last took them, and the
/// file's size: the host writes these chunks and deletes stored chunks at
/// or past `ceil(size / CHUNK_BYTES)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChangedChunks {
    /// The database file's size in bytes.
    pub size: u64,
    /// Changed chunk indices, ascending.
    pub indices: Vec<u32>,
}

impl ChangedChunks {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// The browser connection's settings: the rollback journal stays in memory
/// (persistence is the host's chunk write after a commit, so there is no
/// file to recover from), no fsync, temporary data in memory.
pub(crate) fn configure(conn: &Connection) -> Result<()> {
    let _mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "MEMORY", |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

impl Store {
    /// Opens (creating if empty) the browser store in `files` and migrates
    /// it. Restore its chunks first. One store per files: a second open
    /// while one is open fails.
    pub fn open_browser(files: &BrowserFiles) -> Result<Self> {
        Self::open_browser_with(files, &OpenOptions::default())
    }

    pub fn open_browser_with(files: &BrowserFiles, options: &OpenOptions) -> Result<Self> {
        if files.database_is_open() {
            return Err(unavailable("the browser database is already open"));
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
            publishing: false,
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

    /// Takes the database chunks changed since the last take, for the host
    /// to write. Only between transactions, when the file holds the last
    /// commit and nothing uncommitted. If writing them fails, give them
    /// back with [`Self::return_changed_chunks`].
    pub fn take_changed_chunks(&self) -> Result<ChangedChunks> {
        let link = self.database_link()?;
        if !self.conn.is_autocommit() {
            return Err(unavailable("a transaction is open"));
        }
        let file = link
            .files
            .database()
            .ok_or_else(|| unavailable("the browser database is missing"))?;
        let mut file = lock(&file);
        let indices = file
            .changed
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
            .into_iter()
            .collect();
        Ok(ChangedChunks {
            size: file.data.len,
            indices,
        })
    }

    /// Marks chunks as changed again (their write to storage failed).
    pub fn return_changed_chunks(&self, chunks: &ChangedChunks) -> Result<()> {
        let link = self.database_link()?;
        if let Some(file) = link.files.database() {
            let mut file = lock(&file);
            let count = chunk_count(file.data.len) as u32;
            if let Some(changed) = &mut file.changed {
                changed.extend(chunks.indices.iter().copied().filter(|&i| i < count));
            }
        }
        Ok(())
    }

    /// Copies chunk `index` of the database into `out` (replacing its
    /// contents): [`CHUNK_BYTES`] bytes, fewer for the last chunk. Read
    /// every chunk of one take before the next write, so storage receives
    /// one commit's state.
    pub fn read_chunk(&self, index: u32, out: &mut Vec<u8>) -> Result<()> {
        let link = self.database_link()?;
        let file = link
            .files
            .database()
            .ok_or_else(|| unavailable("the browser database is missing"))?;
        let file = lock(&file);
        let bytes = file
            .data
            .chunk(index as usize)
            .ok_or_else(|| StoreError::InvalidData(format!("no chunk {index}")))?;
        out.clear();
        out.extend_from_slice(bytes);
        Ok(())
    }

    /// Whether a publish copy is open ([`Self::begin_publish`]).
    pub fn is_publishing(&self) -> bool {
        self.browser.as_ref().is_some_and(|link| link.publishing)
    }

    /// Starts a publish on a copy of the database: returns a store on the
    /// copy, where a long replacement runs in slices
    /// ([`crate::CatalogReplacement`], [`crate::EpgReplacement`]) while this
    /// store keeps serving reads of the last commit.
    ///
    /// Until [`Self::finish_publish`] or [`Self::abandon_publish`], this
    /// store refuses writes (`PRAGMA query_only`): a write here would be
    /// lost in the swap. The caller holds writes back until the swap and
    /// runs them after it, in order, as the native writer queue does.
    pub fn begin_publish(&mut self) -> Result<Self> {
        let link = self.database_link()?;
        if link.publishing {
            return Err(unavailable("a publish is already running"));
        }
        if !self.conn.is_autocommit() {
            return Err(unavailable("a transaction is open"));
        }
        let files = link.files.clone();
        let database = files
            .database()
            .ok_or_else(|| unavailable("the browser database is missing"))?;
        let copy = FileState {
            data: lock(&database).data.clone(),
            changed: None,
            scratch: false,
        };
        {
            let mut names = lock(&files.0.namespace.files);
            if names.get(PUBLISH_COPY).is_some_and(BrowserFiles::is_open) {
                return Err(unavailable("a publish copy is still open"));
            }
            names.insert(PUBLISH_COPY.to_owned(), Arc::new(Mutex::new(copy)));
        }
        let opened = Self::open_role(
            &files,
            Role::PublishCopy,
            &OpenOptions {
                prefer_fts: self.fts,
                ..OpenOptions::default()
            },
        )
        .and_then(|mut copy| {
            copy.clock = self.clock;
            self.conn.pragma_update(None, "query_only", true)?;
            Ok(copy)
        });
        match opened {
            Ok(copy) => {
                if let Some(link) = &mut self.browser {
                    link.publishing = true;
                }
                Ok(copy)
            }
            Err(e) => {
                lock(&files.0.namespace.files).remove(PUBLISH_COPY);
                Err(e)
            }
        }
    }

    fn check_copy(&self, copy: &Self) -> Result<BrowserFiles> {
        let link = self.database_link()?;
        let theirs = copy.link()?;
        if !link.publishing || theirs.role != Role::PublishCopy || !link.files.same(&theirs.files) {
            return Err(unavailable("not this store's publish copy"));
        }
        Ok(link.files.clone())
    }

    /// Replaces the database with the publish copy, whose replacement has
    /// committed, and reopens this store on it. Marks the chunks that differ
    /// from the database's as changed. Writes may resume afterwards.
    pub fn finish_publish(&mut self, copy: Self) -> Result<()> {
        let files = self.check_copy(&copy)?;
        if !copy.conn.is_autocommit() {
            return Err(unavailable("the publish has not committed"));
        }
        drop(copy);
        let copy = lock(&files.0.namespace.files)
            .remove(PUBLISH_COPY)
            .ok_or_else(|| unavailable("the publish copy is missing"))?;
        let database = files
            .database()
            .ok_or_else(|| unavailable("the browser database is missing"))?;
        // Close this connection before its file changes under it.
        drop(std::mem::replace(
            &mut self.conn,
            Connection::open_in_memory()?,
        ));
        {
            let copy = std::mem::take(&mut lock(&copy).data);
            let mut database = lock(&database);
            let old = std::mem::replace(&mut database.data, copy);
            let new = &database.data;
            let differing: Vec<u32> = (0..new.chunks.len())
                .filter(|&i| {
                    old.chunk(i).is_none_or(|before| {
                        !Arc::ptr_eq(&old.chunks[i], &new.chunks[i]) && Some(before) != new.chunk(i)
                    })
                })
                .map(|i| i as u32)
                .collect();
            let count = new.chunks.len() as u32;
            if let Some(changed) = &mut database.changed {
                changed.retain(|&i| i < count);
                changed.extend(differing);
            }
        }
        let options = OpenOptions {
            prefer_fts: self.fts,
            ..OpenOptions::default()
        };
        // The copy was migrated by this build: opening only configures.
        self.conn = Self::open_role(&files, Role::Database, &options)?.conn;
        if let Some(link) = &mut self.browser {
            link.publishing = false;
        }
        Ok(())
    }

    /// Drops a publish copy whose replacement failed or was canceled; the
    /// database is unchanged and accepts writes again.
    pub fn abandon_publish(&mut self, copy: Self) -> Result<()> {
        let files = self.check_copy(&copy)?;
        drop(copy);
        lock(&files.0.namespace.files).remove(PUBLISH_COPY);
        self.conn.pragma_update(None, "query_only", false)?;
        if let Some(link) = &mut self.browser {
            link.publishing = false;
        }
        Ok(())
    }
}
