//! Network casting via the shared sender. The runtime authorizes relay admission.
use iptv_cast::{
    Cast, Confirmation, Connect, Connection, Delivery, ErrorKind, Link, Media, OutputChange,
    Playback, Route, Session, Snapshot, Stage, StopOutcome, Updates, Volume, VolumeScope,
};
use iptv_core::{
    cast::{Command, Error, NetworkProtocol, Receiver, RemoteState},
    playback::PlaySource,
};
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use iptv_runtime::cast::{Admitted, Authorization, Port, Prepared, Remote, Retirement, Task};
use std::{
    collections::HashMap,
    ffi::OsStr,
    net::SocketAddr,
    num::NonZeroU16,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    runtime::Handle,
    time::{Instant, timeout, timeout_at},
};

#[derive(Default)]
pub(super) struct Sender {
    cast: OnceLock<Cast>,
    routes: Mutex<HashMap<String, Route>>,
    #[cfg(feature = "test-support")]
    loopback: Option<SocketAddr>,
}

const DISCOVERY_WINDOW: Duration = Duration::from_secs(5);
const DEFAULT_PORT: u16 = 49152;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(15);

fn configured_relay_port(value: Option<&OsStr>) -> Result<u16, Error> {
    let Some(value) = value else {
        return Ok(DEFAULT_PORT);
    };
    value
        .to_str()
        .and_then(|port| port.parse::<NonZeroU16>().ok())
        .map(NonZeroU16::get)
        .ok_or(Error::Unavailable)
}

fn map_error(error: &iptv_cast::Error) -> Error {
    match error.kind() {
        ErrorKind::Unavailable | ErrorKind::Unreachable => Error::Unavailable,
        ErrorKind::ReceiverRefused => Error::Refused,
        ErrorKind::Cancelled => Error::Cancelled,
        ErrorKind::Unsupported | ErrorKind::InvalidInput => Error::Unsupported,
        ErrorKind::Busy => Error::Busy,
        _ => Error::Unknown,
    }
}

impl Sender {
    #[cfg(feature = "test-support")]
    pub(super) fn for_loopback(address: SocketAddr) -> Self {
        Self {
            loopback: Some(address),
            ..Self::default()
        }
    }

    fn new_cast(&self) -> Result<Cast, iptv_cast::Error> {
        #[cfg(feature = "test-support")]
        if let Some(address) = self.loopback {
            return iptv_cast::test_support::sender(address);
        }
        Cast::builder().build()
    }

    fn cast(&self) -> Result<&Cast, Error> {
        if let Some(cast) = self.cast.get() {
            return Ok(cast);
        }
        let cast = self.new_cast().map_err(|error| map_error(&error))?;
        Ok(self.cast.get_or_init(|| cast))
    }
}

impl Port for Sender {
    fn discover(&self) -> Task<'_, Vec<Receiver>> {
        Box::pin(async {
            let discovery = self
                .cast()?
                .discover(DISCOVERY_WINDOW)
                .await
                .map_err(|error| map_error(&error))?;
            let mut route_cache = self.routes.lock().map_err(|_| Error::Unavailable)?;
            route_cache.clear();
            Ok(discovery
                .routes()
                .iter()
                .filter(|route| route.protocol().as_str() == "googlecast")
                .map(|route| {
                    let id = route.id().as_str().to_owned();
                    route_cache.insert(id.clone(), route.clone());
                    Receiver {
                        id,
                        name: route.name().to_owned(),
                        protocol: NetworkProtocol::GoogleCast,
                    }
                })
                .collect())
        })
    }

    fn prepare(&self, receiver: Receiver, source: PlaySource) -> Task<'_, Box<dyn Prepared>> {
        Box::pin(async move {
            let relay_port =
                configured_relay_port(std::env::var_os("PATINA_CAST_RELAY_PORT").as_deref())?;
            #[cfg(feature = "test-support")]
            let relay_port = if self.loopback.is_some() {
                0
            } else {
                relay_port
            };
            let route = self
                .routes
                .lock()
                .map_err(|_| Error::Unavailable)?
                .get(&receiver.id)
                .cloned()
                .ok_or(Error::Unavailable)?;
            let connection = match self
                .cast()?
                .connect(&route)
                .await
                .map_err(|error| map_error(&error))?
            {
                Connect::Ready(connection) => connection,
                Connect::PinRequired(pairing) => {
                    pairing.cancel().await;
                    return Err(Error::Unsupported);
                }
                _ => return Err(Error::Unsupported),
            };
            let (local, remote) = connection.control_addrs().ok_or(Error::Unavailable)?;
            let listen = SocketAddr::new(local.ip(), relay_port);
            let (relay, gate) = tokio::task::spawn_blocking(move || {
                PlaybackRelay::start_cast(
                    &source.url,
                    &source.headers,
                    super::transport::policy(source.public_only),
                    listen,
                    remote.ip(),
                )
            })
            .await
            .map_err(|_| Error::Unavailable)?
            .map_err(|_| Error::Unavailable)?;
            #[cfg(feature = "test-support")]
            let relay_port = relay
                .url()
                .split('/')
                .nth(2)
                .and_then(|authority| authority.parse::<SocketAddr>().ok())
                .map_or(relay_port, |address| address.port());
            Ok(Box::new(Handoff {
                pending: Some(Pending {
                    sender: PendingSender::Ready(connection),
                    relay,
                    relay_port,
                    gate,
                }),
                handle: Handle::current(),
                guard: None,
            }) as Box<dyn Prepared>)
        })
    }
}

enum PendingSender {
    Ready(Connection),
    Loading(Session),
}

struct Pending {
    sender: PendingSender,
    relay: PlaybackRelay,
    relay_port: u16,
    gate: ExternalAdmission,
}

impl Pending {
    async fn rollback(self) {
        let Pending {
            sender,
            relay,
            gate,
            ..
        } = self;
        drop(gate);
        relay.shutdown().await;
        match sender {
            PendingSender::Ready(connection) => connection.close().await,
            PendingSender::Loading(session) => {
                let _ = stop_remote(session).await;
            }
        }
    }
}

struct Handoff {
    pending: Option<Pending>,
    handle: Handle,
    guard: Option<Box<dyn Fn() -> bool + Send + Sync>>,
}

impl Drop for Handoff {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            // Prepared can be dropped by the UI outside the network runtime.
            self.handle.spawn(pending.rollback());
        }
    }
}

impl Prepared for Handoff {
    fn guard(&mut self, current: Box<dyn Fn() -> bool + Send + Sync>) {
        self.guard = Some(current);
    }

    fn load(&mut self) -> Task<'_, ()> {
        Box::pin(async move {
            if self.guard.as_ref().is_some_and(|current| !current()) {
                return Err(Error::Denied);
            }
            let pending = self.pending.take().ok_or(Error::Unknown)?;
            let Pending {
                sender,
                relay,
                relay_port,
                gate,
            } = pending;
            let PendingSender::Ready(connection) = sender else {
                self.pending = Some(Pending {
                    sender,
                    relay,
                    relay_port,
                    gate,
                });
                return Err(Error::Unknown);
            };
            let media = Media::live(relay.url())
                .and_then(|media| media.with_content_type("application/x-mpegURL"))
                .map_err(|error| map_error(&error))?;
            let session = connection.load(media).map_err(|error| map_error(&error))?;
            let mut updates = session.subscribe();
            let pending = self.pending.insert(Pending {
                sender: PendingSender::Loading(session),
                relay,
                gate,
                relay_port,
            });
            let result = wait_for_request(
                &mut pending.gate,
                &mut updates,
                relay_port,
                self.guard.as_deref(),
            )
            .await;
            if result.is_err()
                && let Some(pending) = self.pending.take()
            {
                pending.rollback().await;
            }
            result
        })
    }

    fn allow(mut self: Box<Self>, authorization: &Authorization) -> Task<'_, Admitted> {
        Box::pin(async move {
            let pending = self.pending.take().ok_or(Error::Unknown)?;
            let Pending {
                sender,
                relay,
                gate,
                ..
            } = pending;
            let PendingSender::Loading(session) = sender else {
                relay.shutdown().await;
                return Err(Error::Unknown);
            };
            let mut updates = session.subscribe();
            let outcome = authorization
                .while_current(async {
                    if self.guard.as_ref().is_some_and(|current| !current()) {
                        return Err(Error::Denied);
                    }
                    if !gate.allow() {
                        return Err(Error::Unknown);
                    }
                    tokio::select! {
                        result = timeout(SETTLE_TIMEOUT, initial_playback(&mut updates)) => {
                            result.map_err(|_| Error::Unknown)?
                        }
                        () = obsolete(self.guard.as_deref()) => Err(Error::Denied),
                    }
                })
                .await;
            match outcome {
                Ok(paused) => Ok(Admitted {
                    remote: Arc::new(Playing {
                        session: tokio::sync::Mutex::new(Some(session)),
                        relay: tokio::sync::Mutex::new(Some(relay)),
                    }),
                    paused,
                }),
                Err(error) => {
                    relay.shutdown().await;
                    let _ = stop_remote(session).await;
                    Err(error)
                }
            }
        })
    }
}

async fn obsolete(guard: Option<&(dyn Fn() -> bool + Send + Sync)>) {
    if let Some(current) = guard {
        while current() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    } else {
        std::future::pending::<()>().await;
    }
}

async fn wait_for_request(
    gate: &mut ExternalAdmission,
    updates: &mut Updates,
    relay_port: u16,
    guard: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(), Error> {
    let no_request = Error::NoRequest { relay_port };
    let deadline = Instant::now() + FETCH_TIMEOUT;
    tokio::select! {
        result = timeout_at(deadline, gate.connected()) => {
            if result == Ok(true) { Ok(()) } else { Err(no_request) }
        }
        snapshot = first_failed_update(updates) => {
            if let Some(snapshot) = snapshot {
                if !gate.received_request()
                    && matches!(snapshot.playback(), Playback::Failed(error)
                        if error.stage() == Stage::Load && error.kind() == ErrorKind::ReceiverRefused)
                {
                    return Err(no_request);
                }
                Err(snapshot_error(&snapshot))
            } else {
                Err(Error::Unknown)
            }
        }
        () = obsolete(guard) => Err(Error::Denied),
    }
}

async fn first_failed_update(updates: &mut Updates) -> Option<Snapshot> {
    while let Some(snapshot) = updates.next().await {
        if finished(&snapshot) || snapshot.link() == Link::Disconnected {
            return Some(snapshot);
        }
    }
    None
}

async fn initial_playback(updates: &mut Updates) -> Result<bool, Error> {
    while let Some(snapshot) = updates.next().await {
        if finished(&snapshot) || snapshot.link() == Link::Disconnected {
            return Err(snapshot_error(&snapshot));
        }
        match snapshot.playback() {
            Playback::Playing => return Ok(false),
            Playback::Paused => return Ok(true),
            _ => {}
        }
    }
    Err(Error::Unknown)
}

fn finished(snapshot: &Snapshot) -> bool {
    matches!(
        snapshot.playback(),
        Playback::Ended | Playback::NoLongerPresent | Playback::Failed(_) | Playback::OwnershipLost
    )
}

fn snapshot_error(snapshot: &Snapshot) -> Error {
    if let Playback::Failed(error) = snapshot.playback() {
        map_error(error)
    } else {
        snapshot.link_error().map_or(Error::Refused, map_error)
    }
}

async fn stop_remote(session: Session) -> Result<(), Error> {
    let outcome = session.stop().await;
    match outcome {
        Ok(StopOutcome::AlreadyEnded | StopOutcome::CancelledBeforeSend)
        | Ok(StopOutcome::StopRequested(Confirmation::ReceiverConfirmed)) => Ok(()),
        Err(ref error)
            if error.kind() == ErrorKind::OwnershipLost
                && error.delivery() == Delivery::NotSent =>
        {
            Ok(())
        }
        other => {
            log::debug!("cast stop outcome: {other:?}");
            Err(Error::StopUnconfirmed)
        }
    }
}

struct Playing {
    session: tokio::sync::Mutex<Option<Session>>,
    relay: tokio::sync::Mutex<Option<PlaybackRelay>>,
}

impl Remote for Playing {
    fn command(&self, command: Command) -> Task<'_, ()> {
        Box::pin(async move {
            let command = match command {
                Command::Volume(value) => output_control(OutputChange::SetVolume(
                    Volume::percent(value).map_err(|error| map_error(&error))?,
                )),
                Command::StepVolume(value) => output_control(OutputChange::StepVolume(value)),
                Command::ToggleMute => output_control(OutputChange::ToggleMute),
                Command::Play => iptv_cast::Command::Play,
                Command::Pause => iptv_cast::Command::Pause,
                Command::Stop => {
                    return if let Some(session) = self.session.lock().await.take() {
                        stop_remote(session).await
                    } else {
                        Ok(())
                    };
                }
            };
            let mut session = self.session.lock().await;
            session
                .as_mut()
                .ok_or(Error::Unavailable)?
                .command(command)
                .await
                .map(drop)
                .map_err(|error| map_error(&error))
        })
    }

    fn state(&self) -> Task<'_, RemoteState> {
        Box::pin(async {
            let slot = self.session.lock().await;
            let snapshot = slot.as_ref().ok_or(Error::Unavailable)?.snapshot();
            if finished(&snapshot) {
                return Ok(RemoteState::Ended);
            }
            match snapshot.link() {
                Link::Disconnected => Err(snapshot_error(&snapshot)),
                Link::Connected => match snapshot.playback() {
                    Playback::Playing => Ok(RemoteState::Playing),
                    Playback::Paused => Ok(RemoteState::Paused),
                    _ => Ok(RemoteState::Loading),
                },
                _ => Ok(RemoteState::Loading),
            }
        })
    }

    fn retire(&self) -> Retirement<'_> {
        Box::pin(async move {
            let mut slot = self.relay.lock().await;
            if let Some(relay) = slot.take() {
                relay.shutdown().await;
            }
        })
    }

    fn stop_checked(&self) -> Task<'_, ()> {
        Box::pin(async {
            self.retire().await;
            self.command(Command::Stop).await
        })
    }
}

fn output_control(change: OutputChange) -> iptv_cast::Command {
    iptv_cast::Command::Output {
        scope: VolumeScope::Receiver,
        change,
    }
}

#[cfg(test)]
mod tests;
