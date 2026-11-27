use std::fmt;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use url::Url;

use super::DiscoveryIssue;

/// A sanitized error. Receiver replies, source URLs and credentials are
/// deliberately absent from its display, debug output and source chain.
#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
    stage: Stage,
    delivery: Delivery,
    issues: Vec<DiscoveryIssue>,
    diagnostic: Option<Diagnostic>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, stage: Stage, delivery: Delivery) -> Self {
        Self {
            kind,
            stage,
            delivery,
            issues: Vec::new(),
            diagnostic: None,
        }
    }

    pub(crate) fn discovery(issues: Vec<DiscoveryIssue>) -> Self {
        let kind = if issues
            .iter()
            .all(|issue| issue.error().kind() == ErrorKind::TimedOut)
        {
            ErrorKind::TimedOut
        } else {
            ErrorKind::Unavailable
        };
        Self {
            issues,
            ..Self::new(kind, Stage::Discovery, Delivery::NotSent)
        }
    }

    // Used by backends that own a separate UDP timing listener.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "AirPlay integration follows this contract")
    )]
    pub(crate) fn with_diagnostic(mut self, diagnostic: Diagnostic) -> Self {
        self.diagnostic = Some(diagnostic);
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn delivery(&self) -> Delivery {
        self.delivery
    }

    pub fn discovery_issues(&self) -> &[DiscoveryIssue] {
        &self.issues
    }

    pub fn diagnostic(&self) -> Option<&Diagnostic> {
        self.diagnostic.as_ref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "casting {:?}: {:?} ({:?})",
            self.stage, self.kind, self.delivery
        )
    }
}

impl std::error::Error for Error {}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    TimingListener { local_addr: SocketAddr },
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Configuration,
    Discovery,
    Connect,
    Pair,
    Store,
    Load,
    Observe,
    Control,
    Stop,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Unsupported,
    Unavailable,
    Busy,
    Cancelled,
    Expired,
    InvalidPin,
    ApprovalDenied,
    IdentityMismatch,
    Storage,
    Unreachable,
    TimedOut,
    ReceiverRefused,
    OwnershipLost,
    Protocol,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    NotSent,
    MayHaveReachedReceiver,
    Acknowledged,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerTrust {
    VerifiedPairing,
    LanEndpoint,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    LoadVideoUrl,
    Play,
    Pause,
    Seek,
    SetVolume,
    StepVolume,
    SetMuted,
    ToggleMute,
    Stop,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable,
    Unsupported,
    Unknown,
}

#[derive(Clone, Debug)]
pub(crate) struct Features([Availability; 9]);

impl Features {
    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn with(mut self, feature: Feature, availability: Availability) -> Self {
        self.0[feature as usize] = availability;
        self
    }

    pub(crate) fn new(default: Availability) -> Self {
        Self([default; 9])
    }

    pub(crate) fn availability(&self, feature: Feature) -> Availability {
        self.0[feature as usize]
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeScope {
    Media,
    Receiver,
}

/// A volume on an inclusive 0–100 scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume(u8);

impl Volume {
    pub fn percent(value: u8) -> Result<Self, Error> {
        if value > 100 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                Stage::Control,
                Delivery::NotSent,
            ));
        }
        Ok(Self(value))
    }

    pub fn as_percent(self) -> u8 {
        self.0
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputChange {
    SetVolume(Volume),
    StepVolume(i8),
    SetMuted(bool),
    ToggleMute,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Play,
    Pause,
    Seek(Duration),
    Output {
        scope: VolumeScope,
        change: OutputChange,
    },
}

impl Command {
    pub(crate) fn feature(self) -> Feature {
        match self {
            Self::Play => Feature::Play,
            Self::Pause => Feature::Pause,
            Self::Seek(_) => Feature::Seek,
            Self::Output { change, .. } => match change {
                OutputChange::SetVolume(_) => Feature::SetVolume,
                OutputChange::StepVolume(_) => Feature::StepVolume,
                OutputChange::SetMuted(_) => Feature::SetMuted,
                OutputChange::ToggleMute => Feature::ToggleMute,
            },
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    ReceiverConfirmed,
    Accepted,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    CancelledBeforeSend,
    AlreadyEnded,
    StopRequested(Confirmation),
    TransportClosed,
}

#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Playback {
    Pending,
    Accepted,
    Buffering,
    Playing,
    Paused,
    Ended,
    NoLongerPresent,
    Failed(Error),
    OwnershipLost,
}

impl Playback {
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Ended | Self::NoLongerPresent | Self::Failed(_) | Self::OwnershipLost
        )
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Connected,
    Recovering,
    Disconnected,
}

/// A compatible receiver-fetchable HTTP(S) URL. Debug output omits the URL.
pub struct Media {
    url: Url,
    content_type: Option<mime::Mime>,
    is_live: bool,
}

impl fmt::Debug for Media {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Media")
            .field("is_live", &self.is_live)
            .finish_non_exhaustive()
    }
}

impl Media {
    pub fn live(url: &str) -> Result<Self, Error> {
        Self::new(url, true)
    }

    pub fn on_demand(url: &str) -> Result<Self, Error> {
        Self::new(url, false)
    }

    fn new(input: &str, is_live: bool) -> Result<Self, Error> {
        let invalid = || Error::new(ErrorKind::InvalidInput, Stage::Load, Delivery::NotSent);
        // Require absolute URL syntax before the WHATWG parser can normalize an
        // input such as http:relative or discard an empty user-info field.
        let (scheme, rest) = input.split_once("://").ok_or_else(invalid)?;
        let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or_default();
        if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            || authority.is_empty()
            || authority.contains('@')
            || input.chars().any(char::is_whitespace)
        {
            return Err(invalid());
        }
        let url = Url::parse(input).map_err(|_| invalid())?;
        Ok(Self {
            url,
            content_type: None,
            is_live,
        })
    }

    pub fn with_content_type(mut self, mime: &str) -> Result<Self, Error> {
        let content_type: mime::Mime = mime
            .parse()
            .map_err(|_| Error::new(ErrorKind::InvalidInput, Stage::Load, Delivery::NotSent))?;
        if content_type.type_() == mime::STAR || content_type.subtype() == mime::STAR {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                Stage::Load,
                Delivery::NotSent,
            ));
        }
        self.content_type = Some(content_type);
        Ok(self)
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn url(&self) -> &str {
        self.url.as_str()
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn content_type(&self) -> Option<&str> {
        self.content_type.as_ref().map(AsRef::as_ref)
    }

    pub(crate) fn is_live(&self) -> bool {
        self.is_live
    }
}

/// A versioned observation of the media and its connection.
#[derive(Clone, Debug)]
pub struct Snapshot {
    revision: u64,
    playback: Playback,
    link: Link,
    link_error: Option<Error>,
    playback_observed_at: Option<Instant>,
    position: Option<Duration>,
    duration: Option<Duration>,
    features: Features,
    volume_scope: Option<VolumeScope>,
    volume: Option<Volume>,
    muted: Option<bool>,
    is_live: bool,
}

impl Snapshot {
    pub(crate) fn pending(is_live: bool) -> Self {
        Self {
            revision: 0,
            playback: Playback::Pending,
            link: Link::Connected,
            link_error: None,
            playback_observed_at: None,
            position: None,
            duration: None,
            features: Features::new(Availability::Unknown),
            volume_scope: None,
            volume: None,
            muted: None,
            is_live,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn playback(&self) -> &Playback {
        &self.playback
    }

    pub fn link(&self) -> Link {
        self.link
    }

    pub fn link_error(&self) -> Option<&Error> {
        self.link_error.as_ref()
    }

    pub fn playback_observed_at(&self) -> Option<Instant> {
        self.playback_observed_at
    }

    pub fn position(&self) -> Option<Duration> {
        self.position
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    pub fn availability(&self, feature: Feature) -> Availability {
        if self.playback.is_terminal() || self.link != Link::Connected {
            return Availability::Unavailable;
        }
        if feature == Feature::Seek && self.is_live {
            return Availability::Unsupported;
        }
        self.features.availability(feature)
    }

    pub fn volume_scope(&self) -> Option<VolumeScope> {
        self.volume_scope
    }

    pub fn volume(&self) -> Option<Volume> {
        self.volume
    }

    pub fn muted(&self) -> Option<bool> {
        self.muted
    }

    pub(crate) fn apply(&mut self, change: Observation) {
        self.revision += 1;
        if let Some((link, error)) = change.link {
            self.link = link;
            self.link_error = error;
        }
        if self.playback.is_terminal() {
            return;
        }
        if let Some(playback) = change.playback {
            self.playback = playback;
            if let Some(observed_at) = change.observed_at {
                self.playback_observed_at = Some(observed_at);
            }
        }
        if let Some(position) = change.position {
            self.position = position;
        }
        if let Some(duration) = change.duration {
            self.duration = duration;
        }
        if let Some(features) = change.features {
            self.features = features;
        }
        if let Some((scope, volume, muted)) = change.output {
            self.volume_scope = scope;
            self.volume = volume;
            self.muted = muted;
        }
    }
}

/// Backend evidence for one session. Absent fields leave prior evidence intact.
#[derive(Default)]
pub(crate) struct Observation {
    playback: Option<Playback>,
    observed_at: Option<Instant>,
    link: Option<(Link, Option<Error>)>,
    position: Option<Option<Duration>>,
    duration: Option<Option<Duration>>,
    features: Option<Features>,
    output: Option<(Option<VolumeScope>, Option<Volume>, Option<bool>)>,
}

impl Observation {
    pub(crate) fn playback(playback: Playback) -> Self {
        let observed_at =
            (!matches!(playback, Playback::Pending | Playback::Failed(_))).then(Instant::now);
        Self {
            playback: Some(playback),
            observed_at,
            ..Self::default()
        }
    }

    pub(crate) fn link(link: Link, error: Option<Error>) -> Self {
        Self {
            link: Some((link, error)),
            ..Self::default()
        }
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn with_link(mut self, link: Link, error: Option<Error>) -> Self {
        self.link = Some((link, error));
        self
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn with_position(mut self, position: Option<Duration>) -> Self {
        self.position = Some(position);
        self
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn with_duration(mut self, duration: Option<Duration>) -> Self {
        self.duration = Some(duration);
        self
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn with_features(mut self, features: Features) -> Self {
        self.features = Some(features);
        self
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn with_output(
        mut self,
        scope: Option<VolumeScope>,
        volume: Option<Volume>,
        muted: Option<bool>,
    ) -> Self {
        self.output = Some((scope, volume, muted));
        self
    }
}
