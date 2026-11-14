//! Sans-IO HTTP descriptions.
//!
//! Provider clients build [`HttpRequest`]s and parse [`HttpResponse`]s; an
//! adapter crate performs the I/O (reqwest natively, `fetch` on the web).
//! Both types redact in `Debug`, because Xtream puts credentials in the URL
//! path and account responses echo the password.
//!
//! Web note: browsers forbid setting `Cookie`, `User-Agent` and `Connection`
//! from `fetch`, so Stalker (which needs the `mac` cookie) and providers that
//! filter on User-Agent need a proxy on the web target.

use std::borrow::Cow;
use std::fmt;

use crate::error::ProviderError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// A request for the networking adapter to perform.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    /// Header name/value pairs in send order. Names are as the provider
    /// expects them; matching is case-insensitive via [`HttpRequest::header`].
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// Remaining end-to-end import time. Adapters enforce it across response reads.
    pub timeout: Option<std::time::Duration>,
    /// Content-supplied destinations require public hosts and DNS answers.
    pub public_only: bool,
    /// A source/guide path the user explicitly selected.
    pub picked_file: bool,
    /// Let the XMLTV parser count and decode gzip equally for HTTP and .gz files.
    pub preserve_gzip: bool,
    /// Substrings of `url` that are secrets, replaced in `Debug` output.
    redact: Vec<String>,
}

/// Headers whose values are always redacted in `Debug`.
const SENSITIVE_HEADERS: &[&str] = &["authorization", "cookie", "sn"];
/// Query keys whose values are always redacted in [`HttpRequest::redacted_url`].
const SENSITIVE_QUERY_KEYS: &[&str] = &[
    "username",
    "password",
    "login",
    "token",
    "mac",
    "sn",
    "device_id",
    "device_id2",
    "signature",
    "signature2",
    "prehash",
    "metrics",
];

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: None,
            public_only: false,
            picked_file: false,
            preserve_gzip: false,
            redact: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// First value of a header, matching the name case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Marks `secret` (and its percent-encoded form) for redaction.
    pub(crate) fn redacting(mut self, secret: &str) -> Self {
        if !secret.is_empty() {
            let encoded = crate::urlenc::component(secret);
            if encoded != secret {
                self.redact.push(encoded);
            }
            self.redact.push(secret.to_owned());
            // Longest first so a secret containing another is fully replaced.
            self.redact.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
        self
    }

    /// An origin for generic URLs, whose path and query may contain arbitrary
    /// credentials. Provider requests explicitly register their known secrets
    /// and retain their redacted API parameters for diagnostics.
    pub fn redacted_url(&self) -> String {
        if self.redact.is_empty() {
            return url::Url::parse(&self.url)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .map(|u| u.origin().ascii_serialization())
                .unwrap_or_else(|| "[local or invalid URL]".into());
        }
        let mut out = self.url.clone();
        for secret in &self.redact {
            out = out.replace(secret.as_str(), "***");
        }
        match url::Url::parse(&out) {
            Ok(mut parsed) => {
                let _ = parsed.set_password(None);
                if !parsed.username().is_empty() {
                    let _ = parsed.set_username("***");
                }
                if parsed.query().is_some() {
                    let pairs: Vec<(String, String)> = parsed
                        .query_pairs()
                        .map(|(k, v)| {
                            let sensitive = SENSITIVE_QUERY_KEYS
                                .iter()
                                .any(|s| k.eq_ignore_ascii_case(s));
                            (
                                k.into_owned(),
                                if sensitive {
                                    "***".to_owned()
                                } else {
                                    v.into_owned()
                                },
                            )
                        })
                        .collect();
                    parsed.query_pairs_mut().clear().extend_pairs(pairs);
                }
                parsed.to_string()
            }
            Err(_) => out,
        }
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(n, v)| {
                let sensitive = SENSITIVE_HEADERS.iter().any(|s| n.eq_ignore_ascii_case(s));
                (
                    n.clone(),
                    if sensitive {
                        "***".to_owned()
                    } else {
                        v.clone()
                    },
                )
            })
            .collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.redacted_url())
            .field("headers", &headers)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// What the adapter got back. Only status and body matter to the parsers.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, body)
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as text: UTF-8 (lossy), BOM removed.
    pub fn text(&self) -> Cow<'_, str> {
        let bytes = self
            .body
            .strip_prefix(b"\xEF\xBB\xBF")
            .unwrap_or(&self.body);
        String::from_utf8_lossy(bytes)
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Bodies can echo credentials (Xtream `user_info.password`).
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// Parses a JSON body after checking the status. Tolerates a UTF-8 BOM and
/// surrounding whitespace.
pub(crate) fn json_body(response: &HttpResponse) -> Result<serde_json::Value, ProviderError> {
    if !response.is_success() {
        return Err(ProviderError::HttpStatus(response.status));
    }
    let bytes = response
        .body
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(&response.body);
    let trimmed = bytes.trim_ascii();
    if trimmed.is_empty() {
        return Err(ProviderError::InvalidBody("empty body".into()));
    }
    serde_json::from_slice(trimmed)
        .map_err(|_| ProviderError::InvalidBody("body is not JSON".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_registered_secrets_and_sensitive_parts() {
        let req =
            HttpRequest::get("http://tv.example.com/live/demo-user/p%40ss/1.ts?token=abc&x=1")
                .redacting("demo-user")
                .redacting("p@ss")
                .with_header("Authorization", "Bearer t0k3n")
                .with_header("Accept", "*/*");
        let shown = format!("{req:?}");
        assert!(!shown.contains("demo-user"), "{shown}");
        assert!(!shown.contains("p%40ss"), "{shown}");
        assert!(!shown.contains("abc"), "{shown}");
        assert!(!shown.contains("t0k3n"), "{shown}");
        assert!(shown.contains("x=1"));
        assert!(shown.contains("*/*"));
    }

    #[test]
    fn response_debug_hides_body() {
        let resp = HttpResponse::ok(r#"{"user_info":{"password":"secret-synthetic"}}"#);
        assert!(!format!("{resp:?}").contains("secret"));
    }

    #[test]
    fn generic_urls_never_show_path_query_fragment_or_user_info() {
        let req = HttpRequest::get(
            "https://synthetic-user:synthetic-pass@feed.example.test:8443/path-token/list?apikey=query-token#fragment-token",
        );
        assert_eq!(req.redacted_url(), "https://feed.example.test:8443");
        for secret in [
            "synthetic-user",
            "synthetic-pass",
            "path-token",
            "query-token",
            "fragment-token",
        ] {
            assert!(!format!("{req:?}").contains(secret));
        }
        assert_eq!(
            HttpRequest::get("file:///private/path-token").redacted_url(),
            "[local or invalid URL]"
        );
        assert_eq!(
            HttpRequest::get("invalid-path-token").redacted_url(),
            "[local or invalid URL]"
        );
    }

    #[test]
    fn json_body_handles_bom_status_and_garbage() {
        assert!(json_body(&HttpResponse::ok(b"\xEF\xBB\xBF [1]".to_vec())).is_ok());
        assert_eq!(
            json_body(&HttpResponse::new(403, "[]")),
            Err(ProviderError::HttpStatus(403))
        );
        assert!(matches!(
            json_body(&HttpResponse::ok("<html>")),
            Err(ProviderError::InvalidBody(_))
        ));
        assert!(matches!(
            json_body(&HttpResponse::ok("  ")),
            Err(ProviderError::InvalidBody(_))
        ));
    }
}
