use super::*;
use crate::Snapshot;
use crate::googlecast::tests::{answer, next_request, session_pair};

const URL: &str = "http://example.test/generated/index.m3u8";
const TRANSPORT: &str = "synthetic-app";

fn status(request: &Value, entries: Value) -> Value {
    json!({"type":"MEDIA_STATUS", "requestId":request["requestId"], "status":entries})
}

async fn assert_only_heartbeats_until_closed(receiver: &mut tokio::io::DuplexStream) {
    use prost::Message;
    use tokio::io::AsyncReadExt;

    while let Ok(length) = receiver.read_u32().await {
        let mut frame = vec![0; length as usize];
        receiver.read_exact(&mut frame).await.unwrap();
        let envelope = super::super::Envelope::decode(frame.as_slice()).unwrap();
        assert_eq!(envelope.namespace, super::super::HEARTBEAT);
    }
}

fn owned(load: &Value, id: i64, state: &str) -> Value {
    json!({"mediaSessionId":id, "media":load["media"], "playerState":state,
        "currentTime":4.5, "supportedMediaCommands":3})
}

fn snap(observation: Observation, live: bool) -> Snapshot {
    let mut snapshot = Snapshot::pending(live);
    snapshot.apply(observation);
    snapshot
}

#[tokio::test(start_paused = true)]
async fn same_url_loads_only_establish_ownership_of_the_correlated_instance() {
    let (wire, mut receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    let dispatch = Dispatch::default();
    let sender = async {
        assert_eq!(cast.peer_trust(), PeerTrust::LanEndpoint);
        let (local, remote) = cast.control_addrs().unwrap();
        assert!(local.ip().is_loopback() && remote.ip().is_loopback());
        let media = Media::live(URL).unwrap();
        cast.validate(&media).unwrap();
        let snapshot = snap(cast.load(media, &dispatch).await.unwrap(), true);
        assert!(matches!(snapshot.playback(), Playback::Playing));
        assert_eq!(
            snapshot.availability(Feature::Seek),
            Availability::Unsupported
        );
        assert_eq!(snapshot.volume_scope(), Some(VolumeScope::Receiver));
        assert_eq!(snapshot.position(), Some(Duration::from_millis(4500)));
        assert_eq!(
            cast.stop().await.unwrap(),
            StopOutcome::StopRequested(Confirmation::ReceiverConfirmed)
        );
        cast.close().await;
    };
    let peer = async {
        let (_, load) = next_request(&mut receiver).await;
        assert_eq!(load["media"]["contentType"], "application/x-mpegURL");
        assert_eq!(load["media"]["streamType"], "LIVE");
        assert_eq!(load["media"]["contentId"], URL);
        assert!(load["media"]["customData"]["castRsLoad"].is_string());
        let item = owned(&load, 7, "PLAYING");
        let mut other = owned(&load, 9, "PLAYING");
        other["media"]["customData"]["castRsLoad"] = json!("another-sender");
        answer(
            &mut receiver,
            TRANSPORT,
            status(&load, json!([other, item])),
        )
        .await;

        for verb in ["GET_STATUS", "STOP", "GET_STATUS"] {
            let (_, request) = next_request(&mut receiver).await;
            assert_eq!(request["type"], verb);
            let entries = if verb == "GET_STATUS" && request["requestId"] == 4 {
                json!([])
            } else {
                json!([item])
            };
            if verb == "STOP" {
                assert_eq!(request["mediaSessionId"], 7);
            }
            answer(&mut receiver, TRANSPORT, status(&request, entries)).await;
        }
    };
    tokio::join!(sender, peer);
    socket.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stop_can_reconcile_a_late_load_reply_after_cancellation() {
    let (wire, mut receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    let dispatch = Dispatch::default();
    let sender = async {
        let media = Media::live(URL).unwrap();
        assert!(
            timeout(Duration::from_millis(100), cast.load(media, &dispatch))
                .await
                .is_err()
        );
        assert_eq!(
            cast.stop().await.unwrap(),
            StopOutcome::StopRequested(Confirmation::ReceiverConfirmed)
        );
        cast.close().await;
    };
    let peer = async {
        let (_, load) = next_request(&mut receiver).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let item = owned(&load, 19, "PLAYING");
        answer(&mut receiver, TRANSPORT, status(&load, json!([item]))).await;
        for (verb, entries) in [
            ("GET_STATUS", json!([item])),
            ("STOP", json!([item])),
            ("GET_STATUS", json!([])),
        ] {
            let (_, request) = next_request(&mut receiver).await;
            assert_eq!(request["type"], verb);
            if verb == "STOP" {
                assert_eq!(request["mediaSessionId"], 19);
            }
            answer(&mut receiver, TRANSPORT, status(&request, entries)).await;
        }
    };
    tokio::join!(sender, peer);
    socket.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn absent_load_reply_never_stops_another_sender_at_the_same_url() {
    for include_own_tag in [false, true] {
        let (wire, mut receiver, socket) = session_pair();
        let mut cast = CastDriver::new(wire).unwrap();
        let dispatch = Dispatch::default();
        let sender = async {
            let media = Media::live(URL).unwrap();
            assert!(
                timeout(Duration::from_millis(100), cast.load(media, &dispatch))
                    .await
                    .is_err()
            );
            let result = cast.stop().await;
            if include_own_tag {
                assert_eq!(
                    result.unwrap(),
                    StopOutcome::StopRequested(Confirmation::ReceiverConfirmed)
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), ErrorKind::Protocol);
                assert_eq!(error.delivery(), Delivery::NotSent);
            }
            cast.close().await;
        };
        let peer = async {
            let (_, load) = next_request(&mut receiver).await;
            let mut item = owned(&load, 4, "PLAYING");
            if !include_own_tag {
                item["media"]["customData"]["castRsLoad"] = json!("another-sender");
            }
            let (_, query) = next_request(&mut receiver).await;
            assert_eq!(query["type"], "GET_STATUS");
            answer(&mut receiver, TRANSPORT, status(&query, json!([item]))).await;
            if include_own_tag {
                for (verb, entries) in [
                    ("GET_STATUS", json!([item])),
                    ("STOP", json!([item])),
                    ("GET_STATUS", json!([])),
                ] {
                    let (_, request) = next_request(&mut receiver).await;
                    assert_eq!(request["type"], verb);
                    if verb == "STOP" {
                        assert_eq!(request["mediaSessionId"], 4);
                    }
                    answer(&mut receiver, TRANSPORT, status(&request, entries)).await;
                }
            }
            assert_only_heartbeats_until_closed(&mut receiver).await;
        };
        tokio::join!(sender, peer);
        socket.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn same_id_with_a_foreign_tag_refuses_receiver_volume_and_scoped_stop() {
    let (wire, mut receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    let dispatch = Dispatch::default();
    let sender = async {
        cast.load(Media::live(URL).unwrap(), &dispatch)
            .await
            .unwrap();
        let error = cast
            .command(Command::Output {
                scope: VolumeScope::Receiver,
                change: OutputChange::ToggleMute,
            })
            .await
            .unwrap_err();
        assert_eq!(
            (error.kind(), error.delivery()),
            (ErrorKind::OwnershipLost, Delivery::NotSent)
        );
        let error = cast.stop().await.unwrap_err();
        assert_eq!(
            (error.kind(), error.delivery()),
            (ErrorKind::OwnershipLost, Delivery::NotSent)
        );
        cast.close().await;
    };
    let peer = async {
        let (_, load) = next_request(&mut receiver).await;
        let mut item = owned(&load, 17, "PLAYING");
        answer(&mut receiver, TRANSPORT, status(&load, json!([item]))).await;
        item["media"]["customData"]["castRsLoad"] = json!("different-instance");
        for _ in 0..2 {
            let (_, request) = next_request(&mut receiver).await;
            assert_eq!(request["type"], "GET_STATUS");
            answer(&mut receiver, TRANSPORT, status(&request, json!([item]))).await;
        }
    };
    tokio::join!(sender, peer);
    socket.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn control_and_observations_use_fresh_status_and_preserve_confirmation_kind() {
    let (wire, mut receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    let dispatch = Dispatch::default();
    let sender = async {
        let media =
            Media::on_demand("https://example.test/generated/video.MP4?synthetic=1").unwrap();
        cast.validate(&media).unwrap();
        let mut initial = snap(cast.load(media, &dispatch).await.unwrap(), false);
        assert!(matches!(initial.playback(), Playback::Accepted));
        initial.apply(cast.observe().await.unwrap());
        assert_eq!(initial.link(), Link::Recovering);
        let timestamp = initial.playback_observed_at();
        initial.apply(cast.observe().await.unwrap());
        assert_eq!(initial.link(), Link::Connected);
        assert!(initial.playback_observed_at() > timestamp);
        assert!(matches!(initial.playback(), Playback::Paused));
        assert_eq!(initial.duration(), Some(Duration::from_secs(90)));
        assert_eq!(initial.volume(), Some(Volume::percent(45).unwrap()));
        assert_eq!(initial.muted(), Some(true));
        let absolute = Duration::from_secs(5);
        assert_eq!(
            cast.command(Command::Seek(absolute)).await.unwrap(),
            Confirmation::ReceiverConfirmed
        );
        let mute = Command::Output {
            scope: VolumeScope::Receiver,
            change: OutputChange::SetMuted(false),
        };
        assert_eq!(cast.command(mute).await.unwrap(), Confirmation::Accepted);
        let invalid = Command::Output {
            scope: VolumeScope::Media,
            change: OutputChange::SetMuted(false),
        };
        assert_eq!(
            cast.command(invalid).await.unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        cast.close().await;
    };
    let peer = async {
        let (_, load) = next_request(&mut receiver).await;
        assert_eq!(load["media"]["streamType"], "BUFFERED");
        assert_eq!(load["media"]["contentType"], "video/mp4");
        let mut item = owned(&load, 11, "PAUSED");
        item["media"]["duration"] = json!(90.0);
        let mut ack = item.clone();
        ack.as_object_mut().unwrap().remove("playerState");
        answer(&mut receiver, TRANSPORT, status(&load, json!([ack]))).await;
        for (i, (verb, ns)) in [
            ("GET_STATUS", super::super::MEDIA),
            ("GET_STATUS", super::super::MEDIA),
            ("GET_STATUS", super::super::RECEIVER),
            ("GET_STATUS", super::super::MEDIA),
            ("SEEK", super::super::MEDIA),
            ("GET_STATUS", super::super::MEDIA),
            ("GET_STATUS", super::super::MEDIA),
            ("SET_VOLUME", super::super::RECEIVER),
        ]
        .into_iter()
        .enumerate()
        {
            let (frame, request) = next_request(&mut receiver).await;
            assert_eq!(
                (frame.namespace.as_str(), request["type"].as_str()),
                (ns, Some(verb))
            );
            if ns == super::super::RECEIVER {
                use crate::googlecast::tests::frame_bytes;
                use tokio::io::AsyncWriteExt;
                if verb == "SET_VOLUME" {
                    assert_eq!(request["volume"]["muted"], false);
                }
                receiver
                    .write_all(&frame_bytes(
                        ns,
                        super::super::ROOT,
                        json!({"type":"RECEIVER_STATUS","requestId":request["requestId"],
                        "status":{"volume":{"level":0.45,"muted":true}}}),
                    ))
                    .await
                    .unwrap();
                continue;
            }
            if verb == "SEEK" {
                assert_eq!(request["mediaSessionId"], 11);
                assert_eq!(request["currentTime"], 5.0);
                item["currentTime"] = json!(5.0);
            }
            let entries = if i == 0 { json!([{}]) } else { json!([item]) };
            answer(&mut receiver, TRANSPORT, status(&request, entries)).await;
        }
    };
    tokio::join!(sender, peer);
    socket.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn validates_media_before_dispatch() {
    let (wire, _receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    for media in [
        Media::live("http://example.test/generated").unwrap(),
        Media::on_demand(&format!(
            "http://example.test/{}.mp4",
            "x".repeat(super::super::MAX_FRAME)
        ))
        .unwrap(),
    ] {
        let error = cast.validate(&media).unwrap_err();
        assert_eq!(
            (error.kind(), error.stage(), error.delivery()),
            (ErrorKind::InvalidInput, Stage::Load, Delivery::NotSent)
        );
    }
    cast.validate(
        &Media::on_demand("http://example.test/generated")
            .unwrap()
            .with_content_type("video/mp4")
            .unwrap(),
    )
    .unwrap();
    cast.close().await;
    socket.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn idle_observations_never_authorize_more_commands_or_confirm_stop() {
    let (wire, _receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    let playing = json!({"status":[{"mediaSessionId":7,"playerState":"PLAYING"}]});
    let idle = json!({"status":[{"mediaSessionId":7,"playerState":"IDLE"}]});
    let mut snapshot = snap(cast.snapshot(&playing, 7), true);
    let observed_at = snapshot.playback_observed_at();
    snapshot.apply(cast.snapshot(&idle, 7));
    assert_eq!(snapshot.playback_observed_at(), observed_at);
    assert_eq!(snapshot.link(), Link::Recovering);
    assert_eq!(
        snapshot.availability(Feature::Play),
        Availability::Unavailable
    );
    tokio::time::advance(IDLE_GRACE / 2).await;
    snapshot.apply(cast.snapshot(&playing, 7));
    assert_eq!(snapshot.link(), Link::Connected);
    snapshot.apply(cast.snapshot(&idle, 7));
    tokio::time::advance(IDLE_GRACE).await;
    snapshot.apply(cast.snapshot(&idle, 7));
    assert!(matches!(snapshot.playback(), Playback::Failed(_)));
    cast.close().await;
    socket.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn terminal_report_dispositions_are_distinct() {
    let (wire, _receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    for (state, reason) in [
        ("IDLE", "FINISHED"),
        ("IDLE", "CANCELLED"),
        ("IDLE", "INTERRUPTED"),
        ("IDLE", "ERROR"),
        ("BUFFERING", ""),
        ("UNKNOWN", ""),
    ] {
        let reply =
            json!({"status":[{"mediaSessionId":7,"playerState":state,"idleReason":reason}]});
        let observed = snap(cast.snapshot(&reply, 7), true);
        match (state, reason, observed.playback()) {
            ("IDLE", "FINISHED" | "CANCELLED", Playback::Ended)
            | ("IDLE", "INTERRUPTED", Playback::NoLongerPresent)
            | ("IDLE", "ERROR", Playback::Failed(_))
            | ("BUFFERING", "", Playback::Buffering)
            | ("UNKNOWN", "", Playback::Pending) => {}
            _ => panic!("wrong observed status: {observed:?}"),
        }
    }
    let takeover = json!({"status":[{"mediaSessionId":10,"playerState":"PLAYING"}]});
    assert!(matches!(
        snap(cast.snapshot(&takeover, 7), true).playback(),
        Playback::OwnershipLost
    ));
    cast.close().await;
    socket.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn scoped_stop_waits_for_proof_and_reports_incomplete_or_replaced_state() {
    for (entries, expected) in [
        (json!([]), Ok(StopOutcome::AlreadyEnded)),
        (
            json!([{"mediaSessionId":8, "playerState":"BUFFERING"}]),
            Err(ErrorKind::OwnershipLost),
        ),
        (
            json!([{"mediaSessionId":7, "playerState":"IDLE","idleReason":"FINISHED"}]),
            Ok(StopOutcome::AlreadyEnded),
        ),
        (
            json!([{"mediaSessionId":7, "playerState":"NEW_STATE"}]),
            Err(ErrorKind::Protocol),
        ),
        (json!([{"mediaSessionId":7}]), Err(ErrorKind::Protocol)),
    ] {
        let (wire, mut receiver, socket) = session_pair();
        let mut cast = CastDriver::new(wire).unwrap();
        let sender = async {
            cast.owned = Some(7);
            assert_eq!(
                cast.stop().await.map_err(|error| {
                    assert_eq!(error.delivery(), Delivery::NotSent);
                    error.kind()
                }),
                expected
            );
            cast.close().await;
        };
        let peer = async {
            let (_, request) = next_request(&mut receiver).await;
            assert_eq!(request["type"], "GET_STATUS");
            answer(&mut receiver, TRANSPORT, status(&request, entries)).await;
            assert_only_heartbeats_until_closed(&mut receiver).await;
        };
        tokio::join!(sender, peer);
        socket.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn transport_loss_cannot_discover_or_resend_a_previous_load() {
    let (wire, receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    cast.owned = Some(7);
    drop(receiver);
    let error = cast.observe().await.err().unwrap();
    assert_eq!(error.stage(), Stage::Observe);
    assert_eq!(error.delivery(), Delivery::NotSent);
    let error = cast.stop().await.unwrap_err();
    assert_eq!(error.stage(), Stage::Stop);
    assert_eq!(error.delivery(), Delivery::NotSent);
    cast.close().await;
    socket.await.unwrap();
}

#[test]
fn discovery_suppresses_conflicting_identity_claims_and_keeps_exact_duplicates_once() {
    let receiver = |name: &str, id: Option<&str>, address: u8| discovery::Receiver {
        address: ([127, 0, 0, address], 8009).into(),
        id: id.map(str::to_owned),
        name: name.into(),
    };
    let selectable = receiver("Generated TV", Some("confirmed"), 1);
    let ephemeral = receiver("Older device", None, 2);
    let separate = receiver("Other service", Some("other"), 1);
    assert_eq!(
        selectable_receivers(vec![
            receiver("Conflicting device", Some("conflict"), 7),
            selectable.clone(),
            receiver("Conflicting device", Some("conflict"), 8),
            ephemeral.clone(),
            selectable.clone(),
            separate.clone(),
            receiver("Conflicting device", Some("conflict"), 7),
        ]),
        [selectable, ephemeral, separate],
    );
}

#[tokio::test(start_paused = true)]
async fn endpoint_selection_rejects_missing_ambiguous_and_expired_identities() {
    let address: SocketAddr = ([127, 0, 0, 1], 8009).into();
    let moved: SocketAddr = ([127, 0, 0, 1], 6881).into();
    let original = discovery::Receiver {
        id: Some("generated".into()),
        name: "Generated".into(),
        address,
    };
    let relocated = discovery::Receiver {
        address: moved,
        ..original.clone()
    };
    assert_eq!(
        unique_endpoint("generated", vec![relocated.clone()]).unwrap(),
        moved
    );
    assert!(unique_endpoint("generated", vec![original, relocated]).is_err());
    assert!(unique_endpoint("generated", Vec::new()).is_err());
    let ephemeral = Endpoint {
        address,
        id: None,
        scanned: Instant::now(),
    };
    assert_eq!(
        ephemeral.resolve(&GoogleCast::default()).await.unwrap(),
        address
    );
    tokio::time::advance(RECENT_SCAN).await;
    assert!(ephemeral.resolve(&GoogleCast::default()).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn replacement_during_a_receiver_volume_read_never_applies_the_relative_change() {
    use crate::googlecast::tests::frame_bytes;
    use tokio::io::AsyncWriteExt;

    let (wire, mut receiver, socket) = session_pair();
    let mut cast = CastDriver::new(wire).unwrap();
    let dispatch = Dispatch::default();
    let sender = async {
        cast.load(Media::live(URL).unwrap(), &dispatch)
            .await
            .unwrap();
        let error = cast
            .command(Command::Output {
                scope: VolumeScope::Receiver,
                change: OutputChange::StepVolume(5),
            })
            .await
            .unwrap_err();
        assert_eq!(
            (error.kind(), error.delivery()),
            (ErrorKind::OwnershipLost, Delivery::NotSent)
        );
        cast.close().await;
    };
    let peer = async {
        let (_, load) = next_request(&mut receiver).await;
        let mut item = owned(&load, 17, "PLAYING");
        answer(&mut receiver, TRANSPORT, status(&load, json!([item]))).await;
        let (_, query) = next_request(&mut receiver).await;
        assert_eq!(query["type"], "GET_STATUS");
        answer(&mut receiver, TRANSPORT, status(&query, json!([item]))).await;
        let (frame, query) = next_request(&mut receiver).await;
        assert_eq!(frame.namespace, super::super::RECEIVER);
        assert_eq!(query["type"], "GET_STATUS");
        receiver.write_all(&frame_bytes(
            super::super::RECEIVER,
            super::super::ROOT,
            json!({"type":"RECEIVER_STATUS","requestId":query["requestId"],"status":{"volume":{"level":0.45}}}),
        )).await.unwrap();
        let (_, query) = next_request(&mut receiver).await;
        assert_eq!(query["type"], "GET_STATUS");
        item["media"]["customData"]["castRsLoad"] = json!("different-instance");
        answer(&mut receiver, TRANSPORT, status(&query, json!([item]))).await;
        assert_only_heartbeats_until_closed(&mut receiver).await;
    };
    tokio::join!(sender, peer);
    socket.await.unwrap();
}

#[tokio::test]
async fn production_adapter_drives_the_public_session_against_a_loopback_receiver() {
    use crate::test_support;

    let key_pair = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let receiver = test_support::Receiver::start(
        test_support::Load::Immediate,
        key_pair.cert.der().to_vec(),
        key_pair.key_pair.serialize_der(),
    )
    .await;
    let cast = test_support::sender(receiver.address()).unwrap();
    let discovery = cast.discover(Duration::from_secs(1)).await.unwrap();
    let route = &discovery.routes()[0];
    assert_eq!(route.protocol().as_str(), "googlecast");
    assert_eq!(
        route.availability(Feature::LoadVideoUrl),
        Availability::Available
    );
    let crate::Connect::Ready(connection) = cast.connect(route).await.unwrap() else {
        panic!("Google Cast must connect without pairing");
    };
    let mut session = connection.load(Media::live(URL).unwrap()).unwrap();
    let mut updates = session.subscribe();
    timeout(Duration::from_secs(5), async {
        while let Some(update) = updates.next().await {
            if matches!(update.playback(), Playback::Playing) {
                return;
            }
        }
        panic!("the receiver did not confirm playback");
    })
    .await
    .unwrap();
    let volume = Command::Output {
        scope: VolumeScope::Receiver,
        change: OutputChange::SetVolume(Volume::percent(60).unwrap()),
    };
    assert_eq!(
        session.command(volume).await.unwrap(),
        Confirmation::Accepted
    );
    assert_eq!(
        session.command(Command::Pause).await.unwrap(),
        Confirmation::ReceiverConfirmed
    );
    assert_eq!(
        session.command(Command::Play).await.unwrap(),
        Confirmation::ReceiverConfirmed
    );
    assert_eq!(
        session.stop().await.unwrap(),
        StopOutcome::StopRequested(Confirmation::ReceiverConfirmed)
    );
    assert_eq!(
        receiver.events(),
        [
            test_support::Event::Load,
            test_support::Event::Paused,
            test_support::Event::Playing,
            test_support::Event::Stop,
        ]
    );
    timeout(Duration::from_secs(5), receiver.finish())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn loading_reports_whether_a_receiver_could_have_seen_the_request() {
    use crate::googlecast::tests::session_channel;

    let (wire, requests) = session_channel();
    let mut cast = CastDriver::new(wire).unwrap();
    drop(requests);
    let dispatch = Dispatch::default();
    let error = cast
        .load(Media::live(URL).unwrap(), &dispatch)
        .await
        .err()
        .unwrap();
    assert_eq!(
        (error.kind(), error.delivery()),
        (ErrorKind::Unreachable, Delivery::NotSent)
    );

    #[derive(Clone, Copy)]
    enum Response {
        LostConnection,
        Refused(WireError),
        Unanswered,
    }
    for (mode, expected) in [
        (
            Response::LostConnection,
            (ErrorKind::Unreachable, Delivery::MayHaveReachedReceiver),
        ),
        (
            Response::Refused(WireError::LoadFailed),
            (ErrorKind::ReceiverRefused, Delivery::Acknowledged),
        ),
        (
            Response::Refused(WireError::Refused),
            (ErrorKind::ReceiverRefused, Delivery::Acknowledged),
        ),
        (
            Response::Unanswered,
            (ErrorKind::TimedOut, Delivery::MayHaveReachedReceiver),
        ),
    ] {
        let (wire, mut requests) = session_channel();
        let mut cast = CastDriver::new(wire).unwrap();
        let dispatch = Dispatch::default();
        let receiver = async {
            let request = requests.recv().await.unwrap();
            match mode {
                Response::LostConnection => drop(request),
                Response::Refused(reason) => request.reply.send(Err(reason)).unwrap(),
                Response::Unanswered => {
                    tokio::time::sleep(super::super::DEADLINE + Duration::from_secs(1)).await;
                    drop(request);
                }
            }
        };
        let (result, ()) = tokio::join!(cast.load(Media::live(URL).unwrap(), &dispatch), receiver);
        let error = result.err().unwrap();
        assert_eq!((error.kind(), error.delivery()), expected);
        assert_eq!(
            error.diagnostic(),
            matches!(mode, Response::Refused(WireError::LoadFailed))
                .then_some(&Diagnostic::MediaLoadFailed),
        );
    }
}

#[tokio::test]
async fn a_discovered_route_whose_receiver_has_gone_fails_before_loading() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let cast = crate::test_support::sender(listener.local_addr().unwrap()).unwrap();
    let discovery = cast.discover(Duration::from_secs(1)).await.unwrap();
    drop(listener);
    let result = cast.connect(&discovery.routes()[0]).await;
    let error = result.err().unwrap();
    assert_eq!(
        (error.kind(), error.stage(), error.delivery()),
        (ErrorKind::Unreachable, Stage::Connect, Delivery::NotSent)
    );
}

#[tokio::test(start_paused = true)]
async fn controls_recheck_the_receiver_capability_and_whether_owned_media_is_present() {
    use crate::googlecast::tests::session_channel;
    for (entries, command, expected) in [
        (json!([]), Command::Play, ErrorKind::Unavailable),
        (
            json!([{"mediaSessionId":8,"playerState":"PLAYING"}]),
            Command::Play,
            ErrorKind::OwnershipLost,
        ),
        (
            json!([{"mediaSessionId":7,"playerState":"PLAYING","supportedMediaCommands":0}]),
            Command::Pause,
            ErrorKind::Unsupported,
        ),
        (
            json!([{"mediaSessionId":7,"playerState":"PLAYING"}]),
            Command::Seek(Duration::from_secs(3)),
            ErrorKind::Unavailable,
        ),
    ] {
        let (wire, mut requests) = session_channel();
        let mut cast = CastDriver::new(wire).unwrap();
        let sender = async {
            cast.owned = Some(7);
            let error = cast.command(command).await.unwrap_err();
            assert_eq!(
                (error.kind(), error.delivery()),
                (expected, Delivery::NotSent)
            );
            cast.close().await;
        };
        let receiver = async {
            let request = requests.recv().await.unwrap();
            assert_eq!(request.body["type"], "GET_STATUS");
            request.reply.send(Ok(json!({"status": entries}))).unwrap();
            assert!(requests.recv().await.is_none());
        };
        tokio::join!(sender, receiver);
    }
}

#[tokio::test(start_paused = true)]
async fn correlated_media_replies_never_confirm_effects_on_a_reused_id() {
    use crate::googlecast::tests::session_channel;
    for (command, verb, confirm_state) in [
        (Some(Command::Play), "PLAY", "PLAYING"),
        (Some(Command::Pause), "PAUSE", "PAUSED"),
        (
            Some(Command::Seek(Duration::from_secs(5))),
            "SEEK",
            "PAUSED",
        ),
        (None, "STOP", "IDLE"),
    ] {
        for delayed in [false, true] {
            let (wire, mut requests) = session_channel();
            let mut cast = CastDriver::new(wire).unwrap();
            cast.owned = Some(7);
            let owned = json!({
                "mediaSessionId":7,"playerState":"BUFFERING","supportedMediaCommands":3,
                "media":{"customData":{"castRsLoad":cast.token}},
            });
            let foreign = json!({
                "mediaSessionId":7,"playerState":confirm_state,"currentTime":5.0,
                "idleReason":"CANCELLED","media":{"customData":{"castRsLoad":"foreign"}},
            });
            let sender = async {
                let error = if let Some(command) = command {
                    cast.command(command).await.unwrap_err()
                } else {
                    cast.stop().await.unwrap_err()
                };
                assert_eq!(
                    (error.kind(), error.delivery()),
                    (ErrorKind::OwnershipLost, Delivery::MayHaveReachedReceiver,)
                );
                cast.close().await;
            };
            let receiver = async {
                let query = requests.recv().await.unwrap();
                assert_eq!(query.body["type"], "GET_STATUS");
                query.reply.send(Ok(json!({"status":[owned]}))).unwrap();
                let request = requests.recv().await.unwrap();
                assert_eq!(request.body["type"], verb);
                let reply = if delayed { &owned } else { &foreign };
                let mut reply = reply.clone();
                if delayed && verb == "PLAY" {
                    reply["playerState"] = json!("PAUSED");
                }
                request.reply.send(Ok(json!({"status":[reply]}))).unwrap();
                if delayed {
                    let query = requests.recv().await.unwrap();
                    assert_eq!(query.body["type"], "GET_STATUS");
                    query.reply.send(Ok(json!({"status":[foreign]}))).unwrap();
                }
                assert!(requests.recv().await.is_none());
            };
            tokio::join!(sender, receiver);
        }
    }
}
