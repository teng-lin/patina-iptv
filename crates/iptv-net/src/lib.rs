#![forbid(unsafe_code)]
//! Native network adapter of iptv-rs.
//!
//! [`NetClient`] performs `iptv_providers::HttpRequest`s with reqwest and
//! implements [`iptv_import::Fetch`] (buffered [`NetClient::fetch`] and
//! chunked [`NetClient::fetch_stream`]).
//!
//! - **TLS:** rustls with the ring provider and the Mozilla root set
//!   (webpki-roots), plus optional extra roots. No OpenSSL, no aws-lc, no
//!   platform verifier, so it cross-compiles for Android with only the NDK
//!   clang (for ring) and needs no JNI initialisation. User-installed
//!   Android CAs are therefore not trusted (pass them as extra roots).
//! - **HTTP:** HTTP/1.1; `gzip`/`deflate` `Content-Encoding` decoded
//!   transparently (a `.gz` *file* is passed through for the parsers to
//!   detect); up to [`NetConfig::max_redirects`] redirects, including
//!   HTTP to HTTPS; HTTPS to HTTP is refused. A redirect to another origin
//!   keeps only content negotiation, range, `User-Agent` and `Origin`
//!   headers and the origin of a `Referer` (`cross_origin`); no `Referer`
//!   is ever added (it would carry Xtream credentials from the query). So
//!   neither portal identity nor a playlist's own credential header leaks
//!   to a CDN.
//! - **Headers:** sent to the request's origin exactly as the request
//!   carries them (User-Agent, Cookie, Referer…). [`NetConfig::user_agent`]
//!   applies only when the request has none. No cookie store: `Set-Cookie`
//!   is ignored.
//! - **Timeouts:** connect; idle ([`NetConfig::read_timeout`], which ends a
//!   stalled download); and a total per request, the shorter of the
//!   import's remaining budget and [`NetConfig::request_timeout`]. Streamed
//!   and buffered bodies both enforce byte budgets. The wait before an
//!   import's retry ([`iptv_import::Fetch::sleep`]) is a tokio sleep.
//! - **Local files:** `file://` URLs and absolute paths are read with tokio
//!   (picked playlists), unless [`NetConfig::allow_files`] is off.
//! - **Redaction:** logs use `HttpRequest::redacted_url`; errors never carry
//!   URLs (`reqwest::Error::without_url`) or bodies.
//!
//! Needs a tokio runtime (reqwest's I/O driver). Not part of the portable
//! core.

mod body;
mod client;
mod cross_origin;
mod error;
mod file;
pub mod playback;
pub mod live;
pub mod recording;
pub mod recording_file;
mod redirect;
mod tls;

pub use body::NetBody;
pub use client::{NetClient, NetConfig};
pub use iptv_import::{FetchError, FetchErrorKind, StreamingResponse};

mod public_resolver;
pub use public_resolver::PublicResolver;
