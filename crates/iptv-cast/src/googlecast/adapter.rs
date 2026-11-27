//! The Google Cast wire protocol behind the shared sender handles.
use super::{Command as WireCommand, Error as WireError, Session as WireSession, Status};
use crate::{
    Availability, Command, Confirmation, Credential, Delivery, Diagnostic, Error, ErrorKind,
    Feature, Link, Media, OutputChange, PeerTrust, Playback, ProtocolId, Route, Stage, StopOutcome,
    Volume, VolumeScope, discovery,
    sender::{
        Backend, BackendConnect, BoxFuture, Candidate, Dispatch, Driver, Features, Observation,
        ScanSink,
    },
};
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    sync::oneshot,
    time::{Instant, timeout, timeout_at},
};

// The shared connect worker has a 30-second deadline. Finish first so a
// transport error can retain the receiver's LAUNCH delivery evidence.
const CONNECT_WINDOW: Duration = Duration::from_secs(25);
const ROUTE_WINDOW: Duration = Duration::from_secs(5);
const RECENT_SCAN: Duration = Duration::from_secs(30);
const RECONCILE: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(750);
const OUTPUT_PROBE: Duration = Duration::from_millis(500);
const IDLE_GRACE: Duration = Duration::from_secs(15);

#[derive(Default)]
pub(crate) struct GoogleCast {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) loopback: Option<SocketAddr>,
}

impl GoogleCast {
    async fn discover_endpoints(
        &self,
        deadline: Instant,
    ) -> std::io::Result<Vec<discovery::Receiver>> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(address) = self.loopback {
            return Ok(vec![discovery::Receiver {
                name: "Generated Chromecast".into(),
                address,
                id: Some("generated-loopback".into()),
            }]);
        }
        discovery::discover_until(deadline).await
    }
}

#[derive(Clone)]
struct Endpoint {
    address: SocketAddr,
    id: Option<String>,
    scanned: Instant,
}

impl Endpoint {
    async fn resolve(&self, backend: &GoogleCast) -> Result<SocketAddr, Error> {
        if let Some(id) = &self.id {
            let deadline = Instant::now() + ROUTE_WINDOW;
            let devices = backend
                .discover_endpoints(deadline)
                .await
                .map_err(|_| connect_error())?;
            unique_endpoint(id, devices)
        } else if self.scanned.elapsed() < RECENT_SCAN {
            Ok(self.address)
        } else {
            Err(connect_error())
        }
    }
}

fn unique_endpoint(id: &str, devices: Vec<discovery::Receiver>) -> Result<SocketAddr, Error> {
    let mut devices = devices
        .into_iter()
        .filter(|receiver| receiver.id.as_deref() == Some(id))
        .map(|receiver| receiver.address);
    let address = devices.next().ok_or_else(connect_error)?;
    if devices.any(|candidate| candidate != address) {
        return Err(connect_error());
    }
    Ok(address)
}

fn connect_error() -> Error {
    Error::new(ErrorKind::Unreachable, Stage::Connect, Delivery::NotSent)
}

impl Backend for GoogleCast {
    fn protocol(&self) -> ProtocolId {
        ProtocolId::new("googlecast")
    }

    fn discover(&self, deadline: Instant, sink: ScanSink) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let receivers = self.discover_endpoints(deadline).await.map_err(|error| {
                let kind = if error.kind() == std::io::ErrorKind::TimedOut {
                    ErrorKind::TimedOut
                } else {
                    ErrorKind::Unreachable
                };
                Error::new(kind, Stage::Discovery, Delivery::NotSent)
            })?;
            let scanned = Instant::now();
            for receiver in receivers {
                sink.push(Candidate::new(
                    receiver.name,
                    receiver.id.clone(),
                    Endpoint {
                        address: receiver.address,
                        id: receiver.id,
                        scanned,
                    },
                    Features::new(Availability::Unknown)
                        .with(Feature::LoadVideoUrl, Availability::Available),
                ));
            }
            Ok(())
        })
    }

    fn connect<'a>(
        &'a self,
        route: &'a Route,
        _credential: Option<Credential>,
    ) -> BoxFuture<'a, Result<BackendConnect, Error>> {
        Box::pin(async move {
            let deadline = Instant::now() + CONNECT_WINDOW;
            let address = route
                .payload::<Endpoint>()
                .ok_or_else(connect_error)?
                .resolve(self)
                .await?;
            let wire = WireSession::connect_bounded(address, deadline)
                .await
                .map_err(|(error, delivery)| translate(error, Stage::Connect, delivery))?;
            Ok(BackendConnect::Ready(Box::new(CastDriver::new(wire)?)))
        })
    }
}

struct PendingLoad {
    url: String,
    reply: oneshot::Receiver<super::Result<Value>>,
}

struct CastDriver {
    wire: Option<WireSession>,
    token: String,
    pending: Option<PendingLoad>,
    owned: Option<i64>,
    live: bool,
    idle_since: Option<Instant>,
    poll_after: Instant,
}

impl CastDriver {
    fn new(wire: WireSession) -> Result<Self, Error> {
        let token = super::random_token().map_err(|_| connect_error())?;
        Ok(Self {
            wire: Some(wire),
            token: format!("{token:032x}"),
            pending: None,
            owned: None,
            live: false,
            idle_since: None,
            poll_after: Instant::now(),
        })
    }

    fn wire(&self) -> &WireSession {
        self.wire
            .as_ref()
            .expect("driver owns the control socket until close")
    }

    async fn submit(&mut self, media: Media, dispatch: &Dispatch) -> Result<Observation, Error> {
        let content_type = content_type(&media)?;
        let wire = self.wire();
        let permit = wire
            .request_slot()
            .await
            .map_err(|error| translate(error, Stage::Load, Delivery::NotSent))?;
        dispatch.mark_sent()?;
        let reply = wire.queue_load(
            permit,
            super::load_body(
                media.url(),
                content_type,
                media.is_live(),
                Some(&self.token),
            ),
        );
        self.live = media.is_live();
        self.pending = Some(PendingLoad {
            url: media.url().to_string(),
            reply,
        });
        let pending = self.pending.as_mut().expect("load was just queued");
        let received = timeout(super::DEADLINE, &mut pending.reply)
            .await
            .map_err(|_| {
                Error::new(
                    ErrorKind::TimedOut,
                    Stage::Load,
                    Delivery::MayHaveReachedReceiver,
                )
            })?;
        let pending = self
            .pending
            .take()
            .expect("the response consumed its receiver");
        let reply = received
            .map_err(|_| {
                Error::new(
                    ErrorKind::Unreachable,
                    Stage::Load,
                    Delivery::MayHaveReachedReceiver,
                )
            })?
            .map_err(|error| translate(error, Stage::Load, Delivery::Acknowledged))?;
        let id = super::load_instance(&reply, &pending.url, Some(&self.token))
            .map_err(|error| translate(error, Stage::Load, Delivery::Acknowledged))?;
        self.owned = Some(id);
        let observation = match super::state(&reply, id) {
            Ok(Status::Playing | Status::Paused | Status::Buffering) => self.snapshot(&reply, id),
            _ => Observation::playback(Playback::Accepted),
        };
        Ok(observation)
    }

    fn snapshot(&mut self, reply: &Value, id: i64) -> Observation {
        if replaced(reply, id, &self.token) {
            return Observation::playback(Playback::OwnershipLost);
        }
        let unavailable = || Observation::link(Link::Recovering, None);
        let observed = match super::state(reply, id) {
            Ok(Status::Playing) => Playback::Playing,
            Ok(Status::Paused) => Playback::Paused,
            Ok(Status::Buffering) => Playback::Buffering,
            Ok(Status::Finished | Status::Canceled) => Playback::Ended,
            Ok(Status::Failed) => Playback::Failed(Error::new(
                ErrorKind::ReceiverRefused,
                Stage::Observe,
                Delivery::NotSent,
            )),
            Ok(Status::Interrupted) | Err(WireError::Ended) => {
                if other_active_media(reply, id) {
                    Playback::OwnershipLost
                } else {
                    Playback::NoLongerPresent
                }
            }
            Ok(Status::Idle) => {
                if self.idle_since.get_or_insert_with(Instant::now).elapsed() < IDLE_GRACE {
                    return unavailable();
                }
                Playback::Failed(Error::new(
                    ErrorKind::Protocol,
                    Stage::Observe,
                    Delivery::NotSent,
                ))
            }
            Ok(Status::Unknown) | Err(_) => return unavailable(),
        };
        self.idle_since = None;
        match super::media_entry(reply, id) {
            Ok(entry)
                if matches!(
                    observed,
                    Playback::Playing | Playback::Paused | Playback::Buffering
                ) =>
            {
                let features = capabilities(entry, self.live);
                let position = seconds(&entry["currentTime"]);
                let duration = (!self.live)
                    .then(|| seconds(&entry["media"]["duration"]))
                    .flatten();
                Observation::playback(observed)
                    .with_link(Link::Connected, None)
                    .with_position(position)
                    .with_duration(duration)
                    .with_features(features)
                    .with_output(Some(VolumeScope::Receiver), None, None)
            }
            _ => Observation::playback(observed),
        }
    }

    async fn observe_owned(&mut self) -> Result<Observation, Error> {
        tokio::time::sleep_until(self.poll_after).await;
        self.poll_after = Instant::now() + POLL_INTERVAL;
        let id = self.owned.ok_or_else(observe_missing)?;
        let reply = self
            .wire()
            .media(json!({"type":"GET_STATUS"}))
            .await
            .map_err(|error| translate(error, Stage::Observe, Delivery::NotSent))?;
        let observed = self.snapshot(&reply, id);
        if !matches!(
            super::state(&reply, id),
            Ok(Status::Playing | Status::Paused | Status::Buffering)
        ) {
            return Ok(observed);
        }
        let report = timeout(OUTPUT_PROBE, self.wire().receiver_status())
            .await
            .ok()
            .and_then(Result::ok);
        let (volume, muted) = report.as_ref().map(output).unwrap_or_default();
        Ok(observed.with_output(Some(VolumeScope::Receiver), volume, muted))
    }

    async fn control(&self, command: Command) -> Result<Confirmation, Error> {
        let (feature, wire) = match command {
            Command::Play => (Feature::Play, WireCommand::Play),
            Command::Pause => (Feature::Pause, WireCommand::Pause),
            Command::Seek(position) => (Feature::Seek, WireCommand::Seek(position)),
            Command::Output {
                scope: VolumeScope::Receiver,
                change,
            } => match change {
                OutputChange::SetVolume(volume) => {
                    (Feature::SetVolume, WireCommand::Volume(volume.as_percent()))
                }
                OutputChange::StepVolume(step) => {
                    (Feature::StepVolume, WireCommand::StepVolume(step))
                }
                OutputChange::SetMuted(muted) => (Feature::SetMuted, WireCommand::SetMuted(muted)),
                OutputChange::ToggleMute => (Feature::ToggleMute, WireCommand::ToggleMute),
            },
            Command::Output { .. } => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    Stage::Control,
                    Delivery::NotSent,
                ));
            }
        };
        let id = self
            .owned
            .ok_or_else(|| Error::new(ErrorKind::Unavailable, Stage::Control, Delivery::NotSent))?;
        let latest = self
            .wire()
            .media(json!({"type":"GET_STATUS"}))
            .await
            .map_err(|error| translate(error, Stage::Control, Delivery::NotSent))?;
        let entry = owned_active_entry(&latest, id, &self.token, Stage::Control)?;
        match capability(entry, self.live, feature) {
            Availability::Available => {}
            Availability::Unsupported => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    Stage::Control,
                    Delivery::NotSent,
                ));
            }
            Availability::Unknown | Availability::Unavailable => {
                return Err(Error::new(
                    ErrorKind::Unavailable,
                    Stage::Control,
                    Delivery::NotSent,
                ));
            }
        }
        self.wire()
            .command_owned(id, wire, Some(&self.token))
            .await
            .map_err(|error| {
                let delivery = if matches!(error, WireError::OwnershipLost { dispatched: false }) {
                    Delivery::NotSent
                } else {
                    Delivery::MayHaveReachedReceiver
                };
                translate(error, Stage::Control, delivery)
            })?;
        Ok(match command {
            Command::Output { .. } => Confirmation::Accepted,
            _ => Confirmation::ReceiverConfirmed,
        })
    }

    async fn reconcile(&mut self) -> Result<i64, Error> {
        let deadline = Instant::now() + RECONCILE;
        if let Some(mut pending) = self.pending.take()
            && let Ok(Ok(Ok(reply))) = timeout(RECONCILE / 2, &mut pending.reply).await
            && let Ok(id) = super::load_instance(&reply, &pending.url, Some(&self.token))
        {
            self.owned = Some(id);
            return Ok(id);
        }
        let status = timeout_at(deadline, self.wire().media(json!({"type":"GET_STATUS"})))
            .await
            .map_err(|_| Error::new(ErrorKind::TimedOut, Stage::Stop, Delivery::NotSent))?
            .map_err(|error| translate(error, Stage::Stop, Delivery::NotSent))?;
        let id = super::tagged_instance(&status, &self.token)
            .map_err(|error| translate(error, Stage::Stop, Delivery::NotSent))?
            .ok_or_else(|| Error::new(ErrorKind::Protocol, Stage::Stop, Delivery::NotSent))?;
        self.owned = Some(id);
        Ok(id)
    }

    async fn stop_owned(&mut self) -> Result<StopOutcome, Error> {
        let id = match self.owned {
            Some(id) => id,
            None => self.reconcile().await?,
        };
        let status = self
            .wire()
            .media(json!({"type":"GET_STATUS"}))
            .await
            .map_err(|error| translate(error, Stage::Stop, Delivery::NotSent))?;
        if replaced(&status, id, &self.token) {
            return Err(Error::new(
                ErrorKind::OwnershipLost,
                Stage::Stop,
                Delivery::NotSent,
            ));
        }
        match super::state(&status, id) {
            Err(WireError::Ended) | Ok(Status::Interrupted) if other_active_media(&status, id) => {
                return Err(Error::new(
                    ErrorKind::OwnershipLost,
                    Stage::Stop,
                    Delivery::NotSent,
                ));
            }
            Err(WireError::Ended) | Ok(Status::Finished | Status::Canceled | Status::Failed) => {
                return Ok(StopOutcome::AlreadyEnded);
            }
            Ok(Status::Playing | Status::Buffering | Status::Paused | Status::Idle) => {}
            Ok(Status::Interrupted | Status::Unknown) | Err(_) => {
                return Err(Error::new(
                    ErrorKind::Protocol,
                    Stage::Stop,
                    Delivery::NotSent,
                ));
            }
        }
        self.wire()
            .command_owned(id, WireCommand::Stop, Some(&self.token))
            .await
            .map_err(|error| translate(error, Stage::Stop, Delivery::MayHaveReachedReceiver))?;
        Ok(StopOutcome::StopRequested(Confirmation::ReceiverConfirmed))
    }
}

impl Driver for CastDriver {
    fn control_addrs(&self) -> Option<(SocketAddr, SocketAddr)> {
        Some(self.wire().control_addrs())
    }

    fn peer_trust(&self) -> PeerTrust {
        PeerTrust::LanEndpoint
    }

    fn validate(&self, media: &Media) -> Result<(), Error> {
        let content_type = content_type(media)?;
        let body = super::load_body(
            media.url(),
            content_type,
            media.is_live(),
            Some(&self.token),
        );
        if body.to_string().len() > super::MAX_FRAME - 256 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                Stage::Load,
                Delivery::NotSent,
            ));
        }
        Ok(())
    }

    fn load<'a>(
        &'a mut self,
        media: Media,
        dispatch: &'a Dispatch,
    ) -> BoxFuture<'a, Result<Observation, Error>> {
        Box::pin(self.submit(media, dispatch))
    }

    fn observe(&mut self) -> BoxFuture<'_, Result<Observation, Error>> {
        Box::pin(self.observe_owned())
    }

    fn command(&mut self, command: Command) -> BoxFuture<'_, Result<Confirmation, Error>> {
        Box::pin(self.control(command))
    }

    fn stop(&mut self) -> BoxFuture<'_, Result<StopOutcome, Error>> {
        Box::pin(self.stop_owned())
    }

    fn close(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async {
            if let Some(wire) = self.wire.take() {
                wire.close().await;
            }
        })
    }
}

fn content_type(media: &Media) -> Result<&str, Error> {
    if let Some(mime) = media.content_type() {
        return Ok(mime);
    }
    let path = media.url().split(['?', '#']).next().unwrap_or_default();
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("m3u8") => Ok("application/x-mpegURL"),
        Some("mp4") => Ok("video/mp4"),
        _ => Err(Error::new(
            ErrorKind::InvalidInput,
            Stage::Load,
            Delivery::NotSent,
        )),
    }
}

fn observe_missing() -> Error {
    Error::new(ErrorKind::Unavailable, Stage::Observe, Delivery::NotSent)
}

fn other_active_media(reply: &Value, id: i64) -> bool {
    super::media_entries(reply).is_ok_and(|entries| {
        entries.iter().any(|entry| {
            entry["mediaSessionId"].as_i64() != Some(id)
                && matches!(
                    entry["playerState"].as_str(),
                    Some("PLAYING" | "PAUSED" | "BUFFERING")
                )
        })
    })
}

fn replaced(reply: &Value, id: i64, token: &str) -> bool {
    super::different_load_tag(reply, id, token)
}

fn owned_active_entry<'a>(
    reply: &'a Value,
    id: i64,
    token: &str,
    stage: Stage,
) -> Result<&'a Value, Error> {
    if replaced(reply, id, token) {
        return Err(Error::new(
            ErrorKind::OwnershipLost,
            stage,
            Delivery::NotSent,
        ));
    }
    match super::state(reply, id) {
        Ok(Status::Playing | Status::Paused | Status::Buffering) => super::media_entry(reply, id)
            .map_err(|error| translate(error, stage, Delivery::NotSent)),
        Ok(Status::Interrupted) | Err(WireError::Ended) if other_active_media(reply, id) => Err(
            Error::new(ErrorKind::OwnershipLost, stage, Delivery::NotSent),
        ),
        _ => Err(Error::new(ErrorKind::Unavailable, stage, Delivery::NotSent)),
    }
}

fn capabilities(entry: &Value, live: bool) -> Features {
    let mut features = Features::new(Availability::Unknown);
    for feature in [
        Feature::LoadVideoUrl,
        Feature::Play,
        Feature::Pause,
        Feature::Seek,
        Feature::SetVolume,
        Feature::StepVolume,
        Feature::SetMuted,
        Feature::ToggleMute,
        Feature::Stop,
    ] {
        features = features.with(feature, capability(entry, live, feature));
    }
    features
}

fn capability(entry: &Value, live: bool, feature: Feature) -> Availability {
    let mask = entry["supportedMediaCommands"].as_u64();
    match feature {
        Feature::Seek if live || mask.is_some_and(|mask| mask & 2 == 0) => {
            Availability::Unsupported
        }
        Feature::Seek if mask.is_none() => Availability::Unknown,
        Feature::Play | Feature::Pause if mask.is_some_and(|mask| mask & 1 == 0) => {
            Availability::Unsupported
        }
        _ => Availability::Available,
    }
}

fn seconds(value: &Value) -> Option<Duration> {
    Duration::try_from_secs_f64(value.as_f64()?).ok()
}

fn output(receiver_report: &Value) -> (Option<Volume>, Option<bool>) {
    let volume = &receiver_report["status"]["volume"];
    let level = volume["level"]
        .as_f64()
        .filter(|n| (0.0..=1.0).contains(n))
        .and_then(|n| Volume::percent((n * 100.0).round() as u8).ok());
    (level, volume["muted"].as_bool())
}

fn translate(error: WireError, stage: Stage, delivery: Delivery) -> Error {
    let kind = match error {
        WireError::Unreachable => ErrorKind::Unreachable,
        WireError::Refused | WireError::LoadFailed => ErrorKind::ReceiverRefused,
        WireError::Unknown => ErrorKind::Protocol,
        WireError::Ended => ErrorKind::Unavailable,
        WireError::OwnershipLost { .. } => ErrorKind::OwnershipLost,
        WireError::TimedOut => ErrorKind::TimedOut,
    };
    let report = Error::new(kind, stage, delivery);
    if error == WireError::LoadFailed {
        report.with_diagnostic(Diagnostic::MediaLoadFailed)
    } else {
        report
    }
}

#[cfg(test)]
mod tests;
