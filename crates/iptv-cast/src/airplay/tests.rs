use super::*;
use crate::{
    Cast, Connect, CredentialKey, CredentialStore, Link, StoreError,
    airplay::{
        hap::tests::{Accessory, PIN, ProofReply, assert_transient_start, reject_transient},
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
    pin_hint: PinHint,
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
                addresses: vec![self.address],
                pin_hint: self.pin_hint,
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
            let record = route.payload::<Record>().unwrap();
            assert_eq!(record.addresses, [self.address]);
            self.backend
                .connect_addresses(&record.addresses, record.pin_hint, credential)
                .await
        })
    }
}

fn client(address: SocketAddr, timing: u16, store: TestStore) -> Cast {
    client_with_hint(address, timing, store, PinHint::Unspecified)
}

fn client_with_hint(address: SocketAddr, timing: u16, store: TestStore, pin_hint: PinHint) -> Cast {
    Cast::builder()
        .credential_store(store)
        .build_with(vec![Arc::new(LocalAirPlay {
            backend: AirPlay::new(NonZeroU16::new(timing)),
            address,
            pin_hint,
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
        panic!("the receiver requires an approved PIN");
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
async fn guest_playback_encrypts_control_and_events_without_saving_a_pairing() {
    for pin_hint in [PinHint::Unspecified, PinHint::Suppressed] {
        timeout(Duration::from_secs(10), async {
            let accessory = Accessory::bind().await;
            let address = accessory.address();
            let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream = origin.local_addr().unwrap();
            let (finish, gate) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (control, shared) = accessory.transient(ProofReply::Valid).await;
                serve(
                    control,
                    &shared,
                    upstream,
                    0,
                    ReceiverEnding::PlayToEnd(gate),
                )
                .await;
            });

            let store = TestStore::default();
            let cast = client_with_hint(address, 0, store.clone(), pin_hint);
            let route = discovered(&cast).await;
            let Connect::Ready(connection) = cast.connect(&route).await.unwrap() else {
                panic!("open receiver must accept guest playback without a PIN");
            };
            assert_eq!(connection.peer_trust(), PeerTrust::TransientPairing);
            let session = connection
                .load(Media::on_demand(&format!("http://{upstream}/generated.m3u8")).unwrap())
                .unwrap();
            let (mut request, _) = origin.accept().await.unwrap();
            let mut verb = [0; 23];
            request.read_exact(&mut verb).await.unwrap();
            assert_eq!(&verb, b"GET /generated.m3u8 HTT");

            let mut updates = session.subscribe();
            loop {
                let snapshot = updates.next().await.unwrap();
                if matches!(snapshot.playback(), Playback::Playing) {
                    assert_eq!(snapshot.duration(), Some(Duration::from_secs(21)));
                    break;
                }
            }
            finish.send(()).unwrap();
            let terminal = updates.next().await.unwrap();
            assert!(matches!(terminal.playback(), Playback::NoLongerPresent));
            assert_eq!(session.stop().await.unwrap(), StopOutcome::AlreadyEnded);
            server.await.unwrap();
            assert!(store.0.lock().unwrap().is_none());
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn rejected_or_invalid_guest_proof_does_not_trigger_pin_approval() {
    for (proof, expected) in [
        (ProofReply::Invalid, ErrorKind::Protocol),
        (ProofReply::Refused, ErrorKind::ReceiverRefused),
    ] {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let server = tokio::spawn(async move {
            let (mut wire, _) = accessory.transient(proof).await;
            assert!(matches!(wire.receive().await, Err(Fault::Closed)));
            assert!(
                timeout(Duration::from_millis(30), accessory.accept())
                    .await
                    .is_err()
            );
        });
        let cast = client(address, 0, TestStore::default());
        let route = discovered(&cast).await;
        let error = cast
            .connect(&route)
            .await
            .err()
            .expect("guest proof failed");
        assert_eq!(error.kind(), expected);
        assert_eq!(error.stage(), Stage::Connect);
        assert_eq!(error.delivery(), Delivery::Acknowledged);
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn password_home_membership_and_ambiguous_auth_failures_do_not_show_pin_prompt() {
    for (status, body, expected) in [
        (401, &[][..], ErrorKind::ReceiverRefused),
        (403, &[][..], ErrorKind::ReceiverRefused),
        (200, &[6, 1, 2, 7, 1, 2][..], ErrorKind::ReceiverRefused),
        (200, &[6, 1, 2, 7, 1, 3][..], ErrorKind::Unavailable),
    ] {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let server = tokio::spawn(async move {
            let mut wire = accessory.accept().await;
            let first = wire.receive().await.unwrap();
            assert_transient_start(&first);
            wire.respond(&first, status, body).await;
            assert!(matches!(wire.receive().await, Err(Fault::Closed)));
            assert!(
                timeout(Duration::from_millis(30), accessory.accept())
                    .await
                    .is_err()
            );
        });
        let cast = client(address, 0, TestStore::default());
        let route = discovered(&cast).await;
        let error = cast
            .connect(&route)
            .await
            .err()
            .expect("receiver rejected guest setup");
        assert_eq!(error.kind(), expected);
        assert_eq!(error.stage(), Stage::Connect);
        assert_eq!(error.delivery(), Delivery::Acknowledged);
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

fn advertised_pin_hint(properties: &[(&str, &str)]) -> PinHint {
    let service = mdns_sd::ServiceInfo::new(
        SERVICE,
        "Policy fixture",
        "patina-policy.local.",
        "127.0.0.1",
        7000,
        properties,
    )
    .unwrap();
    Record::from_service(&service.as_resolved_service())
        .unwrap()
        .pin_hint
}

#[test]
fn access_hints_accept_known_values_and_resolve_conflicts_conservatively() {
    assert_eq!(advertised_pin_hint(&[]), PinHint::Unspecified);
    for properties in [
        &[("act", "2")][..],
        &[("acl", "1")],
        &[("pw", "TrUe")],
        &[("sf", "0x80")],
        &[("flags", "0X84")],
        &[("sf", "0x8"), ("flags", "0x88")],
        &[("pw", "false"), ("flags", "80")],
        &[("act", "2"), ("flags", "0x8")],
    ] {
        assert_eq!(
            advertised_pin_hint(properties),
            PinHint::Suppressed,
            "{properties:?}"
        );
    }
    for properties in [
        &[("sf", "0x8")][..],
        &[("flags", "08")],
        &[("sf", "0x8"), ("flags", "0xC")],
        &[("sf", "0x8oops"), ("flags", "0x8")],
    ] {
        assert_eq!(
            advertised_pin_hint(properties),
            PinHint::Required,
            "{properties:?}"
        );
    }
    for properties in [
        &[("act", "2x"), ("acl", "-1"), ("pw", "1")][..],
        &[("sf", "+80"), ("flags", "0x")],
        &[("sf", "0x200"), ("flags", "0x200")],
        &[("sf", "0x8"), ("flags", "0x0")],
        &[("sf", "0x100000080"), ("flags", "0x80,0x0")],
    ] {
        assert_eq!(
            advertised_pin_hint(properties),
            PinHint::Unspecified,
            "{properties:?}"
        );
    }
}

#[tokio::test]
async fn advertised_non_pin_access_still_tries_guest_then_preserves_a_470_refusal() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let server = tokio::spawn(async move {
        accessory.deny_transient().await;
        assert!(
            timeout(Duration::from_millis(30), accessory.accept())
                .await
                .is_err()
        );
    });
    let hint = advertised_pin_hint(&[("acl", "1"), ("flags", "0x8")]);
    let cast = client_with_hint(address, 0, TestStore::default(), hint);
    let route = discovered(&cast).await;
    let error = cast
        .connect(&route)
        .await
        .err()
        .expect("guest access was refused");
    assert_eq!(error.kind(), ErrorKind::ReceiverRefused);
    assert_eq!(error.stage(), Stage::Connect);
    assert_eq!(error.delivery(), Delivery::Acknowledged);
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn advertised_pin_requirement_requests_approval_without_trying_guest() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let server = tokio::spawn(async move {
        let (public, id) = accessory.pair().await.unwrap();
        let (mut wire, _) = accessory.verify(&public, &id).await.unwrap();
        assert!(matches!(wire.receive().await, Err(Fault::Closed)));
    });
    let hint = advertised_pin_hint(&[("flags", "0x8")]);
    let store = TestStore::default();
    let cast = client_with_hint(address, 0, store.clone(), hint);
    let route = discovered(&cast).await;
    paired_connection(&cast, &route).await.close().await;
    assert!(store.0.lock().unwrap().is_some());
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
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
            let (public, id) = accessory.pair_for_cast().await.unwrap();
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
            let (public, id) = accessory.pair_for_cast().await.unwrap();
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
        let (reply, replied) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (public, id) = accessory.pair_for_cast().await.unwrap();
            let (control, shared) = accessory.verify(&public, &id).await.unwrap();
            serve(
                control,
                &shared,
                upstream,
                port,
                ReceiverEnding::AwaitTransportClose(reply),
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
        replied.await.unwrap();
        assert_eq!(session.stop().await.unwrap(), StopOutcome::TransportClosed);
        server.await.unwrap();
        UdpSocket::bind(("127.0.0.1", port)).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn playback_and_stop_reported_before_the_insert_reply_are_preserved() {
    timeout(Duration::from_secs(10), async {
        let accessory = Accessory::bind().await;
        let address = accessory.address();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = origin.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (public, id) = accessory.pair_for_cast().await.unwrap();
            let (control, shared) = accessory.verify(&public, &id).await.unwrap();
            serve(
                control,
                &shared,
                upstream,
                0,
                ReceiverEnding::EndBeforeLoadReply,
            )
            .await;
        });
        let cast = client(address, 0, TestStore::default());
        let route = discovered(&cast).await;
        let session = paired_connection(&cast, &route)
            .await
            .load(Media::on_demand(&format!("http://{upstream}/generated.m3u8")).unwrap())
            .unwrap();
        let mut updates = session.subscribe();
        drop(origin.accept().await.unwrap());
        let mut last = updates.next().await.unwrap();
        while let Some(snapshot) = updates.next().await {
            last = snapshot;
        }
        assert!(matches!(last.playback(), Playback::NoLongerPresent));
        assert_eq!(last.link(), Link::Disconnected);
        assert_eq!(session.stop().await.unwrap(), StopOutcome::AlreadyEnded);
        server.await.unwrap();
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
    let server = tokio::spawn(async move {
        accessory.deny_transient().await;
        let mut socket = accessory.accept().await;
        let message = socket.receive().await.unwrap();
        assert_eq!(message.request_target, "POST /pair-pin-start");
        assert!(matches!(socket.receive().await, Err(Fault::Closed)));
    });
    let Connect::PinRequired(challenge) = cast.connect(&route).await.unwrap() else {
        panic!()
    };
    challenge.cancel().await;
    server.await.unwrap();
}

#[test]
fn service_id_changes_and_duplicates_are_resolved_by_the_current_scan() {
    let mut records = Records::default();
    let address = "127.0.0.1:7777".parse().unwrap();
    let record = Record {
        identity: normalize_identity("02:12:34:56:78:9A"),
        name: "TV".into(),
        addresses: vec![address],
        pin_hint: PinHint::Unspecified,
    };
    let identity = "02123456789a";
    records.update("name".into(), Some(record.clone()), None);
    assert_eq!(
        records.resolve_unique(identity).unwrap().addresses,
        [address]
    );
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
    use std::sync::atomic::{AtomicBool, Ordering};

    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let (confirm, cancelled) = oneshot::channel();
    let server = tokio::spawn(async move {
        accessory.deny_transient().await;
        let mut control = accessory.accept().await;
        let request = control.receive().await.unwrap();
        assert_eq!(request.request_target, "POST /pair-pin-start");
        confirm.send(()).unwrap();
        assert!(matches!(control.receive().await, Err(Fault::Closed)));
    });

    let mut backend = AirPlay::new(None);
    let initial_scan = AtomicBool::new(true);
    backend.browse_daemon = Some(Box::new(move || {
        let act = if initial_scan.swap(false, Ordering::Relaxed) {
            "2"
        } else {
            "0"
        };
        let daemon = ServiceDaemon::new().unwrap();
        daemon.disable_interface(IfKind::All).unwrap();
        daemon.enable_interface(IfKind::LoopbackV4).unwrap();
        for (name, properties) in [
            (
                "Patina fixture with identity",
                &[("deviceid", "02:FA:12:CA:14:22"), ("act", act)][..],
            ),
            ("Patina fixture without identity", &[("act", "2")]),
        ] {
            let mut fixture = ServiceInfo::new(
                SERVICE,
                name,
                "patina-fixture.local.",
                address.ip(),
                address.port(),
                properties,
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
    assert_eq!(
        selectable.payload::<Record>().unwrap().pin_hint,
        PinHint::Suppressed
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
    assert_eq!(parsed.addresses, ["[::1]:7000".parse().unwrap()]);
    advertisement.addresses.insert(ScopedIp::from(
        "127.0.0.1".parse::<std::net::IpAddr>().unwrap(),
    ));
    assert_eq!(
        Record::from_service(&advertisement).unwrap().addresses,
        [
            "127.0.0.1:7000".parse().unwrap(),
            "[::1]:7000".parse().unwrap()
        ]
    );
    advertisement.fullname = format!("  .{SERVICE}");
    assert_eq!(
        Record::from_service(&advertisement).unwrap().name,
        "AirPlay"
    );
    for input in ["::", "ff02::fb", "fe80::1", "0.0.0.0", "224.0.0.251"] {
        let mut bad_address = advertisement.clone();
        bad_address.addresses = [ScopedIp::from(input.parse::<std::net::IpAddr>().unwrap())]
            .into_iter()
            .collect();
        assert!(Record::from_service(&bad_address).is_none(), "{input}");
    }
    let mut bad_port = advertisement.clone();
    bad_port.port = 0;
    assert!(Record::from_service(&bad_port).is_none());
    for fullname in ["malformed", "fake._chromecast._tcp.local."] {
        let mut bad_name = advertisement.clone();
        bad_name.fullname = fullname.into();
        assert!(Record::from_service(&bad_name).is_none(), "{fullname}");
    }
}

#[tokio::test]
async fn required_timing_port_conflict_is_reported_before_submission() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let timing = socket.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (public, id) = accessory.pair_for_cast().await.unwrap();
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
            let (public, id) = accessory.pair_for_cast().await.unwrap();
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
            assert!(!matches!(
                snapshot.playback(),
                Playback::Playing | Playback::NoLongerPresent
            ));
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
            let (public, id) = accessory.pair_for_cast().await.unwrap();
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
                let (credential, public, id) = accessory.saved_pairing();
                let server = tokio::spawn(async move {
                    let (control, _) = accessory.verify(&public, &id).await.unwrap();
                    fail_base_setup(control, failure).await
                });
                let store = TestStore(Arc::new(Mutex::new(Some(Credential::from_bytes(credential)))));
                let cast = client(address, 0, store);
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
                let timing = server.await.unwrap();
                assert_eq!(
                    matches!(error.diagnostic(), Some(Diagnostic::TimingListener { local_addr }) if *local_addr == timing),
                    expect_timing,
                );
                assert_eq!(session.stop().await.unwrap(), StopOutcome::CancelledBeforeSend);
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

#[test]
fn revalidation_tracks_only_the_selected_identity_and_rejects_saturation() {
    use mdns_sd::ServiceInfo;
    let identified = ServiceInfo::new(
        SERVICE,
        "Selected",
        "patina-fixture.local.",
        "127.0.0.1",
        7000,
        std::collections::HashMap::from([("deviceid".into(), "02:12:34:56:78:9A".into())]),
    )
    .unwrap()
    .as_resolved_service();
    let unrelated = ServiceInfo::new(
        SERVICE,
        "Other",
        "patina-fixture.local.",
        "127.0.0.1",
        7000,
        None,
    )
    .unwrap()
    .as_resolved_service();

    let identity = "02123456789a";
    let mut records = Records::default();
    let resolve = |records: &mut Records, record: &ResolvedService, index: usize| {
        let mut instance = record.clone();
        instance.fullname = format!("Test {index}.{SERVICE}");
        records.record_event(
            ServiceEvent::ServiceResolved(Box::new(instance)),
            Some(identity),
            None,
        );
    };
    resolve(&mut records, &identified, 0);
    for index in 1..=MAX_ROUTES {
        resolve(&mut records, &unrelated, index);
    }
    assert!(records.resolve_unique(identity).is_ok());

    for index in 1..=MAX_ROUTES {
        resolve(&mut records, &identified, index);
    }
    for index in 1..MAX_ROUTES {
        records.record_event(
            ServiceEvent::ServiceRemoved(SERVICE.into(), format!("Test {index}.{SERVICE}")),
            Some(identity),
            None,
        );
    }
    // The overflowed duplicate was never retained. The one remaining entry
    // alone cannot establish that its service owns the selected identity.
    assert!(records.unique(identity).is_some());
    assert!(matches!(
        records.resolve_unique(identity),
        Err(ErrorKind::Unavailable)
    ));
}

#[tokio::test]
async fn alternate_service_addresses_fail_over_before_any_pairing_request() {
    use tokio::net::TcpSocket;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unused = TcpSocket::new_v4().unwrap();
    unused.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let refused = unused.local_addr().unwrap();
    let addresses = [refused, listener.local_addr().unwrap()];

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        reject_transient(&mut wire::Wire::new(socket)).await;
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = wire::Wire::new(socket);
        let request = socket.receive().await.unwrap();
        assert_eq!(request.request_target, "POST /pair-pin-start");
        assert!(matches!(socket.receive().await, Err(Fault::Closed)));
    });
    let backend = AirPlay::new(None);
    let BackendConnect::PinRequired(mut pairing) = backend
        .connect_addresses(&addresses, PinHint::Unspecified, None)
        .await
        .unwrap()
    else {
        panic!("the reachable address must start pairing");
    };
    pairing.cancel().await;
    server.await.unwrap();
    let error = backend
        .connect_addresses(&[refused], PinHint::Unspecified, None)
        .await
        .err()
        .expect("the reserved socket is not accepting connections");
    assert!(matches!(
        error.kind(),
        ErrorKind::Unreachable | ErrorKind::TimedOut
    ));
    assert_eq!(error.stage(), Stage::Connect);
    assert_eq!(error.delivery(), Delivery::NotSent);
}

#[tokio::test]
async fn verification_refusal_does_not_send_a_trust_message_to_another_address() {
    async fn observed_verify(accessory: Accessory) -> bool {
        let Ok(mut wire) = timeout(Duration::from_millis(350), accessory.accept()).await else {
            return false;
        };
        match wire.receive().await {
            Ok(message) => {
                assert_eq!(message.request_target, "POST /pair-verify");
                wire.respond(&message, 403, &[]).await;
                true
            }
            Err(Fault::Closed) => false,
            Err(other) => panic!("unexpected peer result: {other:?}"),
        }
    }

    let first = Accessory::bind().await;
    let second = Accessory::bind().await;
    let (keys, _, _) = first.saved_pairing();
    let addresses = [first.address(), second.address()];
    let backend = AirPlay::new(None);
    let (result, first, second) = tokio::join!(
        backend.connect_addresses(
            &addresses,
            PinHint::Required,
            Some(Credential::from_bytes(keys))
        ),
        observed_verify(first),
        observed_verify(second),
    );
    let error = result
        .err()
        .expect("the selected peer refused verification");
    assert_eq!(error.kind(), ErrorKind::Protocol);
    assert_eq!(error.delivery(), Delivery::Acknowledged);
    assert_ne!(
        first, second,
        "exactly one peer should receive a verification request"
    );
}

#[cfg(all(target_os = "linux", feature = "avahi"))]
#[test]
fn avahi_records_merge_dual_stack_addresses_filter_unsafe_entries_and_publish_unique_routes() {
    use crate::discovery::avahi::ResolvedRecord;
    use std::net::IpAddr;

    let entry = |interface, name: &str, address: &str, port, txt: &[&str]| ResolvedRecord {
        interface,
        name: name.into(),
        address: address.parse::<IpAddr>().unwrap(),
        port,
        txt: txt.iter().map(|s| s.as_bytes().to_vec()).collect(),
    };
    let resolved = vec![
        // Ignored: port 0, unspecified, multicast, link-local without scope.
        entry(
            1,
            "Zero Port",
            "192.0.2.1",
            0,
            &["deviceid=02:00:00:00:00:01"],
        ),
        entry(
            1,
            "Unspec",
            "0.0.0.0",
            7000,
            &["deviceid=02:00:00:00:00:02"],
        ),
        entry(
            1,
            "Multicast",
            "224.0.0.1",
            7000,
            &["deviceid=02:00:00:00:00:03"],
        ),
        entry(
            0,
            "Unscoped V6",
            "fe80::1",
            7000,
            &["deviceid=02:00:00:00:00:04"],
        ),
        // Dual-stack receiver: IPv4 + scoped link-local IPv6 + global IPv6 merged into one record.
        entry(2, "Living\x07Room", "fe80::10", 7000, &["sf=0x8"]),
        entry(
            2,
            "living\x07room",
            "192.0.2.10",
            7000,
            &["deviceid=02:AA:BB:CC:DD:EE", "sf=0x8"],
        ),
        entry(
            2,
            "Living\x07Room",
            "2001:db8::10",
            7000,
            &["deviceid=02:AA:BB:CC:DD:EE", "sf=0x8"],
        ),
        // Duplicate identity across two instances must not be published.
        entry(
            1,
            "Dup A",
            "192.0.2.20",
            7000,
            &["deviceid=02:11:22:33:44:55"],
        ),
        entry(
            1,
            "Dup B",
            "192.0.2.21",
            7000,
            &["deviceid=02:11:22:33:44:55"],
        ),
        // Identity-less receiver with blank name falls back to "AirPlay".
        entry(1, "   ", "192.0.2.30", 7000, &[]),
    ];

    let records = Records::from_avahi(resolved.clone(), None);
    let living = records.resolve_unique("02aabbccddee").unwrap();
    assert_eq!(living.name, "LivingRoom");
    assert_eq!(living.pin_hint, PinHint::Required);
    assert_eq!(living.addresses.len(), 3);
    assert!(records.resolve_unique("021122334455").is_err());

    let sink = ScanSink::new();
    records.publish(&sink);
    let published = sink.finish();
    assert_eq!(published.len(), 2);

    // Identity-filtered scan retains only the requested deviceid.
    let filtered = Records::from_avahi(resolved, Some("02aabbccddee"));
    assert_eq!(filtered.instances.len(), 1);
    assert!(filtered.resolve_unique("02aabbccddee").is_ok());
}
