//! Isolated Cast receiver and discovery for synthetic integration scenarios.
use crate::{
    Cast, Delivery, Error, ErrorKind, Stage,
    googlecast::{APP, CONNECTION, HEARTBEAT, MEDIA, RECEIVER, adapter::GoogleCast},
    wire::{Envelope, MAX_FRAME, send},
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_rustls::{TlsAcceptor, rustls};

const TRANSPORT: &str = "generated-receiver";
const MEDIA_ID: i64 = 13;

/// Resolve a generated endpoint through the production Google Cast adapter.
pub fn sender(address: SocketAddr) -> Result<Cast, Error> {
    if !address.ip().is_loopback() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            Stage::Configuration,
            Delivery::NotSent,
        ));
    }
    Cast::builder().build_with(vec![Arc::new(GoogleCast {
        loopback: Some(address),
    })])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    Immediate,
    FetchRelay,
    Refuse,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Load,
    FetchedRelay,
    Playing,
    Paused,
    Stop,
}

pub struct Receiver {
    address: SocketAddr,
    events: Arc<Mutex<Vec<Event>>>,
    driver: Option<JoinHandle<io::Result<()>>>,
}

impl Receiver {
    pub async fn start(load: Load, certificate: Vec<u8>, private_key: Vec<u8>) -> Self {
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.into()], private_key.try_into().unwrap())
        .unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observations = events.clone();
        let driver = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let socket = TlsAcceptor::from(Arc::new(config)).accept(socket).await?;
            ReceiverState::new(load, observations).serve(socket).await
        });
        Self {
            address,
            events,
            driver: Some(driver),
        }
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
    pub async fn finish(mut self) -> io::Result<()> {
        self.driver
            .take()
            .unwrap()
            .await
            .map_err(io::Error::other)?
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
        }
    }
}

struct ReceiverState {
    load: Load,
    events: Arc<Mutex<Vec<Event>>>,
    running: bool,
    state: Option<&'static str>,
    media: Value,
    volume: Value,
}

impl ReceiverState {
    fn new(load: Load, events: Arc<Mutex<Vec<Event>>>) -> Self {
        Self {
            load,
            events,
            running: false,
            state: None,
            media: Value::Null,
            volume: json!({"level":0.5,"muted":false}),
        }
    }

    async fn serve(mut self, mut socket: impl AsyncRead + AsyncWrite + Unpin) -> io::Result<()> {
        while let Ok(length) = socket.read_u32().await {
            if length as usize > MAX_FRAME {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut bytes = vec![0; length as usize];
            socket.read_exact(&mut bytes).await?;
            let envelope = Envelope::decode(bytes.as_slice()).map_err(io::Error::other)?;
            if envelope.namespace == CONNECTION {
                continue;
            }
            let request: Value = serde_json::from_str(envelope.message.as_deref().unwrap_or(""))
                .map_err(io::Error::other)?;
            let mut response = match envelope.namespace.as_str() {
                MEDIA => self.media(request.clone()).await?,
                RECEIVER => self.receiver(&request),
                HEARTBEAT => json!({"type":"PONG"}),
                _ => return Err(io::ErrorKind::InvalidData.into()),
            };
            if let Some(id) = request.get("requestId") {
                response["requestId"] = id.clone();
            }
            let sent = send(
                &mut socket,
                &envelope.destination,
                &envelope.namespace,
                &envelope.source,
                response,
            )
            .await;
            // A PONG can race a close after the sender consumed its status.
            if envelope.namespace == HEARTBEAT
                && sent.as_ref().is_err_and(|error| {
                    matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                    )
                })
            {
                break;
            }
            sent?;
        }
        Ok(())
    }

    fn receiver(&mut self, request: &Value) -> Value {
        match request["type"].as_str() {
            Some("LAUNCH") => {
                assert_eq!(request["appId"], APP);
                self.running = true;
            }
            Some("SET_VOLUME") => {
                for (key, value) in request["volume"].as_object().unwrap() {
                    self.volume[key] = value.clone();
                }
            }
            Some("GET_STATUS") => {}
            _ => panic!("unexpected receiver request"),
        }
        let applications = if self.running {
            json!([{"appId":APP,"transportId":TRANSPORT}])
        } else {
            json!([])
        };
        json!({"type":"RECEIVER_STATUS","status":{"applications":applications,"volume":self.volume}})
    }

    async fn media(&mut self, request: Value) -> io::Result<Value> {
        assert!(self.running);
        match request["type"].as_str() {
            Some("LOAD") => {
                self.note(Event::Load);
                if self.load == Load::Refuse {
                    return Ok(json!({"type":"LOAD_FAILED"}));
                }
                self.media = request["media"].clone();
                if self.load == Load::FetchRelay {
                    fetch_relay(self.media["contentId"].as_str().unwrap()).await?;
                    self.note(Event::FetchedRelay);
                }
                self.state = Some("PLAYING");
            }
            Some("PLAY") => {
                assert_eq!(request["mediaSessionId"], MEDIA_ID);
                self.note(Event::Playing);
                self.state = Some("PLAYING");
            }
            Some("PAUSE") => {
                assert_eq!(request["mediaSessionId"], MEDIA_ID);
                self.note(Event::Paused);
                self.state = Some("PAUSED");
            }
            Some("STOP") => {
                assert_eq!(request["mediaSessionId"], MEDIA_ID);
                self.note(Event::Stop);
                self.state = None;
            }
            Some("GET_STATUS") => {}
            _ => panic!("unexpected media request"),
        }
        let entry = self.state.map(|state| {
            json!({
                "mediaSessionId":MEDIA_ID, "playerState":state, "media":self.media,
                "currentTime":4.0, "supportedMediaCommands":1,
            })
        });
        Ok(json!({"type":"MEDIA_STATUS","status":entry.into_iter().collect::<Vec<_>>()}))
    }

    fn note(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn reply_after_peer_close(namespace: &str, kind: &str) -> io::Result<()> {
        let (mut peer, socket) = tokio::io::duplex(1024);
        send(
            &mut peer,
            "sender-0",
            namespace,
            "receiver-0",
            json!({"type": kind}),
        )
        .await?;
        drop(peer);
        ReceiverState::new(Load::Immediate, Default::default())
            .serve(socket)
            .await
    }

    #[tokio::test]
    async fn a_peer_can_close_before_the_optional_pong_reply() {
        reply_after_peer_close(HEARTBEAT, "PING").await.unwrap();
    }

    #[tokio::test]
    async fn a_peer_close_still_fails_a_required_status_reply() {
        let error = reply_after_peer_close(RECEIVER, "GET_STATUS")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}

async fn fetch_relay(url: &str) -> io::Result<()> {
    let url = url::Url::parse(url).map_err(io::Error::other)?;
    let address = SocketAddr::new(
        url.host_str()
            .unwrap_or("")
            .parse()
            .map_err(io::Error::other)?,
        url.port().ok_or(io::ErrorKind::InvalidInput)?,
    );
    if !address.ip().is_loopback() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut socket = TcpStream::connect(address).await?;
    socket
        .write_all(
            format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                url.path(),
                address
            )
            .as_bytes(),
        )
        .await?;
    let mut reply = Vec::new();
    socket.read_to_end(&mut reply).await?;
    if !reply.starts_with(b"HTTP/1.1 200") || !reply.windows(7).any(|line| line == b"#EXTM3U") {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(())
}
