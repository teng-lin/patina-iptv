#![forbid(unsafe_code)]
//! Native SQLite store of iptv-rs.
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

mod backup;
mod encryption;
pub use encryption::DatabaseKey;
mod catalog;
mod catalog_stage;
pub use catalog_stage::CatalogStage;
mod channels;
mod codec;
mod epg;
mod epg_assign;
mod epg_extent;
mod epg_stage;
mod epg_titles;
mod epg_window;
mod error;
mod guide_sources;
mod history;
mod library_preferences;
pub mod migrate;
mod order;
mod organise;
mod overlay;
mod search;
mod search_history;
mod source_settings;
mod sources;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
pub use organise::{CustomGroup, CustomGroupId, OrderScope};
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
fn system_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

pub struct Store {
    conn: Connection,
    /// The FTS index exists and must be maintained by writes.
    fts_index: bool,
    /// Searches query the FTS index (else `LIKE`).
    fts: bool,
    clock: fn() -> i64,
    key: Option<DatabaseKey>,
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
    /// Opens (creating if needed) a store file and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, &OpenOptions::default())
    }

    pub fn open_with(path: impl AsRef<Path>, options: &OpenOptions) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI;
        Self::init(Connection::open_with_flags(path, flags)?, options)
    }

    /// A private in-memory store (tests, previews).
    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with(&OpenOptions::default())
    }

    pub fn open_in_memory_with(options: &OpenOptions) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, options)
    }

    fn init(mut conn: Connection, options: &OpenOptions) -> Result<Self> {
        conn.busy_timeout(options.busy_timeout)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // Persistent for files; in-memory databases answer "memory".
        let _mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        // Durable across app crashes in WAL; a power cut may lose the last
        // commit, never corrupt the file.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // 16 MiB page cache (default 2 MiB): a 50k-channel refresh touches
        // several index B-trees.
        conn.pragma_update(None, "cache_size", -CACHE_KIB)?;
        conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT_BYTES)?;
        migrate::migrate_to(&mut conn, SCHEMA_VERSION, system_now())?;
        let fts_index = search::prepare(&mut conn)?;
        Ok(Self {
            conn,
            fts_index,
            fts: options.prefer_fts && fts_index,
            clock: system_now,
            key: None,
        })
    }

    /// A second connection to this store's file that can only read: the
    /// same key, no migration (this store migrated the file when it
    /// opened) and `PRAGMA query_only`, so SQLite refuses any write through
    /// it, including from `&self` methods that would write. In WAL mode it
    /// reads the last commit while this store holds a write transaction,
    /// so reads need not queue behind a long catalog or guide publish.
    pub fn open_reader(&self) -> Result<Self> {
        let path = self
            .conn
            .path()
            .filter(|path| !path.is_empty())
            .ok_or(StoreError::Unavailable("an in-memory store has no reader"))?;
        // No CREATE: the file must be the one this store opened.
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
            conn,
            fts_index: self.fts_index,
            fts: self.fts,
            clock: self.clock,
            key: self.key.clone(),
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
