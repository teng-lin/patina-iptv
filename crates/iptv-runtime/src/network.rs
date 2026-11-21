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
//! runtime's executors are threaded ([`crate::MaybeSend`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use iptv_import::{BodyStream, Fetch, FetchError, StreamingResponse};
use iptv_providers::{HttpRequest, HttpResponse};

use crate::ports::{MaybeSend, MaybeSync};

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
pub trait Transport: MaybeSend + MaybeSync {
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
        self.transport.fetch(request)
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<StreamingResponse<Body>, FetchError>> {
        self.transport.fetch_stream(request)
    }
}
