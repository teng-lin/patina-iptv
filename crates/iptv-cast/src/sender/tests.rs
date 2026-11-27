use std::{
    error::Error as _,
    future::pending,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore, broadcast},
    time::{Instant as TokioInstant, sleep_until, timeout},
};

use super::*;

const WINDOW: Duration = Duration::from_millis(10);
const URL: &str = "http://127.0.0.1:8000/generated.m3u8?cap=synthetic-token";

fn error(kind: ErrorKind, stage: Stage, delivery: Delivery) -> Error {
    Error::new(kind, stage, delivery)
}

fn assert_error(error: Error, kind: ErrorKind, stage: Stage, delivery: Delivery) {
    assert_eq!(
        (error.kind(), error.stage(), error.delivery()),
        (kind, stage, delivery)
    );
}

fn hints() -> Features {
    Features::new(Availability::Unknown).with(Feature::LoadVideoUrl, Availability::Available)
}

fn ready_features() -> Features {
    Features::new(Availability::Available)
        .with(Feature::Pause, Availability::Unavailable)
        .with(Feature::StepVolume, Availability::Unsupported)
}

#[derive(Default)]
struct Count {
    value: AtomicU32,
    changed: Notify,
}

impl Count {
    fn bump(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
        self.changed.notify_one();
    }

    fn read(&self) -> u32 {
        self.value.load(Ordering::Relaxed)
    }

    async fn wait(&self, target: u32) {
        timeout(Duration::from_secs(5), async {
            while self.read() < target {
                self.changed.notified().await;
            }
        })
        .await
        .expect("fixture progress");
    }
}

struct Fixture {
    next: AtomicU32,
    owner: AtomicU32,
    send_gate: Arc<Semaphore>,
    acknowledge: Arc<Semaphore>,
    pairing_gate: Arc<Semaphore>,
    fetch: Option<SocketAddr>,
    connect_entered: Count,
    load_entered: Count,
    load_sent: Count,
    stops: Count,
    controls: Count,
    block_command: AtomicBool,
    block_stop: AtomicBool,
    closes: Count,
    pairs: Count,
    cancelled_pairs: Count,
    queue_failed: AtomicBool,
    identity: AtomicU32,
    events: broadcast::Sender<Event>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            next: AtomicU32::new(1),
            owner: AtomicU32::new(0),
            send_gate: Arc::new(Semaphore::new(0)),
            acknowledge: Arc::new(Semaphore::new(0)),
            pairing_gate: Arc::new(Semaphore::new(0)),
            fetch: None,
            connect_entered: Count::default(),
            load_entered: Count::default(),
            load_sent: Count::default(),
            stops: Count::default(),
            controls: Count::default(),
            block_command: AtomicBool::new(false),
            block_stop: AtomicBool::new(false),
            closes: Count::default(),
            pairs: Count::default(),
            cancelled_pairs: Count::default(),
            queue_failed: AtomicBool::new(false),
            identity: AtomicU32::new(34),
            events: broadcast::channel(16).0,
        }
    }
}

impl Fixture {
    fn event(&self, event: Event) {
        self.events.send(event).unwrap();
    }

    fn backend(self: &Arc<Self>) -> Fake {
        Fake {
            fixture: self.clone(),
            pair: false,
            expires: false,
            unsent_load: false,
            insecure_pair: false,
            ignore_stored_credentials: false,
            block_connect: false,
        }
    }
}

#[derive(Clone, Debug)]
enum Event {
    Playback(Playback),
    Recover,
    LoseLink,
}

struct Fake {
    fixture: Arc<Fixture>,
    pair: bool,
    expires: bool,
    unsent_load: bool,
    insecure_pair: bool,
    ignore_stored_credentials: bool,
    block_connect: bool,
}

impl Backend for Fake {
    fn protocol(&self) -> ProtocolId {
        ProtocolId::new("test-protocol")
    }

    fn uses_credentials(&self) -> bool {
        self.pair
    }

    fn discover(&self, _: TokioInstant, sink: ScanSink) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            sink.push(Candidate::new(
                "old route name".into(),
                Some("stable".into()),
                1_u8,
                hints(),
            ));
            sink.push(Candidate::new(
                "Generated receiver".into(),
                Some("stable".into()),
                2_u8,
                hints(),
            ));
            sink.push(Candidate::new(
                "Generated receiver".into(),
                None,
                3_u8,
                hints(),
            ));
            sink.push(Candidate::new(
                "removed".into(),
                Some("gone".into()),
                4_u8,
                hints(),
            ));
            sink.remove("gone");
            Ok(())
        })
    }

    fn connect<'a>(
        &'a self,
        route: &'a Route,
        credential: Option<Credential>,
    ) -> BoxFuture<'a, Result<BackendConnect, Error>> {
        Box::pin(async move {
            assert!(matches!(route.payload::<u8>(), Some(2 | 3)));
            if self.block_connect {
                self.fixture.connect_entered.bump();
                std::future::pending::<()>().await;
            }
            let driver = Receiver {
                fixture: self.fixture.clone(),
                events: self.fixture.events.subscribe(),
                id: self.fixture.next.fetch_add(1, Ordering::Relaxed),
                proven: false,
                unsent_load: self.unsent_load,
                trust: if self.pair && !self.insecure_pair {
                    PeerTrust::VerifiedPairing
                } else {
                    PeerTrust::LanEndpoint
                },
            };
            if !self.pair {
                return Ok(BackendConnect::Ready(Box::new(driver)));
            }
            let identity = self.fixture.identity.load(Ordering::Relaxed);
            match credential.filter(|_| !self.ignore_stored_credentials) {
                Some(credential) if credential.as_bytes() == identity.to_le_bytes() => {
                    Ok(BackendConnect::Ready(Box::new(driver)))
                }
                Some(_) => Err(error(
                    ErrorKind::IdentityMismatch,
                    Stage::Connect,
                    Delivery::NotSent,
                )),
                None => {
                    let expires = self
                        .expires
                        .then(|| (TokioInstant::now() + Duration::from_secs(1)).into_std());
                    Ok(BackendConnect::PinRequired(Box::new(Approval {
                        expires,
                        identity,
                        driver: Some(driver),
                    })))
                }
            }
        })
    }
}

struct Approval {
    expires: Option<Instant>,
    identity: u32,
    driver: Option<Receiver>,
}

impl PairDriver for Approval {
    fn expires_at(&self) -> Option<Instant> {
        self.expires
    }

    fn submit(&mut self, pin: PinCode) -> BoxFuture<'_, Result<VerifiedConnection, Error>> {
        Box::pin(async move {
            let receiver = self.driver.as_ref().unwrap();
            receiver.fixture.pairs.bump();
            receiver
                .fixture
                .pairing_gate
                .acquire()
                .await
                .unwrap()
                .forget();
            if pin.as_str() != "0042" {
                return Err(error(
                    ErrorKind::InvalidPin,
                    Stage::Pair,
                    Delivery::Acknowledged,
                ));
            }
            let driver: Box<dyn Driver> = Box::new(self.driver.take().unwrap());
            Ok((
                driver,
                Credential::from_bytes(self.identity.to_le_bytes().into()),
            ))
        })
    }

    fn cancel(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Some(receiver) = self.driver.take() {
                receiver.fixture.cancelled_pairs.bump();
            }
        })
    }
}

struct Receiver {
    fixture: Arc<Fixture>,
    events: broadcast::Receiver<Event>,
    id: u32,
    proven: bool,
    unsent_load: bool,
    trust: PeerTrust,
}

impl Receiver {
    fn observe_playback(playback: Playback) -> Observation {
        Observation::playback(playback)
            .with_features(ready_features())
            .with_position(Some(Duration::from_secs(4)))
            .with_duration(Some(Duration::from_secs(21)))
            .with_output(
                Some(VolumeScope::Receiver),
                Some(Volume::percent(20).unwrap()),
                Some(false),
            )
    }

    fn check_owner(&self, stage: Stage) -> Result<(), Error> {
        if !self.proven {
            return Err(error(ErrorKind::Unsupported, stage, Delivery::NotSent));
        }
        if self.id != self.fixture.owner.load(Ordering::Relaxed) {
            return Err(error(ErrorKind::OwnershipLost, stage, Delivery::NotSent));
        }
        Ok(())
    }
}

impl Driver for Receiver {
    fn control_addrs(&self) -> Option<(SocketAddr, SocketAddr)> {
        Some((
            "127.0.0.1:40000".parse().unwrap(),
            "127.0.0.1:40001".parse().unwrap(),
        ))
    }

    fn peer_trust(&self) -> PeerTrust {
        self.trust
    }

    fn validate(&self, media: &Media) -> Result<(), Error> {
        if let Some(address) = self.fixture.fetch {
            assert_eq!(media.url(), format!("http://{address}/generated.m3u8"));
        } else {
            assert_eq!(media.url(), URL);
        }
        if media.content_type().is_none() {
            return Err(error(
                ErrorKind::InvalidInput,
                Stage::Load,
                Delivery::NotSent,
            ));
        }
        Ok(())
    }

    fn load<'a>(
        &'a mut self,
        _: Media,
        dispatch: &'a Dispatch,
    ) -> BoxFuture<'a, Result<Observation, Error>> {
        Box::pin(async move {
            self.fixture.load_entered.bump();
            if self.unsent_load {
                return Err(error(
                    ErrorKind::ReceiverRefused,
                    Stage::Load,
                    Delivery::NotSent,
                ));
            }
            self.fixture.send_gate.acquire().await.unwrap().forget();
            dispatch.mark_sent()?;
            self.fixture.owner.store(self.id, Ordering::Relaxed);
            self.fixture.load_sent.bump();
            if let Some(address) = self.fixture.fetch {
                let mut stream = TcpStream::connect(address).await.unwrap();
                stream
                    .write_all(b"GET /generated.m3u8 HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .await
                    .unwrap();
                let mut reply = Vec::new();
                stream.read_to_end(&mut reply).await.unwrap();
                assert!(reply.ends_with(b"#EXTM3U\n#EXT-X-ENDLIST\n"));
            }
            self.fixture.acknowledge.acquire().await.unwrap().forget();
            if self.fixture.queue_failed.load(Ordering::Relaxed) {
                return Err(error(
                    ErrorKind::TimedOut,
                    Stage::Load,
                    Delivery::MayHaveReachedReceiver,
                ));
            }
            self.proven = true;
            Ok(Self::observe_playback(Playback::Accepted))
        })
    }

    fn observe(&mut self) -> BoxFuture<'_, Result<Observation, Error>> {
        Box::pin(async move {
            match self.events.recv().await.unwrap() {
                Event::Playback(playback) => {
                    Ok(Self::observe_playback(playback).with_link(Link::Connected, None))
                }
                Event::Recover => Ok(Observation::link(Link::Recovering, None)),
                Event::LoseLink => Err(error(
                    ErrorKind::Unreachable,
                    Stage::Observe,
                    Delivery::NotSent,
                )),
            }
        })
    }

    fn command(&mut self, _: Command) -> BoxFuture<'_, Result<Confirmation, Error>> {
        Box::pin(async move {
            self.check_owner(Stage::Control)?;
            self.fixture.controls.bump();
            if self.fixture.block_command.load(Ordering::Relaxed) {
                std::future::pending::<()>().await;
            }
            Ok(Confirmation::ReceiverConfirmed)
        })
    }

    fn stop(&mut self) -> BoxFuture<'_, Result<StopOutcome, Error>> {
        Box::pin(async move {
            if !self.proven {
                return Err(error(ErrorKind::TimedOut, Stage::Stop, Delivery::NotSent));
            }
            self.check_owner(Stage::Stop)?;
            self.fixture.stops.bump();
            if self.fixture.block_stop.load(Ordering::Relaxed) {
                pending::<()>().await;
            }
            Ok(StopOutcome::StopRequested(Confirmation::ReceiverConfirmed))
        })
    }

    fn close(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.fixture.closes.bump() })
    }
}

fn cast(backend: impl Backend + 'static) -> Cast {
    Cast::builder().build_with(vec![Arc::new(backend)]).unwrap()
}

async fn route(cast: &Cast) -> Route {
    cast.discover(WINDOW)
        .await
        .unwrap()
        .routes()
        .first()
        .unwrap()
        .clone()
}

async fn connection(cast: &Cast, route: &Route) -> Connection {
    match cast.connect(route).await.unwrap() {
        Connect::Ready(connection) => connection,
        Connect::PinRequired(_) => panic!("unexpected PIN flow"),
    }
}

async fn pairing(cast: &Cast, route: &Route) -> Pairing {
    match cast.connect(route).await.unwrap() {
        Connect::Ready(_) => panic!("unexpected ready connection"),
        Connect::PinRequired(pairing) => pairing,
    }
}

fn media() -> Media {
    Media::on_demand(URL)
        .unwrap()
        .with_content_type("application/vnd.apple.mpegurl")
        .unwrap()
}

async fn display_start(session: &Session) -> Snapshot {
    timeout(Duration::from_secs(5), async {
        let mut updates = session.subscribe();
        while let Some(snapshot) = updates.next().await {
            if matches!(snapshot.playback(), Playback::Accepted) {
                return snapshot;
            }
        }
        panic!("accepted status expected");
    })
    .await
    .expect("bounded startup")
}

struct Scan {
    protocol: &'static str,
    status: ScanStatus,
}

enum ScanStatus {
    Fail,
    Empty,
    Partial,
    Pending,
}

impl Backend for Scan {
    fn protocol(&self) -> ProtocolId {
        ProtocolId::new(self.protocol)
    }

    fn discover(&self, deadline: TokioInstant, sink: ScanSink) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            match self.status {
                ScanStatus::Fail => Err(error(
                    ErrorKind::Protocol,
                    Stage::Discovery,
                    Delivery::NotSent,
                )),
                ScanStatus::Empty => {
                    sleep_until(deadline).await;
                    Ok(())
                }
                ScanStatus::Partial => {
                    sink.push(Candidate::new(
                        "Generated receiver".into(),
                        Some("stable".into()),
                        (),
                        hints(),
                    ));
                    pending().await
                }
                ScanStatus::Pending => pending().await,
            }
        })
    }

    fn connect<'a>(
        &'a self,
        _: &'a Route,
        _: Option<Credential>,
    ) -> BoxFuture<'a, Result<BackendConnect, Error>> {
        unreachable!()
    }
}

#[tokio::test(start_paused = true)]
async fn discovers_independent_protocols_under_one_deadline() {
    let fixture = Arc::new(Fixture::default());
    let cast = Cast::builder()
        .build_with(vec![
            Arc::new(Scan {
                protocol: "partial",
                status: ScanStatus::Partial,
            }),
            Arc::new(fixture.backend()),
            Arc::new(Scan {
                protocol: "failed",
                status: ScanStatus::Fail,
            }),
            Arc::new(Scan {
                protocol: "empty",
                status: ScanStatus::Empty,
            }),
        ])
        .unwrap();
    let first = cast.discover(WINDOW).await.unwrap();
    let second = cast.discover(WINDOW).await.unwrap();
    assert_eq!(first.routes().len(), 3);
    let [partial, stable, ephemeral] = first.routes() else {
        unreachable!()
    };
    assert_eq!(stable.name(), partial.name());
    assert_eq!(stable.name(), ephemeral.name());
    let debug = format!("{stable:?}");
    assert!(debug.contains(stable.id().as_str()));
    assert!(!debug.contains(stable.name()));
    assert_eq!(stable.id(), second.routes()[1].id());
    assert_ne!(stable.id(), partial.id());
    assert_ne!(ephemeral.id(), second.routes()[2].id());
    assert_eq!(
        stable.availability(Feature::LoadVideoUrl),
        Availability::Available
    );
    assert_eq!(stable.protocol().as_str(), "test-protocol");
    assert!(stable.id().as_str().starts_with("test-protocol:"));
    assert_eq!(first.issues().len(), 2);
    assert_eq!(first.issues()[0].protocol().as_str(), "partial");
    assert_eq!(first.issues()[0].error().kind(), ErrorKind::TimedOut);
    assert_eq!(first.issues()[1].error().kind(), ErrorKind::Protocol);
    let foreign = self::cast(fixture.backend());
    assert_error(
        foreign.connect(stable).await.err().unwrap(),
        ErrorKind::InvalidInput,
        Stage::Connect,
        Delivery::NotSent,
    );
    assert_error(
        foreign.forget(stable).await.unwrap_err(),
        ErrorKind::InvalidInput,
        Stage::Store,
        Delivery::NotSent,
    );
    cast.forget(ephemeral).await.unwrap();
    cast.forget(partial).await.unwrap();
    assert_error(
        cast.discover(Duration::ZERO).await.err().unwrap(),
        ErrorKind::InvalidInput,
        Stage::Discovery,
        Delivery::NotSent,
    );
}

#[tokio::test(start_paused = true)]
async fn distinguishes_empty_scan_from_all_backends_failing() {
    let failed = cast(Scan {
        protocol: "fail",
        status: ScanStatus::Fail,
    });
    let error = failed.discover(WINDOW).await.err().unwrap();
    assert_eq!(error.kind(), ErrorKind::Unavailable);
    assert_eq!(error.discovery_issues().len(), 1);
    let pending = cast(Scan {
        protocol: "pending",
        status: ScanStatus::Pending,
    });
    assert_eq!(
        pending.discover(WINDOW).await.err().unwrap().kind(),
        ErrorKind::TimedOut
    );
    let empty = cast(Scan {
        protocol: "empty",
        status: ScanStatus::Empty,
    });
    let results = empty.discover(WINDOW).await.unwrap();
    assert!(results.routes().is_empty() && results.issues().is_empty());
}

#[test]
fn off_runtime_build_and_shutdown_are_reported() {
    assert_error(
        Cast::builder().build().err().unwrap(),
        ErrorKind::Unavailable,
        Stage::Configuration,
        Delivery::NotSent,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = Arc::new(Fixture::default());
    let (cast, route, connection) = runtime.block_on(async {
        assert_error(
            Cast::builder().build_with(Vec::new()).err().unwrap(),
            ErrorKind::Unavailable,
            Stage::Configuration,
            Delivery::NotSent,
        );
        let mut backend = fixture.backend();
        backend.pair = true;
        let cast = cast(backend);
        let route = route(&cast).await;
        fixture.pairing_gate.add_permits(1);
        let connection = pairing(&cast, &route)
            .await
            .submit(PinCode::parse("0042").unwrap())
            .await
            .unwrap();
        (cast, route, connection)
    });
    drop(runtime);
    assert_error(
        connection.load(media()).err().unwrap(),
        ErrorKind::Unavailable,
        Stage::Load,
        Delivery::NotSent,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert_error(
            cast.discover(WINDOW).await.err().unwrap(),
            ErrorKind::Unavailable,
            Stage::Discovery,
            Delivery::NotSent,
        );
        assert_error(
            cast.connect(&route).await.err().unwrap(),
            ErrorKind::Unavailable,
            Stage::Connect,
            Delivery::NotSent,
        );
        assert_error(
            cast.forget(&route).await.unwrap_err(),
            ErrorKind::Unavailable,
            Stage::Store,
            Delivery::NotSent,
        );
    });
}

#[tokio::test]
async fn loads_promptly_off_runtime_and_retains_consistent_observations() {
    let fixture = Arc::new(Fixture::default());
    let cast = cast(fixture.backend());
    let connection = connection(&cast, &route(&cast).await).await;
    assert_eq!(connection.peer_trust(), PeerTrust::LanEndpoint);
    assert_eq!(connection.control_addrs().unwrap().1.port(), 40001);
    drop(cast);
    let mut session = std::thread::spawn(move || connection.load(media()).unwrap())
        .join()
        .unwrap();
    let mut updates = session.subscribe();
    let mut lagging = session.subscribe();
    let pending = updates.next().await.unwrap();
    assert!(matches!(pending.playback(), Playback::Pending));
    assert_eq!(pending.playback_observed_at(), None);
    assert_eq!(pending.revision(), 0);
    assert!(
        timeout(Duration::from_millis(2), updates.next())
            .await
            .is_err()
    );
    fixture.send_gate.add_permits(1);
    fixture.load_sent.wait(1).await;
    assert!(matches!(session.snapshot().playback(), Playback::Pending));
    fixture.acknowledge.add_permits(1);
    let accepted = updates.next().await.unwrap();
    assert!(matches!(accepted.playback(), Playback::Accepted));
    assert_eq!(accepted.volume_scope(), Some(VolumeScope::Receiver));
    assert_eq!(accepted.volume().unwrap().as_percent(), 20);
    assert_eq!(accepted.muted(), Some(false));
    assert_eq!(accepted.position(), Some(Duration::from_secs(4)));
    assert_eq!(accepted.duration(), Some(Duration::from_secs(21)));
    assert_eq!(
        accepted.availability(Feature::Seek),
        Availability::Available
    );

    fixture.event(Event::Playback(Playback::Playing));
    let playing = updates.next().await.unwrap();
    assert!(matches!(playing.playback(), Playback::Playing));
    assert!(playing.revision() > accepted.revision());
    assert!(playing.playback_observed_at() >= accepted.playback_observed_at());
    assert_eq!(
        session.command(Command::Play).await.unwrap(),
        Confirmation::ReceiverConfirmed
    );
    assert!(
        timeout(Duration::from_millis(2), updates.next())
            .await
            .is_err()
    );

    fixture.event(Event::Recover);
    let recovering = updates.next().await.unwrap();
    assert_eq!(recovering.link(), Link::Recovering);
    assert_eq!(
        recovering.playback_observed_at(),
        playing.playback_observed_at()
    );
    assert!(matches!(recovering.playback(), Playback::Playing));
    assert_eq!(
        recovering.availability(Feature::Play),
        Availability::Unavailable
    );
    assert_error(
        session.command(Command::Play).await.unwrap_err(),
        ErrorKind::Unavailable,
        Stage::Control,
        Delivery::NotSent,
    );

    fixture.event(Event::LoseLink);
    let disconnected = updates.next().await.unwrap();
    assert_eq!(disconnected.link(), Link::Disconnected);
    assert_eq!(
        disconnected.link_error().unwrap().kind(),
        ErrorKind::Unreachable
    );
    assert!(matches!(disconnected.playback(), Playback::Playing));
    assert_eq!(
        disconnected.playback_observed_at(),
        playing.playback_observed_at()
    );
    assert_eq!(lagging.next().await.unwrap().revision(), pending.revision());
    assert_eq!(
        lagging.next().await.unwrap().revision(),
        disconnected.revision()
    );
    assert!(lagging.next().await.is_none() && updates.next().await.is_none());
    assert_error(
        session.stop().await.unwrap_err(),
        ErrorKind::Unreachable,
        Stage::Stop,
        Delivery::NotSent,
    );
    assert_eq!(fixture.controls.read(), 1);
    assert_eq!(fixture.stops.read(), 0);
    assert_eq!(fixture.closes.read(), 1);
}

#[tokio::test]
async fn cancellation_and_terminal_load_results_respect_delivery() {
    let fixture = Arc::new(Fixture::default());
    let cast = cast(fixture.backend());
    let route = route(&cast).await;
    let session = connection(&cast, &route).await.load(media()).unwrap();
    fixture.load_entered.wait(1).await;
    assert_eq!(
        session.stop().await.unwrap(),
        StopOutcome::CancelledBeforeSend
    );
    assert_eq!(fixture.load_sent.read(), 0);

    let session = connection(&cast, &route).await.load(media()).unwrap();
    drop(session);
    fixture.closes.wait(2).await;
    assert_eq!(fixture.load_sent.read(), 0);

    let mut fake = fixture.backend();
    fake.unsent_load = true;
    let unsent_cast = self::cast(fake);
    let unsent_route = self::route(&unsent_cast).await;
    let session = connection(&unsent_cast, &unsent_route)
        .await
        .load(media())
        .unwrap();
    let mut updates = session.subscribe();
    let mut final_failure = None;
    while let Some(snapshot) = updates.next().await {
        if let Playback::Failed(error) = snapshot.playback() {
            final_failure = Some((error.delivery(), snapshot.link()));
        }
    }
    assert_eq!(final_failure, Some((Delivery::NotSent, Link::Disconnected)));
    assert_eq!(
        session.stop().await.unwrap(),
        StopOutcome::CancelledBeforeSend
    );

    let result = connection(&cast, &route)
        .await
        .load(Media::live(URL).unwrap());
    assert_error(
        result.err().unwrap(),
        ErrorKind::InvalidInput,
        Stage::Load,
        Delivery::NotSent,
    );
    fixture.closes.wait(4).await;
}

#[tokio::test]
async fn equal_urls_do_not_authorize_stopping_a_replacement() {
    let fixture = Arc::new(Fixture::default());
    fixture.send_gate.add_permits(2);
    let cast = cast(fixture.backend());
    let route = route(&cast).await;
    let old = connection(&cast, &route).await.load(media()).unwrap();
    fixture.load_sent.wait(1).await;
    // This receiver starts fetching before the old LOAD's acknowledgement.
    // The replacement is a distinct load even though its URL is identical.
    let mut replacement = connection(&cast, &route).await.load(media()).unwrap();
    fixture.load_sent.wait(2).await;
    assert_error(
        old.stop().await.unwrap_err(),
        ErrorKind::TimedOut,
        Stage::Stop,
        Delivery::NotSent,
    );
    assert_eq!(fixture.stops.read(), 0);

    fixture.acknowledge.add_permits(1);
    display_start(&replacement).await;
    assert_error(
        replacement
            .command(Command::Output {
                scope: VolumeScope::Media,
                change: OutputChange::SetVolume(Volume::percent(30).unwrap()),
            })
            .await
            .unwrap_err(),
        ErrorKind::Unsupported,
        Stage::Control,
        Delivery::NotSent,
    );
    assert_error(
        replacement
            .command(Command::Output {
                scope: VolumeScope::Receiver,
                change: OutputChange::StepVolume(-2),
            })
            .await
            .unwrap_err(),
        ErrorKind::Unsupported,
        Stage::Control,
        Delivery::NotSent,
    );
    assert_error(
        replacement.command(Command::Pause).await.unwrap_err(),
        ErrorKind::Unavailable,
        Stage::Control,
        Delivery::NotSent,
    );
    assert_eq!(
        replacement
            .command(Command::Output {
                scope: VolumeScope::Receiver,
                change: OutputChange::SetMuted(true),
            })
            .await
            .unwrap(),
        Confirmation::ReceiverConfirmed
    );
    assert_eq!(
        replacement
            .command(Command::Output {
                scope: VolumeScope::Receiver,
                change: OutputChange::ToggleMute,
            })
            .await
            .unwrap(),
        Confirmation::ReceiverConfirmed
    );
    assert_eq!(
        replacement
            .command(Command::Seek(Duration::from_secs(5)))
            .await
            .unwrap(),
        Confirmation::ReceiverConfirmed
    );

    let mut observer = replacement.subscribe();
    assert!(matches!(
        observer.next().await.unwrap().playback(),
        Playback::Accepted
    ));
    assert_eq!(
        replacement.stop().await.unwrap(),
        StopOutcome::StopRequested(Confirmation::ReceiverConfirmed)
    );
    let final_snapshot = observer.next().await.unwrap();
    assert!(matches!(final_snapshot.playback(), Playback::Ended));
    assert_eq!(final_snapshot.link(), Link::Disconnected);
    assert!(observer.next().await.is_none());
    assert_eq!(fixture.stops.read(), 1);
}

#[tokio::test]
async fn failed_load_reconciles_once_and_retains_the_original_diagnostic() {
    let fixture = Arc::new(Fixture::default());
    fixture.queue_failed.store(true, Ordering::Relaxed);
    fixture.send_gate.add_permits(1);
    fixture.acknowledge.add_permits(1);
    let cast = cast(fixture.backend());
    let session = connection(&cast, &route(&cast).await)
        .await
        .load(media())
        .unwrap();
    fixture.closes.wait(1).await;
    let final_snapshot = session.snapshot();
    let Playback::Failed(failed) = final_snapshot.playback() else {
        panic!("expected failed load")
    };
    assert_eq!(failed.delivery(), Delivery::MayHaveReachedReceiver);
    assert_eq!(final_snapshot.link(), Link::Disconnected);
    assert_error(
        session.stop().await.unwrap_err(),
        ErrorKind::TimedOut,
        Stage::Stop,
        Delivery::NotSent,
    );
    assert_eq!(fixture.stops.read(), 0);
    assert_eq!(fixture.closes.read(), 1);
}

#[tokio::test]
async fn confirmed_takeover_and_natural_end_do_not_send_stop() {
    for playback in [
        Playback::OwnershipLost,
        Playback::NoLongerPresent,
        Playback::Ended,
    ] {
        let fixture = Arc::new(Fixture::default());
        fixture.send_gate.add_permits(1);
        fixture.acknowledge.add_permits(1);
        let cast = cast(fixture.backend());
        let mut session = connection(&cast, &route(&cast).await)
            .await
            .load(media())
            .unwrap();
        display_start(&session).await;
        let mut observer = session.subscribe();
        observer.next().await.unwrap();
        fixture.event(Event::Playback(playback.clone()));
        let final_snapshot = observer.next().await.unwrap();
        assert_eq!(final_snapshot.link(), Link::Disconnected);
        assert!(observer.next().await.is_none());
        let command = session.command(Command::Play).await.unwrap_err();
        let outcome = session.stop().await;
        if matches!(playback, Playback::OwnershipLost) {
            assert_error(
                command,
                ErrorKind::OwnershipLost,
                Stage::Control,
                Delivery::NotSent,
            );
            assert_error(
                outcome.unwrap_err(),
                ErrorKind::OwnershipLost,
                Stage::Stop,
                Delivery::NotSent,
            );
        } else {
            assert_error(
                command,
                ErrorKind::Unavailable,
                Stage::Control,
                Delivery::NotSent,
            );
            assert_eq!(outcome.unwrap(), StopOutcome::AlreadyEnded);
        }
        assert_eq!(fixture.stops.read(), 0);
    }
}

#[tokio::test]
async fn close_is_local_only_and_live_seek_is_disabled() {
    let fixture = Arc::new(Fixture::default());
    fixture.send_gate.add_permits(1);
    fixture.acknowledge.add_permits(1);
    let cast = cast(fixture.backend());
    let route = route(&cast).await;
    let live = Media::live(URL)
        .unwrap()
        .with_content_type("application/vnd.apple.mpegurl")
        .unwrap();
    let mut session = connection(&cast, &route).await.load(live).unwrap();
    assert_eq!(
        display_start(&session).await.availability(Feature::Seek),
        Availability::Unsupported
    );
    assert_error(
        session
            .command(Command::Seek(Duration::from_secs(1)))
            .await
            .unwrap_err(),
        ErrorKind::Unsupported,
        Stage::Control,
        Delivery::NotSent,
    );
    let mut observer = session.subscribe();
    observer.next().await.unwrap();
    session.close().await;
    let final_snapshot = observer.next().await.unwrap();
    assert!(matches!(final_snapshot.playback(), Playback::Accepted));
    assert_eq!(final_snapshot.link(), Link::Disconnected);
    assert!(observer.next().await.is_none());
    connection(&cast, &route).await.close().await;
    assert_eq!(fixture.closes.read(), 2);
    assert_eq!(fixture.stops.read(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_command_retires_with_uncertainty_and_leaves_stop_available() {
    let fixture = Arc::new(Fixture::default());
    fixture.block_command.store(true, Ordering::Relaxed);
    fixture.send_gate.add_permits(1);
    fixture.acknowledge.add_permits(1);
    let cast = cast(fixture.backend());
    let route = route(&cast).await;
    let mut session = connection(&cast, &route).await.load(media()).unwrap();
    display_start(&session).await;
    assert_error(
        session.command(Command::Play).await.unwrap_err(),
        ErrorKind::TimedOut,
        Stage::Control,
        Delivery::MayHaveReachedReceiver,
    );
    assert_eq!(fixture.controls.read(), 1);
    assert_eq!(
        session.stop().await.unwrap(),
        StopOutcome::StopRequested(Confirmation::ReceiverConfirmed),
    );
    assert_eq!(fixture.closes.read(), 1);
}

#[tokio::test(start_paused = true)]
async fn retiring_a_session_interrupts_an_abandoned_command() {
    let fixture = Arc::new(Fixture::default());
    fixture.block_command.store(true, Ordering::Relaxed);
    fixture.send_gate.add_permits(1);
    fixture.acknowledge.add_permits(1);
    let cast = cast(fixture.backend());
    let route = route(&cast).await;
    let mut session = connection(&cast, &route).await.load(media()).unwrap();
    display_start(&session).await;
    {
        let command = session.command(Command::Play);
        tokio::select! {
            _ = command => panic!("stalled command resolved before retirement"),
            () = fixture.controls.wait(1) => {}
        }
    }
    assert_eq!(
        timeout(Duration::from_secs(1), session.stop())
            .await
            .unwrap()
            .unwrap(),
        StopOutcome::StopRequested(Confirmation::ReceiverConfirmed),
    );
    assert_eq!(fixture.closes.read(), 1);
}

#[tokio::test]
async fn pairing_reuses_credentials_and_rejects_identity_mismatch() {
    let fixture = Arc::new(Fixture::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    let cast = cast(backend);
    let route = route(&cast).await;
    let first = pairing(&cast, &route).await;
    assert_eq!(first.expires_at(), None);
    assert_error(
        cast.connect(&route).await.err().unwrap(),
        ErrorKind::Busy,
        Stage::Pair,
        Delivery::NotSent,
    );
    fixture.pairing_gate.add_permits(1);
    let connection = first.submit(PinCode::parse("0042").unwrap()).await.unwrap();
    assert_eq!(connection.peer_trust(), PeerTrust::VerifiedPairing);
    connection.close().await;
    self::connection(&cast, &route).await.close().await;
    assert_eq!(fixture.pairs.read(), 1);
    fixture.identity.fetch_add(1, Ordering::Relaxed);
    assert_error(
        cast.connect(&route).await.err().unwrap(),
        ErrorKind::IdentityMismatch,
        Stage::Connect,
        Delivery::NotSent,
    );
    cast.forget(&route).await.unwrap();
    let pairing = pairing(&cast, &route).await;
    pairing.cancel().await;
    assert_eq!(fixture.cancelled_pairs.read(), 1);
}

#[tokio::test(start_paused = true)]
async fn interrupted_connect_reports_uncertainty_after_backend_dispatch() {
    let fixture = Arc::new(Fixture::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    backend.block_connect = true;
    let cast = cast(backend);
    let route = route(&cast).await;
    assert_error(
        cast.connect(&route).await.err().unwrap(),
        ErrorKind::TimedOut,
        Stage::Connect,
        Delivery::MayHaveReachedReceiver,
    );
    let (worker, selected) = (cast.clone(), route.clone());
    let connecting = tokio::spawn(async move { worker.connect(&selected).await });
    fixture.connect_entered.wait(2).await;
    cast.forget(&route).await.unwrap();
    assert_error(
        connecting.await.unwrap().err().unwrap(),
        ErrorKind::Cancelled,
        Stage::Connect,
        Delivery::MayHaveReachedReceiver,
    );
}

#[tokio::test(start_paused = true)]
async fn pairing_retires_on_expiry_wrong_pin_and_drop() {
    let fixture = Arc::new(Fixture::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    backend.expires = true;
    let cast = cast(backend);
    let route = route(&cast).await;
    let first = pairing(&cast, &route).await;
    assert!(first.expires_at().is_some());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_error(
        first
            .submit(PinCode::parse("0042").unwrap())
            .await
            .err()
            .unwrap(),
        ErrorKind::Expired,
        Stage::Pair,
        Delivery::NotSent,
    );

    let second = pairing(&cast, &route).await;
    fixture.pairing_gate.add_permits(1);
    assert_error(
        second
            .submit(PinCode::parse("42").unwrap())
            .await
            .err()
            .unwrap(),
        ErrorKind::InvalidPin,
        Stage::Pair,
        Delivery::Acknowledged,
    );

    drop(pairing(&cast, &route).await);
    fixture.cancelled_pairs.wait(3).await;
    assert_eq!(fixture.pairs.read(), 1);
}

#[derive(Default)]
struct BlockingStore {
    record: Mutex<Option<(String, Credential)>>,
    loading: Count,
    block_load: AtomicBool,
    saving: Count,
    saving_gate: Notify,
    fail: AtomicBool,
}

fn store_error() -> StoreError {
    StoreError::new(std::io::Error::other("synthetic sensitive supplier text"))
}

impl CredentialStore for Arc<BlockingStore> {
    async fn load(&self, key: &CredentialKey) -> Result<Option<Credential>, StoreError> {
        self.loading.bump();
        if self.block_load.load(Ordering::Relaxed) {
            pending::<()>().await;
        }
        if self.fail.load(Ordering::Relaxed) {
            return Err(store_error());
        }
        Ok(self
            .record
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(stored, _)| stored == key.storage_key())
            .map(|(_, value)| value.clone()))
    }

    async fn save(&self, key: &CredentialKey, value: Credential) -> Result<(), StoreError> {
        self.saving.bump();
        self.saving_gate.notified().await;
        if self.fail.load(Ordering::Relaxed) {
            return Err(store_error());
        }
        *self.record.lock().unwrap() = Some((key.storage_key().to_owned(), value));
        Ok(())
    }

    async fn remove(&self, key: &CredentialKey) -> Result<(), StoreError> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(store_error());
        }
        let mut record = self.record.lock().unwrap();
        if record
            .as_ref()
            .is_some_and(|(stored, _)| stored == key.storage_key())
        {
            record.take();
        }
        Ok(())
    }
}

#[tokio::test]
async fn forget_joins_an_in_flight_save_before_removing_the_record() {
    let fixture = Arc::new(Fixture::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    let store = Arc::new(BlockingStore::default());
    let cast = Cast::builder()
        .credential_store(store.clone())
        .build_with(vec![Arc::new(backend)])
        .unwrap();
    let route = route(&cast).await;
    let pairing = pairing(&cast, &route).await;
    fixture.pairing_gate.add_permits(1);
    let submit = tokio::spawn(pairing.submit(PinCode::parse("0042").unwrap()));
    store.saving.wait(1).await;

    let mut forget = Box::pin(cast.forget(&route));
    assert!(futures_util::poll!(&mut forget).is_pending());
    assert_error(
        cast.connect(&route).await.err().unwrap(),
        ErrorKind::Busy,
        Stage::Pair,
        Delivery::NotSent,
    );
    store.saving_gate.notify_one();
    forget.await.unwrap();
    assert_error(
        submit.await.unwrap().err().unwrap(),
        ErrorKind::Cancelled,
        Stage::Pair,
        Delivery::Acknowledged,
    );
    assert!(store.record.lock().unwrap().is_none());

    let pairing = self::pairing(&cast, &route).await;
    cast.forget(&route).await.unwrap();
    assert_error(
        pairing
            .submit(PinCode::parse("0042").unwrap())
            .await
            .err()
            .unwrap(),
        ErrorKind::Cancelled,
        Stage::Pair,
        Delivery::NotSent,
    );
}

#[tokio::test]
async fn storage_failures_never_expose_sources() {
    let fixture = Arc::new(Fixture::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    let store = Arc::new(BlockingStore::default());
    let cast = Cast::builder()
        .credential_store(store.clone())
        .build_with(vec![Arc::new(backend)])
        .unwrap();
    let route = route(&cast).await;
    let pairing = pairing(&cast, &route).await;
    fixture.pairing_gate.add_permits(1);
    let submit = tokio::spawn(pairing.submit(PinCode::parse("0042").unwrap()));
    store.saving.wait(1).await;
    store.fail.store(true, Ordering::Relaxed);
    store.saving_gate.notify_one();
    for error in [
        submit.await.unwrap().err().unwrap(),
        cast.connect(&route).await.err().unwrap(),
        cast.forget(&route).await.unwrap_err(),
    ] {
        assert!(!format!("{error:?} {error}").contains("synthetic sensitive"));
        assert!(error.source().is_none());
        assert_error(error, ErrorKind::Storage, Stage::Store, Delivery::NotSent);
    }
    let lan_client = Cast::builder()
        .credential_store(store)
        .build_with(vec![Arc::new(fixture.backend())])
        .unwrap();
    let lan_route = self::route(&lan_client).await;
    connection(&lan_client, &lan_route).await.close().await;
    lan_client.forget(&lan_route).await.unwrap();
}

#[test]
fn boundary_types_validate_and_redact() {
    for url in [
        "file:///test",
        "relative",
        "http:relative",
        "http:///path",
        "http://@invalid.test",
        "http://user:pass@invalid.test",
        "http:// a.test",
        "http://[",
    ] {
        assert_error(
            Media::live(url).err().unwrap(),
            ErrorKind::InvalidInput,
            Stage::Load,
            Delivery::NotSent,
        );
    }
    for mime in ["text", "*/*", "video/*"] {
        assert_error(
            Media::live(URL)
                .unwrap()
                .with_content_type(mime)
                .err()
                .unwrap(),
            ErrorKind::InvalidInput,
            Stage::Load,
            Delivery::NotSent,
        );
    }
    assert!(!format!("{:?}", media()).contains("synthetic-token"));
    assert_eq!(Volume::percent(100).unwrap().as_percent(), 100);
    assert_error(
        Volume::percent(101).unwrap_err(),
        ErrorKind::InvalidInput,
        Stage::Control,
        Delivery::NotSent,
    );
    assert_eq!(PinCode::parse("0042").unwrap().as_str(), "0042");
    for pin in ["", " 22", "１２３", "a7"] {
        assert_error(
            PinCode::parse(pin).unwrap_err(),
            ErrorKind::InvalidPin,
            Stage::Pair,
            Delivery::NotSent,
        );
    }
    assert!(!format!("{:?}", PinCode::parse("0042").unwrap()).contains("0042"));
    assert!(
        !format!(
            "{:?}",
            Credential::from_bytes(b"synthetic sensitive".to_vec())
        )
        .contains("synthetic sensitive")
    );
    let storage = store_error();
    assert!(!format!("{storage:?} {storage}").contains("synthetic sensitive"));
    assert!(storage.source().is_none());
    let diagnostic = Diagnostic::TimingListener {
        local_addr: "127.0.0.1:55555".parse().unwrap(),
    };
    let error = error(ErrorKind::TimedOut, Stage::Load, Delivery::Acknowledged)
        .with_diagnostic(diagnostic.clone());
    assert_eq!(error.diagnostic(), Some(&diagnostic));
    let mut local = Snapshot::pending(false);
    local.apply(Observation::playback(Playback::Ended));
    local.apply(Observation::playback(Playback::Playing));
    assert!(matches!(local.playback(), Playback::Ended));
}

#[tokio::test]
async fn receiver_fetches_gated_origin_before_load_acknowledgement() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = Arc::new(Fixture {
        fetch: Some(address),
        ..Fixture::default()
    });
    fixture.send_gate.add_permits(1);
    fixture.acknowledge.add_permits(1);
    let (requested, first_fetch) = tokio::sync::oneshot::channel();
    let (admit, admitted) = tokio::sync::oneshot::channel();
    let origin = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        socket.read_line(&mut line).await.unwrap();
        assert_eq!(line, "GET /generated.m3u8 HTTP/1.1\r\n");
        loop {
            line.clear();
            assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        requested.send(()).unwrap();
        admitted.await.unwrap();
        let mut socket = socket.into_inner();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n#EXTM3U\n#EXT-X-ENDLIST\n")
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    });
    let cast = cast(fixture.backend());
    let route = route(&cast).await;
    let media = Media::on_demand(&format!("http://{address}/generated.m3u8"))
        .unwrap()
        .with_content_type("application/vnd.apple.mpegurl")
        .unwrap();
    let session = connection(&cast, &route).await.load(media).unwrap();
    timeout(Duration::from_secs(5), first_fetch)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(session.snapshot().playback(), Playback::Pending));
    admit.send(()).unwrap();
    display_start(&session).await;
    session.stop().await.unwrap();
    origin.await.unwrap();
}

#[tokio::test]
async fn current_ownership_is_rechecked_before_receiver_wide_commands() {
    let fixture = Arc::new(Fixture::default());
    fixture.send_gate.add_permits(2);
    fixture.acknowledge.add_permits(2);
    let cast = cast(fixture.backend());
    let route = route(&cast).await;
    let mut old = connection(&cast, &route).await.load(media()).unwrap();
    display_start(&old).await;
    let current = connection(&cast, &route).await.load(media()).unwrap();
    display_start(&current).await;
    assert_error(
        old.command(Command::Output {
            scope: VolumeScope::Receiver,
            change: OutputChange::SetVolume(Volume::percent(20).unwrap()),
        })
        .await
        .unwrap_err(),
        ErrorKind::OwnershipLost,
        Stage::Control,
        Delivery::NotSent,
    );
    assert_error(
        old.stop().await.unwrap_err(),
        ErrorKind::OwnershipLost,
        Stage::Stop,
        Delivery::NotSent,
    );
    assert_eq!(fixture.controls.read(), 0);
    assert_eq!(fixture.stops.read(), 0);
    current.close().await;
}

#[tokio::test(start_paused = true)]
async fn load_timeout_records_pre_and_post_dispatch_delivery() {
    for sent in [false, true] {
        let fixture = Arc::new(Fixture::default());
        if sent {
            fixture.send_gate.add_permits(1);
        }
        let cast = cast(fixture.backend());
        let session = connection(&cast, &route(&cast).await)
            .await
            .load(media())
            .unwrap();
        fixture.load_entered.wait(1).await;
        let mut updates = session.subscribe();
        while let Some(snapshot) = updates.next().await {
            if let Playback::Failed(error) = snapshot.playback() {
                let expected = if sent {
                    Delivery::MayHaveReachedReceiver
                } else {
                    Delivery::NotSent
                };
                assert_eq!(
                    (error.kind(), error.delivery()),
                    (ErrorKind::TimedOut, expected)
                );
            }
        }
        if sent {
            assert_error(
                session.stop().await.unwrap_err(),
                ErrorKind::TimedOut,
                Stage::Stop,
                Delivery::NotSent,
            );
        } else {
            assert_eq!(
                session.stop().await.unwrap(),
                StopOutcome::CancelledBeforeSend
            );
        }
    }
}

#[tokio::test]
async fn cancelling_submit_retires_partial_pairing_and_insecure_pairing_is_discarded() {
    let fixture = Arc::new(Fixture::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    backend.insecure_pair = true;
    let cast = cast(backend);
    let route = route(&cast).await;
    let pending = pairing(&cast, &route).await;
    let submit = tokio::spawn(pending.submit(PinCode::parse("0042").unwrap()));
    fixture.pairs.wait(1).await;
    submit.abort();
    assert!(submit.await.err().unwrap().is_cancelled());
    fixture.cancelled_pairs.wait(1).await;

    fixture.pairing_gate.add_permits(1);
    let restart = pairing(&cast, &route).await;
    assert_error(
        restart
            .submit(PinCode::parse("0042").unwrap())
            .await
            .err()
            .unwrap(),
        ErrorKind::IdentityMismatch,
        Stage::Pair,
        Delivery::Acknowledged,
    );
    assert_eq!(fixture.closes.read(), 1);
    pairing(&cast, &route).await.cancel().await;
}

#[tokio::test(start_paused = true)]
async fn stalled_stop_joins_local_cleanup_and_preserves_last_playback_evidence() {
    let fixture = Arc::new(Fixture::default());
    fixture.block_stop.store(true, Ordering::Relaxed);
    fixture.send_gate.add_permits(1);
    fixture.acknowledge.add_permits(1);
    let cast = cast(fixture.backend());
    let mut session = connection(&cast, &route(&cast).await)
        .await
        .load(media())
        .unwrap();
    assert_error(
        session
            .command(Command::Output {
                scope: VolumeScope::Receiver,
                change: OutputChange::SetMuted(true),
            })
            .await
            .unwrap_err(),
        ErrorKind::Unavailable,
        Stage::Control,
        Delivery::NotSent,
    );
    display_start(&session).await;
    let mut observer = session.subscribe();
    assert!(matches!(
        observer.next().await.unwrap().playback(),
        Playback::Accepted
    ));
    assert_error(
        timeout(Duration::from_secs(30), session.stop())
            .await
            .expect("stalled stop must retire within its deadline")
            .unwrap_err(),
        ErrorKind::TimedOut,
        Stage::Stop,
        Delivery::MayHaveReachedReceiver,
    );
    assert_eq!(fixture.stops.read(), 1);
    assert_eq!(fixture.closes.read(), 1);
    let final_snapshot = observer.next().await.unwrap();
    assert!(matches!(final_snapshot.playback(), Playback::Accepted));
    assert_eq!(final_snapshot.link(), Link::Disconnected);
    assert!(observer.next().await.is_none());
}

#[tokio::test]
async fn playback_failure_after_acknowledgement_retires_without_remote_write() {
    let fixture = Arc::new(Fixture::default());
    fixture.send_gate.add_permits(1);
    fixture.acknowledge.add_permits(1);
    let cast = cast(fixture.backend());
    let session = connection(&cast, &route(&cast).await)
        .await
        .load(media())
        .unwrap();
    display_start(&session).await;
    let mut observer = session.subscribe();
    observer.next().await.unwrap();
    fixture.event(Event::Playback(Playback::Failed(error(
        ErrorKind::ReceiverRefused,
        Stage::Observe,
        Delivery::Acknowledged,
    ))));

    let final_snapshot = observer.next().await.unwrap();
    let Playback::Failed(error) = final_snapshot.playback() else {
        panic!("expected receiver-reported playback failure");
    };
    assert_eq!(error.stage(), Stage::Observe);
    assert_eq!(error.delivery(), Delivery::Acknowledged);
    assert_eq!(final_snapshot.link(), Link::Disconnected);
    assert!(observer.next().await.is_none());
    assert_error(
        session.stop().await.unwrap_err(),
        ErrorKind::ReceiverRefused,
        Stage::Stop,
        Delivery::NotSent,
    );
    assert_eq!(fixture.stops.read(), 0);
}

#[tokio::test]
async fn transient_routes_pair_without_creating_a_durable_record() {
    let fixture = Arc::new(Fixture::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    let store = Arc::new(BlockingStore::default());
    let cast = Cast::builder()
        .credential_store(store.clone())
        .build_with(vec![Arc::new(backend)])
        .unwrap();
    let scan = cast.discover(WINDOW).await.unwrap();
    let route = &scan.routes()[1];
    for _ in 0..2 {
        fixture.pairing_gate.add_permits(1);
        let connection = timeout(
            Duration::from_secs(5),
            pairing(&cast, route)
                .await
                .submit(PinCode::parse("0042").unwrap()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(connection.peer_trust(), PeerTrust::VerifiedPairing);
        connection.close().await;
    }
    cast.forget(route).await.unwrap();
    assert_eq!(store.loading.read(), 0);
    assert_eq!(store.saving.read(), 0);
    assert!(store.record.lock().unwrap().is_none());
}

#[tokio::test]
async fn existing_credentials_require_verified_identity_before_playback_or_repair() {
    let fixture = Arc::new(Fixture::default());
    let store = Arc::new(BlockingStore::default());
    let mut backend = fixture.backend();
    backend.pair = true;
    let cast = Cast::builder()
        .credential_store(store.clone())
        .build_with(vec![Arc::new(backend)])
        .unwrap();
    let target = route(&cast).await;
    fixture.pairing_gate.add_permits(1);
    store.saving_gate.notify_one();
    pairing(&cast, &target)
        .await
        .submit(PinCode::parse("0042").unwrap())
        .await
        .unwrap()
        .close()
        .await;
    drop(cast);
    let mut backend = fixture.backend();
    backend.pair = true;
    backend.insecure_pair = true;
    let cast = Cast::builder()
        .credential_store(store.clone())
        .build_with(vec![Arc::new(backend)])
        .unwrap();
    let target = route(&cast).await;
    assert_error(
        cast.connect(&target).await.err().unwrap(),
        ErrorKind::IdentityMismatch,
        Stage::Connect,
        Delivery::NotSent,
    );
    assert_eq!(fixture.closes.read(), 2);
    drop(cast);

    let mut backend = fixture.backend();
    backend.pair = true;
    backend.ignore_stored_credentials = true;
    let cast = Cast::builder()
        .credential_store(store.clone())
        .build_with(vec![Arc::new(backend)])
        .unwrap();
    let target = route(&cast).await;
    assert_error(
        cast.connect(&target).await.err().unwrap(),
        ErrorKind::IdentityMismatch,
        Stage::Connect,
        Delivery::NotSent,
    );
    assert_eq!(fixture.cancelled_pairs.read(), 1);
    assert_eq!(store.saving.read(), 1);
    assert!(store.record.lock().unwrap().is_some());
}

#[tokio::test]
async fn forgetting_during_credential_lookup_cancels_before_network_dispatch() {
    let fixture = Arc::new(Fixture::default());
    let store = Arc::new(BlockingStore::default());
    store.block_load.store(true, Ordering::Relaxed);
    let mut backend = fixture.backend();
    backend.pair = true;
    backend.block_connect = true;
    let cast = Cast::builder()
        .credential_store(store.clone())
        .build_with(vec![Arc::new(backend)])
        .unwrap();
    let target = route(&cast).await;
    let lookup = {
        let cast = cast.clone();
        let target = target.clone();
        tokio::spawn(async move { cast.connect(&target).await })
    };
    store.loading.wait(1).await;
    let result = timeout(Duration::from_secs(5), async {
        cast.forget(&target).await.unwrap();
        lookup.await.unwrap()
    })
    .await
    .expect("forget must interrupt the stalled credential lookup");
    assert_error(
        result.err().unwrap(),
        ErrorKind::Cancelled,
        Stage::Connect,
        Delivery::NotSent,
    );
    assert_eq!(fixture.connect_entered.read(), 0);
}
