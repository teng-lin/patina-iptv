//! Stalker/Ministra portal client (sans-IO).
//!
//! ```
//! use iptv_providers::http::HttpResponse;
//! use iptv_providers::stalker::{AuthStep, MacAddress, PortalMode, StalkerPortal, StalkerSession, endpoint_candidates};
//!
//! let endpoint = &endpoint_candidates("http://portal.example.com/stalker_portal/c/").unwrap()[0];
//! let mac = MacAddress::parse("00-1a-79-00-00-01").unwrap();
//! let portal = StalkerPortal::new(endpoint, mac, PortalMode::Full).unwrap();
//! let mut session = StalkerSession::new(portal);
//! let AuthStep::Send(handshake) = session.poll() else { unreachable!() };
//! session.on_auth_response(&HttpResponse::ok(r#"{"js":{"token":"T1","random":"r"}}"#)).unwrap();
//! let AuthStep::Send(profile) = session.poll() else { unreachable!() };
//! session.on_auth_response(&HttpResponse::ok(r#"{"js":{"status":0,"watchdog_timeout":60}}"#)).unwrap();
//! assert!(matches!(session.poll(), AuthStep::Ready));
//! let genres = session.genres().unwrap();
//! # let _ = (handshake, profile, genres);
//! ```
//!
//! Wire contract (ported from IPTVnator's Stalker contract):
//! - Identity: cookie `mac=…; stb_lang=…; timezone=…`, MAG250
//!   `User-Agent`/`X-User-Agent`, `Authorization: Bearer` for full portals,
//!   `SN` only when a real serial is configured (full mode). `sn` travels only
//!   on `get_profile`.
//! - Query: `cmd` uses the reference STB encoding (`%` passes through, URL-safe
//!   punctuation stays raw, `&`/`#`/`;`/space/non-ASCII are escaped); every
//!   other value is `encodeURIComponent`-encoded; `JsHttpRequest=1-xml` is
//!   always present; any query on the endpoint URL is dropped.
//! - Auth failures arrive as HTTP 200 with a bare text body; see
//!   [`classify_auth_failure`].
//! - The MAC is the account key: [`MacAddress::parse`] normalizes typed input,
//!   [`MacAddress::from_stored`] keeps a stored value byte-for-byte.

mod parse;
mod series;
mod session;
pub use series::{SeriesRow, parse_series_rows};

pub use parse::{
    AuthFailure, Page, PagedCrawl, ProbeResult, StalkerProfile, classify_auth_failure,
    classify_probe, normalize_cmd, parse_all_channels, parse_channel_page, parse_create_link,
    parse_genres, parse_series_page, parse_vod_page, static_playback_url,
};
pub use session::{ApiError, AuthStep, Resolve, SessionState, StalkerSession};

use crate::error::ProviderError;
use crate::http::HttpRequest;
pub use crate::model::MacAddress;
use crate::model::StalkerCredentials;
use crate::secret::Secret;
use crate::urlenc;

/// MAG250 User-Agent, sent as `User-Agent` and `X-User-Agent`.
pub const MAG_USER_AGENT: &str =
    "Mozilla/5.0 (QtEmbedded; U; Linux; C) AppleWebKit/533.3 (KHTML, like Gecko) MAG250";
pub use crate::model::INFOMIR_OUI;
/// A serial older clients sent when the user left the field blank; treated
/// as absent.
const LEGACY_DEFAULT_SERIAL: &str = "BEDACD4569BAF";

/// The constant MAG250 description sent on `get_profile`. Describes the
/// emulated box (consistent with [`MAG_USER_AGENT`]), never the account.
const STB_PROFILE: &[(&str, &str)] = &[
    (
        "ver",
        "ImageDescription: 0.2.18-r14-pub-250; ImageDate: Fri Jan 15 15:20:44 EET 2016; PORTAL version: 5.6.0; API Version: JS API version: 328; STB API version: 134; Player Engine version: 0x566",
    ),
    ("stb_type", "MAG250"),
    ("hw_version", "1.7-BD-00"),
    ("image_version", "218"),
    ("client_type", "STB"),
    ("num_banks", "2"),
    ("video_out", "hdmi"),
    ("hd", "1"),
];

/// Whether the portal enforces the token lifecycle. Mode follows observed
/// behavior (discovery), not URL shape; [`is_full_portal_url`] is only a
/// fallback for configurations saved without a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalMode {
    /// Canonical Stalker/Ministra: handshake, Bearer token, watchdog.
    Full,
    /// Token-free reseller panel: identity cookie and headers only.
    Simple,
}

/// Legacy URL-shape guess: `/stalker_portal` or `/server/load.php`.
pub fn is_full_portal_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("/stalker_portal") || lower.trim_end_matches('/').ends_with("/server/load.php")
}

/// API endpoints to probe for a typed portal address, in order: the URL
/// itself when it names a `.php` file, then `{base}/portal.php`,
/// `{base}/server/load.php`, `{base}/stalker_portal/server/load.php` (unless
/// the base already is a `stalker_portal`). `{base}` drops a trailing `/c`
/// (the browser entry point users copy) or the pasted `.php` file.
///
/// Probe each with [`StalkerPortal::probe_request`] and
/// [`classify_probe`]: `Data` proves a token-free panel, `AuthRequired`
/// means confirm with a real handshake + profile. Any HTTP status or a
/// timeout moves to the next candidate; a connection failure (refused,
/// unresolvable) aborts, since every candidate shares the host.
pub fn endpoint_candidates(input: &str) -> Result<Vec<String>, ProviderError> {
    let mut url = url::Url::parse(input.trim())
        .map_err(|_| ProviderError::InvalidConfig("portal URL is not a valid URL".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ProviderError::InvalidConfig(
            "portal URL needs an http:// or https:// host".into(),
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    let mut path = url.path().trim_end_matches('/').to_owned();
    if path.to_ascii_lowercase().ends_with("/index.html") {
        path.truncate(path.len() - "/index.html".len());
    }
    let lower = path.to_ascii_lowercase();
    let mut out = Vec::new();
    let with_path = |p: &str| {
        let mut u = url.clone();
        u.set_path(p);
        u.to_string()
    };
    let base = if lower.ends_with(".php") {
        out.push(with_path(&path));
        if lower.ends_with("/server/load.php") {
            path[..path.len() - "/server/load.php".len()].to_owned()
        } else {
            path[..path.rfind('/').unwrap_or(0)].to_owned()
        }
    } else if lower.ends_with("/c") {
        path[..path.len() - 2].to_owned()
    } else {
        path.clone()
    };
    out.push(with_path(&format!("{base}/portal.php")));
    out.push(with_path(&format!("{base}/server/load.php")));
    if !base
        .to_ascii_lowercase()
        .split('/')
        .any(|s| s == "stalker_portal")
    {
        out.push(with_path(&format!("{base}/stalker_portal/server/load.php")));
    }
    let mut unique = Vec::new();
    for c in out {
        if !unique.contains(&c) {
            unique.push(c);
        }
    }
    Ok(unique)
}

/// Encodes a `cmd` value the way a real STB sends it (see module docs).
pub fn encode_cmd(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        let safe = c.is_ascii_alphanumeric() || "-_.~!*()/:?@$,+=[]%".contains(c);
        if safe {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// SHA-1 of the uppercased MAC, uppercase hex: the `prehash` STBs send.
pub(crate) fn prehash(mac: &str) -> String {
    sha1_smol::Sha1::from(mac.to_ascii_uppercase())
        .digest()
        .to_string()
        .to_ascii_uppercase()
}

fn normalize_serial(serial: Option<&Secret>) -> Option<Secret> {
    serial
        .map(Secret::trimmed)
        .filter(|s| !s.is_empty() && !s.expose().eq_ignore_ascii_case(LEGACY_DEFAULT_SERIAL))
}

fn opt_secret(s: Option<&Secret>) -> Option<Secret> {
    s.map(Secret::trimmed).filter(|s| !s.is_empty())
}

/// One portal account: endpoint, mode and device identity.
#[derive(Clone, PartialEq, Eq)]
pub struct StalkerPortal {
    endpoint: url::Url,
    mode: PortalMode,
    mac: MacAddress,
    serial: Option<Secret>,
    device_id: Option<Secret>,
    device_id2: Option<Secret>,
    signature: Option<Secret>,
    signature2: Option<Secret>,
    login: Option<Secret>,
    password: Option<Secret>,
    language: String,
    timezone: String,
}

impl std::fmt::Debug for StalkerPortal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StalkerPortal")
            .field("endpoint", &"[redacted]")
            .field("mode", &self.mode)
            .field("mac", &self.mac)
            .finish_non_exhaustive()
    }
}

impl StalkerPortal {
    /// `endpoint` is a resolved API endpoint (see [`endpoint_candidates`]).
    pub fn new(endpoint: &str, mac: MacAddress, mode: PortalMode) -> Result<Self, ProviderError> {
        let mut endpoint = url::Url::parse(endpoint.trim())
            .map_err(|_| ProviderError::InvalidConfig("portal endpoint is not a URL".into()))?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
            return Err(ProviderError::InvalidConfig(
                "portal endpoint needs an http(s) host".into(),
            ));
        }
        if mac.expose().trim().is_empty() {
            return Err(ProviderError::InvalidConfig(
                "MAC address is required".into(),
            ));
        }
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(Self {
            endpoint,
            mode,
            mac,
            serial: None,
            device_id: None,
            device_id2: None,
            signature: None,
            signature2: None,
            login: None,
            password: None,
            language: "en".into(),
            timezone: "UTC".into(),
        })
    }

    /// Builds a portal from stored credentials plus a discovered endpoint/mode.
    pub fn from_credentials(
        c: &StalkerCredentials,
        endpoint: &str,
        mode: PortalMode,
    ) -> Result<Self, ProviderError> {
        let mut p = Self::new(endpoint, c.mac.clone(), mode)?;
        p.serial = normalize_serial(c.serial.as_ref());
        p.device_id = opt_secret(c.device_id.as_ref());
        p.device_id2 = opt_secret(c.device_id2.as_ref());
        p.signature = opt_secret(c.signature.as_ref());
        p.signature2 = opt_secret(c.signature2.as_ref());
        p.login = opt_secret(c.login.as_ref());
        p.password = opt_secret(c.password.as_ref());
        Ok(p)
    }

    /// Optional serial; blank and the legacy default count as absent.
    pub fn with_serial(mut self, serial: Option<Secret>) -> Self {
        self.serial = normalize_serial(serial.as_ref());
        self
    }

    /// User-supplied device ids. Never generated or copied from each other:
    /// the portal pins the first non-empty pair it sees.
    pub fn with_device_ids(
        mut self,
        device_id: Option<Secret>,
        device_id2: Option<Secret>,
    ) -> Self {
        self.device_id = opt_secret(device_id.as_ref());
        self.device_id2 = opt_secret(device_id2.as_ref());
        self
    }

    pub fn with_signatures(
        mut self,
        signature: Option<Secret>,
        signature2: Option<Secret>,
    ) -> Self {
        self.signature = opt_secret(signature.as_ref());
        self.signature2 = opt_secret(signature2.as_ref());
        self
    }

    /// Login for portals whose profile answers status 2.
    pub fn with_login(mut self, login: Option<Secret>, password: Option<Secret>) -> Self {
        self.login = opt_secret(login.as_ref());
        self.password = opt_secret(password.as_ref());
        self
    }

    /// `stb_lang` cookie (default `en`).
    pub fn with_language(mut self, language: &str) -> Self {
        self.language = language.trim().to_owned();
        self
    }

    /// `timezone` cookie; must be a valid PHP zone (default `UTC`). The
    /// portal renders EPG wall-clock times in it.
    pub fn with_timezone(mut self, timezone: &str) -> Self {
        self.timezone = timezone.trim().to_owned();
        self
    }

    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub fn mode(&self) -> PortalMode {
        self.mode
    }

    pub fn mac(&self) -> &MacAddress {
        &self.mac
    }

    pub(crate) fn has_login(&self) -> bool {
        self.login.is_some() && self.password.is_some()
    }

    /// `origin + base path` for resolving relative `cmd`s: the endpoint
    /// without `portal.php` / `server/load.php` / `*.php`.
    pub fn base_url(&self) -> String {
        let path = self.endpoint.path();
        let lower = path.to_ascii_lowercase();
        let base = if lower.ends_with("/server/load.php") {
            &path[..path.len() - "/server/load.php".len()]
        } else if lower.ends_with(".php") {
            &path[..path.rfind('/').unwrap_or(0)]
        } else {
            path.trim_end_matches('/')
        };
        format!("{}{}", self.endpoint.origin().ascii_serialization(), base)
    }

    /// The identity cookie: `mac`, `stb_lang`, `timezone`.
    pub fn cookie(&self) -> String {
        format!(
            "mac={}; stb_lang={}; timezone={}",
            self.mac.expose(),
            self.language,
            self.timezone
        )
    }

    /// Builds a portal API request. `token` adds `Authorization: Bearer`.
    pub(crate) fn request(&self, params: &[(&str, &str)], token: Option<&Secret>) -> HttpRequest {
        let mut query = String::new();
        let mut has_js_http = false;
        for (k, v) in params {
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str(&urlenc::component(k));
            query.push('=');
            if *k == "cmd" {
                query.push_str(&encode_cmd(v));
            } else {
                query.push_str(&urlenc::component(v));
            }
            has_js_http |= *k == "JsHttpRequest";
        }
        if !has_js_http {
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str("JsHttpRequest=1-xml");
        }
        let url = format!(
            "{}{}?{query}",
            self.endpoint.origin().ascii_serialization(),
            self.endpoint.path()
        );
        let mut req = HttpRequest::get(url)
            .with_header("Cookie", self.cookie())
            .with_header("User-Agent", MAG_USER_AGENT)
            .with_header("X-User-Agent", MAG_USER_AGENT)
            .with_header("Accept", "*/*")
            .redacting(self.mac.expose());
        if self.mode == PortalMode::Full {
            if let Some(serial) = &self.serial {
                req = req
                    .with_header("SN", serial.expose())
                    .redacting(serial.expose());
            }
            if let Some(token) = token {
                req = req
                    .with_header("Authorization", format!("Bearer {}", token.expose()))
                    .redacting(token.expose());
            }
        }
        for s in [
            &self.device_id,
            &self.device_id2,
            &self.signature,
            &self.signature2,
            &self.login,
            &self.password,
        ]
        .into_iter()
        .flatten()
        {
            req = req.redacting(s.expose());
        }
        req
    }

    /// A token-less `itv/get_genres` for endpoint discovery.
    pub fn probe_request(&self) -> HttpRequest {
        self.request(&[("type", "itv"), ("action", "get_genres")], None)
    }

    pub(crate) fn handshake_request(&self, present_token: Option<&Secret>) -> HttpRequest {
        let prehash = prehash(self.mac.expose());
        let token = present_token.map(Secret::expose).unwrap_or("");
        let req = self.request(
            &[
                ("type", "stb"),
                ("action", "handshake"),
                ("token", token),
                ("prehash", &prehash),
            ],
            None,
        );
        match present_token {
            Some(t) => req.redacting(t.expose()),
            None => req,
        }
    }

    pub(crate) fn profile_request(
        &self,
        token: &Secret,
        random: &str,
        second_step: bool,
        not_valid: bool,
    ) -> HttpRequest {
        let mut metrics = serde_json::Map::new();
        metrics.insert("mac".into(), self.mac.expose().into());
        if let Some(sn) = &self.serial {
            metrics.insert("sn".into(), sn.expose().into());
        }
        metrics.insert("model".into(), "MAG250".into());
        metrics.insert("type".into(), "STB".into());
        metrics.insert("random".into(), random.into());
        let metrics = serde_json::Value::Object(metrics).to_string();
        let prehash = prehash(self.mac.expose());
        let mut params: Vec<(&str, &str)> = vec![("type", "stb"), ("action", "get_profile")];
        params.extend_from_slice(STB_PROFILE);
        params.push(("not_valid_token", if not_valid { "1" } else { "0" }));
        params.push(("auth_second_step", if second_step { "1" } else { "0" }));
        params.push(("metrics", &metrics));
        let optional = [
            ("sn", &self.serial),
            ("device_id", &self.device_id),
            ("device_id2", &self.device_id2),
            ("signature", &self.signature),
            ("signature2", &self.signature2),
        ];
        for (k, v) in &optional {
            if let Some(v) = v {
                params.push((k, v.expose()));
            }
        }
        params.push(("prehash", &prehash));
        self.request(&params, Some(token))
    }

    pub(crate) fn do_auth_request(&self, token: &Secret) -> HttpRequest {
        let login = self.login.as_ref().map(Secret::expose).unwrap_or("");
        let password = self.password.as_ref().map(Secret::expose).unwrap_or("");
        let mut params: Vec<(&str, &str)> = vec![
            ("type", "stb"),
            ("action", "do_auth"),
            ("login", login),
            ("password", password),
        ];
        if let Some(d) = &self.device_id {
            params.push(("device_id", d.expose()));
        }
        if let Some(d) = &self.device_id2 {
            params.push(("device_id2", d.expose()));
        }
        self.request(&params, Some(token))
    }
}
