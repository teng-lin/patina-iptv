//! Xtream Codes client (sans-IO).
//!
//! ```
//! use iptv_providers::xtream::{XtreamClient, parse_live_streams, StreamFormat};
//! use iptv_providers::http::HttpResponse;
//!
//! let client = XtreamClient::new("http://tv.example.com:8080/player_api.php", "demo", "secret").unwrap();
//! let request = client.live_streams(None);           // hand to the network adapter
//! let response = HttpResponse::ok(r#"[{"stream_id":"7","name":"Seven","num":7}]"#);
//! let channels = parse_live_streams(&response).unwrap();
//! let url = client.live_url(7, &StreamFormat::Auto.resolve(&[]));
//! assert_eq!(url, "http://tv.example.com:8080/live/demo/secret/7.m3u8");
//! # assert_eq!(channels.items.len(), 1); let _ = request;
//! ```
//!
//! Compatibility rules (ported from IPTVnator's Xtream contract):
//! - Server URLs accept `http`/`https` only, reject URL userinfo, trim
//!   whitespace and trailing slashes, reduce `…/player_api.php`,
//!   `…/panel_api.php` and `…/get.php` to the base, and keep provider subpaths.
//! - Credentials are trimmed; they are percent-encoded like
//!   `encodeURIComponent` in paths and queries.
//! - Every API request carries a User-Agent; blank overrides fall back to a
//!   VLC signature because some panels sit behind WAFs that allowlist
//!   player clients.
//! - Catch-up `{start}` is the panel's wall clock, not the viewer's; only
//!   when the panel's offset is unknown is the viewer's clock used.

mod parse;

pub use parse::{
    AccountInfo, AccountStatus, CatalogRow, ListKind, ServerInfo, VodDetails, XtreamListParser,
    parse_account_info, parse_categories, parse_epg, parse_live_streams, parse_series_info,
    parse_series_list, parse_vod_info, parse_vod_streams, resolve_account_status,
};

use crate::error::ProviderError;
use crate::http::HttpRequest;
use crate::model::XtreamCredentials;
use crate::secret::Secret;
use crate::time::{catchup_duration_minutes, format_catchup_start};
use crate::urlenc;

/// Fallback User-Agent for API and playback requests.
pub const DEFAULT_USER_AGENT: &str = "VLC/3.0.18 LibVLC/3.0.18";

/// Normalizes an Xtream server address to its portal base URL.
pub fn normalize_server_url(input: &str) -> Result<String, ProviderError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "Xtream URL is required".into(),
        ));
    }
    let url = url::Url::parse(trimmed)
        .map_err(|_| ProviderError::InvalidConfig("Xtream URL is not a valid URL".into()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ProviderError::InvalidConfig(
            "only http and https Xtream URLs are supported".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProviderError::InvalidConfig(
            "URL credentials are not supported".into(),
        ));
    }
    if url.host_str().is_none() {
        return Err(ProviderError::InvalidConfig(
            "Xtream URL has no host".into(),
        ));
    }
    let path = url.path().trim_end_matches('/');
    let lower = path.to_ascii_lowercase();
    let base = ["/player_api.php", "/panel_api.php", "/get.php"]
        .iter()
        .find_map(|suffix| {
            lower
                .ends_with(suffix)
                .then(|| &path[..path.len() - suffix.len()])
        })
        .unwrap_or(path);
    Ok(format!(
        "{}{}",
        url.origin().ascii_serialization(),
        base.trim_end_matches('/')
    ))
}

/// Username and password from a full `get.php`/`player_api.php` URL.
pub fn credentials_from_url(input: &str) -> Option<(String, Secret)> {
    let url = url::Url::parse(input.trim()).ok()?;
    let mut user = None;
    let mut pass = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "username" => user = Some(v.trim().to_owned()),
            "password" => pass = Some(v.trim().to_owned()),
            _ => {}
        }
    }
    let (user, pass) = (user?, pass?);
    (!user.is_empty() && !pass.is_empty()).then(|| (user, Secret::new(pass)))
}

/// The live stream container a user prefers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFormat {
    /// HLS when allowed, else TS, else the first advertised format;
    /// HLS when the panel advertises nothing.
    Auto,
    Ts,
    M3u8,
}

impl StreamFormat {
    /// The extension to use given `user_info.allowed_output_formats`.
    /// A manual choice the panel does not allow falls back to its first format.
    pub fn resolve(self, allowed: &[String]) -> String {
        let allowed: Vec<String> = allowed
            .iter()
            .map(|f| f.trim().to_ascii_lowercase())
            .filter(|f| !f.is_empty())
            .collect();
        let has = |f: &str| allowed.iter().any(|a| a == f);
        match self {
            Self::Auto if allowed.is_empty() || has("m3u8") => "m3u8".into(),
            Self::Auto if has("ts") => "ts".into(),
            Self::Auto => allowed[0].clone(),
            Self::Ts | Self::M3u8 => {
                let wanted = if self == Self::Ts { "ts" } else { "m3u8" };
                if allowed.is_empty() || has(wanted) {
                    wanted.into()
                } else {
                    allowed[0].clone()
                }
            }
        }
    }
}

/// Archive URL shapes panels use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchupVariant {
    /// `/timeshift/U/P/{minutes}/{start}/{id}.ts`
    RestTs,
    /// `/timeshift/U/P/{minutes}/{start}/{id}.m3u8`
    RestM3u8,
    /// `/streaming/timeshift.php?…&extension=ts`
    LegacyTs,
    LegacyM3u8,
    /// `/streaming/timeshift.php?…` without `extension`.
    Legacy,
}

/// Variants to probe (short ranged GET, accept 200/206), in IPTVnator's
/// order: MPEG-TS before HLS when allowed (some panels serve an HLS manifest
/// whose segments fail), REST before legacy, bare legacy last. Fall back to
/// [`CatchupVariant::RestTs`] when no probe succeeds. Cache the winner per
/// source *and* allowed-format set.
pub fn catchup_probe_order(allowed_formats: &[String]) -> Vec<CatchupVariant> {
    let allowed: Vec<String> = allowed_formats
        .iter()
        .map(|f| f.trim().to_ascii_lowercase())
        .collect();
    let known = |f: &str| allowed.is_empty() || allowed.iter().any(|a| a == f);
    let mut out = Vec::new();
    let mut any = false;
    for (ext, rest, legacy) in [
        ("ts", CatchupVariant::RestTs, CatchupVariant::LegacyTs),
        ("m3u8", CatchupVariant::RestM3u8, CatchupVariant::LegacyM3u8),
    ] {
        if known(ext) {
            any = true;
            out.push(rest);
            out.push(legacy);
        }
    }
    if !any {
        out.extend([CatchupVariant::RestTs, CatchupVariant::LegacyTs]);
    }
    out.push(CatchupVariant::Legacy);
    out
}

/// The panel's clock as reported by `server_info`.
///
/// Xtream panels read the timeshift `{start}` and timestamp-less EPG times in
/// their own zone. The zone name is kept for hosts that have a tz database;
/// this crate (no tzdb, wasm-safe) uses [`ServerClock::offset_minutes`]: a
/// fixed-offset name (`UTC`, `UTC+3`, `UTC-04:30`) wins, otherwise the offset
/// derived from `time_now` − `timestamp_now`, snapped to 15 minutes. That
/// snapshot has no DST rules, so refresh it on every account check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerClock {
    pub timezone: Option<String>,
    pub derived_offset_minutes: Option<i32>,
}

impl ServerClock {
    pub fn new(timezone: Option<&str>, time_now: Option<&str>, timestamp_now: Option<i64>) -> Self {
        let derived = match (
            time_now.and_then(|t| crate::time::parse_local_datetime(t, 0)),
            timestamp_now,
        ) {
            (Some(wall), Some(epoch)) if epoch > 0 => {
                let raw = (i128::from(wall) - i128::from(epoch)) as f64 / 60.0;
                let snapped = (raw / 15.0).round() * 15.0;
                (snapped.abs() <= 14.0 * 60.0).then_some(snapped as i32)
            }
            _ => None,
        };
        Self {
            timezone: timezone
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
            derived_offset_minutes: derived,
        }
    }

    /// Best-known offset; `None` means "use the viewer's clock".
    pub fn offset_minutes(&self) -> Option<i32> {
        self.timezone
            .as_deref()
            .and_then(parse_fixed_offset)
            .or(self.derived_offset_minutes)
    }
}

/// `UTC`, `GMT`, `UTC+3`, `UTC+03:00`, `GMT-0430` → minutes. Not `Etc/GMT+3`
/// (POSIX sign inversion) and not IANA names.
fn parse_fixed_offset(name: &str) -> Option<i32> {
    let n = name.trim();
    let rest = n.strip_prefix("UTC").or_else(|| n.strip_prefix("GMT"))?;
    if rest.is_empty() {
        return Some(0);
    }
    let (sign, digits) = match rest.as_bytes()[0] {
        b'+' => (1, &rest[1..]),
        b'-' => (-1, &rest[1..]),
        _ => return None,
    };
    if !digits.bytes().all(|b| b.is_ascii_digit() || b == b':') {
        return None;
    }
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) => (h, m),
        None if digits.len() > 2 => digits.split_at(digits.len() - 2),
        None => (digits, "0"),
    };
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    (h <= 14 && m < 60).then(|| sign * (h * 60 + m))
}

/// An authenticated Xtream account. `Debug` is safe to log.
#[derive(Clone, PartialEq, Eq)]
pub struct XtreamClient {
    base: String,
    username: Secret,
    password: Secret,
    user_agent: String,
}

impl std::fmt::Debug for XtreamClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XtreamClient")
            .field("base", &"[redacted]")
            .field("username", &self.username)
            .field("password", &self.password)
            .field("user_agent", &"[redacted]")
            .finish()
    }
}

impl XtreamClient {
    /// Normalizes the server URL and trims the credentials.
    pub fn new(server_url: &str, username: &str, password: &str) -> Result<Self, ProviderError> {
        let base = normalize_server_url(server_url)?;
        let (username, password) = (username.trim(), password.trim());
        if username.is_empty() || password.is_empty() {
            return Err(ProviderError::InvalidConfig(
                "Xtream username and password are required".into(),
            ));
        }
        Ok(Self {
            base,
            username: Secret::new(username),
            password: Secret::new(password),
            user_agent: DEFAULT_USER_AGENT.into(),
        })
    }

    pub fn from_credentials(c: &XtreamCredentials) -> Result<Self, ProviderError> {
        Ok(
            Self::new(&c.server_url, c.username.expose(), c.password.expose())?
                .with_user_agent(c.user_agent.as_deref()),
        )
    }

    /// Overrides the User-Agent; `None` or blank keeps [`DEFAULT_USER_AGENT`].
    pub fn with_user_agent(mut self, user_agent: Option<&str>) -> Self {
        self.user_agent = user_agent
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .unwrap_or(DEFAULT_USER_AGENT)
            .to_owned();
        self
    }

    /// The normalized portal base (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Public accounts cannot make the relay reach private destinations.
    /// A user-selected LAN account permits only its own stream origin.
    pub fn stream_must_be_public(&self, stream_url: &str) -> bool {
        let private_account = crate::content_url::validate(&self.base).is_err();
        let same_origin = url::Url::parse(&self.base)
            .ok()
            .zip(url::Url::parse(stream_url).ok())
            .is_some_and(|(base, stream)| base.origin() == stream.origin());
        !(private_account && same_origin)
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Headers players should send for this account's streams.
    pub fn playback_headers(&self) -> Vec<(String, String)> {
        vec![("User-Agent".into(), self.user_agent.clone())]
    }

    fn credential_query(&self) -> String {
        urlenc::query(&[
            ("username", self.username.expose()),
            ("password", self.password.expose()),
        ])
    }

    fn with_secrets(&self, req: HttpRequest) -> HttpRequest {
        req.redacting(self.username.expose())
            .redacting(self.password.expose())
            .with_header("User-Agent", self.user_agent.clone())
    }

    fn api(&self, action: Option<&str>, extra: &[(&str, &str)]) -> HttpRequest {
        let mut url = format!("{}/player_api.php?{}", self.base, self.credential_query());
        if let Some(action) = action {
            url.push_str("&action=");
            url.push_str(&urlenc::component(action));
        }
        for (k, v) in extra {
            url.push('&');
            url.push_str(&urlenc::query(&[(k, v)]));
        }
        self.with_secrets(HttpRequest::get(url).with_header("Accept", "application/json"))
    }

    /// `player_api.php` without an action: `user_info` + `server_info`.
    pub fn account_info(&self) -> HttpRequest {
        self.api(None, &[])
    }

    /// Account probes in the order panels answer them: `get_account_info`,
    /// no action, `get_profile`. Try the next on HTTP failure or a body
    /// without `user_info`.
    pub fn account_info_probes(&self) -> Vec<HttpRequest> {
        vec![
            self.api(Some("get_account_info"), &[]),
            self.api(None, &[]),
            self.api(Some("get_profile"), &[]),
        ]
    }

    pub fn live_categories(&self) -> HttpRequest {
        self.api(Some("get_live_categories"), &[])
    }

    pub fn vod_categories(&self) -> HttpRequest {
        self.api(Some("get_vod_categories"), &[])
    }

    pub fn series_categories(&self) -> HttpRequest {
        self.api(Some("get_series_categories"), &[])
    }

    /// All live streams, or one category's.
    pub fn live_streams(&self, category_id: Option<&str>) -> HttpRequest {
        self.list("get_live_streams", category_id)
    }

    pub fn vod_streams(&self, category_id: Option<&str>) -> HttpRequest {
        self.list("get_vod_streams", category_id)
    }

    pub fn series(&self, category_id: Option<&str>) -> HttpRequest {
        self.list("get_series", category_id)
    }

    fn list(&self, action: &str, category_id: Option<&str>) -> HttpRequest {
        match category_id.map(str::trim).filter(|c| !c.is_empty()) {
            Some(cat) => self.api(Some(action), &[("category_id", cat)]),
            None => self.api(Some(action), &[]),
        }
    }

    pub fn series_info(&self, series_id: u64) -> HttpRequest {
        self.api(
            Some("get_series_info"),
            &[("series_id", &series_id.to_string())],
        )
    }

    pub fn vod_info(&self, vod_id: u64) -> HttpRequest {
        self.api(Some("get_vod_info"), &[("vod_id", &vod_id.to_string())])
    }

    /// Next programs for a channel (titles base64; see [`parse_epg`]).
    pub fn short_epg(&self, stream_id: u64, limit: Option<u32>) -> HttpRequest {
        let id = stream_id.to_string();
        match limit {
            Some(limit) => self.api(
                Some("get_short_epg"),
                &[("stream_id", &id), ("limit", &limit.to_string())],
            ),
            None => self.api(Some("get_short_epg"), &[("stream_id", &id)]),
        }
    }

    /// Full provider EPG for a channel, with `has_archive` flags.
    pub fn simple_data_table(&self, stream_id: u64) -> HttpRequest {
        self.api(
            Some("get_simple_data_table"),
            &[("stream_id", &stream_id.to_string())],
        )
    }

    /// The legacy misspelling some panels still require; try it when
    /// [`XtreamClient::simple_data_table`] fails.
    pub fn simple_date_table_legacy(&self, stream_id: u64) -> HttpRequest {
        self.api(
            Some("get_simple_date_table"),
            &[("stream_id", &stream_id.to_string())],
        )
    }

    /// The panel's XMLTV export.
    pub fn xmltv_url(&self) -> String {
        format!("{}/xmltv.php?{}", self.base, self.credential_query())
    }

    pub fn xmltv_request(&self) -> HttpRequest {
        self.with_secrets(HttpRequest::get(self.xmltv_url()))
    }

    /// The panel's own M3U export (`type=m3u_plus`).
    pub fn m3u_playlist_url(&self, output: &str) -> String {
        format!(
            "{}/get.php?{}&type=m3u_plus&output={}",
            self.base,
            self.credential_query(),
            urlenc::component(output)
        )
    }

    fn path_credentials(&self) -> (String, String) {
        (
            urlenc::component(self.username.expose()),
            urlenc::component(self.password.expose()),
        )
    }

    /// `/live/U/P/{id}.{ext}`; pick `ext` with [`StreamFormat::resolve`].
    pub fn live_url(&self, stream_id: u64, ext: &str) -> String {
        let (u, p) = self.path_credentials();
        format!(
            "{}/live/{u}/{p}/{stream_id}.{}",
            self.base,
            ext.trim_start_matches('.')
        )
    }

    /// `/movie/U/P/{id}.{container}`. `None` without a container: the
    /// extension is never guessed (use the item's `direct_source` instead).
    pub fn movie_url(&self, stream_id: u64, container_extension: Option<&str>) -> Option<String> {
        let ext = container_extension
            .map(|e| e.trim().trim_start_matches('.'))
            .filter(|e| !e.is_empty())?;
        let (u, p) = self.path_credentials();
        (stream_id > 0).then(|| format!("{}/movie/{u}/{p}/{stream_id}.{ext}", self.base))
    }

    /// `/series/U/P/{episode_id}.{container}`.
    pub fn episode_url(
        &self,
        episode_id: u64,
        container_extension: Option<&str>,
    ) -> Option<String> {
        let ext = container_extension
            .map(|e| e.trim().trim_start_matches('.'))
            .filter(|e| !e.is_empty())?;
        let (u, p) = self.path_credentials();
        (episode_id > 0).then(|| format!("{}/series/{u}/{p}/{episode_id}.{ext}", self.base))
    }

    /// Archive URL for `[start, end)` (Unix seconds) on a live stream.
    /// `server_offset_minutes` is [`ServerClock::offset_minutes`]; when it is
    /// `None` the `{start}` is written in the viewer's clock,
    /// `viewer_offset_secs` (IPTVnator's fallback), never silently UTC.
    pub fn catchup_url(
        &self,
        stream_id: u64,
        start: i64,
        end: i64,
        variant: CatchupVariant,
        server_offset_minutes: Option<i32>,
        viewer_offset_secs: i32,
    ) -> String {
        let minutes = catchup_duration_minutes(start, end);
        let offset_secs =
            server_offset_minutes.map_or(viewer_offset_secs, |m| m.saturating_mul(60));
        let when = format_catchup_start(start.saturating_add(i64::from(offset_secs)), 0);
        let ext = match variant {
            CatchupVariant::RestTs | CatchupVariant::LegacyTs => Some("ts"),
            CatchupVariant::RestM3u8 | CatchupVariant::LegacyM3u8 => Some("m3u8"),
            CatchupVariant::Legacy => None,
        };
        match variant {
            CatchupVariant::RestTs | CatchupVariant::RestM3u8 => {
                let (u, p) = self.path_credentials();
                format!(
                    "{}/timeshift/{u}/{p}/{minutes}/{when}/{stream_id}.{}",
                    self.base,
                    ext.unwrap_or("ts")
                )
            }
            _ => {
                let (id, dur) = (stream_id.to_string(), minutes.to_string());
                let mut pairs = vec![
                    ("username", self.username.expose()),
                    ("password", self.password.expose()),
                    ("stream", id.as_str()),
                    ("start", when.as_str()),
                    ("duration", dur.as_str()),
                ];
                if let Some(ext) = ext {
                    pairs.push(("extension", ext));
                }
                format!(
                    "{}/streaming/timeshift.php?{}",
                    self.base,
                    urlenc::query(&pairs)
                )
            }
        }
    }

    /// A ranged GET to probe one catch-up variant (accept 200 or 206).
    pub fn catchup_probe(&self, url: String) -> HttpRequest {
        self.with_secrets(HttpRequest::get(url).with_header("Range", "bytes=0-1023"))
    }
}
