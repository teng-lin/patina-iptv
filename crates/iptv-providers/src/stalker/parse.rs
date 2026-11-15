//! Stalker response classification, catalog parsing and link resolution.

use std::collections::HashSet;

#[path = "typed.rs"]
mod typed;

use crate::typed_row::Fields;
use serde_json::Value;

use super::StalkerPortal;
use crate::error::{ProviderError, StalkerError, StalkerErrorKind};
use crate::http::{HttpResponse, json_body};
use crate::id::{ContentId, GroupId};
use crate::lenient::{Obj, list_items, rating_x10, year_from};
use crate::model::{
    CatchUp, CatchUpKind, Channel, ContentKind, Group, Import, Series, SourceKind,
    StalkerLinkFlags, StreamLocator, VodItem, WarningLocation,
};

/// Which authorization failure a portal reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthFailure {
    /// One of the middleware's bare bodies (`Authorization failed.`,
    /// `Access denied.`, `Unauthorized request.`), canonical spelling.
    Body(&'static str),
    /// A panel's JSON envelope (`{js: "…"}`, `{js: {error|msg: "…"}}`).
    Json(String),
}

const BODIES: &[(&str, &str, bool)] = &[
    ("authorization failed", "Authorization failed.", true),
    ("access denied", "Access denied.", false),
    ("unauthorized request", "Unauthorized request.", false),
];

/// Matches the WHOLE trimmed body (≤ 64 bytes), so a proxy page that merely
/// contains "Access denied" is not read as the middleware speaking.
fn classify_body(text: &str) -> Option<&'static str> {
    let body = text.trim();
    if body.is_empty() || body.len() > 64 {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    BODIES.iter().find_map(|(phrase, canonical, counter)| {
        let want: Vec<&str> = phrase.split(' ').collect();
        if words.len() < want.len() || words[..want.len() - 1] != want[..want.len() - 1] {
            return None;
        }
        let last = words[want.len() - 1].trim_end_matches(['.', '!']);
        if last != want[want.len() - 1] {
            return None;
        }
        let rest = &words[want.len()..];
        let ok = rest.is_empty()
            || (*counter && rest.len() == 1 && rest[0].bytes().all(|b| b.is_ascii_digit()));
        ok.then_some(*canonical)
    })
}

/// The wider phrase set accepted inside structured JSON fields.
fn is_auth_phrase(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.len() > 200 {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    [
        "authorization failed",
        "access denied",
        "unauthorized",
        "auth failed",
        "invalid token",
        "authorization",
    ]
    .iter()
    .any(|p| lower.contains(p))
}

/// Detects an authorization failure in any wire shape.
pub fn classify_auth_failure(response: &HttpResponse) -> Option<AuthFailure> {
    let text = response.text();
    if let Some(body) = classify_body(&text) {
        return Some(AuthFailure::Body(body));
    }
    let probe = auth_probe(response);
    probe.message.into_iter().chain(probe.error).find_map(|s| {
        classify_body(&s)
            .map(AuthFailure::Body)
            .or_else(|| is_auth_phrase(&s).then(|| AuthFailure::Json(s.trim().to_owned())))
    })
}

impl AuthFailure {
    pub(crate) fn into_error(self) -> StalkerError {
        let text = match self {
            Self::Body(b) => b.to_owned(),
            Self::Json(s) => s,
        };
        let kind = if text.eq_ignore_ascii_case("Access denied.") {
            StalkerErrorKind::Blocked
        } else {
            StalkerErrorKind::AuthFailed
        };
        StalkerError::with_text(kind, Some(text))
    }
}

/// Endpoint-discovery verdict for one candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeResult {
    /// Token-less `get_genres` returned data: a simple (token-free) panel.
    Data,
    /// The endpoint enforces a token (plain-text failure or HTTP 401/403).
    /// Confirm with a full handshake + profile before saving it as full.
    AuthRequired,
    /// Not a portal endpoint (4xx/5xx, HTML, unrelated JSON): try the next.
    NotAPortal,
}

pub fn classify_probe(response: &HttpResponse) -> ProbeResult {
    if matches!(response.status, 401 | 403) {
        return ProbeResult::AuthRequired;
    }
    if !response.is_success() {
        return ProbeResult::NotAPortal;
    }
    if classify_auth_failure(response).is_some() {
        return ProbeResult::AuthRequired;
    }
    let Ok(body) = json_body(response) else {
        return ProbeResult::NotAPortal;
    };
    match body.get("js") {
        Some(Value::Array(_)) => ProbeResult::Data,
        Some(Value::Object(js))
            if js.get("data").is_some_and(Value::is_array) && !js.contains_key("error") =>
        {
            ProbeResult::Data
        }
        _ => ProbeResult::NotAPortal,
    }
}

/// The decoded parts of a successful `get_profile`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StalkerProfile {
    /// Seconds between `watchdog/get_events` pings (clamped 30–3600, default 120).
    pub watchdog_timeout: u32,
    /// Per-user jitter (seconds) for the first periodic ping.
    pub timeslot: Option<u32>,
    pub account_name: Option<String>,
    /// Expiry/tariff text as the portal formats it (not normalised).
    pub expires: Option<String>,
}

pub(crate) const DEFAULT_WATCHDOG: u32 = 120;

pub(crate) fn watchdog_from(js: Obj<'_>) -> u32 {
    js.u32("watchdog_timeout")
        .filter(|t| (30..=3600).contains(t))
        .unwrap_or(DEFAULT_WATCHDOG)
}

/// Removes `<tags>` and collapses whitespace in a portal message.
pub(crate) fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Combined `msg` + `block_msg`, markup stripped, `None` when both are blank.
pub(crate) fn portal_text(js: Obj<'_>) -> Option<String> {
    let parts: Vec<String> = ["msg", "block_msg"]
        .iter()
        .filter_map(|k| js.str(k))
        .map(|s| strip_markup(&s))
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" — "))
}

pub(crate) fn is_device_conflict(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "device conflict",
        "device_id mismatch",
        "device id mismatch",
        "device_id2 mismatch",
    ]
    .iter()
    .any(|p| lower.contains(p))
}

/// The `js` payload of a portal API response, failing on auth failures and
/// `js.error`.
pub(crate) fn js_payload(response: &HttpResponse) -> Result<Value, ProviderError> {
    if response.is_success()
        && let Some(f) = classify_auth_failure(response)
    {
        return Err(f.into_error().into());
    }
    let mut body = json_body(response)?;
    let js = body
        .get_mut("js")
        .map(Value::take)
        .ok_or_else(|| ProviderError::InvalidBody("missing js".into()))?;
    if let Some(err) = js
        .get("error")
        .and_then(Value::as_str)
        .filter(|e| !e.trim().is_empty())
    {
        return Err(
            StalkerError::with_text(StalkerErrorKind::Portal, Some(strip_markup(err))).into(),
        );
    }
    Ok(js)
}

/// A page of a `get_ordered_list` response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Import<T>,
    pub total_items: Option<u32>,
    /// The portal's page size (often 14).
    pub max_page_items: Option<u32>,
    pub selected_item: Option<u32>,
}

/// Collects `get_ordered_list` pages until done. Stops on an empty page, a
/// page that adds no new ids (portals that ignore `p` repeat page 1), the
/// declared total, `cap` items, or 1000 pages.
pub struct PagedCrawl<T> {
    items: Vec<T>,
    emitted: usize,
    seen: HashSet<String>,
    next: Option<u32>,
    total: Option<u32>,
    cap: usize,
    /// Skips and warnings across pages.
    pub skipped: usize,
}

const MAX_PAGES: u32 = 1000;

impl<T: HasProviderId> PagedCrawl<T> {
    /// `cap` bounds the crawl (IPTVnator uses 30 000 for ITV).
    pub fn new(cap: usize) -> Self {
        Self {
            items: Vec::new(),
            emitted: 0,
            seen: HashSet::new(),
            next: Some(1),
            total: None,
            cap,
            skipped: 0,
        }
    }

    /// The 1-based page to request next, or `None` when complete.
    pub fn next_page(&self) -> Option<u32> {
        self.next
    }

    /// Adds the page fetched for [`PagedCrawl::next_page`].
    pub fn push(&mut self, page: Page<T>) {
        let Some(current) = self.next else { return };
        self.total = self.total.or(page.total_items);
        self.skipped += page.items.skipped;
        let before = self.emitted;
        let was_empty = page.items.items.is_empty();
        for item in page.items.items {
            if self.emitted >= self.cap {
                break;
            }
            if self.seen.insert(item.provider_key()) {
                self.items.push(item);
                self.emitted += 1;
            }
        }
        let added = self.emitted - before;
        let reached_total = self.total.is_some_and(|t| self.emitted >= t as usize);
        self.next = if was_empty
            || added == 0
            || reached_total
            || self.emitted >= self.cap
            || current >= MAX_PAGES
        {
            None
        } else {
            Some(current + 1)
        };
    }

    /// Stops the crawl (e.g. a page failed twice); keeps what was collected.
    pub fn abort(&mut self) {
        self.next = None;
    }

    /// Move completed rows to a sink while retaining only duplicate identities.
    pub fn take_items(&mut self) -> Vec<T> {
        std::mem::take(&mut self.items)
    }

    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}

/// Lets [`PagedCrawl`] de-duplicate by the provider's id.
pub trait HasProviderId {
    fn provider_key(&self) -> String;
}

impl HasProviderId for Channel {
    fn provider_key(&self) -> String {
        self.provider_id
            .clone()
            .unwrap_or_else(|| self.id.to_string())
    }
}

impl HasProviderId for VodItem {
    fn provider_key(&self) -> String {
        self.provider_id
            .clone()
            .unwrap_or_else(|| self.id.to_string())
    }
}

impl HasProviderId for Series {
    fn provider_key(&self) -> String {
        self.provider_id
            .clone()
            .unwrap_or_else(|| self.id.to_string())
    }
}
/// Native `series/get_ordered_list` catalogue; rows need no playback command.
pub fn parse_series_page(response: &HttpResponse) -> Result<Page<Series>, ProviderError> {
    typed::page(response, |o| {
        let id = o.str("id").ok_or("series without id")?;
        Ok(Series {
            id: ContentId::from_provider(SourceKind::Stalker, ContentKind::Series, &id),
            provider_id: Some(id.clone()),
            name: o
                .first_text(&["name", "o_name"])
                .unwrap_or_else(|| format!("Series {id}")),
            groups: o
                .str("category_id")
                .map(|g| GroupId::from_provider(SourceKind::Stalker, ContentKind::Series, &g))
                .into_iter()
                .collect(),
            poster: o.first_text(&["screenshot_uri", "cover_big", "logo"]),
            plot: o.text("description"),
            year: year_from(o.text("year").as_deref()),
            rating_x10: rating_x10(o.f64("rating_imdb").or_else(|| o.f64("rating_kinopoisk"))),
            last_modified: o.i64("added"),
        })
    })
}

/// `itv/get_genres` and `vod|series/get_categories`. The `*` "All" entry is
/// dropped.
pub fn parse_genres(
    response: &HttpResponse,
    kind: ContentKind,
) -> Result<Import<Group>, ProviderError> {
    let js = js_payload(response)?;
    let items = list_items(&js)
        .ok_or_else(|| ProviderError::InvalidBody("genres are not a list".into()))?;
    let mut out = Import::default();
    for (i, item) in items.into_iter().enumerate() {
        let Some(o) = Obj::of(item) else {
            out.skip(WarningLocation::Index(i), "genre is not an object");
            continue;
        };
        let Some(id) = o.str("id") else {
            out.skip(WarningLocation::Index(i), "genre without id");
            continue;
        };
        if id == "*" {
            continue;
        }
        out.items.push(Group {
            id: GroupId::from_provider(SourceKind::Stalker, kind, &id),
            name: o
                .first_text(&["title", "name", "alias"])
                .unwrap_or_else(|| format!("Genre {id}")),
            kind,
            provider_id: Some(id),
            adult: o.flag("censored"),
        });
    }
    Ok(out)
}

fn link_flags(o: impl Fields) -> StalkerLinkFlags {
    let flag = |k: &str| o.has(k).then(|| o.flag(k));
    StalkerLinkFlags {
        use_http_tmp_link: flag("use_http_tmp_link"),
        use_load_balancing: flag("use_load_balancing"),
    }
}

fn row_cmd(o: impl Fields) -> Option<String> {
    o.text("cmd").or_else(|| o.commands())
}

fn channel_row(o: impl Fields) -> Result<Channel, &'static str> {
    let id = o.str("id").ok_or("channel without id")?;
    let cmd = row_cmd(o).ok_or("channel without cmd")?;
    // `tv_archive_duration` is in hours on Stalker (assumption; see crate docs).
    let hours = o.u32("tv_archive_duration").unwrap_or(0);
    let catch_up = (o.flag("tv_archive") && hours > 0).then(|| CatchUp {
        kind: CatchUpKind::Stalker,
        days: Some(hours.div_ceil(24)),
        source: None,
        correction_secs: 0,
    });
    Ok(Channel {
        id: ContentId::from_provider(SourceKind::Stalker, ContentKind::Live, &id),
        name: o.text("name").unwrap_or_else(|| format!("Channel {id}")),
        number: o.u32("number"),
        logo: o.text("logo"),
        groups: o
            .str("tv_genre_id")
            .map(|g| GroupId::from_provider(SourceKind::Stalker, ContentKind::Live, &g))
            .into_iter()
            .collect(),
        stream: StreamLocator::Stalker {
            cmd,
            link: link_flags(o),
        },
        epg_id: o.text("xmltv_id"),
        epg_name: None,
        epg_shift_minutes: None,
        catch_up,
        radio: false,
        provider_id: Some(id),
    })
}

fn rows<T>(
    js: &Value,
    map: impl Fn(Obj<'_>) -> Result<T, &'static str>,
) -> Result<Import<T>, ProviderError> {
    let data = match js {
        Value::Object(o) => o
            .get("data")
            .ok_or_else(|| ProviderError::InvalidBody("missing js.data".into()))?,
        other => other,
    };
    let items = list_items(data)
        .ok_or_else(|| ProviderError::InvalidBody("js.data is not a list".into()))?;
    let mut out = Import::default();
    for (i, item) in items.into_iter().enumerate() {
        match Obj::of(item).ok_or("row is not an object").and_then(&map) {
            Ok(row) => out.items.push(row),
            Err(why) => out.skip(WarningLocation::Index(i), why),
        }
    }
    Ok(out)
}

/// `itv/get_ordered_list` page.
pub fn parse_channel_page(response: &HttpResponse) -> Result<Page<Channel>, ProviderError> {
    typed::page(response, |o| channel_row(o))
}

/// `itv/get_all_channels`: every channel in one response, de-duplicated by
/// id. An empty list usually means "not implemented"; crawl pages instead.
/// Adult genres are often missing here; fetch those with paged requests.
pub fn parse_all_channels(response: &HttpResponse) -> Result<Import<Channel>, ProviderError> {
    let mut out = rows(&js_payload(response)?, |o| channel_row(o))?;
    let mut seen = HashSet::new();
    let before = out.items.len();
    out.items.retain(|c| seen.insert(c.provider_key()));
    out.skipped += before - out.items.len();
    Ok(out)
}

fn vod_row(o: impl Fields) -> Result<VodItem, &'static str> {
    let id = o.str("id").ok_or("movie without id")?;
    let cmd = row_cmd(o).ok_or("movie without cmd")?;
    let episode_numbers: Vec<u32> = o
        .strings("series")
        .iter()
        .filter_map(|n| n.parse().ok())
        .filter(|n| *n > 0)
        .take(20_000)
        .collect();
    let is_series = o.series_flag() || !episode_numbers.is_empty();
    let rating = o.f64("rating_imdb").or_else(|| o.f64("rating_kinopoisk"));
    Ok(VodItem {
        id: ContentId::from_provider(
            SourceKind::Stalker,
            if is_series {
                ContentKind::Series
            } else {
                ContentKind::Movie
            },
            &id,
        ),
        name: o
            .first_text(&["name", "o_name"])
            .unwrap_or_else(|| format!("Movie {id}")),
        groups: o
            .str("category_id")
            .map(|g| GroupId::from_provider(SourceKind::Stalker, ContentKind::Movie, &g))
            .into_iter()
            .collect(),
        poster: o.first_text(&["screenshot_uri", "cover_big", "logo"]),
        plot: o.text("description"),
        year: year_from(o.text("year").as_deref()),
        rating_x10: rating_x10(rating),
        added: o
            .str("added")
            .and_then(|a| crate::time::parse_local_datetime(&a, 0))
            .or_else(|| o.i64("added").filter(|a| *a > 0)),
        duration_secs: o
            .u32("time")
            .filter(|m| *m > 0)
            .map(|m| m.saturating_mul(60)),
        stream: StreamLocator::Stalker {
            cmd,
            link: link_flags(o),
        },
        is_series,
        episode_numbers,
        provider_id: Some(id),
    })
}

/// `vod/get_ordered_list` page. Rows flagged `is_series` (`true`, `1`, `"1"`
/// only) get series ids and `is_series = true`.
pub fn parse_vod_page(response: &HttpResponse) -> Result<Page<VodItem>, ProviderError> {
    typed::page(response, |o| vod_row(o))
}

/// Strips the `<solution> ` prefix (`ffmpeg http://…` → `http://…`) when the
/// remainder is an HTTP URL, an absolute path or a query.
pub fn normalize_cmd(cmd: &str) -> String {
    let t = cmd.trim();
    if let Some((_, rest)) = t.split_once(' ') {
        let rest = rest.trim();
        if has_http_scheme(rest) || rest.starts_with('/') || rest.starts_with('?') {
            return rest.to_owned();
        }
    }
    t.to_owned()
}

fn has_http_scheme(s: &str) -> bool {
    let lower = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn is_portal_local_host(host: &str) -> bool {
    let h = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if matches!(
        h.as_str(),
        "localhost" | "localhost.localdomain" | "0.0.0.0" | "::1" | "::"
    ) || h.ends_with(".localhost")
    {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_loopback() || v4.is_unspecified(),
        Ok(std::net::IpAddr::V6(v6)) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
        }
        Err(_) => false,
    }
}

/// The playable URL when a row does **not** need `create_link`, else `None`.
///
/// `create_link` is needed when either flag is set, when the row carries
/// neither flag key (absence is no evidence), when the command is relative
/// (`/media/…`, `?token=…`) or not HTTP (`ffrt4://…`), or when it points at a
/// portal-local host (`localhost`, `127.0.0.0/8`, `::1`, …). Never use this
/// for series episodes: `series` selects the episode server-side.
pub fn static_playback_url(cmd: &str, link: StalkerLinkFlags) -> Option<String> {
    if link.use_http_tmp_link.is_none() && link.use_load_balancing.is_none() {
        return None;
    }
    if link.use_http_tmp_link == Some(true) || link.use_load_balancing == Some(true) {
        return None;
    }
    let url = normalize_cmd(cmd);
    let parsed = url::Url::parse(&url)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))?;
    let host = parsed.host_str()?;
    (!is_portal_local_host(host)).then_some(url)
}

/// `create_link` → playable URL. A relative reply (`/media/…`) resolves
/// against the portal base; a query-only reply (`?token=…`) is appended to
/// the original command. Temporary links live ~5 s: resolve right before
/// playback and never store the result.
pub fn parse_create_link(
    response: &HttpResponse,
    portal: &StalkerPortal,
    original_cmd: &str,
) -> Result<String, ProviderError> {
    let js = js_payload(response)?;
    let cmd = Obj::of(&js)
        .and_then(|o| o.text("cmd"))
        .ok_or_else(|| ProviderError::InvalidBody("create_link without cmd".into()))?;
    let url = normalize_cmd(&cmd);
    if has_http_scheme(&url) {
        return Ok(url);
    }
    let base = portal.base_url();
    if url.starts_with('?') {
        let original = normalize_cmd(original_cmd);
        return Ok(if has_http_scheme(&original) {
            format!("{original}{url}")
        } else {
            format!("{base}{original}{url}")
        });
    }
    if url.starts_with('/') {
        return Ok(format!("{base}{url}"));
    }
    Err(ProviderError::InvalidBody(
        "create_link returned an unplayable command".into(),
    ))
}

/// Whether `stream_url` belongs to the portal (same host, any port, no
/// https→http downgrade): only those streams may carry portal credentials.
pub(crate) fn is_portal_owned(stream_url: &str, portal: &StalkerPortal) -> bool {
    let (Ok(stream), Ok(endpoint)) = (
        url::Url::parse(stream_url),
        url::Url::parse(portal.endpoint()),
    ) else {
        return false;
    };
    let host = |u: &url::Url| {
        u.host_str()
            .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
    };
    let downgrade = endpoint.scheme() == "https" && stream.scheme() == "http";
    host(&stream).is_some() && host(&stream) == host(&endpoint) && !downgrade
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failure_bodies_are_anchored() {
        let f = |b: &str| classify_auth_failure(&HttpResponse::ok(b.to_owned()));
        assert_eq!(
            f("Authorization failed."),
            Some(AuthFailure::Body("Authorization failed."))
        );
        assert_eq!(
            f(" authorization failed 1234 "),
            Some(AuthFailure::Body("Authorization failed."))
        );
        assert_eq!(
            f("Access denied."),
            Some(AuthFailure::Body("Access denied."))
        );
        assert_eq!(
            f("Unauthorized request!"),
            Some(AuthFailure::Body("Unauthorized request."))
        );
        assert_eq!(f("<html><body>Access denied</body></html>"), None);
        assert_eq!(f("Access denied 12"), None);
        assert_eq!(f(r#"{"js":{"data":[]}}"#), None);
        assert!(matches!(
            f(r#"{"js":{"error":"Invalid token"}}"#),
            Some(AuthFailure::Json(_))
        ));
        assert!(matches!(
            f(r#"{"js":"Authorization failed."}"#),
            Some(AuthFailure::Body(_))
        ));
        assert_eq!(f(r#"{"js":{"error":"Channel not found"}}"#), None);
    }

    #[test]
    fn static_url_decision_table() {
        let flags = |a, b| StalkerLinkFlags {
            use_http_tmp_link: a,
            use_load_balancing: b,
        };
        let cdn = "ffmpeg http://cdn.example.net/live/1.m3u8";
        assert_eq!(
            static_playback_url(cdn, flags(Some(false), Some(false))).as_deref(),
            Some("http://cdn.example.net/live/1.m3u8")
        );
        assert_eq!(
            static_playback_url(cdn, flags(Some(true), Some(false))),
            None
        );
        assert_eq!(static_playback_url(cdn, flags(None, Some(true))), None);
        assert_eq!(
            static_playback_url(cdn, flags(None, None)),
            None,
            "no flag evidence"
        );
        assert_eq!(
            static_playback_url(cdn, flags(Some(false), None)).as_deref(),
            Some("http://cdn.example.net/live/1.m3u8")
        );
        for local in [
            "ffrt3 http://localhost/ch/1",
            "http://127.0.0.2/x",
            "http://[::1]/x",
            "http://[::ffff:127.0.0.1]/x",
            "http://a.localhost./x",
            "http://0.0.0.0/x",
        ] {
            assert_eq!(
                static_playback_url(local, flags(Some(false), Some(false))),
                None,
                "{local}"
            );
        }
        assert_eq!(
            static_playback_url("/media/file_1.mpg", flags(Some(false), Some(false))),
            None
        );
        assert_eq!(
            static_playback_url("ffrt4://ch/live/1", flags(Some(false), Some(false))),
            None
        );
    }

    #[test]
    fn normalizes_solution_prefix() {
        assert_eq!(
            normalize_cmd("ffmpeg http://cdn.example.net/a"),
            "http://cdn.example.net/a"
        );
        assert_eq!(normalize_cmd("auto /media/1.mpg"), "/media/1.mpg");
        assert_eq!(
            normalize_cmd("  http://cdn.example.net/a "),
            "http://cdn.example.net/a"
        );
        assert_eq!(normalize_cmd("ffrt4 ch/1"), "ffrt4 ch/1");
    }

    #[test]
    fn crawl_stops_on_repeat_empty_total_and_cap() {
        let ch = |id: u32| Channel {
            id: ContentId::from_provider(SourceKind::Stalker, ContentKind::Live, &id.to_string()),
            provider_id: Some(id.to_string()),
            name: format!("C{id}"),
            number: None,
            logo: None,
            groups: vec![],
            stream: StreamLocator::Stalker {
                cmd: String::new(),
                link: StalkerLinkFlags::default(),
            },
            epg_id: None,
            epg_name: None,
            epg_shift_minutes: None,
            catch_up: None,
            radio: false,
        };
        let pg = |ids: &[u32], total| Page {
            items: Import {
                items: ids.iter().map(|i| ch(*i)).collect(),
                ..Import::default()
            },
            total_items: total,
            max_page_items: Some(2),
            selected_item: None,
        };
        // Portal ignoring `p`: page 2 repeats page 1.
        let mut c = PagedCrawl::new(100);
        c.push(pg(&[1, 2], Some(10)));
        assert_eq!(c.next_page(), Some(2));
        c.push(pg(&[1, 2], Some(10)));
        assert_eq!(c.next_page(), None);
        assert_eq!(c.into_items().len(), 2);
        // Total reached.
        let mut c = PagedCrawl::new(100);
        c.push(pg(&[1, 2], Some(3)));
        c.push(pg(&[3], Some(3)));
        assert_eq!(c.next_page(), None);
        // Empty page.
        let mut c = PagedCrawl::new(100);
        c.push(pg(&[1, 2], None));
        c.push(pg(&[], None));
        assert_eq!(c.next_page(), None);
        // Cap.
        let mut c = PagedCrawl::new(3);
        c.push(pg(&[1, 2], None));
        c.push(pg(&[3, 4], None));
        assert_eq!(c.next_page(), None);
        assert_eq!(c.into_items().len(), 3);
    }
}

// Auth classification must not materialise a catalog just to look for js.error.
#[derive(Default)]
struct AuthProbe {
    message: Option<String>,
    error: Option<String>,
}
use serde::{
    Deserialize, Deserializer,
    de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
impl<'de> Deserialize<'de> for AuthProbe {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = AuthProbe;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("auth metadata")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(AuthProbe {
                    message: Some(s.into()),
                    error: None,
                })
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut p = AuthProbe::default();
                while let Some(k) = a.next_key::<String>()? {
                    match k.as_str() {
                        "msg" => p.message = a.next_value::<crate::typed_row::Scalar>()?.string(),
                        "error" => p.error = a.next_value::<crate::typed_row::Scalar>()?.string(),
                        _ => {
                            a.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(p)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                while a.next_element::<IgnoredAny>()?.is_some() {}
                Ok(AuthProbe::default())
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(AuthProbe::default())
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(AuthProbe::default())
            }
        }
        d.deserialize_any(V)
    }
}
fn auth_probe(response: &HttpResponse) -> AuthProbe {
    #[derive(Deserialize)]
    struct Envelope {
        js: AuthProbe,
    }
    let bytes = response
        .body
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(&response.body);
    serde_json::from_slice::<Envelope>(bytes)
        .map(|e| e.js)
        .unwrap_or_default()
}
