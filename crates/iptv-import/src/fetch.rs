//! The networking boundary: an async [`Fetch`] that performs the providers'
//! [`HttpRequest`]s, buffered or as a chunked body.
//!
//! Adapters (reqwest natively in `iptv-net`, `fetch` on the web later)
//! implement it; tests use an in-memory fake. The trait does not require
//! `Send` futures, so single-threaded web adapters fit; with a concrete
//! adapter whose futures are `Send` (iptv-net), the import futures are
//! `Send` too and can be spawned on a multi-threaded runtime.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;

use iptv_providers::{HttpRequest, HttpResponse};

/// Performs HTTP requests for the importer.
///
/// Adapters own transport policy: TLS, timeouts, redirects, decompression
/// of `Content-Encoding`, a default User-Agent when the request has none.
/// Headers in the request (User-Agent, Cookie, Referer…) are sent as given.
/// Error details must never contain credentials (log
/// [`HttpRequest::redacted_url`], never `url`).
pub trait Fetch {
    type Body: BodyStream;

    /// Monotonic adapter clock, for an end-to-end import deadline. Sans-IO
    /// fakes may keep zero or advance a virtual clock. Native adapters must
    /// also bound pending requests and body reads.
    fn elapsed(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }

    /// Waits `duration` on the adapter's timer, between two attempts at a
    /// request that failed for a passing reason. The importer has no
    /// runtime and no clock of its own, so the wait is the adapter's: a
    /// native adapter sleeps on its runtime, a web adapter sets a timer. The
    /// default returns at once (sans-IO fakes; they may record the request
    /// or advance a virtual clock), which makes a retry immediate.
    fn sleep(&self, duration: std::time::Duration) -> impl Future<Output = ()> {
        let _ = duration;
        std::future::ready(())
    }

    /// Sends the request and reads the whole body.
    fn fetch(&self, request: HttpRequest)
    -> impl Future<Output = Result<HttpResponse, FetchError>>;

    /// Sends the request and returns once the status is known; the body is
    /// read chunk by chunk (large playlists and XMLTV files).
    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<StreamingResponse<Self::Body>, FetchError>>;
}

impl<F: Fetch + ?Sized> Fetch for &F {
    type Body = F::Body;
    fn elapsed(&self) -> std::time::Duration {
        (**self).elapsed()
    }

    fn sleep(&self, duration: std::time::Duration) -> impl Future<Output = ()> {
        (**self).sleep(duration)
    }

    fn fetch(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, FetchError>> {
        (**self).fetch(request)
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<StreamingResponse<Self::Body>, FetchError>> {
        (**self).fetch_stream(request)
    }
}

/// A response body read incrementally.
pub trait BodyStream {
    /// The next chunk, `None` at the end of the body. Chunks are never empty.
    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, FetchError>>;
}

/// Status plus a body that has not been read yet.
pub struct StreamingResponse<B> {
    pub status: u16,
    /// The adapter verified that the HTTP response came from the requested origin.
    pub same_request_origin: bool,
    /// `Content-Length` after decompression is unknown, so this is only the
    /// wire length when the adapter knows it (for progress).
    pub content_length: Option<u64>,
    pub body: B,
}

impl<B> StreamingResponse<B> {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

impl<B> fmt::Debug for StreamingResponse<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamingResponse")
            .field("status", &self.status)
            .field("content_length", &self.content_length)
            .finish_non_exhaustive()
    }
}

/// An in-memory body, for fakes and adapters that already hold the bytes.
#[derive(Clone, Debug, Default)]
pub struct MemoryBody {
    chunks: VecDeque<Vec<u8>>,
    /// Returned after the last chunk instead of the end of the body.
    fail_at_end: Option<FetchError>,
}

impl MemoryBody {
    /// The bytes split into chunks of `chunk_size` (at least 1).
    pub fn new(bytes: &[u8], chunk_size: usize) -> Self {
        Self {
            chunks: bytes
                .chunks(chunk_size.max(1))
                .map(<[u8]>::to_vec)
                .collect(),
            fail_at_end: None,
        }
    }

    /// Fails with `error` once the chunks are exhausted (a dropped connection).
    pub fn failing_at_end(mut self, error: FetchError) -> Self {
        self.fail_at_end = Some(error);
        self
    }
}

impl BodyStream for MemoryBody {
    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, FetchError>> {
        let next = match self.chunks.pop_front() {
            Some(c) => Ok(Some(c)),
            None => match self.fail_at_end.take() {
                Some(e) => Err(e),
                None => Ok(None),
            },
        };
        std::future::ready(next)
    }
}

/// What went wrong below HTTP: the request never produced a usable response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FetchErrorKind {
    /// The request could not be built (bad URL, header value).
    InvalidRequest,
    /// The adapter cannot perform this request (scheme, local files on the web).
    Unsupported,
    /// DNS failure, refused or reset connection.
    Connect,
    /// TLS handshake or certificate failure.
    Tls,
    /// Connect or read timeout.
    Timeout,
    /// The total response/import deadline elapsed, even if bytes kept arriving.
    Deadline,
    /// Too many redirects or a redirect to an unusable location.
    Redirect,
    /// The connection failed while reading the body, or decompression failed.
    Body,
    /// The body exceeded the adapter's buffering limit.
    TooLarge,
    /// A local file could not be read.
    Io,
    Other,
}

impl FetchErrorKind {
    /// Whether every request to the same host will fail the same way (so
    /// probing further endpoints of that host is pointless).
    pub fn is_host_failure(self) -> bool {
        matches!(
            self,
            Self::Connect | Self::Tls | Self::InvalidRequest | Self::TooLarge | Self::Deadline
        )
    }
}

/// A transport failure. `detail` is diagnostic text without URLs or secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchError {
    pub kind: FetchErrorKind,
    pub detail: String,
}

impl FetchError {
    pub fn new(kind: FetchErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            FetchErrorKind::InvalidRequest => "invalid request",
            FetchErrorKind::Unsupported => "unsupported request",
            FetchErrorKind::Connect => "connection failed",
            FetchErrorKind::Tls => "secure connection failed",
            FetchErrorKind::Timeout => "timed out",
            FetchErrorKind::Deadline => "import deadline exceeded",
            FetchErrorKind::Redirect => "redirect failed",
            FetchErrorKind::Body => "download interrupted",
            FetchErrorKind::TooLarge => "response too large",
            FetchErrorKind::Io => "file could not be read",
            FetchErrorKind::Other => "network error",
        };
        if self.detail.is_empty() {
            f.write_str(what)
        } else {
            write!(f, "{what}: {}", self.detail)
        }
    }
}

impl std::error::Error for FetchError {}
