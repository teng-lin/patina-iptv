use super::*;

#[tokio::test(start_paused = true)]
async fn invalid_frames_end_the_connection_and_wake_waiting_requests() {
    for bytes in [
        (u32::try_from(MAX_FRAME).unwrap() + 1)
            .to_be_bytes()
            .to_vec(),
        vec![0, 0, 0, 1, 0xff],
    ] {
        let (session, mut peer, driver) = session_pair();
        let receiver = async {
            let _ = next_request(&mut peer).await;
            peer.write_all(&bytes).await.unwrap();
        };
        let started = Instant::now();
        let (result, ()) = tokio::join!(session.status(7), receiver);
        assert_eq!(result, Err(Error::Unknown));
        driver.await.unwrap();
        assert!(
            started.elapsed() < DEADLINE,
            "the connection should close immediately"
        );
        assert_eq!(session.status(7).await, Err(Error::Unknown));
    }
}

#[tokio::test(start_paused = true)]
async fn messages_for_another_sender_and_invalid_payloads_leave_the_request_pending() {
    let (session, mut peer, _driver) = session_pair();
    let receiver = async {
        let (_, request) = next_request(&mut peer).await;
        let status = json!({"type":"MEDIA_STATUS", "requestId":request["requestId"],
            "status":[{"mediaSessionId":7, "playerState":"PAUSED"}]});
        for (destination, kind, message) in [
            ("another-sender", 0, Some(status.to_string())),
            ("sender-0", 1, Some(status.to_string())),
            ("sender-0", 0, Some("{".into())),
        ] {
            let frame = Envelope {
                version: 0,
                source: session.transport.clone(),
                destination: destination.into(),
                namespace: MEDIA.into(),
                kind,
                message,
            }
            .encode_to_vec();
            peer.write_u32(u32::try_from(frame.len()).unwrap())
                .await
                .unwrap();
            peer.write_all(&frame).await.unwrap();
        }
        answer(
            &mut peer,
            &session.transport,
            json!({"type":"MEDIA_STATUS", "requestId":request["requestId"],
                "status":[{"mediaSessionId":7, "playerState":"PLAYING"}]}),
        )
        .await;
    };
    let (result, ()) = tokio::join!(session.status(7), receiver);
    assert_eq!(result, Ok(Status::Playing));
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_stops_reading_cannot_hold_the_socket_open() {
    let (client, _peer) = tokio::io::duplex(1);
    let (_session, requests) = session_channel();
    let started = Instant::now();
    timeout(DEADLINE + Duration::from_secs(1), drive(client, requests))
        .await
        .expect("the blocked write should time out");
    assert_eq!(started.elapsed(), DEADLINE);
}

#[tokio::test(start_paused = true)]
async fn a_reply_type_from_another_namespace_does_not_confirm_stop() {
    let (session, mut peer, _driver) = session_pair();
    let receiver = async {
        let (_, stop) = next_request(&mut peer).await;
        assert_eq!(stop["type"], "STOP");
        answer(
            &mut peer,
            &session.transport,
            json!({"type":"MEDIA_STATUS", "requestId":stop["requestId"],
                "status":[{"mediaSessionId":7, "playerState":"PLAYING"}]}),
        )
        .await;
        let (_, status) = next_request(&mut peer).await;
        assert_eq!(status["type"], "GET_STATUS");
        answer(
            &mut peer,
            &session.transport,
            json!({"type":"RECEIVER_STATUS", "requestId":status["requestId"], "status":[]}),
        )
        .await;
    };
    let (result, ()) = tokio::join!(session.command(7, Command::Stop), receiver);
    assert_eq!(result, Err(Error::Unknown));
}
