use super::*;
use crate::airplay::hap::tests::Accessory;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

pub(in crate::airplay) enum ReceiverEnding {
    CloseWithLoadPending,
    RefuseLoadAfterRequest,
    AwaitTransportClose,
    LoseEventTransport(oneshot::Receiver<()>),
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
    assert_eq!(timing_port, expected_timing_port);
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
    events
        .request(
            Version::Http,
            "POST",
            "/event",
            &[],
            &unaddressed_state("stopped"),
        )
        .await
        .unwrap()
        .require_ok()
        .unwrap();
    let mut upstream = TcpStream::connect(origin).await.unwrap();
    upstream
        .write_all(b"GET /generated.m3u8 HTTP/1.1\r\nHost: synthetic-origin\r\n\r\n")
        .await
        .unwrap();

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

    send_state(&mut events, "unowned-item", "playing").await;
    send_state(&mut events, item_uuid, "playing").await;
    match ending {
        ReceiverEnding::CloseWithLoadPending | ReceiverEnding::RefuseLoadAfterRequest => {
            unreachable!()
        }
        ReceiverEnding::AwaitTransportClose => {
            let feedback = control.receive().await.unwrap();
            assert_eq!(feedback.request_target, "POST /feedback");
            control.respond(&feedback, 200, &[]).await;
        }
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

#[test]
fn media_observations_require_owned_state_and_finite_times() {
    assert!(parse_event(b"", "item", true, true).is_none());
    for (wire, state) in [
        ("buffering", MediaState::Buffering),
        ("paused", MediaState::Paused),
        ("playing", MediaState::Playing),
        ("ended", MediaState::Ended),
        ("stopped", MediaState::Absent),
    ] {
        let event = state_event("item", wire);
        assert!(parse_event(&event, "other", true, true).is_none());
        let evidence = parse_event(&event, "item", true, true).unwrap();
        assert_eq!(evidence.state, state);
        assert_eq!(evidence.duration, Some(Duration::from_secs(21)));
        assert_eq!(evidence.position, Some(Duration::from_millis(5_500)));
    }
    assert!(parse_event(&state_event("item", "unrecognized"), "item", true, true).is_none());
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
        let (updates, receiver) = watch::channel(None);
        let observation = |state| {
            Ok(Evidence {
                state,
                position: None,
                duration: None,
            })
        };
        publish(&updates, observation(first));
        publish(&updates, observation(second));
        publish(&updates, Err(VideoFault::Wire(Fault::Closed)));
        assert_eq!(*receiver.borrow(), Some(observation(first)));
    }
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
fn unaddressed_events_need_queue_acceptance_and_terminal_needs_earlier_activity() {
    let stopped = unaddressed_state("stopped");
    let playing = unaddressed_state("playing");
    assert!(parse_event(&stopped, "current", false, false).is_none());
    assert!(parse_event(&playing, "current", false, false).is_none());
    assert!(parse_event(&stopped, "current", true, false).is_none());
    assert_eq!(
        parse_event(&playing, "current", true, false).unwrap().state,
        MediaState::Playing
    );
    assert_eq!(
        parse_event(&stopped, "current", true, true).unwrap().state,
        MediaState::Absent
    );
    assert_eq!(
        parse_event(&state_event("current", "playing"), "current", false, false)
            .unwrap()
            .state,
        MediaState::Playing
    );
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
