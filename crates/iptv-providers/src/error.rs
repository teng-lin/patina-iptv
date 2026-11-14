//! Errors returned by the provider clients.
//!
//! Messages never contain credentials, tokens, MAC addresses or response
//! bodies; a portal's own explanation (Stalker `msg`/`block_msg`) is kept
//! separately in [`StalkerError::portal_text`] so the UI can quote it.

use std::fmt;

/// Why a provider request or response could not be turned into a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    Limit(iptv_core::import_limits::LimitExceeded),
    /// The source configuration is unusable (bad URL, blank credentials).
    InvalidConfig(String),
    /// The server answered with a non-success HTTP status.
    HttpStatus(u16),
    /// The body is not what the endpoint returns (not JSON, wrong shape,
    /// empty). The text describes the shape, never the body itself.
    InvalidBody(String),
    /// An Xtream panel answered a catalog request with an account object
    /// (for example `user_info.auth = 0`) instead of a list.
    AccountRejected,
    /// A Stalker/Ministra portal refused the request or the session.
    Stalker(StalkerError),
}

impl ProviderError {
    /// Stable failure category for persistence, without messages or secrets.
    pub fn import_error_slug(&self) -> &'static str {
        match self {
            ProviderError::InvalidConfig(_) => "invalid_config",
            ProviderError::HttpStatus(_) => "http_status",
            ProviderError::InvalidBody(_) | ProviderError::Limit(_) => "invalid_body",
            ProviderError::AccountRejected => "account_rejected",
            ProviderError::Stalker(e) => match e.kind {
                StalkerErrorKind::Blocked | StalkerErrorKind::DeviceConflict => "blocked",
                StalkerErrorKind::LoginRequired | StalkerErrorKind::LoginRejected => {
                    "account_rejected"
                }
                StalkerErrorKind::Portal => "other",
                _ => "auth_failed",
            },
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit(error) => error.fmt(f),
            Self::InvalidConfig(why) => write!(f, "invalid source configuration: {why}"),
            Self::HttpStatus(code) => write!(f, "HTTP status {code}"),
            Self::InvalidBody(why) => write!(f, "unexpected response: {why}"),
            Self::AccountRejected => f.write_str("the provider rejected the account"),
            Self::Stalker(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Limit(err) => Some(err),
            Self::Stalker(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StalkerError> for ProviderError {
    fn from(err: StalkerError) -> Self {
        Self::Stalker(err)
    }
}

/// What kind of refusal a Stalker portal produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StalkerErrorKind {
    /// Token missing, stale or replaced (`Authorization failed.` and the JSON
    /// envelopes some panels use instead).
    AuthFailed,
    /// `get_profile` status 1 or `Access denied.`: blocked/disabled account,
    /// malformed MAC.
    Blocked,
    /// `get_profile` status 1 whose message names a device-id binding
    /// conflict. The remedy is restoring the first device id.
    DeviceConflict,
    /// `get_profile` status 2 and no login/password configured.
    LoginRequired,
    /// `do_auth` answered `{js: false}`.
    LoginRejected,
    /// The handshake answered without a token.
    NoToken,
    /// An API request was built before the session was authorized.
    NotAuthorized,
    /// Repeated authorization failures right after a fresh login.
    AuthLoop,
    /// `create_link` or another call answered `js.error`.
    Portal,
}

/// A Stalker refusal plus the portal's own explanation, if it gave one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StalkerError {
    pub kind: StalkerErrorKind,
    /// Markup-stripped `msg`/`block_msg`, or the plain-text failure body.
    pub portal_text: Option<String>,
}

impl StalkerError {
    pub fn new(kind: StalkerErrorKind) -> Self {
        Self {
            kind,
            portal_text: None,
        }
    }

    pub fn with_text(kind: StalkerErrorKind, text: Option<String>) -> Self {
        Self {
            kind,
            portal_text: text.filter(|t| !t.is_empty()),
        }
    }
}

impl fmt::Display for StalkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            StalkerErrorKind::AuthFailed => "portal authorization failed",
            StalkerErrorKind::Blocked => "portal refused the device",
            StalkerErrorKind::DeviceConflict => "portal reports a device id conflict",
            StalkerErrorKind::LoginRequired => "portal requires a login and password",
            StalkerErrorKind::LoginRejected => "portal rejected the login",
            StalkerErrorKind::NoToken => "portal handshake returned no token",
            StalkerErrorKind::NotAuthorized => "portal session is not authorized yet",
            StalkerErrorKind::AuthLoop => "portal keeps rejecting fresh sessions",
            StalkerErrorKind::Portal => "portal returned an error",
        };
        match &self.portal_text {
            Some(text) => write!(f, "{what}: {text}"),
            None => f.write_str(what),
        }
    }
}

impl std::error::Error for StalkerError {}

/// Why no catch-up URL could be built for a programme.
///
/// Unit variants only: neither `Debug` nor `Display` can carry a URL, a
/// template or a credential from the playlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchupError {
    /// The mode needs a `catchup-source` (or a programme `catchup-id`) that
    /// the playlist or the request does not provide.
    MissingTemplate,
    /// The channel has no catch-up, its type has no URL rule here (unknown
    /// `catchup` values), or replay goes through a portal (Stalker).
    Unsupported,
    /// The live URL or the expanded template is not shaped as the mode needs
    /// (not HTTP for `shift`, not Flussonic or Xtream-shaped, not a URL).
    MalformedUrl,
    /// The requested position has not been broadcast yet.
    FutureProgramme,
    /// The requested position is older than the archive window or past the
    /// programme's end.
    OutsideArchive,
}

impl fmt::Display for CatchupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingTemplate => "the channel's catch-up needs a source it does not declare",
            Self::Unsupported => "catch-up is not available for this channel",
            Self::MalformedUrl => "the stream URL does not fit the channel's catch-up type",
            Self::FutureProgramme => "the programme has not been broadcast yet",
            Self::OutsideArchive => "the programme is outside the archive window",
        })
    }
}

impl std::error::Error for CatchupError {}
