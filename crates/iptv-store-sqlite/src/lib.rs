// Unsafe code only in the browser VFS (`browser.rs`), which SQLite calls.
#![deny(unsafe_code)]
// The browser's `std::fs` is a stub whose `File` has no destructor; the
// file-based paths (backups, encrypted opens, file stages) only fail there.
#![cfg_attr(target_family = "wasm", allow(clippy::drop_non_drop))]
//! SQLite store of iptv-rs: SQLCipher files natively, memory files persisted
//! in chunks by the host in the browser (`wasm32-unknown-unknown`).
//!
//! - **Provider data** (groups, channels, VOD, series) is replaced wholesale
//!   on each refresh, inside one transaction ([`Store::replace_catalog`]).
//!   Readers on other connections see the old catalog or the new one, never
//!   a mix (WAL).
//! - **Sources** keep non-secret settings in `sources.config` and secrets
//!   in `source_credentials`. Production uses [`Store::open_encrypted`] with
//!   a platform-keystore key. SQLCipher encrypts every page, including WAL,
//!   stream URLs, headers and indexes. Explicit plaintext opens are reserved
//!   for synthetic tests and user-requested portable backup files. Never log
//!   rows; `Debug` of the row types redacts credential-bearing values.
//!   What the user chose per source: its guide sources in
//!   `guide_sources` ([`Store::guide_sources`]), channels' manual guide
//!   choices in `channel_overlays`; the source the app shows in
//!   `store_meta` ([`Store::active_source`]).
//! - **Migrations** are forward-only and numbered, applied at open, and safe
//!   to run repeatedly ([`migrate`]).
//!
//! One [`Store`] wraps one connection and is `Send`, not `Sync`. Use one
//! writer and any number of reader stores on the same file
//! ([`Store::open_reader`]); WAL lets them read while the writer replaces
//! a catalog.
//! - **Guide data** (EPG) is stored per playlist source and replaced per
//!   refresh ([`Store::replace_epg`]) from a download staged in its own
//!   file ([`EpgStage`]); the app loads a window of it without
//!   descriptions ([`Store::load_epg`]) and reads descriptions on demand.
//!   The wiring for app start and background refresh is documented in
//!   `src/epg.rs` and the README.
//! - **In the browser** (`wasm32`, rusqlite's `sqlite-wasm-rs` backend) a
//!   store opens on in-memory files that record which chunks each commit
//!   changed, for the host to persist (`Store::open_browser`, `browser.rs`).
//!   There is one connection and no encryption; long replacements run on a
//!   copy of the database while reads continue (`Store::begin_publish`).
//! - **Long replacements are resumable**: [`CatalogReplacement`] and
//!   [`EpgReplacement`] write until a row budget is spent and continue
//!   later (the browser's slices); the one-shot methods run them to
//!   completion with the same statements as before (`replacement.rs`).

// The unit tests' browser build: host services for SQLite and a stand-in
// for `tempfile`, shared with the integration executable.
#[cfg(all(test, target_family = "wasm"))]
extern crate self as tempfile;
#[cfg(all(test, target_family = "wasm"))]
#[allow(unsafe_code)]
#[path = "../tests/it/wasm.rs"]
mod wasm_test;
#[cfg(all(test, target_family = "wasm"))]
use wasm_test::{TempDir, tempdir};

mod backup;
#[cfg(target_family = "wasm")]
mod browser;
#[cfg(target_family = "wasm")]
pub use browser::{BrowserFiles, CHUNK_BYTES, ChangedChunks};
mod encryption;
pub use encryption::DatabaseKey;
mod catalog;
mod catalog_replacement;
pub use catalog_replacement::{CatalogPublished, CatalogReplacement, StagePublication};
mod catalog_stage;
pub use catalog_stage::CatalogStage;
mod channels;
mod codec;
mod epg;
mod epg_assign;
mod epg_extent;
mod epg_replacement;
pub use epg_replacement::EpgReplacement;
mod epg_stage;
mod epg_titles;
mod epg_window;
mod error;
mod guide_repair;
mod guide_revisions;
pub use guide_repair::{
    MAX_REPAIR_MAPPINGS, REPAIR_UNDO_SECONDS, RepairBatch, RepairBinding, RepairCandidate,
    RepairChannel, RepairChoice, RepairCommit, RepairFeed, RepairInputs, RepairMapping,
    RepairPublication, RepairResult, RepairVerification,
};
mod guide_sources;
pub use guide_revisions::GuideRevisions;
mod history;
mod inspection;
pub use inspection::InspectionAccess;
mod inspection_stage;
pub use inspection_stage::{INSPECTION_STAGE_BYTES, InspectionStage};
mod library_preferences;
pub mod migrate;
mod order;
mod organize;
mod overlay;
pub mod parental;
mod recordings;
mod replacement;
pub use recordings::RecordingCompletion;
pub use replacement::Step;
mod search;
mod search_history;
mod source_settings;
mod sources;
mod vod;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub use channels::{ChannelOrder, ChannelQuery, ChannelRow, ChannelScope};
pub use epg::{EpgCounts, EpgFeedMeta, EpgWindow, StoredEpgFeed, StoredGuide};
pub use epg_assign::{FeedResult, GuideChannel};
pub use epg_stage::EpgStage;
pub use error::{Result, StoreError};
pub use guide_sources::GuideSource;
pub use history::{HISTORY_LIMIT, HistoryEntry, PRUNE_SLACK};
pub use library_preferences::{GroupRow, SourcePolicy};
pub use migrate::SCHEMA_VERSION;
pub use organize::{CustomGroup, CustomGroupId, MoveTarget, OrderScope};
pub use overlay::{EpgOverride, OrphanedOverlay};
pub use search::SearchBackend;
pub use sources::{CatalogCounts, ImportErrorKind, ImportStatus, SourceSummary, StoredSource};

pub use iptv_library::SourceId;

/// How to open a store.
#[derive(Clone, Debug)]
pub struct OpenOptions {
    /// How long a write waits for another connection's lock.
    pub busy_timeout: Duration,
    /// Use FTS5 for search when the SQLite build has it. `false` forces the
    /// `LIKE` fallback (used by tests of that path).
    pub prefer_fts: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_secs(5),
            prefer_fts: true,
        }
    }
}

/// Page cache per connection, KiB.
const CACHE_KIB: i64 = 16 * 1024;
/// WAL file size limit before checkpoint truncation, bytes.
pub const WAL_SIZE_LIMIT_BYTES: i64 = 16 * 1024 * 1024;

/// Unix seconds from the system clock.
#[cfg(not(target_family = "wasm"))]
fn system_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Unix seconds from the host clock SQLite itself reads in the browser
/// (`rust_sqlite_wasm_host_epoch_timestamp_in_ms`, which the app links in):
/// `std::time::SystemTime` is not available on `wasm32-unknown-unknown`.
#[cfg(target_family = "wasm")]
fn system_now() -> i64 {
    use rsqlite_vfs::OsCallback;
    sqlite_wasm_rs::WasmOsCallback
        .epoch_timestamp_in_ms()
        .map_or(0, |ms| ms.div_euclid(1000))
}

/// Completes SQLite's initialization before a connection opens. Natively
/// that includes SQLCipher's provider ([`iptv_sqlcipher_init`]); the
/// browser's SQLite has no cipher and needs nothing beyond what opening does.
pub(crate) fn initialize_sqlite() -> rusqlite::Result<()> {
    #[cfg(not(target_family = "wasm"))]
    return iptv_sqlcipher_init::initialize();
    #[cfg(target_family = "wasm")]
    Ok(())
}

pub struct Store {
    conn: Connection,
    /// The FTS index exists and must be maintained by writes.
    fts_index: bool,
    /// Searches query the FTS index (else `LIKE`).
    fts: bool,
    clock: fn() -> i64,
    key: Option<DatabaseKey>,
    inspection_access: std::cell::RefCell<Option<inspection::InspectionAccess>>,
    /// The in-memory files of a browser store (after `conn`, which must
    /// close first).
    #[cfg(target_family = "wasm")]
    browser: Option<browser::Link>,
}

/// How a connection journals its writes.
#[derive(Clone, Copy)]
enum Journal {
    /// Files: WAL, so reader connections see the last commit during a write.
    Wal,
    /// The browser's memory files: one connection, the journal in memory.
    #[cfg(target_family = "wasm")]
    Browser,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.conn.path())
            .field("fts", &self.fts)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Run a compound read in one WAL snapshot, sharing a caller's transaction.
    /// Failed reads roll back a snapshot started here; caller ownership is retained.
    pub fn read_snapshot<T>(&self, read: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let snapshot = self.begin_read_snapshot()?;
        let result = read(self)?;
        if let Some(snapshot) = snapshot {
            snapshot.commit()?;
        }
        Ok(result)
    }

    fn begin_read_snapshot(&self) -> Result<Option<rusqlite::Transaction<'_>>> {
        self.conn
            .is_autocommit()
            .then(|| self.conn.unchecked_transaction())
            .transpose()
            .map_err(Into::into)
    }

    /// Opens (creating if needed) a store file and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, &OpenOptions::default())
    }

    pub fn open_with(path: impl AsRef<Path>, options: &OpenOptions) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI;
        crate::initialize_sqlite()?;
        Self::init(Connection::open_with_flags(path, flags)?, options)
    }

    /// A private in-memory store (tests, previews).
    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with(&OpenOptions::default())
    }

    /// In the browser this is a browser store on files of its own
    /// (`BrowserFiles`), so stages and publishing work as they do there.
    pub fn open_in_memory_with(options: &OpenOptions) -> Result<Self> {
        #[cfg(target_family = "wasm")]
        return Self::open_browser_with(&BrowserFiles::new()?, options);
        #[cfg(not(target_family = "wasm"))]
        {
            crate::initialize_sqlite()?;
            Self::init(Connection::open_in_memory()?, options)
        }
    }

    fn init(conn: Connection, options: &OpenOptions) -> Result<Self> {
        Self::init_with(conn, options, Journal::Wal)
    }

    fn init_with(mut conn: Connection, options: &OpenOptions, journal: Journal) -> Result<Self> {
        conn.busy_timeout(options.busy_timeout)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        match journal {
            Journal::Wal => {
                // Persistent for files; in-memory databases answer "memory".
                let _mode: String =
                    conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
                // Durable across app crashes in WAL; a power cut may lose the
                // last commit, never corrupt the file.
                conn.pragma_update(None, "synchronous", "NORMAL")?;
            }
            #[cfg(target_family = "wasm")]
            Journal::Browser => browser::configure(&conn)?,
        }
        // 16 MiB page cache (default 2 MiB): a 50k-channel refresh touches
        // several index B-trees.
        conn.pragma_update(None, "cache_size", -CACHE_KIB)?;
        conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT_BYTES)?;
        migrate::migrate_to(&mut conn, SCHEMA_VERSION, system_now())?;
        let fts_index = search::prepare(&mut conn)?;
        Ok(Self {
            inspection_access: Default::default(),
            conn,
            fts_index,
            fts: options.prefer_fts && fts_index,
            clock: system_now,
            key: None,
            #[cfg(target_family = "wasm")]
            browser: None,
        })
    }

    /// A second connection to this store's file that can only read: the
    /// same key, no migration (this store migrated the file when it
    /// opened) and `PRAGMA query_only`, so SQLite refuses any write through
    /// it, including from `&self` methods that would write. In WAL mode it
    /// reads the last commit while this store holds a write transaction,
    /// so reads need not queue behind a long catalog or guide publish.
    pub fn open_reader(&self) -> Result<Self> {
        #[cfg(target_family = "wasm")]
        if self.browser.is_some() {
            return Err(StoreError::Unavailable("a browser store has no reader"));
        }
        let path = self
            .conn
            .path()
            .filter(|path| !path.is_empty())
            .ok_or(StoreError::Unavailable("an in-memory store has no reader"))?;
        // No CREATE: the file must be the one this store opened.
        crate::initialize_sqlite()?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )?;
        if let Some(key) = &self.key {
            key.apply(&conn)?;
        }
        conn.busy_timeout(OpenOptions::default().busy_timeout)?;
        conn.pragma_update(None, "cache_size", -CACHE_KIB)?;
        conn.pragma_update(None, "query_only", true)?;
        let version = migrate::current_version(&conn)?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::Unavailable(
                "the reader found a schema this build did not migrate",
            ));
        }
        Ok(Self {
            inspection_access: Default::default(),
            conn,
            fts_index: self.fts_index,
            fts: self.fts,
            clock: self.clock,
            key: self.key.clone(),
            #[cfg(target_family = "wasm")]
            browser: None,
        })
    }

    /// Replaces the clock used for `created_at`/import timestamps (tests).
    pub fn set_clock(&mut self, clock: fn() -> i64) {
        self.clock = clock;
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    pub fn schema_version(&self) -> Result<u32> {
        migrate::current_version(&self.conn)
    }

    /// `wal` for files, `memory` for in-memory stores.
    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))?)
    }

    pub fn search_backend(&self) -> SearchBackend {
        if self.fts {
            SearchBackend::Fts5
        } else {
            SearchBackend::Like
        }
    }
}

mod private_file;
pub use private_file::PrivateFileReader;
