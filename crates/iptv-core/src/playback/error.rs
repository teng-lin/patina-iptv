//! Why playback failed, and what that means for retrying and for the words
//! the viewer sees.

/// Why playback failed. Decides whether a retry makes sense ([`Cause`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Connection, DNS, timeout or I/O failure.
    Network,
    /// The server answered with this HTTP status (0: some error status).
    Http(u16),
    /// Container, manifest or codec the player cannot handle.
    Format,
    /// The stream ended (a live TS connection that closed).
    Ended,
    /// Loading or buffering went on too long.
    Stalled,
    /// The provider would not give a stream address (Stalker `create_link`).
    Resolve,
    /// The provider refused the account itself (its login), not one stream.
    Account,
    /// Nothing to play: no stream address or no archive for this programme.
    Unavailable,
    Other,
}

/// What a failure means for the viewer and for retrying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// Retrying the same request normally cannot help: its resource is gone
    /// (404/410), its format is unsupported, or the provider refused the
    /// account. A played live stream may recover from 404/410 after resolving
    /// a fresh resource.
    Fatal,
    /// The provider refused the stream for now: an expired token, the
    /// account's connection limit (another device plays), a rate limit
    /// (401/403/429/458/509). Often over in seconds.
    Refused,
    /// The provider could not be reached (DNS, connect, timeout).
    Network,
    /// Anything else (a server error, a stall, an ended live stream).
    Transient,
}

impl ErrorKind {
    /// Maps a Media3 `PlaybackException.errorCodeName` (and the HTTP status
    /// when the host found one) to a kind.
    pub fn from_player_code(code: &str, http_status: Option<u16>) -> Self {
        if let Some(status) = http_status.filter(|s| *s >= 400) {
            return Self::Http(status);
        }
        let c = code.to_ascii_uppercase();
        if c.contains("PARSING") || c.contains("DECOD") || c.contains("UNSUPPORTED") {
            Self::Format
        } else if c.contains("BAD_HTTP_STATUS") {
            Self::Http(0)
        } else if c.contains("IO_") || c.contains("TIMEOUT") || c.contains("BEHIND_LIVE_WINDOW") {
            Self::Network
        } else {
            Self::Other
        }
    }

    /// Maps the failure kind of a stream-address request (the import
    /// crate's `FailureKind` name, e.g. `Auth`, `Network`) to a kind.
    pub fn from_resolve(kind: &str) -> Self {
        match kind {
            "Auth" => Self::Account,
            "Network" => Self::Network,
            _ => Self::Resolve,
        }
    }

    pub fn cause(&self) -> Cause {
        match self {
            Self::Http(404 | 410) | Self::Format | Self::Unavailable | Self::Account => {
                Cause::Fatal
            }
            Self::Http(401 | 403 | 429 | 458 | 509) => Cause::Refused,
            Self::Network => Cause::Network,
            Self::Http(_) | Self::Ended | Self::Stalled | Self::Resolve | Self::Other => {
                Cause::Transient
            }
        }
    }

    /// Whether an automatic retry can help without playback context.
    /// [`super::Health`] also retries 404/410 for a live stream that played.
    pub fn retryable(&self) -> bool {
        self.cause() != Cause::Fatal
    }
}

/// A failure as shown to the viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: ErrorKind,
    /// The player's or provider's code, for the detail line and logs.
    /// Never contains URLs or credentials.
    pub detail: String,
    /// Automatic attempts made so far (0: none were tried).
    pub attempts: u32,
    /// Slow retries go on behind the card (a live stream): it plays again
    /// by itself once the provider lets it.
    pub retrying: bool,
}
