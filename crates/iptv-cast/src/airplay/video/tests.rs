use super::*;
use crate::airplay::hap::tests::Accessory;
use futures_util::FutureExt as _;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

pub(in crate::airplay) enum ReceiverEnding {
    CloseWithLoadPending,
    RefuseLoadAfterRequest,
    AwaitTransportClose(oneshot::Sender<()>),
    EndBeforeLoadReply,
    LoseEventTransport(oneshot::Receiver<()>),
    LoseControlTransport(oneshot::Receiver<()>),
    PlayToEnd(oneshot::Receiver<()>),
}

pub(in crate::airplay) async fn serve(
    mut control: Wire,
    shared: &[u8; 32],
    origin: SocketAddr,
    expected_timing_port: u16,
    ending: ReceiverEnding,
) {
    let event_socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = control.receive().await.unwrap();
    assert!(base.request_target.starts_with("SETUP rtsp://127.0.0.1/"));
    let setup = unpack(&base.body).unwrap();
    assert_eq!(
        find(&setup, "timingProtocol").unwrap().as_string(),
        Some("NTP")
    );
    let timing_port = find(&setup, "timingPort")
        .unwrap()
        .as_unsigned_integer()
        .unwrap() as u16;
    assert_ne!(timing_port, 0);
    if expected_timing_port != 0 {
        assert_eq!(timing_port, expected_timing_port);
    }
    let client_timing = SocketAddr::from(([127, 0, 0, 1], timing_port));
    let timing = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut request = [0; 32];
    request[..4].copy_from_slice(&[0x80, 0xd2, 0, 7]);
    request[24..].copy_from_slice(&[0xa5; 8]);
    timing.send_to(&request, client_timing).await.unwrap();
    let mut reply = [0; 32];
    let (received, address) = timeout(Duration::from_secs(2), timing.recv_from(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(address, client_timing);
    assert_eq!(received, reply.len());
    assert_eq!(&reply[..4], &[0x80, 0xd3, 0, 7]);
    assert_eq!(&reply[8..16], &[0xa5; 8]);

    let answer = pack(&dictionary([(
        "eventPort",
        u64::from(event_socket.local_addr().unwrap().port()).into(),
    )]))
    .unwrap();
    control.respond(&base, 200, &answer).await;
    let (events, _) = event_socket.accept().await.unwrap();
    let mut events = Wire::new(events);
    Accessory::encrypt_events(shared, &mut events);

    let info = control.receive().await.unwrap();
    assert_eq!(info.request_target, "GET /info");
    control.respond(&info, 404, &[]).await;
    let record = control.receive().await.unwrap();
    assert!(record.request_target.starts_with("RECORD rtsp://"));
    control.respond(&record, 200, &[]).await;
    let setup = control.receive().await.unwrap();
    assert!(setup.request_target.starts_with("SETUP rtsp://"));
    let streams = unpack(&setup.body).unwrap();
    let stream = &find(&streams, "streams").unwrap().as_array().unwrap()[0];
    assert_eq!(
        find(stream, "type").unwrap().as_unsigned_integer(),
        Some(130)
    );
    let answer = pack(&dictionary([(
        "streams",
        Value::Array(vec![dictionary([("streamID", 17u64.into())])]),
    )]))
    .unwrap();
    control.respond(&setup, 200, &answer).await;

    let insert = control.receive().await.unwrap();
    assert_eq!(insert.request_target, "POST /command");
    let packet = unpack(&insert.body).unwrap();
    let payload = unpack(
        find(find(&packet, "params").unwrap(), "data")
            .unwrap()
            .as_data()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        find(&payload, "type").unwrap().as_string(),
        Some("insertPlayQueueItem")
    );
    let item = find(&payload, "item").unwrap();
    let media_url = find(item, "Content-Location").unwrap().as_string().unwrap();
    let item_uuid = find(item, "uuid").unwrap().as_string().unwrap();
    assert_eq!(media_url, format!("http://{origin}/generated.m3u8"));
    send_unaddressed_state(&mut events, "stopped").await;
    let mut upstream = TcpStream::connect(origin).await.unwrap();
    upstream
        .write_all(b"GET /generated.m3u8 HTTP/1.1\r\nHost: synthetic-origin\r\n\r\n")
        .await
        .unwrap();

    if matches!(
        ending,
        ReceiverEnding::EndBeforeLoadReply | ReceiverEnding::RefuseLoadAfterRequest
    ) {
        send_state(&mut events, "unowned-item", "ended").await;
        send_unaddressed_state(&mut events, "playing").await;
        send_unaddressed_state(&mut events, "stopped").await;
    }
    if matches!(ending, ReceiverEnding::CloseWithLoadPending) {
        assert!(matches!(control.receive().await, Err(Fault::Closed)));
        assert!(matches!(events.receive().await, Err(Fault::Closed)));
        return;
    }
    if matches!(ending, ReceiverEnding::RefuseLoadAfterRequest) {
        control.respond(&insert, 403, &[]).await;
        assert!(matches!(control.receive().await, Err(Fault::Closed)));
        assert!(matches!(events.receive().await, Err(Fault::Closed)));
        return;
    }
    control.respond(&insert, 200, &[]).await;
    for (kind, property) in [
        ("setProperty", Some("isInterestedInDateRange")),
        ("setProperty", Some("actionAtItemEnd")),
        ("setRate", None),
    ] {
        let command = control.receive().await.unwrap();
        let packet = unpack(&command.body).unwrap();
        let payload = unpack(
            find(find(&packet, "params").unwrap(), "data")
                .unwrap()
                .as_data()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(find(&payload, "type").unwrap().as_string(), Some(kind));
        assert_eq!(
            find(&payload, "property").and_then(Value::as_string),
            property
        );
        control.respond(&command, 200, &[]).await;
    }

    if !matches!(ending, ReceiverEnding::EndBeforeLoadReply) {
        send_state(&mut events, "unowned-item", "playing").await;
        send_state(&mut events, item_uuid, "playing").await;
    }
    match ending {
        ReceiverEnding::CloseWithLoadPending | ReceiverEnding::RefuseLoadAfterRequest => {
            unreachable!()
        }
        ReceiverEnding::AwaitTransportClose(replied) => {
            let feedback = control.receive().await.unwrap();
            assert_eq!(feedback.request_target, "POST /feedback");
            control.respond(&feedback, 200, &[]).await;
            replied.send(()).unwrap();
        }
        ReceiverEnding::EndBeforeLoadReply => {}
        ReceiverEnding::PlayToEnd(gate) => {
            gate.await.unwrap();
            send_state(&mut events, item_uuid, "stopped").await;
        }
        ReceiverEnding::LoseEventTransport(gate) => {
            gate.await.unwrap();
            drop(events);
            assert!(matches!(control.receive().await, Err(Fault::Closed)));
            return;
        }
        ReceiverEnding::LoseControlTransport(gate) => {
            gate.await.unwrap();
            let feedback = control.receive().await.unwrap();
            assert_eq!(feedback.request_target, "POST /feedback");
            drop(control);
            assert!(matches!(events.receive().await, Err(Fault::Closed)));
            return;
        }
    }
    assert!(matches!(control.receive().await, Err(Fault::Closed)));
    assert!(matches!(events.receive().await, Err(Fault::Closed)));
}

fn state_event(item: &str, state: &str) -> Vec<u8> {
    command_body(dictionary([
        ("type", "playbackState".into()),
        (
            "params",
            dictionary([
                ("item", dictionary([("uuid", item.into())])),
                ("duration", 21u64.into()),
                ("position", 5.5f64.into()),
                ("playbackState", state.into()),
            ]),
        ),
    ]))
    .unwrap()
}

async fn send_state(socket: &mut Wire, item: &str, state: &str) {
    let reply = socket
        .request(
            Version::Http,
            "POST",
            "/event",
            &[("CSeq", "23")],
            &state_event(item, state),
        )
        .await
        .unwrap();
    assert_eq!(reply.sequence, Some(23));
    reply.require_ok().unwrap();
}

async fn send_unaddressed_state(socket: &mut Wire, state: &str) {
    socket
        .request(
            Version::Http,
            "POST",
            "/event",
            &[],
            &unaddressed_state(state),
        )
        .await
        .unwrap()
        .require_ok()
        .unwrap();
}

#[test]
fn media_observations_require_owned_state_and_finite_times() {
    assert!(parse_event(b"", "item").is_none());
    for (wire, state) in [
        ("buffering", MediaState::Buffering),
        ("paused", MediaState::Paused),
        ("playing", MediaState::Playing),
        ("ended", MediaState::Ended),
        ("stopped", MediaState::Absent),
    ] {
        let event = state_event("item", wire);
        assert!(parse_event(&event, "other").is_none());
        let (evidence, explicitly_owned) = parse_event(&event, "item").unwrap();
        assert!(explicitly_owned);
        assert_eq!(evidence.state, state);
        assert_eq!(evidence.duration, Some(Duration::from_secs(21)));
        assert_eq!(evidence.position, Some(Duration::from_millis(5_500)));
    }
    assert!(parse_event(&state_event("item", "unrecognized"), "item").is_none());
    let mut values = Dictionary::new();
    values.insert("position".into(), (-1.5).into());
    assert!(time_property(&values, "position").is_none());
    assert!(time_property(&values, "duration").is_none());
}

#[test]
fn terminal_evidence_cannot_be_overwritten_by_a_late_event_or_socket_loss() {
    for (first, second) in [
        (MediaState::Ended, MediaState::Playing),
        (MediaState::Absent, MediaState::Paused),
    ] {
        let observations = Observations::default();
        let observation = |state| Evidence {
            state,
            position: None,
            duration: None,
        };
        observations.accept();
        observations.media(observation(first), true);
        observations.media(observation(second), true);
        observations.fault(VideoFault::Wire(Fault::Closed));
        assert_eq!(
            observations.next().now_or_never(),
            Some(Ok(observation(first)))
        );
    }
}

#[test]
fn an_unread_playback_observation_is_delivered_before_transport_failure() {
    let observations = Observations::default();
    let (playing, explicitly_owned) = parse_event(&state_event("item", "playing"), "item").unwrap();
    let closed = VideoFault::Wire(Fault::Closed);
    observations.media(playing, explicitly_owned);
    observations.fault(closed);
    observations.accept();
    assert_eq!(observations.next().now_or_never(), Some(Ok(playing)));
    assert_eq!(observations.next().now_or_never(), Some(Err(closed)));

    let empty = Observations::default();
    empty.accept();
    empty.fault(closed);
    assert_eq!(empty.next().now_or_never(), Some(Err(closed)));
}

#[tokio::test]
async fn a_valid_event_is_retained_when_its_acknowledgement_cannot_be_sent() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (connection, receiver) = tokio::join!(TcpStream::connect(address), listener.accept());
    let mut connection = connection.unwrap();
    connection.shutdown().await.unwrap();
    let mut events = Wire::new(connection);
    let mut receiver = Wire::new(receiver.unwrap().0);
    events.enable_encryption(&[0x11; 32], &[0x22; 32]);
    receiver.enable_encryption(&[0x22; 32], &[0x11; 32]);
    let observations = Observations::default();
    observations.accept();
    let body = state_event("item", "playing");
    let (observed, acknowledged) = timeout(Duration::from_secs(5), async {
        tokio::join!(
            event_loop(
                events,
                "item".to_owned(),
                Arc::new(AtomicBool::new(true)),
                &observations,
            ),
            receiver.request(Version::Http, "POST", "/event", &[], &body),
        )
    })
    .await
    .unwrap();
    assert!(acknowledged.is_err());
    let fault = observed.unwrap_err();
    assert!(matches!(fault, VideoFault::Wire(_)));
    observations.fault(fault);
    let (playing, _) = parse_event(&body, "item").unwrap();
    assert_eq!(observations.next().now_or_never(), Some(Ok(playing)));
    assert_eq!(observations.next().now_or_never(), Some(Err(fault)));
}

#[test]
fn validates_timing_datagram_and_uses_ntp_timestamp() {
    let mut packet = [0; 32];
    let now = UNIX_EPOCH + Duration::from_millis(1_500);
    assert!(timing_reply(&packet, now).is_none());
    packet[0] = 0x80;
    assert!(timing_reply(&packet, now).is_none());
    packet[1] = 0xd2;
    assert!(timing_reply(&packet[..31], now).is_none());
    packet[31] = 0xe1;
    let answer = timing_reply(&packet, now).unwrap();
    assert_eq!(&answer[16..20], &2_208_988_801u32.to_be_bytes());
    assert_eq!(&answer[20..24], &0x8000_0000u32.to_be_bytes());
    assert_eq!(&answer[8..16], &packet[24..]);
    assert_eq!(&answer[16..24], &answer[24..]);
    assert!(!same_host(
        "127.0.0.1:1".parse().unwrap(),
        "127.0.0.2:1".parse().unwrap()
    ));
    assert!(same_host(
        "[fe80::1%2]:1".parse().unwrap(),
        "[fe80::1%2]:2".parse().unwrap()
    ));
    assert!(!same_host(
        "[fe80::1%2]:1".parse().unwrap(),
        "[fe80::1%3]:1".parse().unwrap()
    ));
}

fn unaddressed_state(state: &str) -> Vec<u8> {
    command_body(dictionary([
        ("type", "playbackState".into()),
        ("name", state.into()),
    ]))
    .unwrap()
}

#[test]
fn unaddressed_events_wait_for_queue_acceptance_and_terminal_needs_earlier_activity() {
    let observations = Observations::default();
    let (stopped, addressed_stop) = parse_event(&unaddressed_state("stopped"), "current").unwrap();
    let (playing, addressed_play) = parse_event(&unaddressed_state("playing"), "current").unwrap();
    assert!(!addressed_stop);
    assert!(!addressed_play);
    observations.media(stopped, addressed_stop);
    observations.media(playing, addressed_play);
    observations.media(stopped, addressed_stop);
    assert!(observations.next().now_or_never().is_none());
    observations.accept();
    assert_eq!(observations.next().now_or_never(), Some(Ok(playing)));
    assert_eq!(observations.next().now_or_never(), Some(Ok(stopped)));
}

#[test]
fn plist_rejects_expansion_before_materializing_an_aggregate_value() {
    let mut deep = Value::Boolean(true);
    for _ in 0..=MAX_PLIST_DEPTH {
        deep = Value::Array(vec![deep]);
    }
    let many_nodes = Value::Array(vec![Value::Boolean(true); MAX_PLIST_EVENTS]);
    let repeated_data = Value::Array(vec![Value::Data(vec![7; 32 * 1024]); 24]);
    for value in [deep, many_nodes, repeated_data] {
        let bytes = pack(&value).unwrap();
        assert!(
            bytes.len() < 256 * 1024,
            "exercise actual wire-sized messages"
        );
        assert!(matches!(
            unpack(&bytes),
            Err(VideoFault::Wire(Fault::InvalidMessage))
        ));
    }
    let valid = pack(&Value::Data(vec![7; 32 * 1024])).unwrap();
    assert_eq!(unpack(&valid).unwrap().as_data().unwrap().len(), 32 * 1024);
}

#[derive(Clone, Copy)]
pub(in crate::airplay) enum BaseSetupFailure {
    WrongSequence,
    Timeout { timing_worked: bool },
}

pub(in crate::airplay) async fn fail_base_setup(
    mut control: Wire,
    failure: BaseSetupFailure,
) -> SocketAddr {
    let mut setup = control.receive().await.unwrap();
    let payload = unpack(&setup.body).unwrap();
    let port = find(&payload, "timingPort")
        .unwrap()
        .as_unsigned_integer()
        .unwrap();
    let address = SocketAddr::from(([127, 0, 0, 1], u16::try_from(port).unwrap()));
    assert_ne!(address.port(), 0);
    match failure {
        BaseSetupFailure::WrongSequence => {
            setup.sequence = Some(setup.sequence.unwrap() + 1);
            control.respond(&setup, 200, &[]).await;
        }
        BaseSetupFailure::Timeout {
            timing_worked: true,
        } => {
            let timing = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
            let mut request = [0; 32];
            request[0] = 0x80;
            request[1] = 0xd2;
            timing.send_to(&request, address).await.unwrap();
            let mut reply = [0; 32];
            let (_, sender) = timing.recv_from(&mut reply).await.unwrap();
            assert_eq!(sender, address);
            assert_eq!(reply[1], 0xd3);
        }
        BaseSetupFailure::Timeout {
            timing_worked: false,
        } => {}
    }
    assert!(matches!(control.receive().await, Err(Fault::Closed)));
    address
}

#[test]
fn udp_listener_reuses_tcp_scope_for_link_local_ipv6() {
    let local = "[fe80::2]:12345".parse().unwrap();
    let peer = "[fe80::1%9]:7000".parse().unwrap();
    let timing = timing_address(local, peer, NonZeroU16::new(47475));
    assert_eq!(timing, "[fe80::2%9]:47475".parse().unwrap());
    let global = "[2001:db8::1]:12345".parse().unwrap();
    assert_eq!(
        timing_address(global, peer, None),
        "[2001:db8::1]:0".parse().unwrap(),
    );
}

#[test]
fn datagram_errors_distinguish_per_packet_failure_and_socket_failure() {
    for kind in [
        io::ErrorKind::Interrupted,
        io::ErrorKind::WouldBlock,
        io::ErrorKind::ConnectionRefused,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::NetworkUnreachable,
        io::ErrorKind::HostUnreachable,
    ] {
        assert_eq!(datagram_result::<usize>(Err(kind.into())).unwrap(), None);
    }
    assert_eq!(
        datagram_result::<usize>(Err(io::ErrorKind::InvalidInput.into()))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
}

#[tokio::test]
async fn timing_listener_drains_an_oversized_datagram_and_replies_to_the_next_packet() {
    let receiver = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
    let sender = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
    let address = sender.local_addr().unwrap();
    let peer = receiver.local_addr().unwrap();
    let observed = Arc::new(AtomicBool::new(false));
    let replied = observed.clone();
    let worker = tokio::spawn(async move { timing_loop(sender, peer, &replied).await });

    let mut oversized = vec![0; 4 * 1024];
    oversized[..2].copy_from_slice(&[0x80, 0xd2]);
    oversized[24..32].copy_from_slice(&[1; 8]);
    receiver.send_to(&oversized, address).await.unwrap();
    let mut valid = [0; 32];
    valid[..2].copy_from_slice(&[0x80, 0xd2]);
    valid[24..].copy_from_slice(&[2; 8]);
    receiver.send_to(&valid, address).await.unwrap();

    let mut response = [0; 32];
    let (received, source) = timeout(Duration::from_secs(2), receiver.recv_from(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source, address);
    assert_eq!(received, 32);
    assert_eq!(&response[8..16], &[2; 8]);
    assert!(observed.load(Ordering::Acquire));
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
}
