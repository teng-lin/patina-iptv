//! The network port: how the runtime reaches the network without naming an
//! adapter.
//!
//! A host supplies a [`Network`] in [`crate::Ports`]: a factory that makes a
//! [`Client`] for one [`NetworkConfig`]. A client implements
//! [`iptv_import::Fetch`], so imports, on-demand browsing and link resolves
//! run over it unchanged. The adapter behind it owns transport policy: TLS,
//! timeouts, redirects, decompression and the destination checks.
//!
//! Natively the factory is `iptv-net`'s reqwest client (the `native`
//! feature); a browser host supplies one over the page's `fetch`. The
//! runtime keeps the limits it enforces itself (the byte budgets of private
//! documents, the total deadline) in the config, so both see the same
//! numbers.
//!
//! Futures and bodies are boxed: one allocation per request or chunk, which
//! is nothing beside a network round trip. They are `Send` exactly when the
//! runtime's executors are threaded ([`crate::MaybeSend`]). A transport
//! itself is always `Send` and `Sync`, so a client can be kept anywhere: a
//! browser adapter is a handle whose state stays with the page's host.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use iptv_import::{BodyStream, Fetch, FetchError, FetchErrorKind, StreamingResponse};
use iptv_providers::{HttpRequest, HttpResponse};

use crate::threading::MaybeSend;

/// The part of the provider network configuration that no setting changes
/// (limits, the development fixture override). Playback transport reads it
/// on player threads, which act for no owner.
pub fn base_network_config() -> NetworkConfig {
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut config = NetworkConfig::default();
    #[cfg(debug_assertions)]
    {
        config.allow_private_fixture = cfg!(any(test, feature = "test-support"))
            || crate::build_config::DEV_PLAYLIST.is_some();
    }
    config
}

/// Extra native trust anchors shared by the default provider and player transports.
/// Player threads may read this policy before a runtime owner exists.
#[cfg(feature = "native")]
pub fn default_playback_trust_anchors() -> Vec<Vec<u8>> {
    crate::native::network::adapter_config(&base_network_config()).extra_roots_der
}

/// A boxed future of an adapter: `Send` when the runtime is threaded.
#[cfg(feature = "native")]
pub type NetFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A boxed future of an adapter: `Send` when the runtime is threaded.
#[cfg(not(feature = "native"))]
pub type NetFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// What a client is made for: the settings that change requests, and the
/// limits the runtime and the adapter both enforce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkConfig {
    /// Sent when a request has no `User-Agent` of its own.
    pub user_agent: String,
    /// Ceiling on one request's total time; a request's own timeout applies
    /// when it is shorter.
    pub request_timeout: Duration,
    /// Longest silence while waiting for headers or more body bytes.
    pub read_timeout: Duration,
    /// Redirects followed at most.
    pub max_redirects: usize,
    /// Bytes a streamed body may deliver.
    pub max_streamed_bytes: u64,
    /// Bytes a buffered response may hold.
    pub max_buffered_bytes: usize,
    /// Local files (`file://` URLs and absolute paths) may be read.
    pub allow_files: bool,
    /// Explicit synthetic fixture escape hatch; absent from release builds.
    #[cfg(debug_assertions)]
    pub allow_private_fixture: bool,
}

impl Default for NetworkConfig {
    /// The adapter defaults (`iptv-net`'s `NetConfig::default`, which a
    /// native test keeps equal).
    fn default() -> Self {
        Self {
            user_agent: iptv_providers::xtream::DEFAULT_USER_AGENT.to_owned(),
            request_timeout: Duration::from_secs(30 * 60),
            read_timeout: Duration::from_secs(60),
            max_redirects: 10,
            max_streamed_bytes: 512 << 20,
            max_buffered_bytes: 128 * 1024 * 1024,
            allow_files: true,
            #[cfg(debug_assertions)]
            allow_private_fixture: false,
        }
    }
}

/// Makes clients. Supplied by the host in [`crate::Ports`].
pub trait Network: Send + Sync {
    /// A client for `config`. Fails when the adapter cannot be built (an
    /// invalid trust anchor, for example).
    fn client(&self, config: &NetworkConfig) -> Result<Client, FetchError>;
}

/// One adapter client: the object-safe form of [`Fetch`].
pub trait Transport: Send + Sync {
    /// The adapter's monotonic clock since the client was made.
    fn elapsed(&self) -> Duration;
    /// Waits `duration` on the adapter's timer.
    fn sleep(&self, duration: Duration) -> NetFuture<'_, ()>;
    fn fetch(&self, request: HttpRequest) -> NetFuture<'_, Result<HttpResponse, FetchError>>;
    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> NetFuture<'_, Result<StreamingResponse<Body>, FetchError>>;
}

/// A response body of an adapter: the object-safe form of [`BodyStream`].
pub trait Chunks: MaybeSend {
    fn next_chunk(&mut self) -> NetFuture<'_, Result<Option<Vec<u8>>, FetchError>>;
}

/// A streamed body, read chunk by chunk.
pub struct Body(Box<dyn Chunks>);

impl Body {
    pub fn new(chunks: impl Chunks + 'static) -> Self {
        Self(Box::new(chunks))
    }
}

impl BodyStream for Body {
    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, FetchError>> {
        self.0.next_chunk()
    }
}

/// A client made by the host's [`Network`]. Cheap to clone; clones share
/// the adapter's connections and clock.
#[derive(Clone)]
pub struct Client {
    transport: Arc<dyn Transport>,
    config: Arc<NetworkConfig>,
}

impl Client {
    pub fn new(transport: impl Transport + 'static, config: NetworkConfig) -> Self {
        Self {
            transport: Arc::new(transport),
            config: Arc::new(config),
        }
    }

    /// The configuration this client was made for.
    pub fn config(&self) -> &NetworkConfig {
        &self.config
    }
}

impl Client {
    /// `request` within this client's limits: its total time at most the
    /// configured ceiling, and the configured `User-Agent` unless it names
    /// its own.
    fn limited(&self, mut request: HttpRequest) -> HttpRequest {
        let ceiling = self.config.request_timeout;
        request.timeout = Some(request.timeout.map_or(ceiling, |own| own.min(ceiling)));
        if !request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
        {
            request
                .headers
                .push(("User-Agent".to_owned(), self.config.user_agent.clone()));
        }
        request
    }
}

fn too_large(what: &str) -> FetchError {
    FetchError::new(
        FetchErrorKind::TooLarge,
        format!("{what} exceeds its byte budget"),
    )
}

/// The client holds every request to its configuration whatever the adapter
/// does: it asks for a total time no longer than the configured one (the
/// request's timeout, which the adapter enforces: `iptv-net` natively, the
/// web host's Page adapter in the browser), and refuses a successful
/// response whose declared body, or a body whose delivered bytes, exceed
/// the budget. The native adapter enforces the same byte limits itself, so
/// these checks never fire first there.
impl Fetch for Client {
    type Body = Body;

    fn elapsed(&self) -> Duration {
        self.transport.elapsed()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        self.transport.sleep(duration)
    }

    fn fetch(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, FetchError>> {
        let limit = self.config.max_buffered_bytes;
        let response = self.transport.fetch(self.limited(request));
        async move {
            let response = response.await?;
            if response.body.len() > limit {
                return Err(too_large("response"));
            }
            Ok(response)
        }
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<StreamingResponse<Body>, FetchError>> {
        let limit = self.config.max_streamed_bytes;
        let response = self.transport.fetch_stream(self.limited(request));
        async move {
            let response = response.await?;
            // Only a body the caller will read: an error status keeps its
            // own meaning (a 404 with a large declared page is a 404).
            if response.is_success() && response.content_length.is_some_and(|length| length > limit)
            {
                return Err(too_large("declared body"));
            }
            Ok(StreamingResponse {
                status: response.status,
                same_request_origin: response.same_request_origin,
                content_length: response.content_length,
                body: Body::new(Counted {
                    body: response.body,
                    remaining: limit,
                }),
            })
        }
    }
}

/// A body that fails once it delivered more than its budget.
struct Counted {
    body: Body,
    remaining: u64,
}

impl Chunks for Counted {
    fn next_chunk(&mut self) -> NetFuture<'_, Result<Option<Vec<u8>>, FetchError>> {
        Box::pin(async move {
            let chunk = self.body.next_chunk().await?;
            if let Some(bytes) = &chunk {
                self.remaining = self
                    .remaining
                    .checked_sub(bytes.len() as u64)
                    .ok_or_else(|| too_large("body"))?;
            }
            Ok(chunk)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_import::MemoryBody;
    use std::sync::Mutex;

    /// An adapter that answers every request with `body` and keeps what it
    /// was asked.
    struct Fake {
        body: Vec<u8>,
        declared: Option<u64>,
        status: u16,
        asked: Arc<Mutex<Vec<HttpRequest>>>,
    }

    struct Memory(MemoryBody);
    impl Chunks for Memory {
        fn next_chunk(&mut self) -> NetFuture<'_, Result<Option<Vec<u8>>, FetchError>> {
            Box::pin(self.0.next_chunk())
        }
    }

    impl Transport for Fake {
        fn elapsed(&self) -> Duration {
            Duration::ZERO
        }
        fn sleep(&self, _: Duration) -> NetFuture<'_, ()> {
            Box::pin(std::future::ready(()))
        }
        fn fetch(&self, request: HttpRequest) -> NetFuture<'_, Result<HttpResponse, FetchError>> {
            self.asked.lock().unwrap().push(request);
            Box::pin(std::future::ready(Ok(HttpResponse::new(
                200,
                self.body.clone(),
            ))))
        }
        fn fetch_stream(
            &self,
            request: HttpRequest,
        ) -> NetFuture<'_, Result<StreamingResponse<Body>, FetchError>> {
            self.asked.lock().unwrap().push(request);
            Box::pin(std::future::ready(Ok(StreamingResponse {
                status: self.status,
                same_request_origin: false,
                content_length: self.declared,
                body: Body::new(Memory(MemoryBody::new(&self.body, 4))),
            })))
        }
    }

    fn ready<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("the fake adapter answers at once"),
        }
    }

    fn client(body: &[u8], declared: Option<u64>) -> (Client, Arc<Mutex<Vec<HttpRequest>>>) {
        let asked = Arc::default();
        let config = NetworkConfig {
            max_buffered_bytes: 8,
            max_streamed_bytes: 8,
            request_timeout: Duration::from_secs(10),
            user_agent: "SyntheticAgent/1".into(),
            ..NetworkConfig::default()
        };
        let fake = Fake {
            body: body.to_vec(),
            declared,
            status: if body == b"not found" { 404 } else { 200 },
            asked: Arc::clone(&asked),
        };
        (Client::new(fake, config), asked)
    }

    fn drain(body: &mut Body) -> Result<Vec<u8>, FetchError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = ready(body.next_chunk())? {
            bytes.extend(chunk);
        }
        Ok(bytes)
    }

    #[test]
    fn an_adapter_cannot_exceed_the_clients_byte_budgets() {
        let (small, _) = client(b"12345678", None);
        assert!(ready(small.fetch(HttpRequest::get("https://example.test/a"))).is_ok());
        let mut stream =
            ready(small.fetch_stream(HttpRequest::get("https://example.test/a"))).unwrap();
        assert_eq!(drain(&mut stream.body).unwrap(), b"12345678");

        let (large, _) = client(b"123456789", None);
        let error = ready(large.fetch(HttpRequest::get("https://example.test/b"))).unwrap_err();
        assert_eq!(error.kind, FetchErrorKind::TooLarge);
        let mut stream =
            ready(large.fetch_stream(HttpRequest::get("https://example.test/b"))).unwrap();
        assert_eq!(
            drain(&mut stream.body).unwrap_err().kind,
            FetchErrorKind::TooLarge
        );

        let (declared, _) = client(b"", Some(9));
        let refused = ready(declared.fetch_stream(HttpRequest::get("https://example.test/c")));
        assert_eq!(refused.err().unwrap().kind, FetchErrorKind::TooLarge);

        // An error status is the caller's to read, whatever length it declares.
        let (missing, _) = client(b"not found", Some(1 << 30));
        let answer =
            ready(missing.fetch_stream(HttpRequest::get("https://example.test/d"))).unwrap();
        assert_eq!(answer.status, 404);
    }

    #[test]
    fn requests_carry_the_clients_ceiling_and_agent() {
        let (client, asked) = client(b"", None);
        let _ = ready(client.fetch(HttpRequest::get("https://example.test/a")));
        let mut own = HttpRequest::get("https://example.test/b").with_header("user-agent", "Own/2");
        own.timeout = Some(Duration::from_secs(3));
        let _ = ready(client.fetch(own));
        let mut long = HttpRequest::get("https://example.test/c");
        long.timeout = Some(Duration::from_secs(60));
        let _ = ready(client.fetch(long));
        let asked = asked.lock().unwrap();
        let agents = |request: &HttpRequest| {
            request
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(asked[0].timeout, Some(Duration::from_secs(10)));
        assert_eq!(agents(&asked[0]), ["SyntheticAgent/1"]);
        assert_eq!(asked[1].timeout, Some(Duration::from_secs(3)));
        assert_eq!(agents(&asked[1]), ["Own/2"]);
        assert_eq!(asked[2].timeout, Some(Duration::from_secs(10)));
    }
}
