//! Paired AirPlay v2 sender for URL video.
//!
//! The playback sequence follows the paired pyatv trial at 8848ad3f.
//! See NOTICE-pyatv.md for the upstream license.

mod hap;
mod video;
mod wire;

use crate::sender::{
    Availability, Backend, BackendConnect, BoxFuture, Candidate, Command, Confirmation, Credential,
    Delivery, Diagnostic, Dispatch, Driver, Error, ErrorKind, Feature, Features, Media,
    Observation, PairDriver, PeerTrust, PinCode, Playback, ProtocolId, Route, ScanSink, Stage,
    StopOutcome, VerifiedConnection,
};
use hap::{AuthFailure, AuthFault, Challenge, StoredKeys, Verified};
use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent};
use std::{
    collections::BTreeMap,
    net::{SocketAddr, SocketAddrV6},
    num::NonZeroU16,
    time::Duration,
};
use tokio::{
    task::JoinHandle,
    time::{Instant, sleep_until},
};
use video::{MediaState, Video, VideoFault};
use wire::Fault;

const SERVICE: &str = "_airplay._tcp.local.";
const ROUTE_RESOLVE: Duration = Duration::from_millis(1_500);
const APPROVAL_WINDOW: Duration = Duration::from_secs(60);
const MAX_ROUTES: usize = 128;

pub(crate) struct AirPlay {
    timing_port: Option<NonZeroU16>,
    #[cfg(test)]
    browse_daemon: Option<Box<dyn Fn() -> ServiceDaemon + Send + Sync>>,
}

impl AirPlay {
    pub(crate) fn new(timing_port: Option<NonZeroU16>) -> Self {
        Self {
            timing_port,
            #[cfg(test)]
            browse_daemon: None,
        }
    }

    fn new_daemon(&self) -> Result<ServiceDaemon, ErrorKind> {
        #[cfg(test)]
        if let Some(make_daemon) = &self.browse_daemon {
            return Ok(make_daemon());
        }
        ServiceDaemon::new().map_err(|_| ErrorKind::Unavailable)
    }

    async fn connect_route(
        &self,
        selected: &Route,
        credential: Option<Credential>,
    ) -> Result<BackendConnect, Error> {
        let selected = selected
            .payload::<Record>()
            .ok_or_else(|| failure(ErrorKind::InvalidInput, Stage::Connect))?;
        let identity = selected
            .identity
            .as_deref()
            .ok_or_else(|| failure(ErrorKind::Unsupported, Stage::Connect))?;
        // Resolve in the current network before requesting user approval. A
        // changed address is fine, but two instances claiming one ID are ambiguous.
        let records = browse(
            self.new_daemon()
                .map_err(|kind| failure(kind, Stage::Connect))?,
            Instant::now() + ROUTE_RESOLVE,
            None,
        )
        .await
        .map_err(|kind| failure(kind, Stage::Connect))?;
        let address = records
            .unique(identity)
            .ok_or_else(|| failure(ErrorKind::Unreachable, Stage::Connect))?
            .address;
        self.connect_address(address, credential).await
    }

    async fn connect_address(
        &self,
        address: SocketAddr,
        credential: Option<Credential>,
    ) -> Result<BackendConnect, Error> {
        if let Some(credential) = credential {
            let keys = StoredKeys::decode(credential.as_bytes())
                .map_err(|fault| auth_error(fault.into(), Stage::Connect))?;
            let verified = hap::verify(address, &keys)
                .await
                .map_err(|fault| auth_error(fault, Stage::Connect))?;
            return Ok(BackendConnect::Ready(Box::new(AirDriver::new(
                verified,
                self.timing_port,
            )?)));
        }

        let wire = hap::start_pairing(address)
            .await
            .map_err(|fault| auth_error(fault, Stage::Pair))?;
        let expires = Instant::now() + APPROVAL_WINDOW;
        let challenge = tokio::spawn(Challenge::receive(wire, address, expires));
        Ok(BackendConnect::PinRequired(Box::new(AirPairing {
            challenge: Some(challenge),
            expires,
            timing_port: self.timing_port,
        })))
    }
}

impl Backend for AirPlay {
    fn protocol(&self) -> ProtocolId {
        ProtocolId::new("airplay")
    }

    fn uses_credentials(&self) -> bool {
        true
    }

    fn discover(&self, deadline: Instant, sink: ScanSink) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            browse(
                self.new_daemon()
                    .map_err(|kind| failure(kind, Stage::Discovery))?,
                deadline,
                Some(&sink),
            )
            .await
            .map(|_| ())
            .map_err(|kind| failure(kind, Stage::Discovery))
        })
    }

    fn connect<'a>(
        &'a self,
        route: &'a Route,
        credential: Option<Credential>,
    ) -> BoxFuture<'a, Result<BackendConnect, Error>> {
        Box::pin(self.connect_route(route, credential))
    }
}

#[derive(Clone)]
struct Record {
    identity: Option<String>,
    name: String,
    address: SocketAddr,
}

impl Record {
    fn from_service(info: &ResolvedService) -> Option<Self> {
        let full_name = info.get_fullname();
        let suffix_start = full_name.len().checked_sub(SERVICE.len())?;
        let (instance, service) = full_name.split_at_checked(suffix_start)?;
        if !service.eq_ignore_ascii_case(SERVICE) {
            return None;
        }
        let name = instance
            .trim_end_matches('.')
            .chars()
            .filter(|ch| !ch.is_control())
            .take(80)
            .collect::<String>();
        let port = info.get_port();
        if port == 0 {
            return None;
        }
        let address = info
            .get_addresses()
            .iter()
            .filter_map(|ip| match ip {
                ScopedIp::V4(address)
                    if !address.addr().is_unspecified() && !address.addr().is_multicast() =>
                {
                    Some(SocketAddr::from((*address.addr(), port)))
                }
                ScopedIp::V6(address)
                    if !address.addr().is_unspecified()
                        && !address.addr().is_multicast()
                        && (!address.addr().is_unicast_link_local()
                            || address.scope_id().index != 0) =>
                {
                    Some(SocketAddr::V6(SocketAddrV6::new(
                        *address.addr(),
                        port,
                        0,
                        address.scope_id().index,
                    )))
                }
                _ => None,
            })
            .min()?;
        Some(Self {
            identity: info
                .get_property_val_str("deviceid")
                .and_then(normalize_identity),
            name: if name.trim().is_empty() {
                "AirPlay".into()
            } else {
                name
            },
            address,
        })
    }

    fn candidate(&self) -> Candidate {
        let supports_pairing = self.identity.is_some();
        Candidate::new(
            self.name.clone(),
            self.identity.clone(),
            self.clone(),
            Features::new(Availability::Unsupported).with(
                Feature::LoadVideoUrl,
                if supports_pairing {
                    Availability::Unknown
                } else {
                    Availability::Unavailable
                },
            ),
        )
    }
}

fn normalize_identity(value: &str) -> Option<String> {
    let mut out = String::with_capacity(12);
    for octet in value.split(':') {
        if octet.len() != 2 || !octet.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        out.push_str(octet);
    }
    (out.len() == 12 && out != "000000000000").then(|| out.to_ascii_lowercase())
}

#[derive(Default)]
struct Records(BTreeMap<String, Record>);

impl Records {
    fn unique(&self, id: &str) -> Option<&Record> {
        let mut matches = self
            .0
            .values()
            .filter(|record| record.identity.as_deref() == Some(id));
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    fn update(&mut self, name: String, replacement: Option<Record>, sink: Option<&ScanSink>) {
        let name = name.to_ascii_lowercase();
        let next_id = replacement
            .as_ref()
            .and_then(|record| record.identity.clone());
        let previous = if let Some(replacement) = replacement {
            if self.0.len() >= MAX_ROUTES && !self.0.contains_key(&name) {
                return;
            }
            self.0.insert(name, replacement)
        } else {
            self.0.remove(&name)
        };
        let Some(sink) = sink else { return };
        let previous_id = previous.and_then(|record| record.identity);
        for id in [previous_id, next_id].into_iter().flatten() {
            if let Some(record) = self.unique(&id) {
                sink.push(record.candidate());
            } else {
                sink.remove(&id);
            }
        }
    }

    fn record_event(&mut self, event: ServiceEvent, sink: Option<&ScanSink>) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                self.update(
                    info.get_fullname().into(),
                    Record::from_service(&info),
                    sink,
                );
            }
            ServiceEvent::ServiceRemoved(_, name) => self.update(name, None, sink),
            _ => {}
        }
    }
}

async fn browse(
    daemon: ServiceDaemon,
    deadline: Instant,
    sink: Option<&ScanSink>,
) -> Result<Records, ErrorKind> {
    struct Browse(ServiceDaemon);
    impl Drop for Browse {
        fn drop(&mut self) {
            let _ = self.0.stop_browse(SERVICE);
            let _ = self.0.shutdown();
        }
    }

    let daemon = Browse(daemon);
    let receiver = daemon
        .0
        .browse(SERVICE)
        .map_err(|_| ErrorKind::Unavailable)?;
    let mut records = Records::default();
    let sleep = sleep_until(deadline);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            biased;
            _ = &mut sleep => break,
            event = receiver.recv_async() => {
                records.record_event(event.map_err(|_| ErrorKind::Unavailable)?, sink);
            }
        }
    }
    if let Some(sink) = sink {
        for record in records
            .0
            .values()
            .filter(|record| record.identity.is_none())
        {
            sink.push(record.candidate());
        }
    }
    Ok(records)
}

fn failure(kind: ErrorKind, stage: Stage) -> Error {
    Error::new(kind, stage, Delivery::NotSent)
}

fn wire_kind(fault: Fault) -> ErrorKind {
    match fault {
        Fault::Closed | Fault::Unreachable => ErrorKind::Unreachable,
        Fault::InvalidMessage | Fault::Integrity => ErrorKind::Protocol,
        Fault::Refused(_) => ErrorKind::ReceiverRefused,
    }
}

fn auth_error(AuthFailure(fault, delivery): AuthFailure, stage: Stage) -> Error {
    let kind = match fault {
        AuthFault::Wire(fault) => wire_kind(fault),
        AuthFault::TimedOut => ErrorKind::TimedOut,
        AuthFault::ApprovalDenied => ErrorKind::ApprovalDenied,
        AuthFault::InvalidPin => ErrorKind::InvalidPin,
        AuthFault::IdentityMismatch => ErrorKind::IdentityMismatch,
        AuthFault::InvalidCredential => ErrorKind::InvalidCredential,
        AuthFault::Unavailable => ErrorKind::Unavailable,
    };
    Error::new(kind, stage, delivery)
}

fn video_error(fault: VideoFault, stage: Stage, delivery: Delivery) -> Error {
    let kind = match fault {
        VideoFault::Wire(fault) => wire_kind(fault),
        VideoFault::TimedOut
        | VideoFault::Timing {
            timed_out: true, ..
        } => ErrorKind::TimedOut,
        VideoFault::Unavailable
        | VideoFault::Timing {
            timed_out: false, ..
        } => ErrorKind::Unavailable,
    };
    let error = Error::new(kind, stage, delivery);
    if let VideoFault::Timing { local, .. } = fault {
        error.with_diagnostic(Diagnostic::TimingListener { local_addr: local })
    } else {
        error
    }
}

struct AirPairing {
    challenge: Option<JoinHandle<Result<Challenge, AuthFailure>>>,
    expires: Instant,
    timing_port: Option<NonZeroU16>,
}

impl Drop for AirPairing {
    fn drop(&mut self) {
        if let Some(challenge) = &self.challenge {
            challenge.abort()
        }
    }
}

impl PairDriver for AirPairing {
    fn expires_at(&self) -> Option<std::time::Instant> {
        Some(self.expires.into_std())
    }

    fn submit(&mut self, pin: PinCode) -> BoxFuture<'_, Result<VerifiedConnection, Error>> {
        Box::pin(async move {
            let challenge = self
                .challenge
                .as_mut()
                .ok_or_else(|| failure(ErrorKind::Cancelled, Stage::Pair))?;
            let ready = challenge.await;
            self.challenge.take();
            let ready = ready
                .map_err(|_| failure(ErrorKind::Cancelled, Stage::Pair))?
                .map_err(|fault| auth_error(fault, Stage::Pair))?;
            let (verified, keys) = ready
                .finish(pin.as_str())
                .await
                .map_err(|fault| auth_error(fault, Stage::Pair))?;
            let driver: Box<dyn Driver> = Box::new(AirDriver::new(verified, self.timing_port)?);
            Ok((driver, Credential::from_bytes(keys.encode())))
        })
    }

    fn cancel(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Some(challenge) = self.challenge.take() {
                challenge.abort();
                let _ = challenge.await;
            }
        })
    }
}

struct AirDriver {
    video: Video,
    timing_port: Option<NonZeroU16>,
}

impl AirDriver {
    fn new(verified: Verified, timing_port: Option<NonZeroU16>) -> Result<Self, Error> {
        let video = Video::new(verified)
            .map_err(|fault| video_error(fault, Stage::Connect, Delivery::NotSent))?;
        Ok(Self { video, timing_port })
    }
}

fn active_features() -> Features {
    Features::new(Availability::Unsupported)
        .with(Feature::LoadVideoUrl, Availability::Available)
        .with(Feature::Stop, Availability::Available)
}

impl Driver for AirDriver {
    fn control_addrs(&self) -> Option<(SocketAddr, SocketAddr)> {
        Some(self.video.addresses())
    }

    fn peer_trust(&self) -> PeerTrust {
        PeerTrust::VerifiedPairing
    }

    fn validate(&self, _media: &Media) -> Result<(), Error> {
        Ok(())
    }

    fn load<'a>(
        &'a mut self,
        media: Media,
        dispatch: &'a Dispatch,
    ) -> BoxFuture<'a, Result<Observation, Error>> {
        Box::pin(async move {
            let prepared = self
                .video
                .prepare(self.timing_port, media.url())
                .await
                .map_err(|fault| video_error(fault, Stage::Load, Delivery::NotSent))?;
            dispatch.mark_sent()?;
            self.video.start(prepared).await.map_err(|fault| {
                video_error(fault, Stage::Load, Delivery::MayHaveReachedReceiver)
            })?;
            Ok(Observation::playback(Playback::Accepted).with_features(active_features()))
        })
    }

    fn observe(&mut self) -> BoxFuture<'_, Result<Observation, Error>> {
        Box::pin(async move {
            let evidence = self
                .video
                .observe()
                .await
                .map_err(|fault| video_error(fault, Stage::Observe, Delivery::NotSent))?;
            let playback = match evidence.state {
                MediaState::Buffering => Playback::Buffering,
                MediaState::Playing => Playback::Playing,
                MediaState::Paused => Playback::Paused,
                MediaState::Ended => Playback::Ended,
                MediaState::Absent => Playback::NoLongerPresent,
            };
            Ok(Observation::playback(playback)
                .with_features(active_features())
                .with_position(evidence.position)
                .with_duration(evidence.duration))
        })
    }

    fn command(&mut self, _command: Command) -> BoxFuture<'_, Result<Confirmation, Error>> {
        Box::pin(async { Err(failure(ErrorKind::Unsupported, Stage::Control)) })
    }

    fn stop(&mut self) -> BoxFuture<'_, Result<StopOutcome, Error>> {
        Box::pin(async move {
            self.video.close().await;
            Ok(StopOutcome::TransportClosed)
        })
    }

    fn close(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(self.video.close())
    }
}

#[cfg(test)]
mod tests;
