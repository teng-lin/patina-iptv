//! Generated loopback receiver for the sender's public connection and command paths.
use super::*;
use std::sync::Arc;
use tokio::{io::AsyncWriteExt, net::TcpListener};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{self, SupportedProtocolVersion},
};

const TRANSPORT: &str = "generated-receiver";
const MEDIA_ID: i64 = 13;
const URL: &str = "http://example.test/generated/index.m3u8";

async fn receiver(
    version: &'static SupportedProtocolVersion,
    launched: bool,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[version])
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        cert.key_pair.serialize_der().try_into().unwrap(),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = acceptor.accept(socket).await.unwrap();
        let mut running = launched;
        let mut root_connected = false;
        let mut media_connected = false;
        let mut expected_volume = [
            json!({"level":0.45}),
            json!({"level":0.5}),
            json!({"muted":true}),
        ]
        .into_iter();
        let mut state = None;
        let mut volume = json!({"level":0.5, "muted":false});
        while let Ok(len) = socket.read_u32().await {
            let mut buf = vec![0; len as usize];
            socket.read_exact(&mut buf).await.unwrap();
            let request = Envelope::decode(buf.as_slice()).unwrap();
            let body: Value = serde_json::from_str(request.message.as_ref().unwrap()).unwrap();
            let kind = body["type"].as_str().unwrap();
            let mut reply = match request.namespace.as_str() {
                HEARTBEAT => json!({"type":"PONG"}),
                CONNECTION => {
                    assert_eq!(body, json!({"type":"CONNECT", "origin":{}}));
                    match request.destination.as_str() {
                        ROOT => root_connected = true,
                        TRANSPORT if root_connected && running => media_connected = true,
                        other => panic!("unexpected connection to {other}"),
                    }
                    continue;
                }
                RECEIVER => {
                    assert!(root_connected && request.destination == ROOT);
                    match kind {
                        "LAUNCH" => {
                            assert!(!running, "rejoin relaunched the active receiver");
                            assert_eq!(body["appId"], APP);
                            running = true;
                        }
                        "SET_VOLUME" => {
                            assert_eq!(expected_volume.next().as_ref(), Some(&body["volume"]));
                            for (key, value) in body["volume"].as_object().unwrap() {
                                volume[key] = value.clone();
                            }
                        }
                        "GET_STATUS" => {}
                        _ => panic!("unexpected receiver request {kind}"),
                    }
                    let apps = if running {
                        json!([{"appId":APP, "transportId":TRANSPORT}])
                    } else {
                        json!([])
                    };
                    json!({"type":"RECEIVER_STATUS","status":{"applications":apps,"volume":volume}})
                }
                MEDIA => {
                    assert!(media_connected && request.destination == TRANSPORT);
                    match kind {
                        "LOAD" => {
                            assert_eq!(body["media"]["contentId"], URL);
                            state = Some("PLAYING");
                        }
                        "PAUSE" | "PLAY" | "STOP" => {
                            assert_eq!(body["mediaSessionId"], MEDIA_ID);
                            if kind == "STOP" {
                                assert!(expected_volume.next().is_none());
                            }
                            state = match kind {
                                "PAUSE" => Some("PAUSED"),
                                "PLAY" => Some("PLAYING"),
                                _ => None,
                            };
                        }
                        "GET_STATUS" => {}
                        _ => panic!("unexpected media request {kind}"),
                    }
                    let entries = state.map(|s| json!({"mediaSessionId":MEDIA_ID,"media":{"contentId":URL},"playerState":s}));
                    json!({"type":"MEDIA_STATUS","status":entries.into_iter().collect::<Vec<_>>()})
                }
                _ => panic!("unexpected namespace"),
            };
            if let Some(id) = body.get("requestId") {
                reply["requestId"] = id.clone();
            }
            let frame = Envelope {
                version: 0,
                source: request.destination,
                destination: request.source,
                namespace: request.namespace,
                kind: 0,
                message: Some(reply.to_string()),
            }
            .encode_to_vec();
            socket.write_u32(frame.len() as u32).await.unwrap();
            socket.write_all(&frame).await.unwrap();
        }
    });
    (address, task)
}

#[tokio::test]
async fn live_controls_use_the_owned_session_on_both_tls_versions() {
    for (version, launched) in [
        (&rustls::version::TLS12, false),
        (&rustls::version::TLS12, true),
        (&rustls::version::TLS13, false),
        (&rustls::version::TLS13, true),
    ] {
        let (address, server) = receiver(version, launched).await;
        let session = if launched {
            Session::rejoin(address).await
        } else {
            Session::connect(address).await
        }
        .unwrap();
        assert!(session.local_ip().is_loopback());
        assert_eq!(session.lookup(URL).await, Ok(None));
        let id = session.load(URL).await.unwrap();
        assert_eq!(session.lookup(URL).await, Ok(Some(id)));
        assert_eq!(session.status(id).await, Ok(Status::Playing));
        session.command(id, Command::Pause).await.unwrap();
        assert_eq!(session.status(id).await, Ok(Status::Paused));
        for command in [
            Command::Volume(45),
            Command::StepVolume(5),
            Command::ToggleMute,
            Command::Play,
        ] {
            session.command(id, command).await.unwrap();
        }
        session.command(id, Command::Stop).await.unwrap();
        assert_eq!(session.status(id).await, Err(Error::Ended));
        drop(session);
        timeout(Duration::from_secs(5), server)
            .await
            .expect("sender closes the control socket")
            .unwrap();
    }
}

#[tokio::test]
async fn rejoining_an_empty_receiver_leaves_it_idle() {
    let (address, server) = receiver(&rustls::version::TLS13, false).await;
    assert!(matches!(Session::rejoin(address).await, Err(Error::Ended)));
    timeout(Duration::from_secs(5), server)
        .await
        .expect("sender closes the control socket")
        .unwrap();
}

#[tokio::test]
async fn a_peer_that_closes_during_tls_never_opens_a_cast_session() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let disconnect = async {
        drop(listener.accept().await.unwrap());
    };
    let (result, ()) = tokio::join!(Session::connect(address), disconnect);
    assert!(matches!(result, Err(Error::Unreachable)));
}

#[tokio::test]
async fn connecting_to_a_running_receiver_reuses_its_transport_without_relaunching() {
    let (address, server) = receiver(&rustls::version::TLS13, true).await;
    let session = Session::connect(address).await.unwrap();
    assert_eq!(session.lookup(URL).await, Ok(None));
    drop(session);
    timeout(Duration::from_secs(5), server)
        .await
        .expect("sender closes the control socket")
        .unwrap();
}
