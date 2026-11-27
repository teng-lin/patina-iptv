//! A loopback receiver that checks the Cast wire protocol independently of the sender.
use prost::Message;
use serde_json::{Value, json};
use std::{io::ErrorKind, net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{self, SupportedProtocolVersion},
};

const TRANSPORT: &str = "generated-receiver";
const ROOT: &str = "receiver-0";
const APP: &str = "CC1AD845";
const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const MEDIA: &str = "urn:x-cast:com.google.cast.media";
const CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const MEDIA_ID: i64 = 13;
pub(super) const URL: &str = "http://example.test/generated/index.m3u8";

#[derive(Clone, PartialEq, Message)]
struct Envelope {
    #[prost(int32, required, tag = "1")]
    version: i32,
    #[prost(string, required, tag = "2")]
    source: String,
    #[prost(string, required, tag = "3")]
    destination: String,
    #[prost(string, required, tag = "4")]
    namespace: String,
    #[prost(int32, required, tag = "5")]
    kind: i32,
    #[prost(string, optional, tag = "6")]
    message: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AppState {
    Running,
    AwaitingLaunch,
    MustStayIdle,
}

pub(super) async fn start(
    version: &'static SupportedProtocolVersion,
    initial: AppState,
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
        let mut running = initial == AppState::Running;
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
            assert_eq!(request.version, 0);
            assert_eq!(request.kind, 0, "expected a UTF-8 Cast message");
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
                            assert!(
                                initial == AppState::AwaitingLaunch && !running,
                                "unexpected LAUNCH"
                            );
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
            let heartbeat = request.namespace == HEARTBEAT;
            let frame = Envelope {
                version: 0,
                source: request.destination,
                destination: request.source,
                namespace: request.namespace,
                kind: 0,
                message: Some(reply.to_string()),
            }
            .encode_to_vec();
            let sent = async {
                socket.write_u32(frame.len() as u32).await?;
                socket.write_all(&frame).await
            }
            .await;
            // A PONG may race the client's close after it consumes a status reply.
            if heartbeat
                && sent.as_ref().is_err_and(|error| {
                    matches!(
                        error.kind(),
                        ErrorKind::ConnectionAborted
                            | ErrorKind::ConnectionReset
                            | ErrorKind::BrokenPipe
                    )
                })
            {
                break;
            }
            sent.unwrap();
        }
    });
    (address, task)
}
