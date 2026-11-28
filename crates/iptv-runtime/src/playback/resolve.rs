//! Runtime-owned stream resolution for local playback, external handoffs, and Cast.

use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use iptv_core::guide::Schedule;
use iptv_core::playback::PlaySource;
use iptv_guide::playback::{PlaybackContext, Resolution, ResolutionAuthority, StalkerLink};
use iptv_import::{CancelToken, FailureKind, FetchErrorKind, ImportError};
use iptv_library::ImportErrorKind;

/// Interactive deadline for Stalker `create_link` resolution.
pub const STALKER_LINK_TIMEOUT: Duration = Duration::from_secs(12);

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

fn classify_import_error(error: &ImportError) -> ResolveFailure {
    match error.kind() {
        FailureKind::Canceled => ResolveFailure::Cancelled,
        FailureKind::Network => match error {
            ImportError::Network { error, .. }
                if matches!(
                    error.kind,
                    FetchErrorKind::Timeout | FetchErrorKind::Deadline
                ) =>
            {
                ResolveFailure::TimedOut
            }
            _ => ResolveFailure::Network,
        },
        kind => ResolveFailure::Portal(kind.into()),
    }
}

impl crate::Runtime {
    /// The playback authority for `channel` in `schedule`, if it comes from a stored source.
    pub fn playback_authority(
        schedule: &dyn Schedule,
        channel: usize,
    ) -> Option<ResolutionAuthority> {
        let (epg, local) = iptv_guide::combined::epg_channel(schedule, channel)?;
        epg.resolution_authority(local)
    }

    /// The shared playback context for `channel` in `schedule`, if any.
    pub fn playback_context(
        schedule: &dyn Schedule,
        channel: usize,
    ) -> Option<Rc<PlaybackContext>> {
        let (epg, _) = iptv_guide::combined::epg_channel(schedule, channel)?;
        epg.shared_playback()
    }

    /// Cancels any pending stream resolution for `purpose`.
    pub fn cancel_stream_resolution(&self, purpose: StreamPurpose) {
        if let Some((_, cancel)) = self
            .catalog
            .pending_resolutions()
            .borrow_mut()
            .remove(&purpose)
        {
            cancel.cancel();
        }
    }

    /// Cancels every pending stream resolution across all purposes (shutdown).
    pub(crate) fn cancel_all_stream_resolutions(&self) {
        for (_, (_, cancel)) in self.catalog.pending_resolutions().borrow_mut().drain() {
            cancel.cancel();
        }
    }

    fn begin_stream_resolution(&self, purpose: StreamPurpose) -> (u64, CancelToken) {
        self.cancel_stream_resolution(purpose);
        let ticket = self.catalog.next_resolve_ticket();
        let cancel = CancelToken::new();
        self.catalog
            .pending_resolutions()
            .borrow_mut()
            .insert(purpose, (ticket, cancel.clone()));
        (ticket, cancel)
    }

    fn finish_stream_resolution(&self, purpose: StreamPurpose, ticket: u64) -> bool {
        let mut pending = self.catalog.pending_resolutions().borrow_mut();
        if pending.get(&purpose).is_some_and(|(at, _)| *at == ticket) {
            pending.remove(&purpose);
            true
        } else {
            false
        }
    }

    /// Resolves `resolution` to a playable source. Stalker links are fetched
    /// on the network runtime under a tracked, cancellable token with a
    /// bounded interactive deadline, and the refreshed portal session is
    /// written back only if this request is still current.
    pub fn resolve_stream(
        self: &Rc<Self>,
        purpose: StreamPurpose,
        session: u64,
        resolution: Resolution,
        context: Option<Rc<PlaybackContext>>,
    ) -> impl Future<Output = Result<PlaySource, ResolveFailure>> + use<> {
        self.resolve_stream_within(purpose, session, resolution, context, STALKER_LINK_TIMEOUT)
    }

    /// Like [`Runtime::resolve_stream`], with an explicit interactive deadline
    /// for synthetic timeout tests.
    #[cfg(feature = "test-support")]
    pub fn resolve_stream_with_timeout(
        self: &Rc<Self>,
        purpose: StreamPurpose,
        session: u64,
        resolution: Resolution,
        context: Option<Rc<PlaybackContext>>,
        timeout: Duration,
    ) -> impl Future<Output = Result<PlaySource, ResolveFailure>> + use<> {
        self.resolve_stream_within(purpose, session, resolution, context, timeout)
    }

    fn resolve_stream_within(
        self: &Rc<Self>,
        purpose: StreamPurpose,
        _session: u64,
        resolution: Resolution,
        context: Option<Rc<PlaybackContext>>,
        timeout: Duration,
    ) -> impl Future<Output = Result<PlaySource, ResolveFailure>> + use<> {
        self.cancel_stream_resolution(purpose);
        let pending = match resolution {
            Resolution::Play(source) => Err(Ok(source)),
            Resolution::Unavailable(_) => Err(Err(ResolveFailure::Unavailable)),
            Resolution::StalkerLink(link) => {
                let (ticket, cancel) = self.begin_stream_resolution(purpose);
                let client = crate::playback::link_client(self, self.network_config());
                let services = self.services();
                let StalkerLink {
                    session: mut portal,
                    cmd,
                    channel_headers,
                } = *link;
                let token = cancel.clone();
                let deadline = crate::time::Instant::now() + timeout;
                let task_services = services.clone();
                let request = services.spawn(async move {
                    let timed = task_services
                        .timeout_at(deadline, async {
                            let net = match client.await {
                                Ok(net) => net,
                                Err(_) => return Err(ResolveFailure::Network),
                            };
                            iptv_import::stalker_link(&net, &mut portal, &cmd, false, &token)
                                .await
                                .map(|url| StalkerLink::source(&portal, url, &channel_headers))
                                .map_err(|error| classify_import_error(&error))
                        })
                        .await;
                    let outcome = match timed {
                        Ok(result) => result,
                        Err(_) if token.is_canceled() => Err(ResolveFailure::Cancelled),
                        Err(_) => {
                            token.cancel();
                            Err(ResolveFailure::TimedOut)
                        }
                    };
                    (outcome, portal)
                });
                Ok((Rc::downgrade(self), ticket, cancel, request))
            }
        };
        async move {
            let (owner, ticket, cancel, request) = match pending {
                Err(immediate) => return immediate,
                Ok(pending) => pending,
            };
            let answered = request.await;
            let Some(owner) = owner.upgrade() else {
                return Err(ResolveFailure::Cancelled);
            };
            let current = owner.finish_stream_resolution(purpose, ticket);
            if !current {
                return Err(ResolveFailure::Cancelled);
            }
            let Ok((outcome, portal)) = answered else {
                return Err(ResolveFailure::Cancelled);
            };
            if outcome == Err(ResolveFailure::TimedOut) {
                return Err(ResolveFailure::TimedOut);
            }
            if cancel.is_canceled() {
                return Err(ResolveFailure::Cancelled);
            }
            if portal.is_authorized()
                && let Some(context) = context
            {
                context.update_stalker_session(portal);
            }
            outcome
        }
    }
}
