//! Network casting via the shared sender. The runtime authorizes relay admission.
use iptv_cast::{
    Availability, Cast, Confirmation, Connect, Connection, Delivery, Diagnostic, ErrorKind,
    Feature, Link, Media, OutputChange, PeerTrust, Playback, Route, Session, Snapshot, Stage,
    StopOutcome, Updates, Volume, VolumeScope,
};
use iptv_core::{
    cast::{Command, Controls, Error, NetworkProtocol, Receiver, RemoteState},
    playback::PlaySource,
};
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use iptv_runtime::cast::{
    Admitted, AirPlayMode, Authorization, PairingPrompt, Port, Prepared, Remote, Retirement, Task,
};
#[cfg(any(target_os = "linux", windows))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    ffi::OsStr,
    net::SocketAddr,
    num::NonZeroU16,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::runtime::Handle;
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};

#[cfg(any(target_os = "linux", target_os = "android", windows))]
mod airplay;
#[cfg(target_os = "android")]
pub(super) mod android;
#[cfg(any(target_os = "android", test))]
mod android_credential;
#[cfg(any(target_os = "android", test))]
mod android_events;

pub(super) struct Sender {
    client: tokio::sync::Mutex<Option<Client>>,
    routes: Mutex<Routes>,
    #[cfg(feature = "test-support")]
    loopback: Option<SocketAddr>,
    #[cfg(any(target_os = "linux", windows))]
    airplay: airplay::Configuration,
    #[cfg(any(target_os = "linux", windows))]
    airplay_enabled: AtomicBool,
}

#[derive(Default)]
struct Routes {
    entries: HashMap<String, Route>,
    #[cfg(target_os = "android")]
    topology: Option<String>,
}

#[derive(Clone)]
struct Client {
    cast: Cast,
    airplay: bool,
}

const DISCOVERY_WINDOW: Duration = Duration::from_secs(5);
const DEFAULT_PORT: u16 = 49152;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
// Give the sender's bounded setup time to surface a timing diagnostic before
// treating a missing HTTP request as a relay reachability problem.
const AIRPLAY_FETCH_TIMEOUT: Duration = Duration::from_secs(35);
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
    if let Some(Diagnostic::TimingListener { local_addr }) = error.diagnostic() {
        let port = local_addr.port();
        match error.kind() {
            ErrorKind::TimedOut => return Error::AirPlayTiming { port },
            ErrorKind::Unavailable => {
                return Error::AirPlayTimingSocket {
                    port: NonZeroU16::new(port).map(NonZeroU16::get),
                };
            }
            _ => {}
        }
    }
    match error.kind() {
        ErrorKind::Unavailable | ErrorKind::Unreachable => Error::Unavailable,
        ErrorKind::ReceiverRefused => Error::Refused,
        ErrorKind::Cancelled => Error::Cancelled,
        ErrorKind::Unsupported | ErrorKind::InvalidInput => Error::Unsupported,
        ErrorKind::Busy => Error::Busy,
        ErrorKind::ApprovalDenied | ErrorKind::InvalidPin | ErrorKind::Expired => {
            Error::PairingRejected
        }
        ErrorKind::Storage => Error::PairingStorage,
        ErrorKind::InvalidCredential => Error::PairingInvalid,
        ErrorKind::IdentityMismatch => Error::PairingIdentityChanged,
        _ => Error::Unknown,
    }
}

impl Sender {
    #[cfg(feature = "test-support")]
    pub(super) fn for_loopback(address: SocketAddr) -> Self {
        Self {
            client: tokio::sync::Mutex::new(None),
            routes: Mutex::default(),
            loopback: Some(address),
            #[cfg(any(target_os = "linux", windows))]
            airplay: airplay::Configuration::new(Path::new("."), String::new()),
            #[cfg(any(target_os = "linux", windows))]
            airplay_enabled: AtomicBool::new(false),
        }
    }

    pub(super) fn new(directory: &Path, _pairing_label: String) -> Self {
        #[cfg(not(any(target_os = "linux", windows)))]
        let _ = directory;
        Self {
            client: tokio::sync::Mutex::new(None),
            routes: Mutex::default(),
            #[cfg(feature = "test-support")]
            loopback: None,
            #[cfg(any(target_os = "linux", windows))]
            airplay: airplay::Configuration::new(directory, _pairing_label),
            #[cfg(any(target_os = "linux", windows))]
            airplay_enabled: AtomicBool::new(false),
        }
    }

    async fn client(&self) -> Result<Client, Error> {
        let mut slot = self.client.lock().await;
        #[cfg(feature = "test-support")]
        if let Some(address) = self.loopback {
            if let Some(client) = slot.as_ref() {
                return Ok(client.clone());
            }
            let client = Client {
                cast: iptv_cast::test_support::sender(address)
                    .map_err(|error| map_error(&error))?,
                airplay: false,
            };
            *slot = Some(client.clone());
            return Ok(client);
        }
        #[cfg(any(target_os = "linux", windows))]
        let want_airplay = self.airplay_enabled.load(Ordering::Acquire);
        #[cfg(not(any(target_os = "linux", windows)))]
        let want_airplay = cfg!(target_os = "android");
        if let Some(client) = slot.as_ref()
            && client.airplay == want_airplay
        {
            return Ok(client.clone());
        }
        #[cfg(any(target_os = "linux", windows))]
        let client = if want_airplay {
            Client {
                cast: self
                    .airplay
                    .configure(Cast::builder())
                    .build()
                    .map_err(|error| map_error(&error))?,
                airplay: true,
            }
        } else {
            Client {
                cast: Cast::builder()
                    .airplay_enabled(false)
                    .build()
                    .map_err(|error| map_error(&error))?,
                airplay: false,
            }
        };
        #[cfg(not(any(target_os = "linux", windows)))]
        let client = {
            let builder = Cast::builder();
            #[cfg(target_os = "android")]
            let builder = builder.credential_store(android::CastStore);
            Client {
                cast: builder
                    .airplay_enabled(want_airplay)
                    .build()
                    .map_err(|error| map_error(&error))?,
                airplay: want_airplay,
            }
        };
        if slot.is_some()
            && let Ok(mut routes) = self.routes.lock()
        {
            // Routes are bound to the Cast owner that discovered them.
            *routes = Routes::default();
        }
        *slot = Some(client.clone());
        Ok(client)
    }

    fn route(&self, receiver: &Receiver) -> Result<Route, Error> {
        self.routes
            .lock()
            .map_err(|_| Error::Unavailable)?
            .entries
            .get(&receiver.id)
            .cloned()
            .ok_or(Error::Unavailable)
    }
}

impl Port for Sender {
    fn configure_airplay(&self, mode: AirPlayMode) {
        #[cfg(any(target_os = "linux", windows))]
        self.airplay_enabled
            .store(mode == AirPlayMode::Enabled, Ordering::Release);
        #[cfg(not(any(target_os = "linux", windows)))]
        let _ = mode;
    }

    fn forget(&self, receiver: Receiver) -> Task<'_, ()> {
        Box::pin(async move {
            self.client()
                .await?
                .cast
                .forget(&self.route(&receiver)?)
                .await
                .map_err(|error| map_error(&error))
        })
    }

    fn discover(&self) -> Task<'_, Vec<Receiver>> {
        Box::pin(async {
            #[cfg(target_os = "android")]
            let Some(topology) = android::topology()? else {
                *self.routes.lock().map_err(|_| Error::Unavailable)? = Routes::default();
                return Ok(Vec::new());
            };
            #[cfg(target_os = "android")]
            let _multicast = android::Multicast::acquire()?;
            let client = self.client().await?;
            let discovery = client
                .cast
                .discover(DISCOVERY_WINDOW)
                .await
                .map_err(|error| map_error(&error))?;
            #[cfg(target_os = "android")]
            let current = android::topology()?;
            let mut route_cache = self.routes.lock().map_err(|_| Error::Unavailable)?;
            *route_cache = Routes::default();
            #[cfg(target_os = "android")]
            {
                if current.as_ref() != Some(&topology) {
                    return Ok(Vec::new());
                }
                route_cache.topology = current;
            }
            Ok(discovery
                .routes()
                .iter()
                .filter_map(|route| {
                    let protocol = match route.protocol().as_str() {
                        "googlecast"
                            if !cfg!(target_os = "android") || route.has_stable_identity() =>
                        {
                            NetworkProtocol::GoogleCast
                        }
                        "airplay"
                            if client.airplay
                                && matches!(
                                    route.availability(Feature::LoadVideoUrl),
                                    Availability::Available | Availability::Unknown
                                ) =>
                        {
                            NetworkProtocol::AirPlay
                        }
                        _ => return None,
                    };
                    let id = route.id().as_str().to_owned();
                    route_cache.entries.insert(id.clone(), route.clone());
                    Some(Receiver {
                        id,
                        name: route.name().to_owned(),
                        protocol,
                    })
                })
                .collect())
        })
    }

    fn prepare(
        &self,
        receiver: Receiver,
        source: PlaySource,
        pairing_prompt: PairingPrompt,
    ) -> Task<'_, Box<dyn Prepared>> {
        Box::pin(async move {
            #[cfg(any(target_os = "linux", windows))]
            if receiver.protocol == NetworkProtocol::AirPlay {
                self.airplay.validate()?;
            }
            let relay_port =
                configured_relay_port(std::env::var_os("PATINA_CAST_RELAY_PORT").as_deref())?;
            #[cfg(feature = "test-support")]
            let relay_port = if self.loopback.is_some() {
                0
            } else {
                relay_port
            };
            #[cfg(not(target_os = "android"))]
            let route = self.route(&receiver)?;
            #[cfg(target_os = "android")]
            let (route, topology) = {
                let current = android::topology()?.ok_or(Error::Unavailable)?;
                let cache = self.routes.lock().map_err(|_| Error::Unavailable)?;
                if cache.topology.as_ref() != Some(&current) {
                    return Err(Error::Unavailable);
                }
                let route = cache
                    .entries
                    .get(&receiver.id)
                    .cloned()
                    .ok_or(Error::Unavailable)?;
                (route, current)
            };
            let connect = {
                // Stable receiver IDs are refreshed through mDNS before connecting.
                // Release the Wi-Fi lease before waiting for receiver PIN approval.
                #[cfg(target_os = "android")]
                let _multicast = android::Multicast::acquire()?;
                self.client()
                    .await?
                    .cast
                    .connect(&route)
                    .await
                    .map_err(|error| map_error(&error))?
            };
            let connection = match connect {
                Connect::Ready(connection) => connection,
                Connect::PinRequired(pairing) => {
                    #[cfg(target_os = "android")]
                    {
                        if let Err(error) = android::require_topology(&topology) {
                            pairing.cancel().await;
                            return Err(error);
                        }
                        airplay::pair(pairing, pairing_prompt, || {
                            android::require_topology(&topology)
                        })
                        .await?
                    }
                    #[cfg(any(target_os = "linux", windows))]
                    {
                        airplay::pair(pairing, pairing_prompt, || Ok(())).await?
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
                    {
                        drop(pairing_prompt);
                        pairing.cancel().await;
                        return Err(Error::Unsupported);
                    }
                }
                _ => return Err(Error::Unsupported),
            };
            #[cfg(target_os = "android")]
            if let Err(error) = android::require_topology(&topology) {
                connection.close().await;
                return Err(error);
            }
            if receiver.protocol == NetworkProtocol::AirPlay
                && !matches!(
                    connection.peer_trust(),
                    PeerTrust::VerifiedPairing | PeerTrust::TransientPairing
                )
            {
                connection.close().await;
                return Err(Error::PairingIdentityChanged);
            }
            let (local, remote) = connection.control_addrs().ok_or(Error::Unavailable)?;
            let listen = match relay_listen_addr(local, relay_port) {
                Ok(listen) => listen,
                Err(error) => {
                    connection.close().await;
                    return Err(error);
                }
            };
            Ok(Box::new(Handoff {
                ready: Some(Ready {
                    connection,
                    source,
                    listen,
                    remote,
                    protocol: receiver.protocol,
                    #[cfg(target_os = "android")]
                    topology,
                }),
                pending: None,
                handle: Handle::current(),
                guard: None,
                #[cfg(target_os = "android")]
                service: None,
            }) as Box<dyn Prepared>)
        })
    }
}

fn relay_listen_addr(mut local: SocketAddr, port: u16) -> Result<SocketAddr, Error> {
    // An IPv6 interface zone belongs to the sender. The shared Media URL
    // validator and the receiver cannot use a sender-local zone identifier.
    if matches!(local, SocketAddr::V6(addr) if addr.scope_id() != 0 || addr.ip().is_unicast_link_local())
    {
        return Err(Error::AirPlayRelayAddress);
    }
    local.set_port(port);
    Ok(local)
}

struct Ready {
    connection: Connection,
    source: PlaySource,
    listen: SocketAddr,
    remote: SocketAddr,
    protocol: NetworkProtocol,
    #[cfg(target_os = "android")]
    topology: String,
}

struct Pending {
    session: Session,
    relay: PlaybackRelay,
    gate: ExternalAdmission,
}

impl Pending {
    async fn submit(
        connection: Connection,
        relay: PlaybackRelay,
        gate: ExternalAdmission,
        media: Result<Media, iptv_cast::Error>,
    ) -> Result<Self, Error> {
        let media = match media {
            Ok(media) => media,
            Err(error) => {
                drop(gate);
                relay.shutdown().await;
                connection.close().await;
                return Err(map_error(&error));
            }
        };
        match connection.load(media) {
            Ok(session) => Ok(Self {
                session,
                relay,
                gate,
            }),
            Err(error) => {
                drop(gate);
                relay.shutdown().await;
                Err(map_error(&error))
            }
        }
    }

    async fn rollback(self) {
        let Pending {
            session,
            relay,
            gate,
        } = self;
        drop(gate);
        relay.shutdown().await;
        let _ = stop_remote(session).await;
    }
}

struct Handoff {
    ready: Option<Ready>,
    pending: Option<Pending>,
    handle: Handle,
    guard: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    #[cfg(target_os = "android")]
    service: Option<android::PlaybackLease>,
}

impl Drop for Handoff {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            // Prepared can be dropped by the UI outside the network runtime.
            #[cfg(not(target_os = "android"))]
            self.handle.spawn(pending.rollback());
            #[cfg(target_os = "android")]
            {
                let service = self.service.take();
                self.handle.spawn(async move {
                    pending.rollback().await;
                    drop(service);
                });
            }
        } else if let Some(ready) = self.ready.take() {
            self.handle.spawn(ready.connection.close());
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
            let Ready {
                connection,
                source,
                listen,
                remote,
                protocol,
                #[cfg(target_os = "android")]
                topology,
            } = self.ready.take().ok_or(Error::Unknown)?;
            #[cfg(target_os = "android")]
            let mut service_state =
                match android::PlaybackLease::start(listen.ip(), &topology).await {
                    Ok(lease) => self.service.insert(lease).state(),
                    Err(error) => {
                        connection.close().await;
                        return Err(error);
                    }
                };
            // The old relay's listener is joined before the runtime calls load.
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
            let relay_port = listen.port();
            #[cfg(feature = "test-support")]
            let relay_port = relay
                .url()
                .split('/')
                .nth(2)
                .and_then(|authority| authority.parse::<SocketAddr>().ok())
                .map_or(relay_port, |address| address.port());
            if self.guard.as_ref().is_some_and(|current| !current()) {
                drop(gate);
                relay.shutdown().await;
                connection.close().await;
                return Err(Error::Denied);
            }
            #[cfg(target_os = "android")]
            if *service_state.borrow() != android_events::ServiceState::Ready {
                drop(gate);
                relay.shutdown().await;
                connection.close().await;
                return Err(Error::Unavailable);
            }
            let media = Media::live(relay.url())
                .and_then(|media| media.with_content_type("application/x-mpegURL"));
            let pending = Pending::submit(connection, relay, gate, media).await?;
            let mut updates = pending.session.subscribe();
            let pending = self.pending.insert(pending);
            let waiting = wait_for_request(
                &mut pending.gate,
                &mut updates,
                relay_port,
                if protocol == NetworkProtocol::AirPlay {
                    AIRPLAY_FETCH_TIMEOUT
                } else {
                    FETCH_TIMEOUT
                },
                self.guard.as_deref(),
            );
            #[cfg(not(target_os = "android"))]
            let result = waiting.await;
            #[cfg(target_os = "android")]
            let result = tokio::select! {
                biased;
                _ = android::stopped(&mut service_state) => Err(Error::Unavailable),
                outcome = waiting => outcome,
            };
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
            #[cfg(target_os = "android")]
            let service = self.service.take().ok_or(Error::Unknown)?;
            #[cfg(target_os = "android")]
            let mut service_state = service.state();
            let Pending {
                session,
                relay,
                gate,
            } = self.pending.take().ok_or(Error::Unknown)?;
            let mut updates = session.subscribe();
            let outcome = authorization
                .while_current(async {
                    if self.guard.as_ref().is_some_and(|current| !current()) {
                        return Err(Error::Denied);
                    }
                    #[cfg(target_os = "android")]
                    if *service_state.borrow() != android_events::ServiceState::Ready {
                        return Err(Error::Unavailable);
                    }
                    if !gate.allow() {
                        return Err(Error::Unknown);
                    }
                    let settle = async {
                        tokio::select! {
                            result = timeout(SETTLE_TIMEOUT, initial_playback(&mut updates)) => {
                                result.map_err(|_| Error::Unknown)?
                            }
                            () = obsolete(self.guard.as_deref()) => Err(Error::Denied),
                        }
                    };
                    #[cfg(not(target_os = "android"))]
                    return settle.await;
                    #[cfg(target_os = "android")]
                    tokio::select! {
                        biased;
                        _ = android::stopped(&mut service_state) => Err(Error::Unavailable),
                        outcome = settle => outcome,
                    }
                })
                .await;
            match outcome {
                Ok(paused) => {
                    #[cfg(target_os = "android")]
                    let platform_updates = session.subscribe();
                    let remote = Arc::new(Playing::new(session, relay));
                    #[cfg(target_os = "android")]
                    {
                        remote
                            .service
                            .lock()
                            .expect("cast service poisoned")
                            .insert(service)
                            .activate(&remote, platform_updates);
                    }
                    Ok(Admitted { remote, paused })
                }
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
    budget: Duration,
    guard: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(), Error> {
    let no_request = Error::NoRequest { relay_port };
    let deadline = Instant::now() + budget;
    tokio::select! {
        result = timeout_at(deadline, gate.connected()) => {
            if result == Ok(true) { Ok(()) } else { Err(no_request) }
        }
        snapshot = first_failed_update(updates) => {
            if let Some(snapshot) = snapshot {
                if !gate.received_request()
                    && matches!(snapshot.playback(), Playback::Failed(error)
                        if error.stage() == Stage::Load
                            && error.diagnostic() == Some(&Diagnostic::MediaLoadFailed))
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
    session: Arc<tokio::sync::Mutex<Option<Session>>>,
    relay: Arc<tokio::sync::Mutex<RelayRetirement>>,
    stop_result: OnceLock<watch::Receiver<Option<Result<(), Error>>>>,
    #[cfg(target_os = "android")]
    service: Mutex<Option<android::PlaybackLease>>,
}

impl Playing {
    fn new(session: Session, relay: PlaybackRelay) -> Self {
        Self {
            session: Arc::new(tokio::sync::Mutex::new(Some(session))),
            relay: Arc::new(tokio::sync::Mutex::new(RelayRetirement {
                relay: Some(relay),
                shutdown: None,
            })),
            stop_result: OnceLock::new(),
            #[cfg(target_os = "android")]
            service: Mutex::default(),
        }
    }

    async fn stop_owned(&self) -> Result<(), Error> {
        let mut outcome = self
            .stop_result
            .get_or_init(|| {
                let (publish, outcome) = watch::channel(None);
                let relay = self.relay.clone();
                let session = self.session.clone();
                #[cfg(target_os = "android")]
                let service = self.service.lock().expect("cast service poisoned").take();
                // The OS controls and the app may both stop this media. Keep one
                // joined retirement and one receiver result even if a waiter drops.
                tokio::spawn(async move {
                    #[cfg(target_os = "android")]
                    let _service = service;
                    retire_relay(&relay).await;
                    let session = session.lock().await.take();
                    let result = match session {
                        Some(session) => stop_remote(session).await,
                        None => Ok(()),
                    };
                    publish.send_replace(Some(result));
                });
                outcome
            })
            .clone();
        outcome
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|observed| *observed)
            .unwrap_or(Err(Error::StopUnconfirmed))
    }
}

struct RelayRetirement {
    relay: Option<PlaybackRelay>,
    shutdown: Option<JoinHandle<()>>,
}

async fn retire_relay(slot: &tokio::sync::Mutex<RelayRetirement>) {
    let mut slot = slot.lock().await;
    if let Some(relay) = slot.relay.take() {
        slot.shutdown = Some(tokio::spawn(relay.shutdown()));
    }
    // Borrow the handle so cancellation cannot abandon the listener's close
    // acknowledgement. Another waiter will join the same shutdown.
    if let Some(shutdown) = slot.shutdown.as_mut() {
        let _ = shutdown.await;
        slot.shutdown = None;
    }
}

impl Remote for Playing {
    fn controls(&self) -> Option<Controls> {
        self.session
            .try_lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Session::snapshot))
            .map(|snapshot| {
                let available = |feature| snapshot.availability(feature) == Availability::Available;
                let output = snapshot.volume_scope().is_some();
                Controls {
                    play: available(Feature::Play),
                    pause: available(Feature::Pause),
                    volume: output && available(Feature::SetVolume),
                    step_volume: output && available(Feature::StepVolume),
                    toggle_mute: output && available(Feature::ToggleMute),
                }
            })
    }

    fn command(&self, command: Command) -> Task<'_, ()> {
        Box::pin(async move {
            if command == Command::Stop {
                return self.stop_owned().await;
            }
            let mut slot = self.session.lock().await;
            if self.stop_result.get().is_some() {
                return Err(Error::Unavailable);
            }
            let session = slot.as_mut().ok_or(Error::Unavailable)?;
            let output = |change| output_control(session.snapshot().volume_scope(), change);
            let command = match command {
                Command::Volume(value) => output(OutputChange::SetVolume(
                    Volume::percent(value).map_err(|error| map_error(&error))?,
                ))?,
                Command::StepVolume(value) => output(OutputChange::StepVolume(value))?,
                Command::ToggleMute => output(OutputChange::ToggleMute)?,
                Command::Play => iptv_cast::Command::Play,
                Command::Pause => iptv_cast::Command::Pause,
                Command::Stop => unreachable!(),
            };
            session
                .command(command)
                .await
                .map(drop)
                .map_err(|error| map_error(&error))
        })
    }

    fn state(&self) -> Task<'_, RemoteState> {
        Box::pin(async {
            let slot = self.session.lock().await;
            if let Some(outcome) = self.stop_result.get() {
                return match *outcome.borrow() {
                    Some(result) => result.map(|()| RemoteState::Ended),
                    None if outcome.has_changed().is_ok() => Ok(RemoteState::Loading),
                    None => Err(Error::StopUnconfirmed),
                };
            }
            let Some(session) = slot.as_ref() else {
                return Ok(RemoteState::Ended);
            };
            let snapshot = session.snapshot();
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
            retire_relay(&self.relay).await;
            #[cfg(target_os = "android")]
            drop(self.service.lock().expect("cast service poisoned").take());
        })
    }

    fn stop(&self) -> Retirement<'_> {
        Box::pin(async {
            let _ = self.stop_owned().await;
        })
    }

    fn stop_checked(&self) -> Task<'_, ()> {
        Box::pin(self.stop_owned())
    }
}

fn output_control(
    scope: Option<VolumeScope>,
    change: OutputChange,
) -> Result<iptv_cast::Command, Error> {
    Ok(iptv_cast::Command::Output {
        scope: scope.ok_or(Error::Unavailable)?,
        change,
    })
}

#[cfg(test)]
mod tests;
