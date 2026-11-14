//! A per-player loopback transport. Every upstream fetch, including redirects
//! and HLS/DASH subresources, scopes the stream's headers to the original
//! stream's origin: another origin gets only what [`crate::cross_origin`]
//! allows. Players receive no identity headers. Media bodies stream; only
//! manifests are buffered (at most 2 MiB). Dropping the owner cancels every
//! open fetch.
//!
//! The relay serves only requests that name its capability, a random path.
//! A peer has a second to send it ([`UNAUTHENTICATED`]) and is cut off as
//! soon as its first bytes cannot become such a request ([`admit`]).
//! Absolute-form HTTP requests are admitted only when they name this
//! relay and its capability. This does not constrain requests a player
//! makes directly; manifest rewriting remains the native load boundary.

use std::{
    io,
    net::TcpListener,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use reqwest::{
    Url,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{Semaphore, oneshot},
};

use crate::{
    FetchError, FetchErrorKind, cross_origin,
    redirect::{self, Prepared},
};

const MAX_MANIFEST: usize = 2 * 1024 * 1024;
const MAX_REQUEST: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 12;
/// How long a connection may hold one of the [`MAX_CONNECTIONS`] slots
/// before it has shown the capability. The player is on this machine and
/// sends its request at once; any other local process gets no more.
const UNAUTHENTICATED: Duration = Duration::from_secs(1);
/// How long the player has for the whole request head.
const REQUEST_HEAD: Duration = Duration::from_secs(15);

/// Provenance of the original playable URL. A trusted URL authorizes only its
/// exact origin; derived or redirected resources still require public hosts.
#[derive(Clone)]
pub struct PlaybackPolicy {
    pub public_only: bool,
    /// DER certificates trusted in addition to the Mozilla roots: the
    /// client's [`crate::NetConfig::extra_roots_der`]. A server whose
    /// certificate needs one of them imports; without them here it would
    /// then fail to play.
    pub extra_roots_der: Vec<Vec<u8>>,
    #[cfg(debug_assertions)]
    pub allow_private_fixture: bool,
}
impl Default for PlaybackPolicy {
    fn default() -> Self {
        Self {
            public_only: true,
            extra_roots_der: Vec::new(),
            #[cfg(debug_assertions)]
            allow_private_fixture: false,
        }
    }
}

/// Owns the lifetime of one stream and all of its adaptive subresources.
/// Its URL is a random capability, never logged or persisted.
pub struct PlaybackRelay {
    url: String,
    prefix: String,
    source_url: String,
    url_auth: Option<HeaderValue>,
    cookies: Arc<OriginCookies>,
    stop: Option<oneshot::Sender<()>>,
    headers: Arc<Mutex<HeaderMap>>,
    error: Arc<Mutex<Option<FetchErrorKind>>>,
}

struct Session {
    prefix: String,
    /// `http://127.0.0.1:<port>`: how a request names this relay when the
    /// player uses it as its proxy.
    proxy: String,
    /// `/<token>/`: the capability, as a request names it.
    path: String,
    root: Url,
    headers: Arc<Mutex<HeaderMap>>,
    error: Arc<Mutex<Option<FetchErrorKind>>>,
    client: reqwest::Client,
    public_client: reqwest::Client,
    policy: PlaybackPolicy,
}

/// Cookie persistence is restricted to the exact stream origin, not merely
/// its domain (ordinary cookie jars intentionally ignore ports).
#[derive(Debug)]
struct OriginCookies {
    origin: url::Origin,
    jar: Mutex<cookie_store::CookieStore>,
    seeded: Mutex<Vec<String>>,
}
impl reqwest::cookie::CookieStore for OriginCookies {
    fn set_cookies(&self, cookies: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        if url.origin() == self.origin {
            for cookie in cookies {
                if let Ok(value) = cookie.to_str() {
                    self.insert(value, url);
                }
            }
        }
    }
    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        if self.seeded.is_poisoned() {
            return None;
        }
        if url.origin() == self.origin {
            let jar = self.jar.lock().ok()?;
            let value = jar
                .get_request_values(url)
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            (!value.is_empty())
                .then(|| HeaderValue::from_str(&value).ok())
                .flatten()
        } else {
            None
        }
    }
}
impl OriginCookies {
    fn insert(&self, value: &str, url: &Url) {
        // Preserve existing sessions when a server floods distinct cookies.
        // A candidate update commits only within both storage budgets.
        if value.len() > 4096 {
            return;
        }
        let Ok(mut jar) = self.jar.lock() else {
            return;
        };
        let mut candidate = cookie_store::CookieStore::from_cookies(
            jar.iter_unexpired()
                .cloned()
                .map(Ok::<_, std::convert::Infallible>),
            false,
        )
        .expect("infallible cookie iterator");
        if candidate.parse(value, url).is_err() {
            return;
        }
        let count = candidate.iter_any().count();
        let bytes: usize = candidate
            .iter_any()
            .map(|cookie| cookie.to_string().len())
            .sum();
        if count <= 64 && bytes <= 16 * 1024 {
            *jar = candidate;
        }
    }

    fn seed(&self, fields: &mut HeaderMap, root: &Url) {
        if let Ok(mut seeded) = self.seeded.lock()
            && let Ok(mut jar) = self.jar.lock()
        {
            for name in seeded.drain(..) {
                jar.remove(root.host_str().unwrap_or_default(), "/", &name);
            }
        }
        let Some(value) = fields
            .get("cookie")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
        else {
            return;
        };
        if value
            .split(';')
            .map(str::trim)
            .filter(|pair| !pair.is_empty())
            .any(|pair| !pair.contains('='))
        {
            return;
        }
        fields.remove("cookie");
        for pair in value
            .split(';')
            .map(str::trim)
            .filter(|pair| !pair.is_empty())
        {
            self.insert(&format!("{pair}; Path=/"), root);
            if let Some((name, _)) = pair.split_once('=')
                && let Ok(mut seeded) = self.seeded.lock()
            {
                seeded.push(name.trim().to_owned());
            }
        }
    }
}

fn invalid(message: &'static str) -> FetchError {
    FetchError::new(FetchErrorKind::InvalidRequest, message)
}
fn io_error(_: impl std::fmt::Display) -> FetchError {
    FetchError::new(FetchErrorKind::Io, "Playback transport unavailable")
}

impl PlaybackRelay {
    pub fn start(url: &str, headers: &[(String, String)]) -> Result<Self, FetchError> {
        Self::start_with_policy(url, headers, PlaybackPolicy::default())
    }

    pub fn start_with_policy(
        url: &str,
        headers: &[(String, String)],
        policy: PlaybackPolicy,
    ) -> Result<Self, FetchError> {
        let mut root = Url::parse(url).map_err(|_| invalid("Invalid playback URL"))?;
        if !matches!(root.scheme(), "http" | "https") {
            return Err(invalid("Playback relay requires HTTP or HTTPS"));
        }
        let fields = Arc::new(Mutex::new(parse_headers(headers)?));
        let mut random = [0u8; 24];
        getrandom::fill(&mut random).map_err(io_error)?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(io_error)?;
        listener.set_nonblocking(true).map_err(io_error)?;
        let proxy = format!("http://{}", listener.local_addr().map_err(io_error)?);
        let path = format!("/{token}/");
        let prefix = format!("{proxy}{path}");
        let cookies = Arc::new(OriginCookies {
            origin: root.origin(),
            jar: Mutex::new(cookie_store::CookieStore::default()),
            seeded: Mutex::new(Vec::new()),
        });
        cookies.seed(&mut *fields.lock().map_err(|_| io_error("lock"))?, &root);
        let builder = || -> Result<reqwest::ClientBuilder, FetchError> {
            Ok(reqwest::Client::builder()
                .tls_backend_preconfigured(crate::tls::client_config(&policy.extra_roots_der)?)
                .redirect(reqwest::redirect::Policy::none())
                .referer(false)
                .cookie_provider(cookies.clone())
                .no_proxy()
                .connect_timeout(Duration::from_secs(8))
                .read_timeout(Duration::from_secs(15)))
        };
        let client = builder()?.build().map_err(crate::error::from_reqwest)?;
        let public_client = builder()?
            .dns_resolver(Arc::new(crate::PublicResolver))
            .build()
            .map_err(crate::error::from_reqwest)?;
        let request = client
            .get(root.clone())
            .build()
            .map_err(crate::error::from_reqwest)?;
        root = request.url().clone();
        let url_auth = request.headers().get("authorization").cloned();
        if let Some(auth) = &url_auth {
            fields
                .lock()
                .map_err(|_| io_error("lock"))?
                .entry("authorization")
                .or_insert(auth.clone());
        }
        let source_url = url.to_owned();
        let url = map_url(&prefix, &root)?;
        let error = Arc::new(Mutex::new(None));
        let session = Arc::new(Session {
            prefix,
            proxy: proxy.clone(),
            path,
            root,
            headers: fields.clone(),
            client,
            public_client,
            policy,
            error: error.clone(),
        });
        let prefix = session.prefix.clone();
        let (stop, mut stopped) = oneshot::channel();
        std::thread::Builder::new()
            .name("iptv-playback-http".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                rt.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    let mut tasks = tokio::task::JoinSet::new();
                    loop {
                        tokio::select! {
                            _ = &mut stopped => break,
                            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
                            connection = listener.accept(), if tasks.len() < MAX_CONNECTIONS => {
                                let Ok((socket, _)) = connection else {
                                tokio::time::sleep(Duration::from_millis(50)).await;
                                continue;
                            };
                                let session = session.clone();
                                tasks.spawn(async move { let _ = serve(socket, &session).await; });
                            }
                        }
                    }
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                });
            })
            .map_err(io_error)?;
        Ok(Self {
            prefix,
            url,
            source_url,
            url_auth,
            cookies,
            stop: Some(stop),
            headers: fields,
            error,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Retained by the host to recognize canceled loads after retirement.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// A native player's top-level load must stay inside this capability.
    pub fn owns_url(&self, url: &str) -> bool {
        url.starts_with(&self.prefix)
    }

    pub fn source_url(&self) -> &str {
        &self.source_url
    }

    /// A sanitized upstream failure for the native player's existing error UI.
    pub fn take_error_code(&self) -> Option<&'static str> {
        self.error.lock().ok()?.take().map(|kind| match kind {
            FetchErrorKind::Connect => "ERROR_CODE_IO_NETWORK_CONNECTION_FAILED",
            FetchErrorKind::Timeout => "ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT",
            FetchErrorKind::Unsupported => "ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED",
            _ => "ERROR_CODE_IO_UNSPECIFIED",
        })
    }

    pub fn update_headers(&self, headers: &[(String, String)]) -> Result<(), FetchError> {
        let mut fields = parse_headers(headers)?;
        let root = Url::parse(&self.source_url).map_err(|_| invalid("Invalid playback URL"))?;
        self.cookies.seed(&mut fields, &root);
        if let Some(auth) = &self.url_auth {
            fields.entry("authorization").or_insert(auth.clone());
        }
        *self.headers.lock().map_err(|_| io_error("lock"))? = fields;
        Ok(())
    }

    #[cfg(feature = "test-support")]
    /// Key requests for relays whose source has this exact origin. Tests must
    /// retain their listening socket so another test cannot reuse its port.
    pub fn key_requests_for(source: &str) -> usize {
        let origin = Url::parse(source)
            .expect("valid synthetic source URL")
            .origin();
        TEST_KEY_REQUESTS
            .lock()
            .expect("test counters")
            .get(&origin)
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(feature = "test-support")]
static TEST_KEY_REQUESTS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<url::Origin, usize>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

impl Drop for PlaybackRelay {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn parse_headers(headers: &[(String, String)]) -> Result<HeaderMap, FetchError> {
    let mut fields = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| invalid("Invalid playback header"))?;
        let value = HeaderValue::from_bytes(value.as_bytes())
            .map_err(|_| invalid("Invalid playback header"))?;
        if !matches!(
            name.as_str(),
            "host" | "connection" | "content-length" | "transfer-encoding"
        ) {
            fields.insert(name, value);
        }
    }
    Ok(fields)
}

fn map_url(prefix: &str, url: &Url) -> Result<String, FetchError> {
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid("Unsupported playback subresource scheme"));
    }
    // Preserve paths/extensions/templates for player format detection and DASH
    // substitutions. The capability URL is private and must never be logged.
    let mut url = url.clone();
    url.set_fragment(None);
    Ok(format!("{prefix}{}", url.as_str()))
}

async fn serve(mut socket: TcpStream, session: &Session) -> io::Result<()> {
    let request = read_request(&mut socket, session).await?;
    let mut lines = request.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default();
    let path = first.next().unwrap_or_default();
    // `admit` has compared the capability; this only takes it off.
    let path = path.strip_prefix(&session.proxy).unwrap_or(path);
    let Some(target) = path.strip_prefix(&session.path) else {
        return Err(io::Error::other("Invalid local request"));
    };
    let (raw_key, target) = target
        .strip_prefix("key/")
        .map_or((false, target), |url| (true, url));
    #[cfg(feature = "test-support")]
    if raw_key {
        *TEST_KEY_REQUESTS
            .lock()
            .expect("test counters")
            .entry(session.root.origin())
            .or_default() += 1;
    }
    let Ok(target) = Url::parse(target) else {
        return answer(&mut socket, 400, b"Invalid target").await;
    };
    if !matches!(method, "GET" | "HEAD") || !matches!(target.scheme(), "http" | "https") {
        return answer(&mut socket, 400, b"Unsupported request").await;
    }
    if session.root.scheme() == "https" && target.scheme() == "http" {
        return answer(&mut socket, 502, b"Unencrypted playback downgrade refused").await;
    }
    let mut headers = session
        .headers
        .lock()
        .map_err(|_| io::Error::other("Transport unavailable"))?
        .clone();
    if target.origin() != session.root.origin() {
        cross_origin::restrict(&mut headers, &session.root, &target);
    }
    // Only range negotiation comes from the local player; no client can inject
    // replacement identity, Host, proxy or hop-by-hop headers.
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && matches!(name.to_ascii_lowercase().as_str(), "range" | "if-range")
            && let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value.trim()),
            )
        {
            headers.insert(name, value);
        }
    }
    let prepared = Prepared {
        public_only: session.policy.public_only || target.origin() != session.root.origin(),
        #[cfg(debug_assertions)]
        allow_private_fixture: session.policy.allow_private_fixture,
        // HEAD describes the rewritten entity too, so inspect the same GET
        // response and suppress its body only after representation selection.
        method: reqwest::Method::GET,
        url: target,
        headers,
        body: None,
    };
    let mut prepared = prepared;
    let Some(mut response) = fetch(session, &prepared).await else {
        return answer(&mut socket, 502, b"Playback request failed").await;
    };
    let body_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut upstream_length = content_length(&response);
    // HLS key URIs are opaque bytes, even when a valid key is all printable.
    // This context is minted only while rewriting an EXT-X-KEY attribute.
    if raw_key || !response.status().is_success() {
        return stream_response(
            &mut socket,
            response,
            &[],
            upstream_length,
            method == "HEAD",
        )
        .await;
    }
    let mut first = sniff(session, &mut response, body_deadline).await?;
    let mut kind = classify(&first);
    if kind != Manifest::Binary && response.status() == 206 {
        // Rewriting changes byte offsets and length. Fetch the whole manifest
        // and answer 200 rather than forwarding stale upstream range metadata.
        let nonzero_range = prepared
            .headers
            .get("range")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.split_once('-'))
            .and_then(|(start, _)| start.parse::<u64>().ok())
            .is_some_and(|start| start > 0);
        prepared.headers.remove("range");
        prepared.headers.remove("if-range");
        let Some(mut full) = fetch(session, &prepared).await else {
            return answer(&mut socket, 502, b"Playback request failed").await;
        };
        let full_length = content_length(&full);
        let full_first = if full.status().is_success() {
            sniff(session, &mut full, body_deadline).await?
        } else {
            Vec::new()
        };
        if nonzero_range
            && kind == Manifest::Unsupported
            && classify(&full_first) == Manifest::Binary
            && full.status().is_success()
        {
            // An ASCII slice of a binary container is not a manifest. Inspect
            // only its full-entity prefix, then retain the requested range.
            return stream_response(
                &mut socket,
                response,
                &first,
                upstream_length,
                method == "HEAD",
            )
            .await;
        }
        response = full;
        upstream_length = full_length;
        if !response.status().is_success() {
            return stream_response(
                &mut socket,
                response,
                &[],
                upstream_length,
                method == "HEAD",
            )
            .await;
        }
        if response.status() == 206 {
            return answer(&mut socket, 502, b"Partial manifest refused").await;
        }
        first = full_first;
        kind = classify(&first);
    }
    let hls = kind == Manifest::Hls;
    let dash = kind == Manifest::Xml;
    let final_url = response.url().clone();
    if kind == Manifest::Unsupported {
        if let Ok(mut error) = session.error.lock() {
            *error = Some(FetchErrorKind::Unsupported);
        }
        return answer(&mut socket, 415, b"Unsupported playlist format").await;
    }
    if hls || dash {
        // Across all player surfaces, at most two manifests are buffered or
        // being sent. Slow consumers cannot multiply the per-manifest budget.
        static MANIFESTS: OnceLock<Semaphore> = OnceLock::new();
        let _permit = tokio::time::timeout_at(
            body_deadline,
            MANIFESTS.get_or_init(|| Semaphore::new(2)).acquire(),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Manifest deadline"))?
        .map_err(|_| io::Error::other("Playback transport closed"))?;
        let collected = tokio::time::timeout_at(body_deadline, async {
            let mut body = first.to_vec();
            if body.len() > MAX_MANIFEST {
                return Err(io::Error::other("Manifest exceeds limit"));
            }
            while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
                if body.len().saturating_add(chunk.len()) > MAX_MANIFEST {
                    return Err(io::Error::other("Manifest exceeds limit"));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        })
        .await;
        let body = match collected {
            Ok(Ok(body)) => body,
            Ok(Err(_)) => return answer(&mut socket, 502, b"Manifest exceeds limit").await,
            Err(_) => return answer(&mut socket, 504, b"Manifest took too long").await,
        };
        let rewritten = if hls {
            rewrite_hls(
                body.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&body),
                &final_url,
                &session.prefix,
            )
        } else {
            rewrite_dash(&body, &final_url, &session.prefix)
        };
        let Ok(body) = rewritten else {
            if let Ok(mut error) = session.error.lock() {
                *error = Some(FetchErrorKind::Unsupported);
            }
            return answer(&mut socket, 502, b"Unsupported manifest").await;
        };
        let mime = if hls {
            "application/vnd.apple.mpegurl"
        } else {
            "application/dash+xml"
        };
        let head = format!(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        send_bytes(&mut socket, head.as_bytes()).await?;
        if method != "HEAD" {
            send_bytes(&mut socket, &body).await?;
        }
    } else {
        return stream_response(
            &mut socket,
            response,
            &first,
            upstream_length,
            method == "HEAD",
        )
        .await;
    }
    socket.shutdown().await
}

async fn fetch(session: &Session, prepared: &Prepared) -> Option<reqwest::Response> {
    match redirect::execute(
        &session.client,
        &session.public_client,
        prepared.clone(),
        10,
        "playback",
    )
    .await
    {
        Ok(response) => {
            // A completed retry or actual HTTP response supersedes an old
            // transport failure; do not hide a later 403 behind a stale error.
            if let Ok(mut error) = session.error.lock() {
                *error = None;
            }
            Some(response)
        }
        Err(failure) => {
            if let Ok(mut error) = session.error.lock() {
                *error = Some(failure.kind);
            }
            None
        }
    }
}

fn content_length(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .or_else(|| response.content_length())
}

fn response_head(response: &reqwest::Response) -> String {
    let mut head = format!(
        "HTTP/1.1 {} Playback\r\nConnection: close\r\n",
        response.status().as_u16()
    );
    for name in [
        "content-type",
        "content-range",
        "accept-ranges",
        "retry-after",
    ] {
        if let Some(value) = response.headers().get(name).and_then(|v| v.to_str().ok()) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head
}

async fn stream_response(
    socket: &mut TcpStream,
    mut response: reqwest::Response,
    first: &[u8],
    length: Option<u64>,
    head_only: bool,
) -> io::Result<()> {
    let mut head = response_head(&response);
    if let Some(length) = length {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    head.push_str("\r\n");
    send_bytes(socket, head.as_bytes()).await?;
    if head_only {
        return socket.shutdown().await;
    }
    send_bytes(socket, first).await?;
    while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
        send_bytes(socket, &chunk).await?;
    }
    socket.shutdown().await
}

async fn sniff(
    session: &Session,
    response: &mut reqwest::Response,
    deadline: tokio::time::Instant,
) -> io::Result<Vec<u8>> {
    let result = sniff_until(response, deadline).await;
    if result
        .as_ref()
        .is_err_and(|error| error.kind() == io::ErrorKind::TimedOut)
        && let Ok(mut error) = session.error.lock()
    {
        *error = Some(FetchErrorKind::Timeout);
    }
    result
}

async fn sniff_until(
    response: &mut reqwest::Response,
    deadline: tokio::time::Instant,
) -> io::Result<Vec<u8>> {
    tokio::time::timeout_at(deadline, async {
        let mut first = Vec::new();
        while first.len() < 64 {
            match response.chunk().await.map_err(io::Error::other)? {
                Some(chunk) => first.extend_from_slice(&chunk),
                None => break,
            }
        }
        Ok(first)
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Manifest prefix deadline"))?
}

#[derive(PartialEq)]
enum Manifest {
    Hls,
    Xml,
    Binary,
    Unsupported,
}

fn classify(first: &[u8]) -> Manifest {
    let probe = first.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(first);
    let text = String::from_utf8_lossy(probe);
    let text = text.trim_start();
    if text.starts_with("#EXTM3U") {
        return Manifest::Hls;
    }
    let printable = probe
        .iter()
        .all(|byte| *byte >= 0x20 || byte.is_ascii_whitespace());
    if text.starts_with('<') {
        return Manifest::Xml;
    }
    let other_encoding = probe.starts_with(&[0xff, 0xfe])
        || probe.starts_with(&[0xfe, 0xff])
        || probe.starts_with(&[0, b'<'])
        || probe.starts_with(&[b'<', 0])
        || probe.starts_with(&[0, 0, 0, b'<']);
    let unknown_text = !text.starts_with("WEBVTT") && !text.is_empty() && printable;
    if other_encoding || unknown_text {
        Manifest::Unsupported
    } else {
        Manifest::Binary
    }
}

async fn send_bytes(socket: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), socket.write_all(bytes)).await?
}

/// What the bytes received so far say about a connection.
#[derive(Debug, PartialEq)]
enum Admission {
    /// A request for this relay's capability.
    Admitted,
    /// Too few bytes to tell.
    Undecided,
    /// These bytes cannot become such a request.
    Refused,
}

/// `bytes` after `literal`; `Err(true)` when `bytes` is still a beginning
/// of `literal`, `Err(false)` when it differs.
fn after<'a>(bytes: &'a [u8], literal: &[u8]) -> Result<&'a [u8], bool> {
    match bytes.strip_prefix(literal) {
        Some(rest) => Ok(rest),
        None => Err(literal.starts_with(bytes)),
    }
}

/// Whether `a` equals `b`, in a time that depends on their length alone.
fn same(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    bool::from(a.ct_eq(b))
}

/// Whether `head`, the first bytes of a connection, begins a request for
/// the capability: `GET` or `HEAD`, then the capability path `capability`
/// (`/<token>/`), alone or behind this relay's own address `proxy`
/// (`http://127.0.0.1:<port>`: a player using it as its proxy).
///
/// Nothing but the capability is secret, so everything before it is
/// compared as it arrives and a connection that starts otherwise (another
/// method, another host, a TLS handshake) is refused at once. The
/// capability is compared in one piece once all of it is there and in
/// constant time: neither a partial token nor the time of a refusal says
/// how much of a guess was right.
fn admit(head: &[u8], proxy: &str, capability: &str) -> Admission {
    let target = match (after(head, b"GET "), after(head, b"HEAD ")) {
        (Ok(target), _) | (_, Ok(target)) => target,
        (Err(true), _) | (_, Err(true)) => return Admission::Undecided,
        _ => return Admission::Refused,
    };
    let path = if target.first().is_none_or(|byte| *byte == b'/') {
        target
    } else {
        match after(target, proxy.as_bytes()) {
            Ok(path) => path,
            Err(true) => return Admission::Undecided,
            Err(false) => return Admission::Refused,
        }
    };
    // After the address comes a path (`:43210` is another port).
    if path.first().is_some_and(|byte| *byte != b'/') {
        return Admission::Refused;
    }
    let capability = capability.as_bytes();
    match path.get(..capability.len()) {
        Some(shown) if same(shown, capability) => Admission::Admitted,
        Some(_) => Admission::Refused,
        // A request line that ends before the capability could.
        None if path.iter().any(|byte| b" \r\n".contains(byte)) => Admission::Refused,
        None => Admission::Undecided,
    }
}

/// The request head of a connection that asks for the capability
/// ([`admit`]); an error for every other, which closes it unanswered.
async fn read_request(socket: &mut TcpStream, session: &Session) -> io::Result<String> {
    let refused = || io::Error::other("Invalid local request");
    let accepted = tokio::time::Instant::now();
    let mut admitted = false;
    let mut bytes = Vec::new();
    let mut buf = [0; 1024];
    loop {
        if !admitted {
            match admit(&bytes, &session.proxy, &session.path) {
                Admission::Admitted => admitted = true,
                Admission::Undecided => {}
                Admission::Refused => return Err(refused()),
            }
        }
        if admitted && bytes.windows(4).any(|v| v == b"\r\n\r\n") {
            break;
        }
        let allowed = if admitted {
            REQUEST_HEAD
        } else {
            UNAUTHENTICATED
        };
        let n = tokio::time::timeout_at(accepted + allowed, socket.read(&mut buf)).await??;
        if n == 0 || bytes.len() + n > MAX_REQUEST {
            return Err(refused());
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    String::from_utf8(bytes).map_err(io::Error::other)
}

async fn answer(socket: &mut TcpStream, status: u16, body: &[u8]) -> io::Result<()> {
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status} Error\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    socket.write_all(body).await
}

fn resolve(prefix: &str, base: &Url, value: &str) -> Result<String, FetchError> {
    let target = base
        .join(value)
        .map_err(|_| invalid("Invalid manifest URL"))?;
    if base.scheme() == "https" && target.scheme() == "http" {
        return Err(invalid("Unencrypted playback downgrade refused"));
    }
    map_url(prefix, &target)
}

mod hls;
use hls::rewrite_hls;

mod dash;
use dash::rewrite_dash;

/// The manifest rewriters as `fuzz/fuzz_targets/manifests.rs` calls them.
/// Compiled only with the `fuzzing` feature, which the fuzz crate alone
/// enables; not an API.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    use super::Url;

    /// The rewritten playlist, or `None` when `base` is not a URL or the
    /// playlist is refused.
    pub fn rewrite_hls(body: &[u8], base: &str, prefix: &str) -> Option<Vec<u8>> {
        super::rewrite_hls(body, &Url::parse(base).ok()?, prefix).ok()
    }

    /// As [`rewrite_hls`], for a DASH manifest.
    pub fn rewrite_dash(body: &[u8], base: &str, prefix: &str) -> Option<Vec<u8>> {
        super::rewrite_dash(body, &Url::parse(base).ok()?, prefix).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xml_and_legacy_text_cannot_fall_through_as_binary() {
        for bytes in [
            b"<MPD><!--\x7f--></MPD>".as_slice(),
            b"<MPD><!--\xc3",
            b"<?xml encoding=\"ISO-8859-1\"?><MPD>\xe9",
        ] {
            assert!(classify(bytes) == Manifest::Xml);
        }
        assert!(classify(b"[playlist]\nTitle1=Caf\xe9") == Manifest::Unsupported);
    }

    #[test]
    fn header_octets_preserve_unicode_but_reject_line_injection() {
        let fields = parse_headers(&[("X-User-Agent".into(), "Synthetic Cañal".into())]).unwrap();
        assert_eq!(
            fields["x-user-agent"].as_bytes(),
            "Synthetic Cañal".as_bytes()
        );
        assert!(parse_headers(&[("Cookie".into(), "a=1\r\nInjected: yes".into())]).is_err());
    }

    #[test]
    fn secure_manifests_cannot_introduce_cleartext_resources() {
        let base = Url::parse("https://synthetic.example.test/main.m3u8").unwrap();
        for body in [
            "#EXTM3U\nhttp://other.example.test/media.ts\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"http://other.example.test/key\"\n",
        ] {
            assert!(rewrite_hls(body.as_bytes(), &base, "http://127.0.0.1:1/cap/").is_err());
        }
        assert!(
            rewrite_dash(
                br#"<MPD><BaseURL>http://other.example.test/</BaseURL></MPD>"#,
                &base,
                "http://127.0.0.1:1/cap/"
            )
            .is_err()
        );
    }
    const PROXY: &str = "http://127.0.0.1:4321";
    const CAPABILITY: &str = "/0123456789abcdef0123456789abcdef0123456789abcdef/";

    fn admitted(head: &str) -> Admission {
        admit(head.as_bytes(), PROXY, CAPABILITY)
    }

    #[test]
    fn only_a_request_for_the_capability_is_admitted() {
        for method in ["GET", "HEAD"] {
            // As a player asks, and as one that uses the relay as its proxy.
            for target in [CAPABILITY.to_owned(), format!("{PROXY}{CAPABILITY}")] {
                let line = format!("{method} {target}http://media.example.test/a.ts HTTP/1.1\r\n");
                assert_eq!(admitted(&line), Admission::Admitted, "{line}");
                // Decided with the capability's last byte, not before.
                let shown = format!("{method} {target}");
                assert_eq!(admitted(&shown), Admission::Admitted);
                for cut in 0..shown.len() {
                    assert_eq!(admitted(&shown[..cut]), Admission::Undecided, "{cut}");
                }
            }
        }
    }

    #[test]
    fn first_bytes_that_cannot_ask_for_the_capability_are_refused_at_once() {
        for head in [
            // Another method, a proxy tunnel, a TLS handshake, noise.
            "P",
            "POST /",
            "GET\t",
            "get /",
            "CONNECT 192.0.2.7:443 HTTP/1.1\r\n",
            "\u{16}\u{3}\u{1}",
            "\r\n",
            // A proxy request for another host, port or scheme.
            "GET http://192.0.2.7/key.bin HTTP/1.1\r\n",
            "GET http://127.0.0.1:4322/",
            "GET http://127.0.0.1:43210/",
            "GET https://127.0.0.1:4321/",
            "GET h2",
            "GET *",
            // A request line that ends before the capability could.
            "GET / HTTP/1.1\r\n",
            "GET /0123456789abcdef HTTP/1.1\r\n",
            "GET /0123456789abcdef\n",
            "GET http://127.0.0.1:4321/ HTTP/1.1\r\n",
            "GET http://127.0.0.1:4321 HTTP/1.1\r\n",
        ] {
            assert_eq!(admitted(head), Admission::Refused, "{head:?}");
        }
    }

    /// A guess is answered the same whatever part of it is right: nothing
    /// while it is shorter than the capability, a refusal once it is as
    /// long. So the capability cannot be found a character at a time.
    #[test]
    fn a_wrong_capability_is_judged_whole() {
        let right = CAPABILITY.as_bytes();
        // From the token's first character: the slash before it is no secret.
        for position in 1..right.len() {
            let mut guess = right.to_vec();
            guess[position] = if guess[position] == b'f' { b'0' } else { b'f' };
            let mut head = b"GET ".to_vec();
            head.extend_from_slice(&guess);
            assert_eq!(admit(&head, PROXY, CAPABILITY), Admission::Refused);
            for cut in 4..head.len() {
                assert_eq!(
                    admit(&head[..cut], PROXY, CAPABILITY),
                    Admission::Undecided,
                    "a partial guess wrong at {position} was judged at {cut}"
                );
            }
        }
        assert!(same(b"", b""));
        assert!(same(b"synthetic", b"synthetic"));
        assert!(!same(b"synthetic", b"synthetid"));
        assert!(!same(b"xynthetic", b"synthetic"));
        assert!(!same(b"synthetic", b"synthetic "));
    }

    #[test]
    fn cookie_flood_cannot_grow_the_store_or_replace_existing_session() {
        use reqwest::cookie::CookieStore;
        let url = Url::parse("https://synthetic.example.test/stream").unwrap();
        let cookies = OriginCookies {
            origin: url.origin(),
            jar: Mutex::new(cookie_store::CookieStore::default()),
            seeded: Mutex::new(Vec::new()),
        };
        cookies.insert("session=first; Path=/", &url);
        for n in 0..1000 {
            cookies.insert(&format!("cookie{n}=synthetic; Path=/"), &url);
        }
        assert_eq!(cookies.jar.lock().unwrap().iter_any().count(), 64);
        cookies.insert("session=renewed; Path=/", &url);
        let sent = cookies.cookies(&url).unwrap();
        assert!(sent.to_str().unwrap().contains("session=renewed"));
        assert!(sent.as_bytes().len() <= 16 * 1024);
        cookies.insert(&format!("oversized={}", "x".repeat(5000)), &url);
        assert!(
            !cookies
                .cookies(&url)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("oversized=")
        );
    }

    #[tokio::test]
    async fn trickled_prefix_has_an_absolute_deadline() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let received = socket.read(&mut request).await.unwrap();
            assert!(received > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n#")
                .await
                .unwrap();
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if socket.write_all(b"x").await.is_err() {
                    break;
                }
            }
        });
        let mut response = reqwest::get(format!("http://{address}/")).await.unwrap();
        let error = sniff_until(
            &mut response,
            tokio::time::Instant::now() + Duration::from_millis(30),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        server.abort();
    }
}
