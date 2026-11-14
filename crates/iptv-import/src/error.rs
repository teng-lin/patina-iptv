//! Typed import failures.
//!
//! The UI must never present a failure as "no channels" (AC11): network,
//! server, authentication, unparseable content and a provably empty source
//! are distinct variants, summarised by [`ImportError::kind`]. Messages
//! never contain credentials, tokens or response bodies.

use iptv_core::import_limits::LimitExceeded;
use std::fmt;

use iptv_epg::FeedId;
use iptv_providers::model::ContentKind;
use iptv_providers::xtream::AccountStatus;
use iptv_providers::{ProviderError, StalkerError, StalkerErrorKind};

use crate::fetch::FetchError;

/// Which request or stage failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Playlist,
    AccountInfo,
    PortalDiscovery,
    Authentication,
    Categories(ContentKind),
    Items(ContentKind),
    Guide(FeedId),
    /// A playback link (Stalker `create_link`), outside an import.
    StreamLink,
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Playlist => f.write_str("playlist"),
            Self::AccountInfo => f.write_str("account info"),
            Self::PortalDiscovery => f.write_str("portal discovery"),
            Self::Authentication => f.write_str("authentication"),
            Self::Categories(k) => write!(f, "{} categories", k.slug()),
            Self::Items(k) => write!(f, "{} list", k.slug()),
            Self::Guide(id) => write!(f, "EPG source {}", id.0),
            Self::StreamLink => f.write_str("stream link"),
        }
    }
}

/// Coarse failure class for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// The source settings are unusable.
    Config,
    Cancelled,
    /// No usable response (DNS, refused, TLS, timeout, interrupted).
    Network,
    /// The server answered with an error status or a portal error.
    Server,
    /// Credentials, account state, device or login were refused.
    Auth,
    /// The response is not what the source type serves.
    Parse,
    /// The source answered correctly and has no content.
    Empty,
    /// What was downloaded could not be kept (disk full, I/O).
    Storage,
}

/// Why the provider refused access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthProblem {
    /// HTTP 401/403.
    HttpStatus(u16),
    /// Xtream `user_info` says inactive or expired.
    XtreamAccount {
        status: AccountStatus,
        message: Option<String>,
    },
    /// An Xtream catalog request answered with a rejected account object.
    XtreamRejected,
    /// A Stalker/Ministra refusal (blocked, login, device conflict…).
    Stalker(StalkerError),
}

/// Why a response could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseProblem {
    /// The URL is an HLS stream manifest, not a channel list.
    HlsManifest,
    /// HTML, JSON or other text where an M3U playlist was expected.
    NotAPlaylist,
    /// The body has neither `<channel>` nor `<programme>` elements.
    NotXmltv,
    /// Every entry was malformed.
    NoUsableEntries { skipped: usize },
    /// No probed endpoint behaves like a Stalker portal.
    NotAPortal,
    /// The body has the wrong shape (from the provider parsers).
    InvalidBody(String),
    /// Corrupt gzip or an unsupported text encoding.
    Decode(String),
}

/// An import that produced no catalog or guide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    Limit {
        step: Step,
        error: LimitExceeded,
    },
    InvalidConfig(String),
    Cancelled,
    Network {
        step: Step,
        error: FetchError,
    },
    HttpStatus {
        step: Step,
        status: u16,
    },
    /// A Stalker portal answered `js.error`.
    Portal {
        step: Step,
        error: StalkerError,
    },
    Auth {
        step: Step,
        problem: AuthProblem,
    },
    Parse {
        step: Step,
        problem: ParseProblem,
    },
    /// The source answered correctly and lists nothing.
    Empty {
        step: Step,
    },
    /// The guide being downloaded could not be stored
    /// ([`crate::GuideSink`]); ends the import.
    Storage(String),
}

impl ImportError {
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::InvalidConfig(_) => FailureKind::Config,
            Self::Cancelled => FailureKind::Cancelled,
            Self::Network { .. } => FailureKind::Network,
            Self::HttpStatus { .. } | Self::Portal { .. } => FailureKind::Server,
            Self::Auth { .. } => FailureKind::Auth,
            Self::Parse { .. } | Self::Limit { .. } => FailureKind::Parse,
            Self::Empty { .. } => FailureKind::Empty,
            Self::Storage(_) => FailureKind::Storage,
        }
    }

    /// The step that failed, when there is one.
    pub fn step(&self) -> Option<Step> {
        match self {
            Self::InvalidConfig(_) | Self::Cancelled | Self::Storage(_) => None,
            Self::Limit { step, .. }
            | Self::Network { step, .. }
            | Self::HttpStatus { step, .. }
            | Self::Portal { step, .. }
            | Self::Auth { step, .. }
            | Self::Parse { step, .. }
            | Self::Empty { step } => Some(*step),
        }
    }

    /// Worth an automatic retry later (network trouble, 5xx, 429).
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Network { .. } => true,
            Self::HttpStatus { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        }
    }

    /// Worth sending the same request again within this import: the
    /// connection failed, stalled or broke off, or the server answered 5xx.
    /// Narrower than [`Self::is_transient`]: not a refusal (any 4xx; a 429
    /// asks for a longer pause than an import waits), not a certificate or
    /// redirect problem or content that would be the same again, and not a
    /// size or time budget a second attempt would use up as well.
    pub(crate) fn retry_now(&self) -> bool {
        use crate::FetchErrorKind as Kind;
        match self {
            Self::Network { error, .. } => matches!(
                error.kind,
                Kind::Connect | Kind::Timeout | Kind::Body | Kind::Other
            ),
            Self::HttpStatus { status, .. } => (500..600).contains(status),
            _ => false,
        }
    }

    pub(crate) fn http(step: Step, status: u16) -> Self {
        if matches!(status, 401 | 403) {
            Self::Auth {
                step,
                problem: AuthProblem::HttpStatus(status),
            }
        } else {
            Self::HttpStatus { step, status }
        }
    }

    pub(crate) fn parse(step: Step, problem: ParseProblem) -> Self {
        Self::Parse { step, problem }
    }

    pub(crate) fn stalker(step: Step, error: StalkerError) -> Self {
        if error.kind == StalkerErrorKind::Portal {
            Self::Portal { step, error }
        } else {
            Self::Auth {
                step,
                problem: AuthProblem::Stalker(error),
            }
        }
    }

    pub(crate) fn provider(step: Step, error: ProviderError) -> Self {
        match error {
            ProviderError::Limit(error) => Self::Limit { step, error },
            ProviderError::InvalidConfig(why) => Self::InvalidConfig(why),
            ProviderError::HttpStatus(status) => Self::http(step, status),
            ProviderError::InvalidBody(why) => Self::parse(step, ParseProblem::InvalidBody(why)),
            ProviderError::AccountRejected => Self::Auth {
                step,
                problem: AuthProblem::XtreamRejected,
            },
            ProviderError::Stalker(e) => Self::stalker(step, e),
        }
    }

    /// Errors that end the whole import even in an optional step.
    pub(crate) fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::InvalidConfig(_)
                | Self::Limit { .. }
                | Self::Network {
                    error: FetchError {
                        kind: crate::FetchErrorKind::TooLarge | crate::FetchErrorKind::Deadline,
                        ..
                    },
                    ..
                }
                | Self::Storage(_)
                | Self::Auth {
                    problem: AuthProblem::Stalker(_),
                    ..
                }
        )
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(why) => write!(f, "invalid source settings: {why}"),
            Self::Cancelled => f.write_str("import cancelled"),
            Self::Limit { step, error } => write!(f, "{step}: {error}"),
            Self::Network { step, error } => write!(f, "{step}: {error}"),
            Self::HttpStatus { step, status } => write!(f, "{step}: server answered HTTP {status}"),
            Self::Portal { step, error } => write!(f, "{step}: {error}"),
            Self::Auth { step, problem } => match problem {
                AuthProblem::HttpStatus(s) => write!(f, "{step}: access denied (HTTP {s})"),
                AuthProblem::XtreamAccount { status, message } => {
                    let what = match status {
                        AccountStatus::Expired => "account expired",
                        AccountStatus::Inactive => "account inactive",
                        _ => "account unavailable",
                    };
                    match message {
                        Some(m) => write!(f, "{step}: {what}: {m}"),
                        None => write!(f, "{step}: {what}"),
                    }
                }
                AuthProblem::XtreamRejected => {
                    write!(f, "{step}: the provider rejected the account")
                }
                AuthProblem::Stalker(e) => write!(f, "{step}: {e}"),
            },
            Self::Parse { step, problem } => match problem {
                ParseProblem::HlsManifest => {
                    write!(f, "{step}: this is a stream address, not a playlist")
                }
                ParseProblem::NotAPlaylist => {
                    write!(f, "{step}: the response is not an M3U playlist")
                }
                ParseProblem::NotXmltv => write!(f, "{step}: the response is not XMLTV"),
                ParseProblem::NoUsableEntries { skipped } => {
                    write!(f, "{step}: all {skipped} entries are malformed")
                }
                ParseProblem::NotAPortal => {
                    write!(f, "{step}: no Stalker portal found at this address")
                }
                ParseProblem::InvalidBody(why) => write!(f, "{step}: unexpected response: {why}"),
                ParseProblem::Decode(why) => write!(f, "{step}: {why}"),
            },
            Self::Empty { step } => write!(f, "{step}: the source has no content"),
            Self::Storage(why) => write!(f, "could not store the guide: {why}"),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Limit { error, .. } => Some(error),
            Self::Network { error, .. } => Some(error),
            Self::Portal { error, .. } => Some(error),
            Self::Auth {
                problem: AuthProblem::Stalker(error),
                ..
            } => Some(error),
            _ => None,
        }
    }
}

impl From<FailureKind> for iptv_library::ImportErrorKind {
    fn from(kind: FailureKind) -> Self {
        match kind {
            FailureKind::Config => Self::InvalidConfig,
            FailureKind::Cancelled => Self::Cancelled,
            FailureKind::Network => Self::Network,
            FailureKind::Server => Self::HttpStatus,
            FailureKind::Auth => Self::AuthFailed,
            FailureKind::Parse | FailureKind::Empty => Self::InvalidBody,
            FailureKind::Storage => Self::Other,
        }
    }
}
