//! Value types exposed by application commands. No connection or SQL API.
pub use crate::Error as StoreError;
pub use crate::inspection::{InspectionControl, ProbeLease, ProbeRequest, ProbeTarget};
pub use crate::settings::SaveError;
use iptv_core::add_playlist::{ImportCounts, ImportFailure, ImportProgress};
use iptv_epg::UnixTime;
pub use iptv_library::{
    ChannelScope, CustomGroup, CustomGroupId, GuideChannel, GuideSnapshot, GuideSource,
    ImportErrorKind, SourceId, SourceLine, SourcePolicy, StoredSource,
};
/// The sole adapter-owned boundary value: the credential broker supplies an
/// opaque, redacting key; commands never return storage records or errors.
pub use iptv_store_sqlite::DatabaseKey;
#[cfg(feature = "native")]
pub(crate) use iptv_store_sqlite::RecordingCompletion;

/// The maximum number of past program days a stored guide can contain.
pub const MAX_STORED_GUIDE_PAST_DAYS: u32 = iptv_import::KEEP_PAST_MAX_DAYS;

/// How the guide on screen was last updated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideStatus {
    /// No playlist on screen.
    NoSource,
    /// A guide update is running.
    Updating,
    /// Nothing stored (not downloaded yet, or cleared).
    Empty,
    /// A stored guide is available.
    Updated { at: UnixTime, channels: u64 },
    /// The most recent download failed.
    Failed { at: UnixTime, kind: ImportErrorKind },
}

impl GuideStatus {
    /// Classifies the last attempt in a saved guide snapshot.
    pub fn from_snapshot(stored: Option<&GuideSnapshot>) -> Self {
        let Some(guide) = stored else {
            return Self::Empty;
        };
        match (guide.last_error, guide.attempted_at, guide.replaced_at) {
            (Some(kind), Some(at), _) => Self::Failed { at, kind },
            (_, _, Some(at)) => Self::Updated {
                at,
                channels: guide.bound_channels,
            },
            _ => Self::Empty,
        }
    }
}

/// What early playback needs before the full catalog has been loaded.
/// Storage overlays other than the parental block stay inside the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackChannel {
    pub channel: iptv_providers::model::Channel,
    pub blocked: bool,
}

/// A stored playlist's refresh (or check of new details) this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    Running(Option<ImportProgress>),
    /// Finished; on failure the stored channels stay. New details that
    /// imported are saved.
    Done(Result<ImportCounts, ImportFailure>),
    /// New details are being tried.
    Checking(Option<ImportProgress>),
    /// New details did not import: the stored ones and their data stay.
    Rejected(ImportFailure),
}

/// The current phase of the runtime's backup or restore operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupProgress {
    Idle,
    Selecting,
    Working,
}

/// What the UI must distinguish after an asynchronous save failure.
/// Detailed OS errors remain in diagnostics, outside user-facing messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFailure {
    /// The write did not publish a replacement settings file.
    NotPublished,
    /// The replacement is live, but its directory synchronization failed.
    DurabilityUnconfirmed,
}

#[cfg(test)]
pub use iptv_library::FeedResult;
