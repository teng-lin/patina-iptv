//! The reqwest-backed client.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use iptv_import::{BodyStream, Fetch, FetchError, FetchErrorKind, StreamingResponse};
use iptv_providers::{HttpRequest, HttpResponse, Method};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};

use crate::body::{Inner, NetBody};
use crate::error::from_reqwest;
use crate::file;
use crate::redirect::{self, Prepared};

/// Transport settings.
#[derive(Clone, PartialEq, Eq)]
pub struct NetConfig {
    pub connect_timeout: Duration,
    /// Longest silence while waiting for response headers or for more body
    /// bytes. An idle timeout: it starts again with every read, so it ends a
    /// stalled download however large the download is.
    pub read_timeout: Duration,
    pub max_redirects: usize,
    /// Ceiling on the total time of one request: headers, redirects and the
    /// response body. A request's own `HttpRequest::timeout` (an import's
    /// remaining budget) applies when it is shorter. Large because a slow
    /// but steady download is fine; a stalled one ends at
    /// [`Self::read_timeout`].
    pub request_timeout: Duration,
    /// Bytes delivered after HTTP content decoding, including streamed reads.
    pub max_streamed_bytes: u64,
    /// Sent when a request has no `User-Agent` of its own. Providers that
    /// filter on it get theirs per source (M3U setting, Xtream/Stalker
    /// clients).
    pub user_agent: String,
    /// Buffered [`NetClient::fetch`] fails with `TooLarge` beyond this.
    pub max_buffered_bytes: usize,
    /// DER certificates trusted in addition to the Mozilla roots
    /// (self-hosted servers, user CAs).
    pub extra_roots_der: Vec<Vec<u8>>,
    /// Read `file://` URLs and absolute paths from the filesystem.
    pub allow_files: bool,
    /// `http://host:port` proxy. Environment proxies are ignored.
    ///
    /// It does **not** cover everything. Requests that must reach a public
    /// address (`HttpRequest::public_only`, and every redirect to another
    /// origin) connect directly: their DNS answers are checked and pinned
    /// here ([`crate::PublicResolver`]), which a proxy resolving the name
    /// itself would bypass. The playback relay ([`crate::playback`]) does
    /// not use it either. A caller that needs every connection proxied
    /// (privacy, a filtered network) cannot rely on this field alone. The
    /// app sets no proxy.
    pub proxy: Option<String>,
    /// Explicit synthetic fixture escape hatch; absent from release builds.
    #[cfg(debug_assertions)]
    pub allow_private_fixture: bool,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(60),
            max_redirects: 10,
            request_timeout: Duration::from_secs(30 * 60),
            max_streamed_bytes: 512 << 20,
            user_agent: iptv_providers::xtream::DEFAULT_USER_AGENT.to_owned(),
            max_buffered_bytes: 128 * 1024 * 1024,
            extra_roots_der: Vec::new(),
            allow_files: true,
            proxy: None,
            #[cfg(debug_assertions)]
            allow_private_fixture: false,
        }
    }
}

impl std::fmt::Debug for NetConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetConfig")
            .field("connect_timeout", &self.connect_timeout)
            .field("read_timeout", &self.read_timeout)
            .field("max_redirects", &self.max_redirects)
            .field("user_agent", &self.user_agent)
            .field("max_buffered_bytes", &self.max_buffered_bytes)
            .field("extra_roots", &self.extra_roots_der.len())
            .field("allow_files", &self.allow_files)
            // A proxy URL may carry credentials.
            .field("proxy", &self.proxy.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Performs provider requests. Cheap to clone (shared connection pool).
#[derive(Clone)]
pub struct NetClient {
    client: reqwest::Client,
    public_client: reqwest::Client,
    gzip_clients: Arc<std::sync::OnceLock<Result<(reqwest::Client, reqwest::Client), FetchError>>>,
    config: Arc<NetConfig>,
    started: std::time::Instant,
}

impl std::fmt::Debug for NetClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl NetClient {
    pub fn new(config: NetConfig) -> Result<Self, FetchError> {
        let client = Self::http_client(&config, false, true)?;
        let public_client = Self::http_client(&config, true, true)?;
        Ok(Self {
            client,
            public_client,
            gzip_clients: Arc::new(std::sync::OnceLock::new()),
            config: Arc::new(config),
            started: std::time::Instant::now(),
        })
    }

    fn http_client(
        config: &NetConfig,
        public_only: bool,
        decode_gzip: bool,
    ) -> Result<reqwest::Client, FetchError> {
        let tls = crate::tls::client_config(&config.extra_roots_der)?;
        let mut builder = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            // Redirects are followed in `redirect::execute` (header hygiene).
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .gzip(decode_gzip)
            .deflate(true)
            .no_proxy();
        if public_only {
            builder = builder.dns_resolver(Arc::new(crate::PublicResolver));
        }
        if !public_only && let Some(proxy) = &config.proxy {
            let proxy = reqwest::Proxy::all(proxy).map_err(|_| {
                FetchError::new(FetchErrorKind::InvalidRequest, "invalid proxy address")
            })?;
            builder = builder.proxy(proxy);
        }
        builder.build().map_err(from_reqwest)
    }

    /// With [`NetConfig::default`].
    pub fn with_defaults() -> Result<Self, FetchError> {
        Self::new(NetConfig::default())
    }

    pub fn config(&self) -> &NetConfig {
        &self.config
    }

    /// Sends the request and reads the whole body (up to
    /// [`NetConfig::max_buffered_bytes`]).
    pub async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, FetchError> {
        let limit = self.config.max_buffered_bytes;
        let resp = self.fetch_stream(request).await?;
        if resp.content_length.is_some_and(|n| n > limit as u64) {
            return Err(too_large(limit));
        }
        let mut body = Vec::with_capacity(resp.content_length.unwrap_or(0).min(1 << 20) as usize);
        let mut stream = resp.body;
        while let Some(chunk) = stream.next_chunk().await? {
            if body.len().saturating_add(chunk.len()) > limit {
                return Err(too_large(limit));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse::new(resp.status, body))
    }

    /// Sends the request; the body is read with
    /// [`iptv_import::BodyStream::next_chunk`].
    pub async fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> Result<StreamingResponse<NetBody>, FetchError> {
        let deadline = tokio::time::Instant::now()
            + request
                .timeout
                .unwrap_or(self.config.request_timeout)
                .min(self.config.request_timeout);
        tokio::time::timeout_at(deadline, self.fetch_stream_inner(request, deadline))
            .await
            .map_err(|_| {
                FetchError::new(FetchErrorKind::Deadline, "total response deadline exceeded")
            })?
    }

    async fn fetch_stream_inner(
        &self,
        request: HttpRequest,
        deadline: tokio::time::Instant,
    ) -> Result<StreamingResponse<NetBody>, FetchError> {
        if let Some(path) = file::local_path(&request.url) {
            #[cfg(debug_assertions)]
            let fixture = self.config.allow_private_fixture;
            #[cfg(not(debug_assertions))]
            let fixture = false;
            if !self.config.allow_files
                || (!fixture && (!request.picked_file || request.public_only))
            {
                return Err(FetchError::new(
                    FetchErrorKind::Unsupported,
                    "local files are disabled",
                ));
            }
            let (f, len) = file::open(&path?).await?;
            log::debug!("read local file ({len:?} bytes)");
            return Ok(StreamingResponse {
                status: 200,
                content_length: len,
                body: NetBody {
                    inner: Inner::File(f),
                    remaining: self.config.max_streamed_bytes,
                    deadline,
                },
            });
        }
        let preserve_gzip = request.preserve_gzip;
        let shown = request.redacted_url();
        let req = self.build(request)?;
        let (client, public_client) = if preserve_gzip {
            let pair = self
                .gzip_clients
                .get_or_init(|| {
                    Ok((
                        Self::http_client(&self.config, false, false)?,
                        Self::http_client(&self.config, true, false)?,
                    ))
                })
                .as_ref()
                .map_err(Clone::clone)?;
            (&pair.0, &pair.1)
        } else {
            (&self.client, &self.public_client)
        };
        log::debug!("{} {shown}", req.method);
        let resp = redirect::execute(
            client,
            public_client,
            req,
            self.config.max_redirects,
            &shown,
        )
        .await
        .inspect_err(|e| log::debug!("{shown}: {e}"))?;
        let status = resp.status().as_u16();
        log::debug!("{shown}: HTTP {status}");
        Ok(StreamingResponse {
            status,
            content_length: resp.content_length(),
            body: NetBody {
                inner: Inner::Http(resp),
                remaining: self.config.max_streamed_bytes,
                deadline,
            },
        })
    }

    fn build(&self, request: HttpRequest) -> Result<Prepared, FetchError> {
        let invalid = |what: &str| FetchError::new(FetchErrorKind::InvalidRequest, what);
        let url = reqwest::Url::parse(request.url.trim()).map_err(|_| invalid("invalid URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(FetchError::new(
                FetchErrorKind::Unsupported,
                format!("unsupported scheme {}", url.scheme()),
            ));
        }
        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            let name = HeaderName::from_bytes(name.trim().as_bytes())
                .map_err(|_| invalid("invalid header name"))?;
            // Values may be secrets: never quote them.
            let value =
                HeaderValue::from_str(value.trim()).map_err(|_| invalid("invalid header value"))?;
            headers.append(name, value);
        }
        if !headers.contains_key(USER_AGENT) {
            let ua = HeaderValue::from_str(&self.config.user_agent)
                .map_err(|_| invalid("invalid default User-Agent"))?;
            headers.insert(USER_AGENT, ua);
        }
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        Ok(Prepared {
            public_only: request.public_only,
            #[cfg(debug_assertions)]
            allow_private_fixture: self.config.allow_private_fixture,
            method,
            url,
            headers,
            body: request.body,
        })
    }
}

fn too_large(limit: usize) -> FetchError {
    FetchError::new(FetchErrorKind::TooLarge, format!("more than {limit} bytes"))
}

impl Fetch for NetClient {
    type Body = NetBody;

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        tokio::time::sleep(duration)
    }

    fn fetch(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, FetchError>> {
        NetClient::fetch(self, request)
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<StreamingResponse<NetBody>, FetchError>> {
        NetClient::fetch_stream(self, request)
    }
}
