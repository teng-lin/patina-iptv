use super::*;
use crate::{
    Cast, Connect, CredentialKey, CredentialStore, Link, StoreError,
    airplay::{
        hap::tests::{Accessory, PIN},
        video::tests::{ReceiverEnding, serve},
    },
};
use std::sync::{Arc, Mutex};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, UdpSocket},
    sync::oneshot,
    time::timeout,
};

#[derive(Clone, Default)]
struct TestStore(Arc<Mutex<Option<Credential>>>);

impl CredentialStore for TestStore {
    async fn load(&self, _: &CredentialKey) -> Result<Option<Credential>, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn save(&self, _: &CredentialKey, value: Credential) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = Some(value);
        Ok(())
    }

    async fn remove(&self, _: &CredentialKey) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

// Simulate mDNS record delivery without depending on multicast in test runners.
// Every control, timing, event, and origin connection below is real loopback I/O.
struct LocalAirPlay {
    backend: AirPlay,
    address: SocketAddr,
}

impl Backend for LocalAirPlay {
    fn protocol(&self) -> ProtocolId {
        self.backend.protocol()
    }

    fn uses_credentials(&self) -> bool {
        self.backend.uses_credentials()
    }

    fn discover(&self, _: Instant, sink: ScanSink) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let record = Record {
                identity: Some("021122334455".into()),
                name: "Synthetic TV".into(),
                address: self.address,
            };
            sink.push(record.candidate());
            Ok(())
        })
    }

    fn connect<'a>(
        &'a self,
        route: &'a Route,
        credential: Option<Credential>,
    ) -> BoxFuture<'a, Result<BackendConnect, Error>> {
        Box::pin(async move {
            assert_eq!(route.payload::<Record>().unwrap().address, self.address);
            self.backend.connect_address(self.address, credential).await
        })
    }
}

fn client(address: SocketAddr, timing: u16, store: TestStore) -> Cast {
    Cast::builder()
        .credential_store(store)
        .build_with(vec![Arc::new(LocalAirPlay {
            backend: AirPlay::new(NonZeroU16::new(timing)),
            address,
        })])
        .unwrap()
}

async fn discovered(cast: &Cast) -> Route {
    let routes = cast.discover(Duration::from_secs(1)).await.unwrap();
    assert!(routes.issues().is_empty());
    let [route] = routes.routes() else {
        panic!("expected one route")
    };
    assert_eq!(route.protocol().as_str(), "airplay");
    assert_eq!(
        route.availability(Feature::LoadVideoUrl),
        Availability::Unknown
    );
    route.clone()
}

async fn paired_connection(cast: &Cast, route: &Route) -> crate::Connection {
    let Connect::PinRequired(challenge) = cast.connect(route).await.unwrap() else {
        panic!("first connection must require approved PIN");
    };
    assert!(challenge.expires_at().is_some());
    let connection = challenge
        .submit(PinCode::parse(PIN).unwrap())
        .await
        .unwrap();
    assert_eq!(connection.peer_trust(), PeerTrust::VerifiedPairing);
    assert!(connection.control_addrs().is_some());
    connection
}

#[tokio::test]
async fn receiver_can_fetch_before_load_reply_and_cancel_joins_all_transports() {
    timeout(Duration::from_secs(10), async {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = origin.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = udp.local_addr().unwrap().port();
        drop(udp);
        let server = tokio::spawn(async move {
            let (public, id) = accessory.pair().await.unwrap();
            let (control, shared) = accessory.verify(&public, &id).await.unwrap();
            serve(
                control,
                &shared,
                upstream,
                port,
                ReceiverEnding::CloseWithLoadPending,
            )
            .await;
        });

        let cast = client(address, port, TestStore::default());
        let route = discovered(&cast).await;
        let connection = paired_connection(&cast, &route).await;
        let session = connection
            .load(Media::live(&format!("http://{upstream}/generated.m3u8")).unwrap())
            .unwrap();
        let mut updates = session.subscribe();
        assert!(matches!(
            updates.next().await.unwrap().playback(),
            Playback::Pending
        ));
        let (mut request, _) = origin.accept().await.unwrap();
        let mut verb = [0; 23];
        request.read_exact(&mut verb).await.unwrap();
        assert_eq!(&verb, b"GET /generated.m3u8 HTT");
        assert_eq!(session.stop().await.unwrap(), StopOutcome::TransportClosed);
        let last = updates.next().await.unwrap();
        assert_eq!(last.link(), Link::Disconnected);
        assert!(updates.next().await.is_none());
        server.await.unwrap();
        UdpSocket::bind(("127.0.0.1", port)).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn vod_reports_owned_playback_and_reuses_saved_identity_in_a_fresh_client() {
    timeout(Duration::from_secs(10), async {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = origin.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = udp.local_addr().unwrap().port();
        drop(udp);
        let (finish, gate) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (public, id) = accessory.pair().await.unwrap();
            let (control, shared) = accessory.verify(&public, &id).await.unwrap();
            serve(
                control,
                &shared,
                upstream,
                port,
                ReceiverEnding::PlayToEnd(gate),
            )
            .await;
            let (mut control, _) = accessory.verify(&public, &id).await.unwrap();
            assert!(matches!(control.receive().await, Err(Fault::Closed)));
        });

        let store = TestStore::default();
        let cast = client(address, port, store.clone());
        let route = discovered(&cast).await;
        let mut session = paired_connection(&cast, &route)
            .await
            .load(Media::on_demand(&format!("http://{upstream}/generated.m3u8")).unwrap())
            .unwrap();
        let mut updates = session.subscribe();
        let (_, _) = origin.accept().await.unwrap();
        loop {
            let snapshot = updates.next().await.unwrap();
            if matches!(snapshot.playback(), Playback::Playing) {
                assert_eq!(snapshot.duration(), Some(Duration::from_secs(21)));
                assert_eq!(snapshot.position(), Some(Duration::from_millis(5_500)));
                assert_eq!(
                    snapshot.availability(Feature::Pause),
                    Availability::Unsupported
                );
                break;
            }
        }
        assert_eq!(
            session.command(Command::Pause).await.unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        finish.send(()).unwrap();
        let last = updates.next().await.unwrap();
        assert!(matches!(last.playback(), Playback::NoLongerPresent));
        assert_eq!(last.link(), Link::Disconnected);
        assert!(updates.next().await.is_none());
        assert_eq!(session.stop().await.unwrap(), StopOutcome::AlreadyEnded);
        drop(cast);

        let cast = client(address, port, store);
        let route = discovered(&cast).await;
        let Connect::Ready(verified) = cast.connect(&route).await.unwrap() else {
            panic!("restarted client must verify the stored pairing");
        };
        verified.close().await;
        cast.forget(&route).await.unwrap();
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn feedback_runs_during_playback_and_explicit_retirement_closes_without_global_stop() {
    timeout(Duration::from_secs(10), async {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = origin.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = udp.local_addr().unwrap().port();
        drop(udp);
        let server = tokio::spawn(async move {
            let (public, id) = accessory.pair().await.unwrap();
            let (control, shared) = accessory.verify(&public, &id).await.unwrap();
            serve(
                control,
                &shared,
                upstream,
                port,
                ReceiverEnding::AwaitTransportClose,
            )
            .await;
        });
        let cast = client(address, port, TestStore::default());
        let route = discovered(&cast).await;
        let session = paired_connection(&cast, &route)
            .await
            .load(Media::live(&format!("http://{upstream}/generated.m3u8")).unwrap())
            .unwrap();
        let (_, _) = origin.accept().await.unwrap();
        let mut updates = session.subscribe();
        while !matches!(updates.next().await.unwrap().playback(), Playback::Playing) {}
        tokio::time::sleep(Duration::from_millis(2_100)).await;
        assert_eq!(session.stop().await.unwrap(), StopOutcome::TransportClosed);
        server.await.unwrap();
        UdpSocket::bind(("127.0.0.1", port)).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn declining_pending_approval_closes_before_sending_a_pairing_proof() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let cast = client(address, 0, TestStore::default());
    let route = discovered(&cast).await;
    let Connect::PinRequired(challenge) = cast.connect(&route).await.unwrap() else {
        panic!()
    };
    let mut socket = accessory.accept().await;
    let message = socket.receive().await.unwrap();
    assert_eq!(message.request_target, "POST /pair-pin-start");
    challenge.cancel().await;
    assert!(matches!(socket.receive().await, Err(Fault::Closed)));
}

#[test]
fn service_id_changes_and_duplicates_are_resolved_by_the_current_scan() {
    let mut records = Records::default();
    let address = "127.0.0.1:7777".parse().unwrap();
    let record = Record {
        identity: normalize_identity("02:12:34:56:78:9A"),
        name: "TV".into(),
        address,
    };
    let identity = "02123456789a";
    records.update("name".into(), Some(record.clone()), None);
    assert_eq!(records.unique(identity).unwrap().address, address);
    records.update("collision".into(), Some(record.clone()), None);
    assert!(records.unique(identity).is_none());
    records.update("collision".into(), None, None);
    assert!(records.unique(identity).is_some());
    records.update("name".into(), None, None);
    assert!(records.unique(identity).is_none());
    for invalid in [
        "F",
        "aabbccddeeff",
        "zz:00:00:00:00:01",
        "00:00:00:00:00:00",
    ] {
        assert!(normalize_identity(invalid).is_none());
    }
}

#[tokio::test]
async fn discovers_loopback_advertisements_and_revalidates_the_selected_identity() {
    use mdns_sd::{IfKind, ServiceInfo};

    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let (confirm, cancelled) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut control = accessory.accept().await;
        let request = control.receive().await.unwrap();
        assert_eq!(request.request_target, "POST /pair-pin-start");
        confirm.send(()).unwrap();
        assert!(matches!(control.receive().await, Err(Fault::Closed)));
    });

    let mut backend = AirPlay::new(None);
    backend.browse_daemon = Some(Box::new(move || {
        let daemon = ServiceDaemon::new().unwrap();
        daemon.disable_interface(IfKind::All).unwrap();
        daemon.enable_interface(IfKind::LoopbackV4).unwrap();
        for (name, properties) in [
            (
                "Patina fixture with identity",
                Some([("deviceid", "02:FA:12:CA:14:22")]),
            ),
            ("Patina fixture without identity", None),
        ] {
            let mut fixture = ServiceInfo::new(
                SERVICE,
                name,
                "patina-fixture.local.",
                address.ip(),
                address.port(),
                properties.map(|pairs| {
                    std::collections::HashMap::from(
                        pairs.map(|(key, value)| (key.to_owned(), value.to_owned())),
                    )
                }),
            )
            .unwrap();
            fixture.set_interfaces(vec![IfKind::LoopbackV4]);
            daemon.register(fixture).unwrap();
        }
        daemon
    }));
    let cast = Cast::builder().build_with(vec![Arc::new(backend)]).unwrap();
    let scan = cast.discover(Duration::from_secs(2)).await.unwrap();
    assert!(scan.issues().is_empty());
    let selectable = scan
        .routes()
        .iter()
        .find(|route| route.name() == "Patina fixture with identity")
        .unwrap();
    assert_eq!(
        selectable.availability(Feature::LoadVideoUrl),
        Availability::Unknown
    );
    let ephemeral = scan
        .routes()
        .iter()
        .find(|route| route.name() == "Patina fixture without identity")
        .unwrap();
    assert_eq!(
        ephemeral.availability(Feature::LoadVideoUrl),
        Availability::Unavailable
    );
    assert!(
        matches!(cast.connect(ephemeral).await, Err(error) if error.kind() == ErrorKind::Unsupported)
    );
    let Connect::PinRequired(pairing) = cast.connect(selectable).await.unwrap() else {
        panic!()
    };
    cancelled.await.unwrap();
    pairing.cancel().await;
    server.await.unwrap();
}

#[test]
fn advertisements_reject_unsafe_addresses_and_sanitize_names() {
    use mdns_sd::ServiceInfo;
    let seed =
        ServiceInfo::new(SERVICE, "Seed", "patina-fixture.local.", "::1", 7000, None).unwrap();
    let mut advertisement = seed.as_resolved_service();
    advertisement.fullname = format!("\u{7}Living room.{}", SERVICE.to_ascii_uppercase());
    let parsed = Record::from_service(&advertisement).unwrap();
    assert_eq!(parsed.name, "Living room");
    assert_eq!(parsed.address, "[::1]:7000".parse().unwrap());
    advertisement.fullname = format!("  .{SERVICE}");
    assert_eq!(
        Record::from_service(&advertisement).unwrap().name,
        "AirPlay"
    );
    for input in ["::", "ff02::fb", "fe80::1", "0.0.0.0", "224.0.0.251"] {
        advertisement.addresses = [ScopedIp::from(input.parse::<std::net::IpAddr>().unwrap())]
            .into_iter()
            .collect();
        assert!(Record::from_service(&advertisement).is_none(), "{input}");
    }
    advertisement.port = 0;
    assert!(Record::from_service(&advertisement).is_none());
    advertisement.fullname = "malformed".into();
    assert!(Record::from_service(&advertisement).is_none());
    advertisement.fullname = "fake._chromecast._tcp.local.".into();
    assert!(Record::from_service(&advertisement).is_none());
}

#[tokio::test]
async fn required_timing_port_conflict_is_reported_before_submission() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let timing = socket.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (public, id) = accessory.pair().await.unwrap();
        let (mut control, _) = accessory.verify(&public, &id).await.unwrap();
        assert!(matches!(control.receive().await, Err(Fault::Closed)));
    });
    let cast = client(address, timing.port(), TestStore::default());
    let route = discovered(&cast).await;
    let session = paired_connection(&cast, &route)
        .await
        .load(Media::live("http://127.0.0.1/generated.m3u8").unwrap())
        .unwrap();
    let mut updates = session.subscribe();
    assert!(matches!(
        updates.next().await.unwrap().playback(),
        Playback::Pending
    ));
    let last = updates.next().await.unwrap();
    let Playback::Failed(error) = last.playback() else {
        panic!("expected timing conflict")
    };
    assert_eq!(error.kind(), ErrorKind::Unavailable);
    assert_eq!(error.stage(), Stage::Load);
    assert_eq!(error.delivery(), Delivery::NotSent);
    assert!(
        matches!(error.diagnostic(), Some(Diagnostic::TimingListener { local_addr }) if *local_addr == timing)
    );
    assert_eq!(
        session.stop().await.unwrap(),
        StopOutcome::CancelledBeforeSend
    );
    assert!(updates.next().await.is_none());
    server.await.unwrap();
    drop(socket);
}

#[tokio::test]
async fn receiver_refusal_after_fetch_preserves_delivery_uncertainty() {
    timeout(Duration::from_secs(10), async {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = origin.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = udp.local_addr().unwrap().port();
        drop(udp);
        let server = tokio::spawn(async move {
            let (public, id) = accessory.pair().await.unwrap();
            let (control, shared) = accessory.verify(&public, &id).await.unwrap();
            serve(
                control,
                &shared,
                upstream,
                port,
                ReceiverEnding::RefuseLoadAfterRequest,
            )
            .await;
        });
        let cast = client(address, port, TestStore::default());
        let route = discovered(&cast).await;
        let session = paired_connection(&cast, &route)
            .await
            .load(Media::live(&format!("http://{upstream}/generated.m3u8")).unwrap())
            .unwrap();
        let mut updates = session.subscribe();
        drop(origin.accept().await.unwrap());
        while let Some(snapshot) = updates.next().await {
            if let Playback::Failed(error) = snapshot.playback() {
                assert_eq!(error.kind(), ErrorKind::ReceiverRefused);
                assert_eq!(error.stage(), Stage::Load);
                assert_eq!(error.delivery(), Delivery::MayHaveReachedReceiver);
                assert_eq!(session.stop().await.unwrap(), StopOutcome::TransportClosed);
                server.await.unwrap();
                return;
            }
        }
        panic!("receiver refusal must be delivered");
    })
    .await
    .unwrap();
}

#[test]
fn pairing_failures_are_typed_without_peer_details() {
    for (fault, kind) in [
        (AuthFault::InvalidCredential, ErrorKind::InvalidCredential),
        (AuthFault::IdentityMismatch, ErrorKind::IdentityMismatch),
        (AuthFault::InvalidPin, ErrorKind::InvalidPin),
        (AuthFault::ApprovalDenied, ErrorKind::ApprovalDenied),
        (AuthFault::TimedOut, ErrorKind::TimedOut),
        (AuthFault::Unavailable, ErrorKind::Unavailable),
        (AuthFault::Wire(Fault::Closed), ErrorKind::Unreachable),
        (AuthFault::Wire(Fault::Integrity), ErrorKind::Protocol),
        (
            AuthFault::Wire(Fault::Refused(400)),
            ErrorKind::ReceiverRefused,
        ),
    ] {
        let error = auth_error(fault.into(), Stage::Pair);
        assert_eq!(error.kind(), kind);
        assert_eq!(error.stage(), Stage::Pair);
        assert_eq!(error.delivery(), Delivery::NotSent);
        assert!(error.diagnostic().is_none());
    }
}

#[tokio::test]
async fn transport_loss_preserves_last_observation_and_retires_session() {
    for lose_control in [false, true] {
        exercise_transport_loss(lose_control).await;
    }
}

async fn exercise_transport_loss(lose_control: bool) {
    timeout(Duration::from_secs(10), async move {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = origin.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = udp.local_addr().unwrap().port();
        drop(udp);
        let (disconnect, gate) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (public, id) = accessory.pair().await.unwrap();
            let (control, shared) = accessory.verify(&public, &id).await.unwrap();
            serve(
                control,
                &shared,
                upstream,
                port,
                if lose_control {
                    ReceiverEnding::LoseControlTransport(gate)
                } else {
                    ReceiverEnding::LoseEventTransport(gate)
                },
            )
            .await;
        });

        let cast = client(address, port, TestStore::default());
        let route = discovered(&cast).await;
        let session = paired_connection(&cast, &route)
            .await
            .load(Media::live(&format!("http://{upstream}/generated.m3u8")).unwrap())
            .unwrap();
        let (_, _) = origin.accept().await.unwrap();
        let mut updates = session.subscribe();
        while !matches!(updates.next().await.unwrap().playback(), Playback::Playing) {}
        disconnect.send(()).unwrap();
        let last = updates.next().await.unwrap();
        assert_eq!(last.link(), Link::Disconnected);
        assert!(matches!(last.playback(), Playback::Playing));
        let error = last.link_error().unwrap();
        assert_eq!(error.kind(), ErrorKind::Unreachable);
        assert_eq!(error.stage(), Stage::Observe);
        assert!(updates.next().await.is_none());
        assert_eq!(
            session.stop().await.unwrap_err().kind(),
            ErrorKind::Unreachable
        );
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn setup_failures_distinguish_rtsp_correlation_and_missing_timing() {
    use super::video::tests::{BaseSetupFailure, fail_base_setup};

    // The two timeout cases deliberately exercise the actual receiver deadline,
    // with and without a completed UDP exchange.
    let mut cases = tokio::task::JoinSet::new();
    for (failure, expect_kind, expect_timing) in [
        (BaseSetupFailure::WrongSequence, ErrorKind::Protocol, false),
        (
            BaseSetupFailure::Timeout {
                timing_worked: false,
            },
            ErrorKind::TimedOut,
            true,
        ),
        (
            BaseSetupFailure::Timeout {
                timing_worked: true,
            },
            ErrorKind::TimedOut,
            false,
        ),
    ] {
        cases.spawn(async move {
            timeout(Duration::from_secs(15), async move {
                let accessory = Accessory::bind().await;
                let address = accessory.address();
                let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
                let timing = socket.local_addr().unwrap();
                drop(socket);
                let (credential, public, id) = accessory.saved_pairing();
                let server = tokio::spawn(async move {
                    let (control, _) = accessory.verify(&public, &id).await.unwrap();
                    fail_base_setup(control, timing.port(), failure).await;
                });
                let store = TestStore(Arc::new(Mutex::new(Some(Credential::from_bytes(credential)))));
                let cast = client(address, timing.port(), store);
                let route = discovered(&cast).await;
                let Connect::Ready(connection) = cast.connect(&route).await.unwrap() else { panic!() };
                let session = connection.load(Media::live("http://127.0.0.1/generated.m3u8").unwrap()).unwrap();
                let mut updates = session.subscribe();
                let mut outcome = None;
                while let Some(snapshot) = updates.next().await {
                    if let Playback::Failed(error) = snapshot.playback() {
                        outcome = Some(error.clone());
                    }
                }
                let error = outcome.expect("setup failed before queue dispatch");
                assert_eq!(error.kind(), expect_kind);
                assert_eq!(error.stage(), Stage::Load);
                assert_eq!(error.delivery(), Delivery::NotSent);
                assert_eq!(
                    matches!(error.diagnostic(), Some(Diagnostic::TimingListener { local_addr }) if *local_addr == timing),
                    expect_timing,
                );
                assert_eq!(session.stop().await.unwrap(), StopOutcome::CancelledBeforeSend);
                server.await.unwrap();
            }).await.unwrap();
        });
    }
    while let Some(case) = cases.join_next().await {
        case.unwrap();
    }
}

#[tokio::test]
async fn corrupt_saved_credential_does_not_fall_back_to_an_unrequested_pairing() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let storage = Some(Credential::from_bytes(vec![1; 16]));
    let cast = client(address, 0, TestStore(Arc::new(Mutex::new(storage))));
    let route = discovered(&cast).await;
    let error = cast
        .connect(&route)
        .await
        .err()
        .expect("invalid stored material");
    assert_eq!(error.kind(), ErrorKind::InvalidCredential);
    assert_eq!(error.delivery(), Delivery::NotSent);
    assert!(
        timeout(Duration::from_millis(30), accessory.accept())
            .await
            .is_err()
    );
}
