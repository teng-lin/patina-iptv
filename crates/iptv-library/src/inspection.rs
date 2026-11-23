//! Portable values for source-explicit catalog and guide inspection.
//! No view, database, network connection or player is part of this contract.
use crate::{Choice, SourceId};
use iptv_epg::{FeedId, MatchKind};
use iptv_providers::ContentId;

pub const MAX_PAGE: u32 = 200;
pub const MAX_QUERY_BYTES: usize = 256;
pub const MAX_SOURCES: usize = 64;
pub const MAX_WINDOW_SECONDS: i64 = 31 * 86_400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceScope {
    Source(SourceId),
    Selected(Vec<SourceId>),
    AllEnabled,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChannelRef {
    pub source: SourceId,
    pub content: ContentId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelInfo {
    pub key: ChannelRef,
    pub name: String,
    pub number: Option<u32>,
    pub hidden: bool,
    pub blocked: bool,
    pub epg_id: Option<String>,
    pub epg_name: Option<String>,
    pub choice: Choice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub scope: SourceScope,
    pub text: String,
    pub include_hidden: bool,
    pub limit: u32,
    pub cursor: Option<Cursor>,
}
impl Query {
    pub fn source(source: SourceId) -> Self {
        Self {
            scope: SourceScope::Source(source),
            text: String::new(),
            include_hidden: false,
            limit: 50,
            cursor: None,
        }
    }
    pub fn valid(&self) -> bool {
        (1..=MAX_PAGE).contains(&self.limit)
            && self.text.len() <= MAX_QUERY_BYTES
            && match &self.scope {
                SourceScope::Selected(ids) => !ids.is_empty() && ids.len() <= MAX_SOURCES,
                _ => true,
            }
    }
}

/// An in-process semantic continuation, scoped to its owner and inputs.
/// Every continuation is revalidated; these values are never authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub context: String,
    pub snapshot: String,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<Cursor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeWindow {
    pub from: i64,
    pub to: i64,
}
impl TimeWindow {
    pub fn valid(self) -> bool {
        self.to
            .checked_sub(self.from)
            .is_some_and(|span| span > 0 && span <= MAX_WINDOW_SECONDS)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CandidateRef {
    pub source: SourceId,
    pub feed: FeedId,
    /// Persisted feed incarnation; numeric feed IDs can be reused.
    pub incarnation: String,
    pub epg_channel: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramInfo {
    pub channel: ChannelRef,
    pub candidate: CandidateRef,
    pub source_incarnation: String,
    pub evidence_revision: String,
    /// Provider event identity when supplied; scoped by candidate, never global.
    pub provider_event_id: Option<String>,
    pub start: i64,
    pub stop: i64,
    pub title: String,
}

/// A sample belongs to an XMLTV candidate, which may have no catalog channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateProgram {
    pub candidate: CandidateRef,
    pub source_incarnation: String,
    pub evidence_revision: String,
    pub provider_event_id: Option<String>,
    pub start: i64,
    pub stop: i64,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    Manual,
    ExactId,
    CaseInsensitiveId,
    NormalizedName,
    Alternative,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub key: CandidateRef,
    pub name: Option<String>,
    pub names: Vec<String>,
    /// Full labels retained for regional/language/time-shift distinctions.
    /// Empty metadata means the feed did not supply a structured value.
    pub metadata: Vec<(String, String)>,
    pub assigned: bool,
    pub evidence: Vec<Evidence>,
    pub binding: Option<MatchKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diagnosis {
    InputFailure,
    Disabled,
    UnresolvedOverride,
    Unmatched,
    MissingMetadata,
    Ambiguous,
    Empty,
    Stale,
    Partial,
    SuspiciousTiming,
    Covered,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub channel: ChannelInfo,
    pub diagnosis: Diagnosis,
    pub window: TimeWindow,
    pub covered_seconds: u64,
    pub program_count: u32,
    pub binding: Option<CandidateRef>,
    /// Coverage is schedule evidence, never proof of station identity.
    pub identity_verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProbeId(pub String);
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeEvidence {
    pub stage: ProbeId,
    pub source: SourceId,
    pub feed: Option<FeedId>,
    pub incarnation: String,
    pub digest: [u8; 32],
    pub expires_at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeSummary {
    pub evidence: ProbeEvidence,
    pub channels: u64,
    pub programs: u64,
    pub bytes: u64,
    pub warnings: u64,
}

/// Failures useful to every application consumer; no SQL or HTTP error leaks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectionError {
    InvalidRequest,
    SourceMissing(SourceId),
    SourceDisabled(SourceId),
    ChannelMissing,
    PermissionDenied,
    StaleCursor,
    StaleEvidence,
    Canceled,
    Deadline,
    Busy,
    Storage,
    Network,
    AddressPolicy,
    InvalidFeed,
    LimitExceeded,
    OwnerClosed,
}
impl std::fmt::Display for InspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InspectionError {}

pub type InspectionResult<T> = Result<T, InspectionError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_queries_enforce_page_size_source_count_and_utf8_byte_limits() {
        let mut query = Query::source(SourceId(1));
        assert!(query.valid(), "default source query");

        for (limit, valid) in [
            (0, false),
            (1, true),
            (MAX_PAGE, true),
            (MAX_PAGE + 1, false),
        ] {
            query.limit = limit;
            assert_eq!(query.valid(), valid, "page size {limit}");
        }
        query.limit = MAX_PAGE;

        query.text = "é".repeat(MAX_QUERY_BYTES / 2);
        assert!(query.valid(), "the maximum byte length is inclusive");
        query.text.push('é');
        assert!(
            !query.valid(),
            "multibyte text is bounded by encoded length"
        );
        query.text.clear();

        for (count, valid) in [
            (0, false),
            (1, true),
            (MAX_SOURCES, true),
            (MAX_SOURCES + 1, false),
        ] {
            query.scope = SourceScope::Selected((1..=count as i64).map(SourceId).collect());
            assert_eq!(query.valid(), valid, "selected source count {count}");
        }
        query.scope = SourceScope::AllEnabled;
        assert!(query.valid());
    }

    #[test]
    fn guide_windows_reject_reversed_equal_overlong_and_overflowing_endpoints() {
        for (from, to, valid) in [
            (-1, 0, true),
            (0, 0, false),
            (1, 0, false),
            (0, MAX_WINDOW_SECONDS, true),
            (0, MAX_WINDOW_SECONDS + 1, false),
            (i64::MIN, i64::MIN + 1, true),
            (i64::MAX - 1, i64::MAX, true),
            (i64::MIN, i64::MAX, false),
            (i64::MAX, i64::MIN, false),
        ] {
            assert_eq!(
                TimeWindow { from, to }.valid(),
                valid,
                "window {from} to {to}"
            );
        }
    }
}
