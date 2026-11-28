//! Value types exposed by application commands. No connection or SQL API.
pub use crate::Error as StoreError;
use crate::file_writer::WriteError;
use iptv_core::add_playlist::{ImportCounts, ImportFailure, ImportProgress};
use iptv_epg::{FeedId, UnixTime};
use iptv_import::CancelToken;
pub use iptv_library::{
    ChannelScope, CustomGroup, CustomGroupId, GuideChannel, GuideSnapshot, GuideSource,
    ImportErrorKind, SourceId, SourceLine, SourcePolicy, StoredSource,
};
use iptv_providers::SecretUrl;
use iptv_settings::SettingsError;
/// The sole adapter-owned boundary value: the credential broker supplies an
/// opaque, redacting key; commands never return storage records or errors.
pub use iptv_store_sqlite::DatabaseKey;
/// Supported SQLite catalog schema version, exposed for diagnostic headers.
pub const STORE_SCHEMA_VERSION: u32 = iptv_store_sqlite::SCHEMA_VERSION;

/// Numeric-only observations for one store lane; no query, path or provider data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreQueueStats {
    /// Fixed lane name: writer, reader or analysis.
    pub lane: &'static str,
    /// Requests waiting to start, excluding the request currently running.
    pub pending: usize,
    /// Time since the oldest still-pending request was admitted; zero when empty.
    pub oldest_age: Duration,
    /// Longest observed wait before a request started during this owner's lifetime.
    pub max_wait: Duration,
    /// Queued reader requests skipped because their reply futures were dropped.
    pub skipped_reads: u64,
}
#[cfg(feature = "native")]
pub(crate) use iptv_store_sqlite::RecordingCompletion;
use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

/// Default and maximum end-to-end budget for an inspection query.
pub const QUERY_DEADLINE: Duration = Duration::from_secs(3);

/// Cancellation and deadline for one inspection call.
#[derive(Clone, Debug)]
pub struct InspectionControl {
    /// Caller-controlled cancellation, independent of sibling operations.
    pub cancel: CancelToken,
    /// Requested end-to-end budget, capped by the operation's public maximum.
    pub timeout: Duration,
}

impl Default for InspectionControl {
    fn default() -> Self {
        Self {
            cancel: CancelToken::new(),
            timeout: QUERY_DEADLINE,
        }
    }
}

/// The input to inspect into a private staged guide snapshot.
#[derive(Clone, Debug)]
pub enum ProbeTarget {
    /// A feed already configured for the source.
    Configured(FeedId),
    /// An explicit guide URL whose contents have no durable feed identity.
    Supplied(SecretUrl),
}

/// A guide probe scoped to one playlist.
#[derive(Clone, Debug)]
pub struct ProbeRequest {
    /// The playlist whose access policy and channel identities scope the result.
    pub source: SourceId,
    /// The configured or supplied guide input to inspect.
    pub target: ProbeTarget,
}

/// Sealed marker implemented only by `inspection::probe::Snapshot`.
pub(crate) trait ProbeSnapshot: Any + Send + Sync {}

/// An active lease pinning a staged probe snapshot during guide repair.
#[derive(Clone)]
pub struct ProbeLease(Arc<dyn ProbeSnapshot>);

impl ProbeLease {
    pub(crate) fn new(snapshot: Arc<impl ProbeSnapshot + 'static>) -> Self {
        Self(snapshot)
    }

    pub(crate) fn downcast_ref<T: ProbeSnapshot + 'static>(&self) -> &T {
        (&*self.0 as &dyn Any)
            .downcast_ref::<T>()
            .expect("ProbeLease holds its sealed probe snapshot")
    }
}

/// Validation and persistence are distinct: a failed save is never success.
#[derive(Debug)]
pub enum SaveError {
    Invalid(SettingsError),
    ParentalLocked,
    Storage(std::io::ErrorKind),
    /// Published to disk, but directory synchronization failed.
    Durability(std::io::ErrorKind),
}

impl From<WriteError> for SaveError {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::Storage(kind) => Self::Storage(kind),
            WriteError::Durability(kind) => Self::Durability(kind),
        }
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParentalLocked => f.write_str(crate::error::SETTINGS_LOCKED_REASON),
            Self::Invalid(error) => error.fmt(f),
            Self::Storage(kind) => write!(f, "Could not save settings ({kind})"),
            Self::Durability(kind) => write!(
                f,
                "Settings updated, but saving safely could not be confirmed ({kind})"
            ),
        }
    }
}

impl std::error::Error for SaveError {}

/// Who the resolved stream is for. Each purpose holds at most one pending
/// resolution per slot; starting a new one cancels the previous one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamPurpose {
    /// Local player surface (`0..MAX_SURFACES`).
    Surface(usize),
    /// External-player handoff.
    External,
    /// Cast / AirPlay sender.
    Cast,
}

/// Why a stream could not be resolved to a playable source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveFailure {
    /// The resolution was cancelled or superseded by a newer request.
    Cancelled,
    /// The network client or connection failed.
    Network,
    /// The portal did not answer before the interactive deadline.
    TimedOut,
    /// The portal refused or returned an unusable response.
    Portal(ImportErrorKind),
    /// The channel has no playable stream.
    Unavailable,
}

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
