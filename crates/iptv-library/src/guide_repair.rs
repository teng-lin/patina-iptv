//! Portable guide repair requests, immutable previews and observed outcomes.
//! IDs refer to one runtime owner's plans; committed inverse IDs survive restart.
use crate::inspection::{CandidateRef, ChannelRef, InspectionError, ProbeEvidence, TimeWindow};
use crate::{Choice, SourceId};
use iptv_epg::FeedId;

pub const MAX_MAPPING_CHANGES: usize = 500;
pub const MAX_FEED_CHANGES: usize = 64;
pub const MAX_PREVIEW_PAGE: usize = 200;
pub const PLAN_TTL_SECONDS: i64 = 86_400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Automatic,
    NoGuide,
    Configured(CandidateRef),
    /// `addition` names an entry in the request's additions, not a guessed feed ID.
    Staged {
        evidence: ProbeEvidence,
        epg_channel: String,
        addition: Option<usize>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingRequest {
    pub channel: ChannelRef,
    pub target: Target,
    pub replace_explicit: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedEdit {
    pub feed: FeedId,
    pub enabled: Option<bool>,
    pub shift_minutes: Option<i32>,
    pub remove: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedAddition {
    pub evidence: ProbeEvidence,
    pub name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub source: SourceId,
    pub mappings: Vec<MappingRequest>,
    pub feeds: Vec<FeedEdit>,
    pub additions: Vec<FeedAddition>,
    pub feed_order: Option<Vec<FeedId>>,
    pub window: TimeWindow,
}
impl Request {
    pub fn mapping(channel: ChannelRef, target: Target, window: TimeWindow) -> Self {
        Self {
            source: channel.source,
            mappings: vec![MappingRequest {
                channel,
                target,
                replace_explicit: false,
            }],
            feeds: vec![],
            additions: vec![],
            feed_order: None,
            window,
        }
    }
    pub fn valid(&self) -> bool {
        self.window.valid()
            && self.mappings.len() <= MAX_MAPPING_CHANGES
            && self.feeds.len() + self.additions.len() <= MAX_FEED_CHANGES
            && (!self.mappings.is_empty()
                || !self.feeds.is_empty()
                || !self.additions.is_empty()
                || self.feed_order.is_some())
            && self
                .mappings
                .iter()
                .all(|row| row.channel.source == self.source)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingPreview {
    pub channel: ChannelRef,
    pub name: String,
    pub before: Choice,
    pub after: Target,
    pub explicit_replacement: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedPreview {
    pub feed: Option<FeedId>,
    pub name: String,
    /// Zero-based precedence within this immutable preview, never a feed identity.
    pub before_position: Option<u32>,
    pub after_position: Option<u32>,
    pub before_enabled: Option<bool>,
    pub after_enabled: Option<bool>,
    pub before_shift_minutes: Option<i32>,
    pub after_shift_minutes: Option<i32>,
    pub added: bool,
    pub removed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndirectEffect {
    pub channel: ChannelRef,
    pub name: String,
    pub before: Option<Target>,
    pub after: Option<Target>,
    /// Schedule shifts and feed changes can affect a binding that retains identity.
    pub timing_changed: bool,
    /// Metadata predicts candidates; programme availability may change the effective winner.
    pub availability_unknown: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub id: String,
    pub digest: [u8; 32],
    pub source: SourceId,
    pub created_at: i64,
    pub expires_at: i64,
    pub mapping_count: usize,
    pub indirect_count: usize,
    pub indirect_complete: bool,
    pub mappings: Vec<MappingPreview>,
    pub feeds: Vec<FeedPreview>,
    pub indirect: Vec<IndirectEffect>,
    pub next_mapping: Option<usize>,
    pub next_indirect: Option<usize>,
    /// Uncommitted plans expire when their accepting owner closes/restarts.
    pub expires_on_owner_close: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revisions {
    pub mapping: u64,
    pub configuration: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub source: SourceId,
    pub created_at: i64,
    pub undo_expires_at: i64,
    pub undone: bool,
    pub revisions: Revisions,
    pub added_feeds: Vec<FeedId>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effective {
    /// Persistence succeeded; the previous cached guide is retained pending replacement.
    Pending,
    /// Current cached bindings reflect this configuration revision.
    Installed,
    /// Persistence succeeded but cached publication failed; verification remains pending.
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationPhase {
    Pending,
    Refreshing,
    ReadyToVerify,
    Verified,
    Failed,
    Superseded,
    Canceled,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub commit: Commit,
    pub configuration_committed: bool,
    pub mapping_saved: bool,
    pub effective: Effective,
    pub verification_pending: bool,
    pub operation: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub id: String,
    pub commit: Commit,
    pub phase: OperationPhase,
    pub effective: Effective,
    pub verification: Option<Verification>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    pub commit: String,
    pub source: SourceId,
    pub examined: Revisions,
    pub evidence_revision: String,
    pub current_evidence_revision: String,
    pub expected: Revisions,
    pub current: Revisions,
    pub window: TimeWindow,
    pub channels: usize,
    pub programs_available: usize,
    pub covered_seconds: u64,
    pub identity_unresolved: bool,
    pub current_revision: bool,
    pub configuration_effective: bool,
    pub verified_coverage: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Inspection(InspectionError),
    InvalidRequest,
    PlanMissing,
    PlanExpired,
    WrongDigest,
    CommitMissing,
    /// The writer may already have committed; recover by operation ID before retrying.
    CompletionUnknown {
        operation: String,
    },
    UndoExpired,
    Conflict(Vec<String>),
    Storage,
    Busy,
    Canceled,
}
impl From<InspectionError> for Error {
    fn from(value: InspectionError) -> Self {
        Self::Inspection(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityState {
    Implemented,
    UnsupportedHost,
    ProviderUnverified,
    NotImplemented,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    pub name: &'static str,
    pub state: CapabilityState,
}
