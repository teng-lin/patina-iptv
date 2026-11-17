//! Stream resolution: a channel's [`StreamLocator`] plus its source's
//! playback context → the URL and headers the player opens.
//!
//! | Locator | Live | Catch-up (from a programme's start) |
//! | --- | --- | --- |
//! | `Url` (M3U, Xtream `direct_source`, static Stalker `cmd`) | the URL, live placeholders expanded | [`build_catchup_url`] (all M3U modes, `xc`) |
//! | `Xtream` | `/live/U/P/{id}.{ext}`, `ext` from the panel's allowed formats | `/timeshift/…` in the panel's clock |
//! | `Stalker` | the static `cmd` when the row allows it, else `create_link` (async) | unsupported (portal archive not implemented) |
//!
//! Headers: the source's (M3U playlist User-Agent/Referrer, the Xtream
//! account's User-Agent, Stalker's portal identity for portal-owned
//! streams), then the channel's own (`#EXTVLCOPT`, `|User-Agent=` …), which
//! win. Stalker identity headers are only added for portal-owned URLs.

use std::cell::RefCell;

use iptv_core::playback::PlaySource;
use iptv_providers::CatchupError;
use iptv_providers::m3u::{
    CatchupRequest, DEFAULT_CATCHUP_WINDOW_DAYS, build_catchup_url, expand_live_url,
};
use iptv_providers::model::{
    CatchUpKind, Channel, M3uSourceConfig, SourceConfig, StreamLocator, XtreamCredentials,
};
use iptv_providers::stalker::{StalkerSession, static_playback_url};
use iptv_providers::xtream::{
    AccountInfo, CatchupVariant, StreamFormat, XtreamClient, catchup_probe_order,
};

/// What the source contributes to playback: credentials, the account's
/// stream format and clock, or the authorised Stalker session.
#[derive(Clone, Debug)]
pub struct PlaybackContext {
    inner: Inner,
}

/// Immutable source snapshot that produced a cached playback context. It is
/// deliberately not serializable; diagnostics never expose its credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct SourceAuthority {
    source: i64,
    incarnation: String,
    config: SourceConfig,
}
impl std::fmt::Debug for SourceAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceAuthority")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}
impl SourceAuthority {
    pub fn new(source: i64, incarnation: String, config: SourceConfig) -> Self {
        Self {
            source,
            incarnation,
            config,
        }
    }
    /// Stored source identity captured with this schedule's connection details.
    pub fn source(&self) -> i64 {
        self.source
    }

    /// Changes on source recreation or connection edits, not guide refreshes.
    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }

    pub fn resolve(&self, channel: &Channel) -> ResolutionAuthority {
        ResolutionAuthority {
            source: std::sync::Arc::new(self.clone()),
            channel: channel.clone(),
        }
    }
}

/// Original source and channel used by one resolver, retained across provider
/// I/O and transport retirement. It cannot be rebound to a newer catalogue.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolutionAuthority {
    source: std::sync::Arc<SourceAuthority>,
    channel: Channel,
}
impl std::fmt::Debug for ResolutionAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolutionAuthority")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}
impl ResolutionAuthority {
    pub fn source(&self) -> i64 {
        self.source.source
    }
    pub fn incarnation(&self) -> &str {
        &self.source.incarnation
    }
    pub fn config(&self) -> &SourceConfig {
        &self.source.config
    }
    pub fn channel(&self) -> &Channel {
        &self.channel
    }
}

#[derive(Clone)]
enum Inner {
    M3u {
        user_agent: Option<String>,
        referrer: Option<String>,
    },
    Xtream {
        client: XtreamClient,
        extension: String,
        server_offset_minutes: Option<i32>,
        catchup: CatchupVariant,
    },
    Stalker {
        /// Updated after a re-authentication so the next link reuses it.
        session: RefCell<Box<StalkerSession>>,
    },
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::M3u { .. } => f
                .debug_struct("M3u")
                .field("user_agent", &"[redacted]")
                .field("referrer", &"[redacted]")
                .finish(),
            Self::Xtream {
                client,
                extension,
                server_offset_minutes,
                catchup,
            } => f
                .debug_struct("Xtream")
                .field("client", client)
                .field("extension", extension)
                .field("server_offset_minutes", server_offset_minutes)
                .field("catchup", catchup)
                .finish(),
            Self::Stalker { session } => {
                f.debug_struct("Stalker").field("session", session).finish()
            }
        }
    }
}

impl PlaybackContext {
    /// An M3U playlist: its playlist-wide User-Agent and Referrer.
    pub fn m3u(config: &M3uSourceConfig) -> Self {
        let clean = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            inner: Inner::M3u {
                user_agent: clean(&config.user_agent),
                referrer: clean(&config.referrer),
            },
        }
    }

    /// An Xtream account. `account` (from the import) gives the allowed
    /// output formats and the panel's clock; without it the live format is
    /// HLS and archive times use the viewer's clock.
    pub fn xtream(
        credentials: &XtreamCredentials,
        account: Option<&AccountInfo>,
        format: StreamFormat,
    ) -> Result<Self, iptv_providers::ProviderError> {
        let client = XtreamClient::from_credentials(credentials)?;
        let allowed = account.map_or(&[][..], |a| &a.allowed_output_formats[..]);
        Ok(Self {
            inner: Inner::Xtream {
                client,
                extension: format.resolve(allowed),
                server_offset_minutes: account.and_then(|a| a.server.clock.offset_minutes()),
                // The first variant IPTVnator would probe; probing itself is
                // not done yet (see the module docs of `xtream`).
                catchup: catchup_probe_order(allowed)[0],
            },
        })
    }

    /// A Stalker portal with the session the import authorised.
    pub fn stalker(session: StalkerSession) -> Self {
        Self {
            inner: Inner::Stalker {
                session: RefCell::new(Box::new(session)),
            },
        }
    }

    /// Overrides the Xtream archive URL shape (after probing the panel).
    pub fn with_catchup_variant(mut self, variant: CatchupVariant) -> Self {
        if let Inner::Xtream { catchup, .. } = &mut self.inner {
            *catchup = variant;
        }
        self
    }

    /// Keeps a session that re-authenticated while resolving a link.
    pub fn stalker_session(&self) -> Option<StalkerSession> {
        match &self.inner {
            Inner::Stalker { session } => Some((**session.borrow()).clone()),
            _ => None,
        }
    }

    pub fn update_stalker_session(&self, updated: StalkerSession) {
        if let Inner::Stalker { session } = &self.inner {
            **session.borrow_mut() = updated;
        }
    }

    /// Source-level headers for `url`.
    fn headers(&self, url: &str) -> Vec<(String, String)> {
        match &self.inner {
            Inner::M3u {
                user_agent,
                referrer,
            } => {
                let mut h = Vec::new();
                if let Some(ua) = user_agent {
                    h.push(("User-Agent".to_owned(), ua.clone()));
                }
                if let Some(r) = referrer {
                    h.push(("Referer".to_owned(), r.clone()));
                }
                h
            }
            Inner::Xtream { client, .. } => client.playback_headers(),
            Inner::Stalker { session } => session.borrow().playback_headers(url),
        }
    }

    fn server_offset_minutes(&self) -> Option<i32> {
        match &self.inner {
            Inner::Xtream {
                server_offset_minutes,
                ..
            } => *server_offset_minutes,
            _ => None,
        }
    }
}

/// The viewer's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// Unix seconds.
    pub now: i64,
    /// The viewer's UTC offset (date placeholders, Xtream fallback clock).
    pub local_offset_secs: i32,
}

/// An archived programme to play from its start (Unix seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Archive {
    pub start: i64,
    pub end: i64,
}

/// The result of [`resolve`].
#[derive(Debug)]
pub enum Resolution {
    Play(PlaySource),
    /// The portal must issue a temporary link first (`create_link`, valid
    /// for seconds: resolve right before playing, never store it). Send it
    /// with `session`, then build the source with [`StalkerLink::source`].
    StalkerLink(Box<StalkerLink>),
    Unavailable(Unavailable),
}

impl Resolution {
    pub fn ready(self) -> Option<PlaySource> {
        match self {
            Self::Play(s) => Some(s),
            _ => None,
        }
    }
}

/// A pending Stalker `create_link`.
pub struct StalkerLink {
    pub session: StalkerSession,
    pub cmd: String,
    /// The channel's own headers, applied over the portal's.
    pub channel_headers: Vec<(String, String)>,
}

impl std::fmt::Debug for StalkerLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<_> = self
            .channel_headers
            .iter()
            .map(|(name, _)| (name, "[redacted]"))
            .collect();
        f.debug_struct("StalkerLink")
            .field("session", &self.session)
            .field("cmd", &"[redacted]")
            .field("channel_headers", &headers)
            .finish()
    }
}

impl StalkerLink {
    /// The source for the link the portal returned, with the headers of
    /// `session` (after any re-authentication).
    ///
    /// The link must be on a public address, unless the portal the user
    /// entered is on a private one and serves the stream itself
    /// ([`StalkerSession::stream_must_be_public`]): a portal on the LAN
    /// plays, a public portal cannot point the player into the LAN.
    pub fn source(
        session: &StalkerSession,
        url: String,
        channel_headers: &[(String, String)],
    ) -> PlaySource {
        PlaySource {
            public_only: session.stream_must_be_public(&url),
            headers: session.playback_headers(&url),
            url,
        }
        .with_headers(channel_headers)
    }
}

/// Why nothing can be played.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// No imported channel behind this guide row.
    NoChannel,
    /// The locator needs the source's account (Xtream credentials, Stalker
    /// session) and the schedule has none.
    NeedsAccount,
    /// The programme cannot be replayed.
    NoArchive(CatchupError),
}

/// The URL and headers for `channel`, live or (`archive`) from a
/// programme's start.
pub fn resolve(
    context: Option<&PlaybackContext>,
    channel: &Channel,
    archive: Option<Archive>,
    clock: Clock,
) -> Resolution {
    match archive {
        None => live(context, channel, clock),
        Some(a) => catch_up(context, channel, a, clock),
    }
}

fn channel_headers(channel: &Channel) -> &[(String, String)] {
    match &channel.stream {
        StreamLocator::Url { headers, .. } => headers,
        _ => &[],
    }
}

fn source(context: Option<&PlaybackContext>, url: String, channel: &Channel) -> PlaySource {
    let headers = context.map_or_else(Vec::new, |c| c.headers(&url));
    let public_only = match (&channel.stream, context.map(|c| &c.inner)) {
        (StreamLocator::Xtream { .. }, Some(Inner::Xtream { client, .. })) => {
            client.stream_must_be_public(&url)
        }
        // A static Stalker link follows the rule of a resolved one
        // ([`StalkerLink::source`]): private only on a private portal's
        // own host.
        (StreamLocator::Stalker { .. }, Some(Inner::Stalker { session })) => {
            session.borrow().stream_must_be_public(&url)
        }
        _ => true,
    };
    PlaySource {
        url,
        headers,
        public_only,
    }
    .with_headers(channel_headers(channel))
}

fn live(context: Option<&PlaybackContext>, channel: &Channel, clock: Clock) -> Resolution {
    let inner = context.map(|c| &c.inner);
    match &channel.stream {
        StreamLocator::Url { url, .. } => {
            let url = expand_live_url(url, clock.now, clock.local_offset_secs);
            Resolution::Play(source(context, url, channel))
        }
        StreamLocator::Xtream { stream_id, .. } => match inner {
            Some(Inner::Xtream {
                client, extension, ..
            }) => Resolution::Play(source(
                context,
                client.live_url(*stream_id, extension),
                channel,
            )),
            _ => Resolution::Unavailable(Unavailable::NeedsAccount),
        },
        StreamLocator::Stalker { cmd, link } => match (static_playback_url(cmd, *link), inner) {
            (Some(url), _) => Resolution::Play(source(context, url, channel)),
            (None, Some(Inner::Stalker { session })) => {
                Resolution::StalkerLink(Box::new(StalkerLink {
                    session: (**session.borrow()).clone(),
                    cmd: cmd.clone(),
                    channel_headers: Vec::new(),
                }))
            }
            (None, _) => Resolution::Unavailable(Unavailable::NeedsAccount),
        },
    }
}

fn catch_up(
    context: Option<&PlaybackContext>,
    channel: &Channel,
    archive: Archive,
    clock: Clock,
) -> Resolution {
    let no = |e| Resolution::Unavailable(Unavailable::NoArchive(e));
    let Some(cu) = &channel.catch_up else {
        return no(CatchupError::Unsupported);
    };
    match (&channel.stream, context.map(|c| &c.inner)) {
        (
            StreamLocator::Xtream { stream_id, .. },
            Some(Inner::Xtream {
                client,
                server_offset_minutes,
                catchup,
                ..
            }),
        ) => {
            if !matches!(cu.kind, CatchUpKind::Xtream) {
                return no(CatchupError::Unsupported);
            }
            let days = i64::from(cu.window_days(DEFAULT_CATCHUP_WINDOW_DAYS));
            if archive.start > clock.now {
                return no(CatchupError::FutureProgramme);
            }
            if archive.start < clock.now - days * 86_400 || archive.end <= archive.start {
                return no(CatchupError::OutsideArchive);
            }
            let url = client.catchup_url(
                *stream_id,
                archive.start,
                archive.end,
                *catchup,
                *server_offset_minutes,
                clock.local_offset_secs,
            );
            Resolution::Play(source(context, url, channel))
        }
        (StreamLocator::Xtream { .. }, _) => Resolution::Unavailable(Unavailable::NeedsAccount),
        (StreamLocator::Stalker { .. }, _) => no(CatchupError::Unsupported),
        (StreamLocator::Url { .. }, _) => {
            let mut req = CatchupRequest::new(archive.start, archive.end, clock.now);
            req.local_offset_secs = clock.local_offset_secs;
            req.server_offset_minutes = context.and_then(PlaybackContext::server_offset_minutes);
            match build_catchup_url(channel, req) {
                Ok(url) => Resolution::Play(source(context, url, channel)),
                Err(e) => no(e),
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod diagnostic_regression {
    use super::*;
    #[test]
    fn contexts_pending_commands_and_resolutions_redact_secrets() {
        let context = PlaybackContext {
            inner: Inner::M3u {
                user_agent: Some("synthetic-private-marker".into()),
                referrer: Some("synthetic-private-marker".into()),
            },
        };
        assert!(!format!("{context:?}").contains("synthetic-private-marker"));
        let client = XtreamClient::new(
            "http://portal.example.test/synthetic-private-marker",
            "synthetic-user",
            "synthetic-password",
        )
        .unwrap()
        .with_user_agent(Some("synthetic-private-marker"));
        assert!(!format!("{client:?}").contains("synthetic-private-marker"));
        let xtream = PlaybackContext {
            inner: Inner::Xtream {
                client,
                extension: "ts".into(),
                server_offset_minutes: None,
                catchup: CatchupVariant::RestTs,
            },
        };
        assert!(!format!("{xtream:?}").contains("synthetic-private-marker"));
        let portal = iptv_providers::stalker::StalkerPortal::new(
            "http://portal.example.test/api",
            iptv_providers::model::MacAddress::parse("00:1A:79:00:00:42").unwrap(),
            iptv_providers::stalker::PortalMode::Full,
        )
        .unwrap();
        let link = StalkerLink {
            session: StalkerSession::new(portal),
            cmd: "synthetic-private-marker".into(),
            channel_headers: vec![("X-Key".into(), "synthetic-private-marker".into())],
        };
        assert!(!format!("{link:?}").contains("synthetic-private-marker"));
        assert!(
            !format!(
                "{:?}",
                Resolution::Play(PlaySource::new("synthetic-private-marker"))
            )
            .contains("synthetic-private-marker")
        );
    }
}
