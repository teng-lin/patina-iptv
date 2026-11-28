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
use hap::{AuthFailure, AuthFault, Challenge, SecureSession, StoredKeys};
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
use video::{MediaState, TimingPort, Video, VideoFault};
use wire::Fault;

const SERVICE: &str = "_airplay._tcp.local.";
const ROUTE_RESOLVE: Duration = Duration::from_millis(1_500);
const APPROVAL_WINDOW: Duration = Duration::from_secs(60);
const MAX_ROUTES: usize = 128;

pub(crate) struct AirPlay {
    timing_port: TimingPort,
    #[cfg(test)]
    browse_daemon: Option<Box<dyn Fn() -> ServiceDaemon + Send + Sync>>,
}

impl AirPlay {
    pub(crate) fn new(timing_port: Option<NonZeroU16>) -> Self {
        Self {
            timing_port: TimingPort {
                port: timing_port,
                fallback: false,
            },
            #[cfg(test)]
            browse_daemon: None,
        }
    }

    /// Bind an ephemeral timing port when the preferred one is unavailable.
    pub(crate) fn with_timing_fallback(mut self, fallback: bool) -> Self {
        self.timing_port.fallback = fallback;
        self
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
        let deadline = Instant::now() + ROUTE_RESOLVE;
        let records = self
            .discover_records(deadline, Some(identity), None)
            .await
            .map_err(|kind| failure(kind, Stage::Connect))?;
        let record = records
            .resolve_unique(identity)
            .map_err(|kind| failure(kind, Stage::Connect))?;
        self.connect_addresses(&record.addresses, record.pin_hint, credential)
            .await
    }

    async fn discover_records(
        &self,
        deadline: Instant,
        identity: Option<&str>,
        sink: Option<&ScanSink>,
    ) -> Result<Records, ErrorKind> {
        #[cfg(all(target_os = "linux", feature = "avahi"))]
        {
            // Unit tests never read the host's Avahi daemon: real LAN devices
            // would make them nondeterministic.
            let use_avahi = !cfg!(test);
            if use_avahi {
                // Keep part of the common budget for mDNS if Avahi is
                // unavailable or cannot see this network.
                let avahi_deadline = std::time::Instant::now()
                    + deadline.saturating_duration_since(Instant::now()) / 2;
                if let Ok(Ok(resolved)) = tokio::task::spawn_blocking(move || {
                    crate::discovery::avahi::discover_airplay(avahi_deadline)
                })
                .await
                {
                    let records = Records::from_avahi(resolved, identity);
                    let matched = match identity {
                        Some(id) => records.unique(id).is_some(),
                        None => !records.instances.is_empty(),
                    };
                    if matched {
                        if let Some(sink) = sink {
                            records.publish(sink);
                        }
                        return Ok(records);
                    }
                }
            }
        }
        browse(self.new_daemon()?, deadline, identity, sink).await
    }

    async fn connect_addresses(
        &self,
        addresses: &[SocketAddr],
        pin_hint: PinHint,
        credential: Option<Credential>,
    ) -> Result<BackendConnect, Error> {
        let keys = credential
            .map(|credential| StoredKeys::decode(credential.as_bytes()))
            .transpose()
            .map_err(|fault| auth_error(fault.into(), Stage::Connect))?;
        // Fail over only while opening TCP. Once a socket wins, its peer alone
        // receives trust messages, even if it rejects them or disappears.
        let (address, wire) = hap::open_candidates(addresses)
            .await
            .map_err(|fault| auth_error(fault.into(), Stage::Connect))?;
        if let Some(keys) = keys {
            let verified = hap::verify_on_wire(wire, &keys)
                .await
                .map_err(|fault| auth_error(fault, Stage::Connect))?;
            return Ok(BackendConnect::Ready(Box::new(AirDriver::new(
                verified,
                self.timing_port,
            )?)));
        }

        let pairing = if pin_hint == PinHint::Required {
            hap::start_pairing_on_wire(wire).await
        } else {
            match hap::transient_on_wire(wire).await {
                Ok(secure) => {
                    return Ok(BackendConnect::Ready(Box::new(AirDriver::new(
                        secure,
                        self.timing_port,
                    )?)));
                }
                Err(AuthFailure(AuthFault::ApprovalDenied, Delivery::Acknowledged)) => {
                    if pin_hint == PinHint::Suppressed {
                        return Err(Error::new(
                            ErrorKind::ReceiverRefused,
                            Stage::Connect,
                            Delivery::Acknowledged,
                        ));
                    }
                    // Restart approval only at the chosen endpoint that asked for it.
                    hap::start_pairing(address).await
                }
                Err(fault) => return Err(auth_error(fault, Stage::Connect)),
            }
        };
        let wire = pairing.map_err(|fault| auth_error(fault, Stage::Pair))?;
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
            self.discover_records(deadline, None, Some(&sink))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinHint {
    Unspecified,
    Required,
    Suppressed,
}

impl PinHint {
    fn from_service(info: &ResolvedService) -> Self {
        Self::from_txt(|name| info.get_property_val_str(name))
    }

    fn from_txt<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Self {
        const PIN_REQUIRED: u32 = 0x8;
        const PASSWORD_REQUIRED: u32 = 0x80;
        let flags = ["sf", "flags"].map(|name| {
            let value = get(name)?;
            let digits = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
                .unwrap_or(value);
            if digits.len() > 8 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            u32::from_str_radix(digits, 16).ok()
        });
        // TXT access hints only choose the on-screen flow. Authentication
        // still depends on the handshake on the selected connection.
        if get("act") == Some("2")
            || get("acl") == Some("1")
            || get("pw").is_some_and(|value| value.eq_ignore_ascii_case("true"))
            || flags
                .iter()
                .flatten()
                .any(|value| value & PASSWORD_REQUIRED != 0)
        {
            Self::Suppressed
        } else if flags.iter().any(Option::is_some)
            && flags
                .iter()
                .flatten()
                .all(|value| value & PIN_REQUIRED != 0)
        {
            Self::Required
        } else {
            Self::Unspecified
        }
    }
}

#[derive(Clone)]
struct Record {
    identity: Option<String>,
    name: String,
    addresses: Vec<SocketAddr>,
    pin_hint: PinHint,
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
        let mut addresses: Vec<_> = info
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
            .collect();
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.is_empty() {
            return None;
        }
        Some(Self {
            identity: info
                .get_property_val_str("deviceid")
                .and_then(normalize_identity),
            name: if name.trim().is_empty() {
                "AirPlay".into()
            } else {
                name
            },
            addresses,
            pin_hint: PinHint::from_service(info),
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
struct Records {
    instances: BTreeMap<String, Record>,
    saturated: bool,
}

impl Records {
    fn unique(&self, id: &str) -> Option<&Record> {
        let mut matches = self
            .instances
            .values()
            .filter(|record| record.identity.as_deref() == Some(id));
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    fn resolve_unique(&self, id: &str) -> Result<&Record, ErrorKind> {
        if self.saturated {
            return Err(ErrorKind::Unavailable);
        }
        self.unique(id).ok_or(ErrorKind::Unreachable)
    }

    fn update(&mut self, name: String, replacement: Option<Record>, sink: Option<&ScanSink>) {
        let name = name.to_ascii_lowercase();
        let next_id = replacement
            .as_ref()
            .and_then(|record| record.identity.clone());
        let previous = if let Some(replacement) = replacement {
            if self.instances.len() >= MAX_ROUTES && !self.instances.contains_key(&name) {
                self.saturated = true;
                return;
            }
            self.instances.insert(name, replacement)
        } else {
            self.instances.remove(&name)
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

    fn record_event(
        &mut self,
        event: ServiceEvent,
        identity: Option<&str>,
        sink: Option<&ScanSink>,
    ) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let record = Record::from_service(&info).filter(|record| {
                    identity.is_none_or(|identity| record.identity.as_deref() == Some(identity))
                });
                self.update(info.get_fullname().into(), record, sink);
            }
            ServiceEvent::ServiceRemoved(_, name) => self.update(name, None, sink),
            _ => {}
        }
    }

    #[cfg(all(target_os = "linux", feature = "avahi"))]
    fn from_avahi(
        resolved: Vec<crate::discovery::avahi::ResolvedRecord>,
        identity: Option<&str>,
    ) -> Self {
        use std::net::IpAddr;

        let mut records = Self::default();
        let mut grouped: BTreeMap<String, Record> = BTreeMap::new();
        for entry in resolved {
            if entry.port == 0 {
                continue;
            }
            let address = match entry.address {
                IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_multicast() => {
                    SocketAddr::from((ip, entry.port))
                }
                IpAddr::V6(ip) if !ip.is_unspecified() && !ip.is_multicast() => {
                    let scope = if ip.is_unicast_link_local() {
                        match u32::try_from(entry.interface) {
                            Ok(index) if index != 0 => index,
                            _ => continue,
                        }
                    } else {
                        0
                    };
                    SocketAddr::V6(SocketAddrV6::new(ip, entry.port, 0, scope))
                }
                _ => continue,
            };
            let txt = |key: &str| {
                entry.txt.iter().find_map(|raw| {
                    let text = std::str::from_utf8(raw).ok()?;
                    let (name, value) = text.split_once('=')?;
                    name.eq_ignore_ascii_case(key).then_some(value)
                })
            };
            let record_identity = txt("deviceid").and_then(normalize_identity);
            if identity.is_some_and(|want| record_identity.as_deref() != Some(want)) {
                continue;
            }
            let name = entry
                .name
                .chars()
                .filter(|ch| !ch.is_control())
                .take(80)
                .collect::<String>();
            let key = entry.name.to_ascii_lowercase();
            let pin_hint = PinHint::from_txt(txt);
            grouped
                .entry(key)
                .and_modify(|existing| {
                    if !existing.addresses.contains(&address) {
                        existing.addresses.push(address);
                        existing.addresses.sort_unstable();
                    }
                    if existing.identity.is_none() {
                        existing.identity = record_identity.clone();
                    }
                    // A sparse resolve must not hide a PIN another resolve requires.
                    if pin_hint == PinHint::Required || existing.pin_hint == PinHint::Unspecified {
                        existing.pin_hint = pin_hint;
                    }
                })
                .or_insert_with(|| Record {
                    identity: record_identity,
                    name: if name.trim().is_empty() {
                        "AirPlay".into()
                    } else {
                        name
                    },
                    addresses: vec![address],
                    pin_hint,
                });
        }
        for (key, record) in grouped {
            records.update(key, Some(record), None);
        }
        records
    }

    #[cfg(all(target_os = "linux", feature = "avahi"))]
    fn publish(&self, sink: &ScanSink) {
        let mut seen = std::collections::BTreeSet::new();
        for record in self.instances.values() {
            match &record.identity {
                Some(id) if seen.insert(id.as_str()) => {
                    if let Some(unique) = self.unique(id) {
                        sink.push(unique.candidate());
                    }
                }
                Some(_) => {}
                None => sink.push(record.candidate()),
            }
        }
    }
}

async fn browse(
    daemon: ServiceDaemon,
    deadline: Instant,
    identity: Option<&str>,
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
                records.record_event(event.map_err(|_| ErrorKind::Unavailable)?, identity, sink);
            }
        }
    }
    if let Some(sink) = sink {
        for record in records
            .instances
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
        AuthFault::GuestRefused => ErrorKind::ReceiverRefused,
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
    timing_port: TimingPort,
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
    trust: PeerTrust,
    timing_port: TimingPort,
}

impl AirDriver {
    fn new(secure: SecureSession, timing_port: TimingPort) -> Result<Self, Error> {
        let trust = secure.trust;
        let video = Video::new(secure)
            .map_err(|fault| video_error(fault, Stage::Connect, Delivery::NotSent))?;
        Ok(Self {
            video,
            trust,
            timing_port,
        })
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
        self.trust
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
