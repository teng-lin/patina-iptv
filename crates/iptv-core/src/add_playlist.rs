//! Provider-neutral inputs and results for adding a playlist.

use std::fmt;

/// A credential. `Debug` prints `***`; read it with [`SecretText::expose`]
/// only where it goes on the wire or into secure storage.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct SecretText(String);

impl SecretText {
    pub fn new(value: impl Into<String>) -> Self {
        SecretText(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText(***)")
    }
}

/// A URL that may embed credentials. `Debug` prints only its origin.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct UrlText(String);

impl UrlText {
    pub fn new(value: impl Into<String>) -> Self {
        UrlText(value.into())
    }

    /// The URL as entered (trimmed). Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UrlText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UrlText({})", url_origin(&self.0))
    }
}

/// `scheme://host[:port]` with any userinfo, path, query and fragment gone.
pub(crate) fn url_origin(url: &str) -> String {
    let (scheme, rest) = match url.find("://") {
        Some(at) => (&url[..at + 3], &url[at + 3..]),
        None => ("", url),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let more = if authority.len() < rest.len() || host.len() < authority.len() {
        "/…"
    } else {
        ""
    };
    format!("{scheme}{host}{more}")
}

/// The provider-neutral input to playlist import, mapped to a provider
/// configuration. Values are trimmed (passwords excepted) and the MAC is
/// normalized to `AA:BB:CC:DD:EE:FF`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistDraft {
    pub source: DraftSource,
    pub name: Option<String>,
    pub epg_url: Option<UrlText>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftSource {
    M3uUrl {
        url: UrlText,
    },
    M3uFile {
        path: String,
    },
    /// `server` as typed; a missing scheme means `https`.
    Xtream {
        server: UrlText,
        username: SecretText,
        password: SecretText,
    },
    /// `portal` as typed; a missing scheme means `https`.
    Stalker {
        portal: UrlText,
        mac: SecretText,
    },
}

/// Whether an address may omit its HTTP scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlScheme {
    Required,
    Optional,
}

/// Why a playlist or guide address failed its shape check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlError {
    ContainsWhitespace,
    MissingScheme,
    UnsupportedScheme,
    InvalidHost,
    InvalidPort,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContainsWhitespace => f.write_str("address contains whitespace"),
            Self::MissingScheme => f.write_str("address is missing its scheme"),
            Self::UnsupportedScheme => f.write_str("address uses an unsupported scheme"),
            Self::InvalidHost => f.write_str("address has an invalid host"),
            Self::InvalidPort => f.write_str("address has an invalid port"),
        }
    }
}

impl std::error::Error for UrlError {}

/// Checks the shape of an `http`/`https` URL: no whitespace, a known scheme
/// (optional when requested), a host made of letter/digit/
/// hyphen labels or a bracketed IPv6 literal, and a port in 1..=65535.
/// Userinfo, path, query and fragment are not inspected.
///
/// # Errors
///
/// Returns the reason the URL's scheme, host, port or whitespace is invalid.
pub fn validate_url(input: &str, scheme: UrlScheme) -> Result<(), UrlError> {
    if input.chars().any(char::is_whitespace) {
        return Err(UrlError::ContainsWhitespace);
    }
    let rest = match input.find("://") {
        Some(at) => {
            let scheme = input[..at].to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(UrlError::UnsupportedScheme);
            }
            &input[at + 3..]
        }
        None if scheme == UrlScheme::Required => return Err(UrlError::MissingScheme),
        None => input,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let (host_ok, port) = if let Some(v6) = host_port.strip_prefix('[') {
        let Some((addr, after)) = v6.split_once(']') else {
            return Err(UrlError::InvalidHost);
        };
        let ok = addr.contains(':')
            && addr
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':').ok_or(UrlError::InvalidHost)?),
        };
        (ok, port)
    } else {
        let (host, port) = match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        };
        (valid_host_name(host), port)
    };
    if !host_ok {
        return Err(UrlError::InvalidHost);
    }
    if let Some(port) = port {
        let valid = !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p));
        if !valid {
            return Err(UrlError::InvalidPort);
        }
    }
    Ok(())
}

fn valid_host_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.chars().count() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
}

/// `00:1a:79:12:34:56`, `00-1A-79-12-34-56`, `001A.7912.3456` or
/// `001A79123456` → `00:1A:79:12:34:56`; `None` unless 12 hex digits.
pub fn normalize_mac(input: &str) -> Option<String> {
    let digits: String = input
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.') && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if digits.len() != 12 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let pairs: Vec<&str> = (0..6).map(|i| &digits[i * 2..i * 2 + 2]).collect();
    Some(pairs.join(":"))
}

/// Correlates an import with its progress and result; results for any id
/// but the running one are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImportId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportStage {
    Connecting,
    Downloading,
    Parsing,
    Saving,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportProgress {
    pub stage: ImportStage,
    /// Bytes or items, as the importer counts them.
    pub done: u64,
    pub total: Option<u64>,
}

impl ImportProgress {
    /// 0..=100 when the total is known.
    pub fn percent(&self) -> Option<u8> {
        let total = self.total.filter(|t| *t > 0)?;
        Some((u128::from(self.done.min(total)) * 100 / u128::from(total)) as u8)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportCounts {
    pub channels: u32,
    pub groups: u32,
    pub movies: u32,
    pub series: u32,
}

impl ImportCounts {
    /// Nothing playable arrived. Groups alone do not count.
    pub fn is_empty(&self) -> bool {
        self.channels == 0 && self.movies == 0 && self.series == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// The bounded import queue is full; retry after another import finishes.
    Busy,
    /// No connection, DNS, timeout, TLS, HTTP 5xx.
    Network,
    /// Local database, staging, or credential storage failed.
    Storage,
    /// The provider refused the account.
    Auth,
    /// The response or file is not a playlist.
    Parse,
    /// A valid playlist with nothing playable in it.
    EmptyPlaylist,
    /// A source this app cannot use (another portal type, a format we lack).
    Unsupported,
    /// The import was stopped before it finished.
    Canceled,
    /// This device cannot import playlists (a browser TV, whose host has no
    /// workers for imports yet).
    Unavailable,
}

/// The playlist import input implicated by a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportInput {
    PlaylistAddress,
    Credentials,
    GuideAddress,
}

/// A failed playlist import with any specific input the caller can correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportFailure {
    pub kind: FailureKind,
    /// The input implicated by the failure, when the import can identify it.
    pub input: Option<ImportInput>,
}

impl ImportFailure {
    pub fn new(kind: FailureKind) -> Self {
        ImportFailure { kind, input: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_rules() {
        let ok = [
            "http://example.test",
            "HTTPS://Example.TEST:8443/path?q=1#f",
            "http://user:pw@example.test/get.php?username=a&password=b",
            "http://192.0.2.10:8080",
            "http://[2001:db8::1]:8080/x",
            "http://localhost/list.m3u",
            "http://xn--bcher-kva.example/",
            "http://bücher.example/",
        ];
        for url in ok {
            assert_eq!(validate_url(url, UrlScheme::Required), Ok(()), "{url}");
        }
        let bad = [
            ("example.test/list.m3u", UrlError::MissingScheme),
            ("ftp://example.test", UrlError::UnsupportedScheme),
            ("rtsp://example.test", UrlError::UnsupportedScheme),
            ("://example.test", UrlError::UnsupportedScheme),
            ("http://exa mple.test", UrlError::ContainsWhitespace),
            ("http://", UrlError::InvalidHost),
            ("http:///path", UrlError::InvalidHost),
            ("http://a..b", UrlError::InvalidHost),
            ("http://-a.test", UrlError::InvalidHost),
            ("http://a_b.test", UrlError::InvalidHost),
            ("http://a.test:", UrlError::InvalidPort),
            ("http://a.test:0", UrlError::InvalidPort),
            ("http://a.test:65536", UrlError::InvalidPort),
            ("http://a.test:80a", UrlError::InvalidPort),
            ("http://a.test:1:2", UrlError::InvalidPort),
            ("http://[2001:db8::1", UrlError::InvalidHost),
            ("http://[zz]", UrlError::InvalidHost),
            ("http://[2001:db8::1]x", UrlError::InvalidHost),
        ];
        for (url, err) in bad {
            assert_eq!(validate_url(url, UrlScheme::Required), Err(err), "{url}");
        }
    }
}
