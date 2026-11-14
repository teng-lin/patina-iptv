//! Portable library model and screen state machines. No persistence or UI toolkit.
#![forbid(unsafe_code)]

use iptv_epg::UnixTime;
use iptv_providers::{GroupId, SecretUrl};

/// Row id of a source. Overlay keys are `(SourceId, ContentId)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePolicy {
    pub enabled: bool,
    /// Zero disables periodic refresh. Foreground scheduling only.
    pub refresh_seconds: u32,
    pub refresh_on_start: bool,
}
impl Default for SourcePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            refresh_seconds: 0,
            refresh_on_start: true,
        }
    }
}
impl SourcePolicy {
    /// Failed attempts count too: a failed import cannot create a tight retry loop.
    pub fn due(self, now: i64, last_attempt: Option<i64>, startup: bool) -> bool {
        self.enabled
            && ((startup && self.refresh_on_start)
                || (self.refresh_seconds > 0
                    && last_attempt
                        .is_none_or(|t| now.saturating_sub(t) >= i64::from(self.refresh_seconds))))
    }
}

/// One guide source of a playlist source. `Debug` is safe: the address is
/// a redacting [`SecretUrl`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideSource {
    /// Its feed id in the stored guide (unique within the playlist source).
    pub feed: iptv_epg::FeedId,
    /// `None`: added by the user. `Some(n)`: the provider's n-th own guide.
    pub provider_index: Option<u32>,
    /// May carry credentials (Xtream `xmltv.php`).
    pub url: SecretUrl,
    /// The name the user gave it.
    pub name: Option<String>,
    pub enabled: bool,
    /// Added to every programme time of the feed (minutes).
    pub shift_minutes: i32,
}

impl GuideSource {
    pub fn is_provider(&self) -> bool {
        self.provider_index.is_some()
    }
}

/// The last download of one feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedResult {
    pub feed: iptv_epg::FeedId,
    pub attempted_at: UnixTime,
    /// `None`: it loaded.
    pub error: Option<ImportErrorKind>,
    /// What was stored from it (bound EPG channels and their programmes).
    pub channel_count: u64,
    pub programme_count: u64,
    /// The channels it lists (0 for a guide stored before schema v6).
    pub listed_count: u64,
}

/// A channel one of the stored feeds lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideChannel {
    pub feed: iptv_epg::FeedId,
    pub epg_channel: String,
    /// Its first display name, when the feed gave one.
    pub name: Option<String>,
    /// A playlist channel takes its guide from it.
    pub assigned: bool,
}

/// A list whose order the user can set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderScope {
    /// The all-channels list.
    All,
    /// One provider group.
    Group(GroupId),
}

impl OrderScope {
    pub fn key(&self) -> &str {
        match self {
            Self::All => "",
            Self::Group(g) => g.as_str(),
        }
    }
}

/// Row id of a custom group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CustomGroupId(pub i64);

/// Why an import failed, coarse enough to persist and show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImportErrorKind {
    /// DNS, connect, TLS or reset.
    Network,
    Timeout,
    HttpStatus,
    InvalidBody,
    InvalidConfig,
    /// Xtream `auth = 0`, Stalker login rejected.
    AccountRejected,
    /// Stalker token/session failures.
    AuthFailed,
    /// Stalker device blocked or bound to another device id.
    Blocked,
    Cancelled,
    Other,
}

impl ImportErrorKind {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::HttpStatus => "http_status",
            Self::InvalidBody => "invalid_body",
            Self::InvalidConfig => "invalid_config",
            Self::AccountRejected => "account_rejected",
            Self::AuthFailed => "auth_failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
            Self::Other => "other",
        }
    }

    /// Unknown slugs (written by a newer build) read as [`Self::Other`].
    pub fn from_slug(slug: &str) -> Self {
        [
            Self::Network,
            Self::Timeout,
            Self::HttpStatus,
            Self::InvalidBody,
            Self::InvalidConfig,
            Self::AccountRejected,
            Self::AuthFailed,
            Self::Blocked,
            Self::Cancelled,
        ]
        .into_iter()
        .find(|k| k.slug() == slug)
        .unwrap_or(Self::Other)
    }
}

impl From<&iptv_providers::ProviderError> for ImportErrorKind {
    fn from(error: &iptv_providers::ProviderError) -> Self {
        Self::from_slug(error.import_error_slug())
    }
}

use iptv_epg::MatchKind;
use iptv_providers::model::SourceKind;

/// A stored playlist, as listed. Carries no secrets but the guide
/// addresses, which redact themselves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playlist {
    pub policy: SourcePolicy,
    pub id: SourceId,
    pub name: String,
    pub kind: SourceKind,
    /// Live channels of the last successful import.
    pub channels: u32,
    pub updated_at: Option<i64>,
    /// The last refresh failed (and when); the channels before it remain.
    pub failed: Option<(ImportErrorKind, Option<i64>)>,
    /// Its guide sources in the user's order.
    pub guides: Vec<GuideSource>,
    /// The last guide download failed (its stored guide stays).
    pub guide_failed: bool,
    /// The User-Agent its requests send (M3U and Xtream only).
    pub user_agent: Option<String>,
    /// An M3U playlist read from a file (no link to change).
    pub from_file: bool,
}

/// One guide source with how its last download went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLine {
    pub source: GuideSource,
    pub result: Option<FeedResult>,
}

/// A channel's guide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Matched by its guide id and name (no choice saved).
    Automatic,
    NoGuide,
    Channel {
        feed: iptv_epg::FeedId,
        epg_channel: String,
    },
}

/// What a channel's guide is now: the saved choice and the guide channel
/// its stored programmes come from (with how it was matched).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Current {
    pub choice: Option<Choice>,
    pub bound: Option<(iptv_epg::FeedId, String, MatchKind)>,
}

pub mod details;
pub mod epg_sources;
pub mod guide_assign;
pub mod guides;
pub mod playlists;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrganizationScope {
    Channels,
    Groups,
    Favorites,
    GroupChannels(GroupId),
    CustomChannels(CustomGroupId),
}
