use super::*;

#[tokio::test(start_paused = true)]
async fn malformed_status_never_confirms_a_session_has_ended() {
    let (session, mut peer, _driver) = session_pair();
    let receiver = async {
        for status in [
            Value::Null,
            json!({}),
            json!([{}]),
            json!([{"mediaSessionId":"7"}]),
            json!([{"mediaSessionId":7}]),
            json!([{"mediaSessionId":7,"playerState":false}]),
            json!([]),
        ] {
            let (_, request) = next_request(&mut peer).await;
            assert_eq!(request["type"], "GET_STATUS");
            answer(
                &mut peer,
                &session.transport,
                json!({"type":"MEDIA_STATUS", "requestId":request["requestId"], "status":status}),
            )
            .await;
        }
    };
    let sender = async {
        for _ in 0..6 {
            assert_eq!(session.status(7).await, Err(Error::Unknown));
        }
        assert_eq!(session.status(7).await, Err(Error::Ended));
    };
    tokio::join!(sender, receiver);
}

#[tokio::test(start_paused = true)]
async fn play_and_pause_require_confirmation_for_the_owned_session() {
    let (session, mut peer, _driver) = session_pair();
    let cases = [
        (
            Command::Pause,
            "PAUSE",
            8,
            &["PLAYING"][..],
            Err(Error::Ended),
        ),
        (
            Command::Pause,
            "PAUSE",
            7,
            &["PLAYING", "PAUSED"][..],
            Ok(()),
        ),
        (Command::Pause, "PAUSE", 7, &["PAUSED"][..], Ok(())),
        (
            Command::Play,
            "PLAY",
            7,
            &["PAUSED", "BUFFERING"][..],
            Ok(()),
        ),
        (Command::Play, "PLAY", 7, &["PLAYING"][..], Ok(())),
    ];
    let receiver = async {
        for (_, verb, id, states, _) in cases {
            for (index, state) in states.iter().enumerate() {
                let (_, request) = next_request(&mut peer).await;
                if index == 0 {
                    assert_eq!(request["type"], verb);
                    assert_eq!(request["mediaSessionId"], 7);
                } else {
                    assert_eq!(request["type"], "GET_STATUS");
                }
                answer(
                    &mut peer,
                    &session.transport,
                    json!({"type":"MEDIA_STATUS", "requestId":request["requestId"],
                        "status":[{"mediaSessionId":id, "playerState":state}]}),
                )
                .await;
            }
        }
    };
    let sender = async {
        for (command, _, _, _, expected) in cases {
            assert_eq!(session.command(7, command).await, expected, "{command:?}");
        }
    };
    tokio::join!(sender, receiver);
}

#[tokio::test(start_paused = true)]
async fn pause_times_out_when_the_owned_session_never_leaves_playing() {
    let (session, mut requests) = session_channel();
    let receiver = tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let _ = request.reply.send(Ok(json!({
                "type":"MEDIA_STATUS", "status":[{"mediaSessionId":7, "playerState":"PLAYING"}]
            })));
        }
    });
    let started = Instant::now();
    assert_eq!(
        session.command(7, Command::Pause).await,
        Err(Error::Unknown)
    );
    assert_eq!(started.elapsed(), DEADLINE);
    drop(session);
    receiver.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn receiver_status_distinguishes_idle_reasons_and_transitional_states() {
    let (session, mut peer, _driver) = session_pair();
    let cases = [
        ("IDLE", "FINISHED", Status::Finished),
        ("IDLE", "INTERRUPTED", Status::Interrupted),
        ("IDLE", "CANCELLED", Status::Canceled),
        ("IDLE", "ERROR", Status::Failed),
        ("IDLE", "", Status::Idle),
        ("BUFFERING", "", Status::Buffering),
        ("NEW_STATE", "", Status::Unknown),
    ];
    let receiver = async {
        for (state, reason, _) in cases {
            let (_, request) = next_request(&mut peer).await;
            answer(
                &mut peer,
                &session.transport,
                json!({"type":"MEDIA_STATUS", "requestId":request["requestId"], "status":[
                    {"mediaSessionId":8, "playerState":"PLAYING"},
                    {"mediaSessionId":7, "playerState":state, "idleReason":reason}
                ]}),
            )
            .await;
        }
    };
    let sender = async {
        for (_, _, expected) in cases {
            assert_eq!(session.status(7).await, Ok(expected));
        }
    };
    tokio::join!(sender, receiver);
}

#[tokio::test(start_paused = true)]
async fn malformed_media_does_not_prove_ownership_is_absent() {
    let (session, mut peer, _driver) = session_pair();
    let url = "http://example.test/generated/index.m3u8";
    let receiver = async {
        let other_media =
            json!([{"mediaSessionId":11, "media":{"contentId":"http://example.test/other"}}]);
        let unidentified = json!({"mediaSessionId":7, "playerState":"PLAYING"});
        for (verb, status) in [
            ("LOAD", other_media.clone()),
            ("GET_STATUS", json!([{"media":{"contentId":url}}])),
            ("GET_STATUS", json!([unidentified.clone()])),
            (
                "GET_STATUS",
                json!([unidentified, {"mediaSessionId":8, "media":{"contentId":url}}]),
            ),
            ("GET_STATUS", other_media),
            ("GET_STATUS", json!([])),
        ] {
            let (_, request) = next_request(&mut peer).await;
            assert_eq!(request["type"], verb);
            answer(
                &mut peer,
                &session.transport,
                json!({"type":"MEDIA_STATUS", "requestId":request["requestId"], "status":status}),
            )
            .await;
        }
    };
    let sender = async {
        assert_eq!(session.load(url).await, Err(Error::Unknown));
        assert_eq!(session.lookup(url).await, Err(Error::Unknown));
        assert_eq!(session.lookup(url).await, Err(Error::Unknown));
        assert_eq!(session.lookup(url).await, Ok(Some(8)));
        assert_eq!(session.lookup(url).await, Ok(None));
        assert_eq!(session.lookup(url).await, Ok(None));
    };
    tokio::join!(sender, receiver);
}
