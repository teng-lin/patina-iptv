use super::*;
use tokio::io::{AsyncWriteExt, DuplexStream};

async fn next_frame(peer: &mut DuplexStream) -> (Envelope, Value) {
    let len = peer.read_u32().await.unwrap();
    let mut bytes = vec![0; len as usize];
    peer.read_exact(&mut bytes).await.unwrap();
    let frame = Envelope::decode(bytes.as_slice()).unwrap();
    let body = serde_json::from_str(frame.message.as_deref().unwrap()).unwrap();
    (frame, body)
}

pub(super) async fn next_request(peer: &mut DuplexStream) -> (Envelope, Value) {
    loop {
        let (frame, body) = next_frame(peer).await;
        if frame.namespace != HEARTBEAT {
            return (frame, body);
        }
    }
}

pub(super) fn frame_bytes(namespace: &str, src: &str, value: Value) -> Vec<u8> {
    let data = Envelope {
        version: 0,
        source: src.into(),
        destination: "sender-0".into(),
        namespace: namespace.into(),
        kind: 0,
        message: Some(value.to_string()),
    }
    .encode_to_vec();
    let mut frame = (data.len() as u32).to_be_bytes().to_vec();
    frame.extend(data);
    frame
}

pub(super) async fn answer(peer: &mut DuplexStream, src: &str, value: Value) {
    peer.write_all(&frame_bytes(MEDIA, src, value))
        .await
        .unwrap();
}

pub(super) fn session_pair() -> (Session, DuplexStream, tokio::task::JoinHandle<()>) {
    let (client, receiver) = tokio::io::duplex(8192);
    let (session, requests) = session_channel();
    let driver = tokio::spawn(drive(client, requests, "sender-0".into()));
    (session, receiver, driver)
}

pub(super) fn session_channel() -> (Session, mpsc::Receiver<Request>) {
    let (outgoing, requests) = mpsc::channel(8);
    let session = Session {
        outgoing,
        local_addr: ([127, 0, 0, 1], 12345).into(),
        peer_addr: ([127, 0, 0, 1], 8009).into(),
        driver: None,
        transport: "synthetic-app".into(),
    };
    (session, requests)
}

#[tokio::test]
async fn only_the_current_transports_correlated_load_can_establish_ownership() {
    let (session, mut receiver, owner) = session_pair();
    let url = "http://localhost/generated/stream.m3u8";
    let fake = async {
        let (_, load) = next_request(&mut receiver).await;
        let id = &load["requestId"];
        assert_eq!(load["media"]["streamType"], "LIVE");
        assert!(load.get("volume").is_none() && load.get("currentTime").is_none());
        answer(
            &mut receiver,
            "previous-app",
            json!({"requestId":id,"type":"LOAD_FAILED"}),
        )
        .await;
        answer(&mut receiver,"synthetic-app",json!({"type":"MEDIA_STATUS","status":[{"mediaSessionId":1,"playerState":"IDLE","idleReason":"INTERRUPTED"}]})).await;
        answer(&mut receiver,"synthetic-app",json!({"requestId":id,"type":"MEDIA_STATUS","status":[{"mediaSessionId":2,"media":{"contentId":url}}]})).await;
    };
    let (actual, _) = tokio::join!(session.load(url), fake);
    assert_eq!(actual, Ok(2));
    let refusal = async {
        let (_, load) = next_request(&mut receiver).await;
        answer(
            &mut receiver,
            "synthetic-app",
            json!({"requestId":load["requestId"],"type":"LOAD_FAILED"}),
        )
        .await;
    };
    let (actual, _) = tokio::join!(session.load(url), refusal);
    assert_eq!(actual, Err(Error::LoadFailed));
    drop(session);
    owner.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn concurrent_loads_receive_their_own_result_when_replies_arrive_in_reverse() {
    let (session, mut peer, _driver) = session_pair();
    let (first, second) = ("http://example.test/first", "http://example.test/second");
    let receiver = async {
        let (_, a) = next_request(&mut peer).await;
        let (_, b) = next_request(&mut peer).await;
        assert_ne!(a["requestId"], b["requestId"]);
        for req in [b, a] {
            let url = &req["media"]["contentId"];
            let message = if url == first {
                json!({"type":"LOAD_CANCELLED", "requestId":req["requestId"]})
            } else {
                assert_eq!(url, second);
                json!({"type":"MEDIA_STATUS", "requestId":req["requestId"],
                    "status":[{"mediaSessionId":7, "media":{"contentId":url}}]})
            };
            answer(&mut peer, &session.transport, message).await;
        }
    };
    let (a, b, ()) = tokio::join!(session.load(first), session.load(second), receiver);
    assert_eq!(a, Err(Error::Refused));
    assert_eq!(b, Ok(7));
}

#[tokio::test(start_paused = true)]
async fn receiver_volume_requires_an_active_owned_media_session() {
    for (entry, error) in [
        (
            json!({"mediaSessionId":8, "playerState":"PLAYING"}),
            Error::Ended,
        ),
        (
            json!({"mediaSessionId":7, "playerState":"IDLE", "idleReason":"INTERRUPTED"}),
            Error::Ended,
        ),
        (
            json!({"mediaSessionId":7, "playerState":"UNKNOWN"}),
            Error::Unknown,
        ),
    ] {
        let (session, mut peer, driver) = session_pair();
        let receiver = async {
            let (frame, req) = next_request(&mut peer).await;
            assert_eq!(frame.namespace, MEDIA);
            assert_eq!(req["type"], "GET_STATUS");
            answer(
                &mut peer,
                &session.transport,
                json!({"type":"MEDIA_STATUS", "requestId":req["requestId"], "status":[entry]}),
            )
            .await;
            // A subsequent independent query must be next, with no intervening SET_VOLUME.
            let (frame, req) = next_request(&mut peer).await;
            assert_eq!(
                (frame.namespace.as_str(), req["type"].as_str()),
                (MEDIA, Some("GET_STATUS"))
            );
            answer(
                &mut peer,
                &session.transport,
                json!({"type":"MEDIA_STATUS", "requestId":req["requestId"], "status":[]}),
            )
            .await;
        };
        let sender = async {
            assert_eq!(session.command(7, Command::Volume(25)).await, Err(error));
            assert_eq!(
                session.lookup("http://example.test/previous").await,
                Ok(None)
            );
        };
        tokio::join!(sender, receiver);
        drop(session);
        driver.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn stop_waits_for_the_owned_session_to_finish_buffering_and_go_idle() {
    let (session, mut peer, _driver) = session_pair();
    let receiver = async {
        for (verb, state, reason) in [
            ("STOP", "PLAYING", ""),
            ("GET_STATUS", "BUFFERING", ""),
            ("GET_STATUS", "IDLE", "CANCELLED"),
        ] {
            let (_, req) = next_request(&mut peer).await;
            assert_eq!(req["type"], verb);
            if verb == "STOP" {
                assert_eq!(req["mediaSessionId"], 7);
            }
            answer(
                &mut peer,
                &session.transport,
                json!({"type":"MEDIA_STATUS","requestId":req["requestId"],
                "status":[{"mediaSessionId":7,"playerState":state,"idleReason":reason}]}),
            )
            .await;
        }
    };
    let (result, ()) = tokio::join!(session.command(7, Command::Stop), receiver);
    assert_eq!(result, Ok(()));
}

#[tokio::test(start_paused = true)]
async fn a_silent_receiver_expires_pending_requests_and_releases_the_socket() {
    let (session, mut peer, driver) = session_pair();
    let caller = session.clone();
    let query = tokio::spawn(async move { caller.status(7).await });
    let (_, request) = next_request(&mut peer).await;
    assert_eq!(request["type"], "GET_STATUS");
    // A healthy socket stays open for the first two heartbeat intervals.
    for _ in 0..2 {
        tokio::time::advance(Duration::from_secs(5)).await;
        let (frame, message) = next_frame(&mut peer).await;
        assert_eq!(frame.namespace, HEARTBEAT);
        assert_eq!(message["type"], "PING");
        assert!(!driver.is_finished() && !query.is_finished());
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(query.await.unwrap(), Err(Error::Unknown));
    timeout(Duration::from_secs(1), driver)
        .await
        .expect("silent socket is closed")
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_fragmented_heartbeat_and_status_in_one_read_complete_the_pending_request() {
    let ping = frame_bytes(HEARTBEAT, ROOT, json!({"type":"PING"}));
    let (client, mut peer) = tokio::io::duplex(8192);
    let (socket_read, socket_write) = tokio::io::split(client);
    // Each slice ends a separate successful read, splitting both the header and payload.
    let reader = ping[..2].chain(&ping[2..9]).chain(socket_read);
    let (session, requests) = session_channel();
    let driver = drive(
        tokio::io::join(reader, socket_write),
        requests,
        "sender-0".into(),
    );
    let transport = session.transport.clone();
    let receiver = async {
        let (_, req) = next_request(&mut peer).await;
        let status = frame_bytes(
            MEDIA,
            &transport,
            json!({"type":"MEDIA_STATUS","requestId":req["requestId"],
            "status":[{"mediaSessionId":7,"playerState":"BUFFERING"}]}),
        );
        peer.write_all(&[&ping[9..], &status].concat())
            .await
            .unwrap();
        loop {
            let (frame, body) = next_frame(&mut peer).await;
            if body["type"] == "PONG" {
                assert_eq!(frame.namespace, HEARTBEAT);
                assert_eq!(frame.destination, ROOT);
                break;
            }
        }
    };
    let scenario = async {
        let (result, ()) = tokio::join!(session.status(7), receiver);
        assert_eq!(result, Ok(Status::Buffering));
        drop(session);
    };
    tokio::join!(driver, scenario);
}

#[tokio::test(start_paused = true)]
async fn receiver_close_releases_all_pending_requests_and_rejects_new_work() {
    let (session, mut peer, driver) = session_pair();
    let receiver = async {
        let _ = next_request(&mut peer).await;
        let _ = next_request(&mut peer).await;
        peer.write_all(&frame_bytes(CONNECTION, ROOT, json!({"type":"CLOSE"})))
            .await
            .unwrap();
    };
    let started = Instant::now();
    let (a, b, ()) = tokio::join!(session.status(7), session.status(8), receiver);
    assert_eq!((a, b), (Err(Error::Unknown), Err(Error::Unknown)));
    assert_eq!(session.status(9).await, Err(Error::Unknown));
    assert!(
        started.elapsed() < DEADLINE,
        "CLOSE must wake callers before their deadlines"
    );
    driver.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_request_expires_while_heartbeats_keep_the_session_usable() {
    let (session, mut peer, _driver) = session_pair();
    let receiver = async {
        let (_, first) = next_request(&mut peer).await;
        let second = loop {
            let (frame, body) = next_frame(&mut peer).await;
            if frame.namespace == MEDIA {
                break body;
            }
            assert_eq!(frame.namespace, HEARTBEAT);
            assert_eq!(body["type"], "PING");
            peer.write_all(&frame_bytes(HEARTBEAT, ROOT, json!({"type":"PONG"})))
                .await
                .unwrap();
        };
        assert_ne!(first["requestId"], second["requestId"]);
        answer(
            &mut peer,
            &session.transport,
            json!({"requestId":first["requestId"], "type":"INVALID_REQUEST"}),
        )
        .await;
        answer(&mut peer, &session.transport, json!({"requestId":second["requestId"], "type":"MEDIA_STATUS", "status":[{"mediaSessionId":7,"playerState":"PAUSED"}]})).await;
    };
    let sender = async {
        let started = Instant::now();
        assert_eq!(session.status(7).await, Err(Error::Unknown));
        assert!(started.elapsed() >= DEADLINE);
        assert_eq!(session.status(7).await, Ok(Status::Paused));
    };
    tokio::join!(sender, receiver);
}

mod failures;
mod media;

#[tokio::test(start_paused = true)]
async fn receiver_launch_retains_delivery_evidence_from_its_own_request() {
    let (session, requests) = session_channel();
    drop(requests);
    let mut delivery = Delivery::NotSent;
    assert_eq!(session.launch(&mut delivery).await, Err(Error::Unreachable));
    assert_eq!(delivery, Delivery::NotSent);

    for (response, expected, disposition) in [
        (
            Some(Err(Error::Refused)),
            Err(Error::Refused),
            Delivery::Acknowledged,
        ),
        (
            Some(Ok(
                json!({"status":{"applications":[{"appId":APP,"transportId":"generated"}]}}),
            )),
            Ok("generated".to_owned()),
            Delivery::Acknowledged,
        ),
        (None, Err(Error::TimedOut), Delivery::MayHaveReachedReceiver),
    ] {
        let (session, mut requests) = session_channel();
        let mut delivery = Delivery::NotSent;
        let receiver = async {
            let request = requests.recv().await.unwrap();
            assert_eq!(request.namespace, RECEIVER);
            assert_eq!(request.body["type"], "LAUNCH");
            if let Some(response) = response {
                request.reply.send(response).unwrap();
            } else {
                tokio::time::sleep(DEADLINE + Duration::from_secs(1)).await;
            }
        };
        let (result, ()) = tokio::join!(session.launch(&mut delivery), receiver);
        assert_eq!(result, expected);
        assert_eq!(delivery, disposition);
    }
}
