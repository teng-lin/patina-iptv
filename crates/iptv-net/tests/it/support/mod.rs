//! A tiny local HTTP/1.1 (and HTTPS) test server on tokio. No internet.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

#[path = "../../../../iptv-import/tests/it/common/synth.rs"]
pub mod synth;

use synth::SimRequest;

/// What the handler answers.
#[derive(Clone, Debug, Default)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Send the body with chunked transfer encoding in these pieces,
    /// pausing between them.
    pub chunks: Option<(Vec<Vec<u8>>, Duration)>,
    /// Sleep before sending anything.
    pub delay: Option<Duration>,
    /// Simulate an upstream disconnect before any response headers.
    pub close_without_response: bool,
}

impl Resp {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            ..Default::default()
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            ..Default::default()
        }
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self::status(status).header("Location", location)
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn chunked(mut self, pieces: Vec<Vec<u8>>, pause: Duration) -> Self {
        self.chunks = Some((pieces, pause));
        self
    }

    pub fn delayed(mut self, d: Duration) -> Self {
        self.delay = Some(d);
        self
    }

    pub fn disconnect() -> Self {
        Self {
            close_without_response: true,
            ..Self::default()
        }
    }
}

impl From<synth::SimResponse> for Resp {
    fn from(r: synth::SimResponse) -> Self {
        Resp {
            status: r.status,
            body: r.body,
            ..Default::default()
        }
    }
}

type Handler = Arc<dyn Fn(&SimRequest) -> Resp + Send + Sync>;

pub struct TestServer {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<SimRequest>>>,
    pub tls: bool,
}

impl TestServer {
    pub async fn http(handler: impl Fn(&SimRequest) -> Resp + Send + Sync + 'static) -> Self {
        Self::start(Arc::new(handler), None).await
    }

    /// HTTPS with the given certificate chain and key (PEM via rcgen).
    pub async fn https(
        handler: impl Fn(&SimRequest) -> Resp + Send + Sync + 'static,
        acceptor: tokio_rustls::TlsAcceptor,
    ) -> Self {
        Self::start(Arc::new(handler), Some(acceptor)).await
    }

    async fn start(handler: Handler, tls: Option<tokio_rustls::TlsAcceptor>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let is_tls = tls.is_some();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (handler, log, tls) = (handler.clone(), log.clone(), tls.clone());
                tokio::spawn(async move {
                    match tls {
                        Some(acceptor) => {
                            if let Ok(s) = acceptor.accept(stream).await {
                                serve(s, handler, log).await;
                            }
                        }
                        None => serve(stream, handler, log).await,
                    }
                });
            }
        });
        Self {
            addr,
            requests,
            tls: is_tls,
        }
    }

    pub fn url(&self, path: &str) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        // `localhost` so the rcgen certificate's name matches.
        format!("{scheme}://localhost:{}{path}", self.addr.port())
    }

    pub fn requests(&self) -> Vec<SimRequest> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    handler: Handler,
    log: Arc<Mutex<Vec<SimRequest>>>,
) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        match s.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let target = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let req = SimRequest::parse(&target, headers);
    let resp = handler(&req);
    log.lock().unwrap().push(req);
    if resp.close_without_response {
        return;
    }
    if let Some(d) = resp.delay {
        tokio::time::sleep(d).await;
    }
    let mut out = format!("HTTP/1.1 {} X\r\nConnection: close\r\n", resp.status);
    for (k, v) in &resp.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    match &resp.chunks {
        None => {
            out.push_str(&format!("Content-Length: {}\r\n\r\n", resp.body.len()));
            let _ = s.write_all(out.as_bytes()).await;
            let _ = s.write_all(&resp.body).await;
        }
        Some((pieces, pause)) => {
            out.push_str("Transfer-Encoding: chunked\r\n\r\n");
            let _ = s.write_all(out.as_bytes()).await;
            for p in pieces {
                let _ = s.write_all(format!("{:x}\r\n", p.len()).as_bytes()).await;
                let _ = s.write_all(p).await;
                let _ = s.write_all(b"\r\n").await;
                let _ = s.flush().await;
                tokio::time::sleep(*pause).await;
            }
            let _ = s.write_all(b"0\r\n\r\n").await;
        }
    }
    let _ = s.flush().await;
    let _ = s.shutdown().await;
}

/// A self-signed certificate for `localhost`: (acceptor, DER for the client).
pub fn self_signed() -> (tokio_rustls::TlsAcceptor, Vec<u8>) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let der = cert.cert.der().to_vec();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![der.clone().into()], key)
    .unwrap();
    (tokio_rustls::TlsAcceptor::from(Arc::new(config)), der)
}
