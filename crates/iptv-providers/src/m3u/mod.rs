//! Streaming M3U/M3U8 playlist parser.
//!
//! Feed bytes in any chunk size with [`M3uParser::feed`]; lines are processed
//! as they complete, so memory is bounded by the output, not the input, and
//! work is linear in the input. [`M3uParser::take_channels`] lets an importer
//! write batches while parsing a large file.
//!
//! Tolerated: UTF-8 BOM, CRLF and bare CR line ends, blank lines, a missing
//! `#EXTM3U`, invalid UTF-8 (replaced), URLs without `#EXTINF` (plain lists).
//! An `#EXTINF` without a following URL is skipped with a warning.
//!
//! Recognized lines: `#EXTM3U` (header attributes: `url-tvg`/`x-tvg-url`/
//! `tvg-url`, `catchup`, `catchup-days`, `catchup-source`,
//! `catchup-correction`, `tvg-shift`),
//! `#EXTINF`, `#EXTGRP`, `#EXTVLCOPT` and `#KODIPROP`. Option lines may come
//! before or after `#EXTINF`; either way they belong to the next URL.

mod catchup;
mod extinf;

use iptv_core::import_limits::{ImportLimits, LimitExceeded, LimitKind};
use std::collections::HashMap;

pub use catchup::{
    CatchupError, CatchupRequest, DEFAULT_CATCHUP_WINDOW_DAYS, build_catchup_url, expand_live_url,
};

use crate::id::{ContentId, GroupId, IdDeduper};
use crate::model::{
    CatchUp, CatchUpKind, Channel, ContentKind, Group, Import, StreamLocator, WarningLocation,
};

/// Playlist-level attributes from `#EXTM3U`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct M3uHeader {
    /// XMLTV URLs from `url-tvg`, `x-tvg-url` and `tvg-url` (comma lists
    /// split, duplicates removed, order kept).
    pub epg_urls: Vec<String>,
    /// Defaults applied to entries that do not declare their own.
    pub catchup: Option<String>,
    pub catchup_days: Option<String>,
    pub catchup_source: Option<String>,
    pub catchup_correction: Option<String>,
    pub tvg_shift: Option<String>,
    /// Every header attribute, lowercased keys, in file order.
    pub attributes: Vec<(String, String)>,
    /// `#EXTM3U` was present.
    pub declared: bool,
    /// HLS tags (`#EXT-X-TARGETDURATION`, `#EXT-X-STREAM-INF`) were seen:
    /// this is a stream manifest, not a channel list.
    pub looks_like_hls: bool,
}

/// The parse result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct M3uPlaylist {
    /// A resource failure invalidates the entire result, including completed rows.
    pub error: Option<LimitExceeded>,
    pub header: M3uHeader,
    /// Groups in first-seen order.
    pub groups: Vec<Group>,
    pub channels: Import<Channel>,
}

#[derive(Default)]
struct Options {
    extgrp: Option<String>,
    headers: Vec<(String, String)>,
    properties: Vec<(String, String)>,
}

struct Pending {
    line: u64,
    attrs: Vec<(String, String)>,
    title: Option<String>,
    opts: Options,
}

/// Incremental parser. See the module docs.
#[derive(Default)]
pub struct M3uParser {
    limits: ImportLimits,
    bytes: u64,
    items: u64,
    error: Option<LimitExceeded>,
    line_no: u64,
    last_cr: bool,
    partial: Vec<u8>,
    header: M3uHeader,
    pending: Option<Pending>,
    /// Option lines seen before the next `#EXTINF`.
    carry: Options,
    groups: Vec<Group>,
    group_index: HashMap<GroupId, usize>,
    out: Import<Channel>,
    dedup: IdDeduper,
}

/// Parses a whole playlist held in memory.
pub fn parse_m3u(text: &str) -> M3uPlaylist {
    let mut p = M3uParser::new();
    p.feed(text.as_bytes());
    p.finish()
}

impl M3uParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(limits: ImportLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Compatibility entry point. Resource failures remain sticky and are
    /// returned in `M3uPlaylist::error`; importers use `try_feed` to stop early.
    pub fn feed(&mut self, chunk: &[u8]) {
        if let Err(error) = self.try_feed(chunk) {
            self.error = Some(error);
        }
    }

    pub fn try_feed(&mut self, chunk: &[u8]) -> Result<(), LimitExceeded> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let result = self.feed_checked(chunk);
        if let Err(error) = result {
            self.error = Some(error);
        }
        result
    }

    fn feed_checked(&mut self, chunk: &[u8]) -> Result<(), LimitExceeded> {
        self.bytes = self.bytes.saturating_add(chunk.len() as u64);
        LimitExceeded::check(
            LimitKind::DecodedBytes,
            self.bytes,
            self.limits.decoded_bytes,
        )?;
        for part in chunk.split_inclusive(|b| matches!(b, b'\n' | b'\r')) {
            if self.last_cr && part == b"\n" {
                self.last_cr = false;
                continue;
            }
            self.last_cr = part.last() == Some(&b'\r');
            LimitExceeded::check(
                LimitKind::RecordBytes,
                self.partial.len().saturating_add(part.len()) as u64,
                self.limits.record_bytes as u64,
            )?;
            self.partial.extend_from_slice(part);
            if part.last().is_some_and(|b| matches!(b, b'\n' | b'\r')) {
                let line = std::mem::take(&mut self.partial);
                self.line(&line);
                if let Some(error) = self.error {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Channels parsed so far, removed from the parser (for batched writes).
    pub fn take_channels(&mut self) -> Vec<Channel> {
        std::mem::take(&mut self.out.items)
    }

    /// The header as seen so far.
    pub fn header(&self) -> &M3uHeader {
        &self.header
    }

    /// Flushes the last line and returns everything not yet taken.
    pub fn finish(mut self) -> M3uPlaylist {
        if self.error.is_none() && !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            self.line(&line);
        }
        if let Some(p) = self.pending.take() {
            self.out.skip(
                WarningLocation::Line(p.line),
                "#EXTINF without a stream URL",
            );
        }
        M3uPlaylist {
            error: self.error,
            header: self.header,
            groups: self.groups,
            channels: self.out,
        }
    }

    fn line(&mut self, raw: &[u8]) {
        self.line_no += 1;
        let text = String::from_utf8_lossy(raw);
        // A bare CR also ends a line (classic Mac files); CRLF leaves an empty tail.
        for part in text.split('\r') {
            let part = if self.line_no == 1 {
                part.trim_start_matches('\u{feff}')
            } else {
                part
            };
            let line = part.trim();
            if !line.is_empty() {
                self.process(line);
            }
        }
    }

    fn process(&mut self, line: &str) {
        if !line.starts_with('#') {
            self.url_line(line);
            return;
        }
        // The tag ends at the first ':' or whitespace (`#EXTM3U url-tvg="http://…"`
        // has no ':' after the tag, but one inside the URL).
        let (tag, rest) = match line.find(|c: char| c == ':' || c.is_whitespace()) {
            Some(i) if line[i..].starts_with(':') => (&line[..i], &line[i + 1..]),
            Some(i) => (&line[..i], &line[i..]),
            None => (line, ""),
        };
        let tag = tag.to_ascii_uppercase();
        match tag.as_str() {
            "#EXTM3U" => self.header_line(rest),
            "#EXTINF" => self.extinf_line(rest),
            "#EXTGRP" => {
                let value = rest.trim();
                if !value.is_empty() {
                    self.opts_mut().extgrp = Some(value.to_owned());
                }
            }
            "#EXTVLCOPT" => {
                if let Some((k, v)) = rest.split_once('=') {
                    let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_owned());
                    let header = match k.as_str() {
                        "http-user-agent" => Some("User-Agent"),
                        "http-referrer" | "http-referer" => Some("Referer"),
                        "http-origin" => Some("Origin"),
                        _ => None,
                    };
                    let opts = self.opts_mut();
                    match header {
                        Some(h) => set_header(&mut opts.headers, h, v),
                        None => opts.properties.push((k, v)),
                    }
                }
            }
            "#KODIPROP" => {
                if let Some((k, v)) = rest.split_once('=') {
                    let (k, v) = (k.trim().to_owned(), v.trim().to_owned());
                    let opts = self.opts_mut();
                    if k.eq_ignore_ascii_case("inputstream.adaptive.stream_headers")
                        || k.eq_ignore_ascii_case("inputstream.adaptive.common_headers")
                    {
                        for (hk, hv) in url::form_urlencoded::parse(v.as_bytes()) {
                            set_header(&mut opts.headers, &hk, hv.into_owned());
                        }
                    }
                    opts.properties.push((k, v));
                }
            }
            "#EXT-X-TARGETDURATION" | "#EXT-X-STREAM-INF" | "#EXT-X-MEDIA-SEQUENCE" => {
                self.header.looks_like_hls = true
            }
            _ => {} // comments and unknown directives
        }
    }

    fn opts_mut(&mut self) -> &mut Options {
        match &mut self.pending {
            Some(p) => &mut p.opts,
            None => &mut self.carry,
        }
    }

    fn header_line(&mut self, rest: &str) {
        self.header.declared = true;
        let (attrs, _) = extinf::parse_attrs(rest, false);
        for (k, v) in attrs {
            match k.as_str() {
                "url-tvg" | "x-tvg-url" | "tvg-url" => {
                    for u in v.split(',').map(str::trim).filter(|u| !u.is_empty()) {
                        if !self.header.epg_urls.iter().any(|e| e == u) {
                            self.header.epg_urls.push(u.to_owned());
                        }
                    }
                }
                "catchup" | "catchup-type" => self.header.catchup = non_empty(&v),
                "catchup-days" => self.header.catchup_days = non_empty(&v),
                "catchup-source" => self.header.catchup_source = non_empty(&v),
                "catchup-correction" => self.header.catchup_correction = non_empty(&v),
                "tvg-shift" => self.header.tvg_shift = non_empty(&v),
                _ => {}
            }
            self.header.attributes.push((k, v));
        }
    }

    fn extinf_line(&mut self, rest: &str) {
        if let Some(p) = self.pending.take() {
            self.out.skip(
                WarningLocation::Line(p.line),
                "#EXTINF without a stream URL",
            );
        }
        let info = extinf::parse_extinf(rest);
        let opts = std::mem::take(&mut self.carry);
        self.pending = Some(Pending {
            line: self.line_no,
            attrs: info.attrs,
            title: info.title.and_then(non_empty),
            opts,
        });
    }

    fn url_line(&mut self, line: &str) {
        self.items = self.items.saturating_add(1);
        if let Err(error) = LimitExceeded::check(LimitKind::Items, self.items, self.limits.items) {
            self.error = Some(error);
            return;
        }
        let pending = self.pending.take().unwrap_or_else(|| Pending {
            line: self.line_no,
            attrs: Vec::new(),
            title: None,
            opts: std::mem::take(&mut self.carry),
        });
        let (url, pipe) = split_pipe(line);
        if url.is_empty() {
            self.out
                .skip(WarningLocation::Line(self.line_no), "empty stream URL");
            return;
        }
        let channel = self.build(pending, url, pipe);
        self.out.items.push(channel);
    }

    fn build(&mut self, p: Pending, url: &str, pipe_headers: Vec<(String, String)>) -> Channel {
        let attr = |key: &str| {
            p.attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
                .filter(|v| !v.is_empty())
        };
        let supplied_name = p
            .title
            .clone()
            .or_else(|| attr("tvg-name").map(str::to_owned));
        // Persisted identities predate the neutral display label. Keep the
        // original fallback as the identity input so refresh keeps overlays.
        let identity_name = supplied_name
            .clone()
            .unwrap_or_else(|| legacy_fallback_name(url));
        let id = self.dedup.unique(ContentId::from_m3u(url, &identity_name));
        let name = supplied_name.unwrap_or_else(|| UNTITLED_CHANNEL_NAME.to_owned());

        let mut groups = Vec::new();
        let titles: Vec<&str> = match attr("group-title") {
            Some(g) => g
                .split(';')
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .collect(),
            None => p.opts.extgrp.as_deref().into_iter().collect(),
        };
        for title in titles {
            let gid = self.group(title);
            if !groups.contains(&gid) {
                groups.push(gid);
            }
        }

        let mut headers = pipe_headers;
        for (k, v) in &p.opts.headers {
            set_header(&mut headers, k, v.clone());
        }
        for (key, header) in [
            ("user-agent", "User-Agent"),
            ("http-user-agent", "User-Agent"),
            ("http-referrer", "Referer"),
            ("referrer", "Referer"),
        ] {
            if let Some(v) = attr(key)
                && !headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(header))
            {
                headers.push((header.to_owned(), v.to_owned()));
            }
        }

        let catch_up = catch_up(
            attr("catchup")
                .or(attr("catchup-type"))
                .or(self.header.catchup.as_deref()),
            attr("catchup-days")
                .or(attr("timeshift"))
                .or(attr("tvg-rec"))
                .or(self.header.catchup_days.as_deref()),
            attr("catchup-source").or(self.header.catchup_source.as_deref()),
            attr("catchup-correction").or(self.header.catchup_correction.as_deref()),
        );
        let shift = attr("tvg-shift")
            .or(self.header.tvg_shift.as_deref())
            .and_then(|s| s.replace(',', ".").parse::<f64>().ok());
        Channel {
            id,
            provider_id: None,
            number: attr("tvg-chno")
                .or(attr("channel-number"))
                .or(attr("tvg-num"))
                .and_then(parse_number),
            logo: attr("tvg-logo").or(attr("logo")).map(str::to_owned),
            groups,
            epg_id: attr("tvg-id").map(str::to_owned),
            epg_name: attr("tvg-name").map(str::to_owned),
            epg_shift_minutes: shift
                .filter(|h| h.abs() <= 24.0)
                .map(|h| (h * 60.0).round() as i32),
            catch_up,
            radio: attr("radio").is_some_and(|r| r.eq_ignore_ascii_case("true") || r == "1"),
            stream: StreamLocator::Url {
                url: url.to_owned(),
                headers,
                properties: p.opts.properties,
            },
            name,
        }
    }

    fn group(&mut self, title: &str) -> GroupId {
        let gid = GroupId::from_m3u_title(title);
        if !self.group_index.contains_key(&gid) {
            self.group_index.insert(gid.clone(), self.groups.len());
            self.groups.push(Group {
                id: gid.clone(),
                name: title.to_owned(),
                kind: ContentKind::Live,
                provider_id: None,
                adult: false,
            });
        }
        gid
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    match headers
        .iter_mut()
        .find(|(h, _)| h.eq_ignore_ascii_case(name))
    {
        Some(slot) => slot.1 = value,
        None => headers.push((name.to_owned(), value)),
    }
}

/// `url|User-Agent=x&Referer=y` → (`url`, headers). Kodi's pipe syntax.
fn split_pipe(line: &str) -> (&str, Vec<(String, String)>) {
    match line.split_once('|') {
        Some((url, opts)) => {
            let mut headers = Vec::new();
            for (k, v) in url::form_urlencoded::parse(opts.as_bytes()) {
                let k = k.trim();
                if !k.is_empty() {
                    let name = match k.to_ascii_lowercase().as_str() {
                        "user-agent" => "User-Agent".to_owned(),
                        "referer" | "referrer" => "Referer".to_owned(),
                        "origin" => "Origin".to_owned(),
                        _ => k.to_owned(),
                    };
                    set_header(&mut headers, &name, v.trim().to_owned());
                }
            }
            (url.trim(), headers)
        }
        None => (line, Vec::new()),
    }
}

fn parse_number(s: &str) -> Option<u32> {
    let s = s.trim();
    s.parse::<u32>().ok().or_else(|| {
        s.parse::<f64>()
            .ok()
            .filter(|f| *f >= 0.0 && f.fract() == 0.0 && *f < 4e9)
            .map(|f| f as u32)
    })
}

/// Display label for M3U entries with no title or `tvg-name`.
pub const UNTITLED_CHANNEL_NAME: &str = "Unnamed channel";

/// Legacy URL-derived name, retained only as an input to persisted content IDs
/// and to recognize old imported labels during database upgrade.
pub fn legacy_fallback_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    let stem = last.split_once('.').map_or(last, |(s, _)| s);
    if stem.is_empty() {
        url.to_owned()
    } else {
        stem.to_owned()
    }
}

/// Catch-up from the entry's attributes (header defaults already merged).
///
/// Days come from `catchup-days`, then `timeshift`, then `tvg-rec`. A declared
/// type or source without days keeps catch-up with an unknown window
/// (`days: None`, Kodi's behavior; IPTVnator drops it). Without a type, a
/// source means `default` (Kodi) and days alone mean `shift` (IPTVnator).
/// Non-positive or unparsable days count as unknown.
fn catch_up(
    kind: Option<&str>,
    days: Option<&str>,
    source: Option<&str>,
    correction: Option<&str>,
) -> Option<CatchUp> {
    let days = days
        .and_then(parse_decimal)
        .filter(|d| *d > 0.0)
        .map(|d| d.ceil().min(f64::from(u16::MAX)) as u32);
    let source = source.and_then(non_empty);
    let kind = match kind.map(|k| k.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") if source.is_some() => CatchUpKind::Default,
        None | Some("") if days.is_some() => CatchUpKind::Shift,
        None | Some("") => return None,
        Some("default") => CatchUpKind::Default,
        Some("append") => CatchUpKind::Append,
        Some("shift" | "timeshift") => CatchUpKind::Shift,
        Some("flussonic" | "flussonic-hls" | "flussonic-ts" | "fs") => CatchUpKind::Flussonic,
        Some("xc") => CatchUpKind::Xtream,
        Some("vod") => CatchUpKind::Vod,
        Some(other) => CatchUpKind::Unsupported(other.to_owned()),
    };
    // Kodi's setting range for the correction is −12…+14 hours.
    let correction_secs = correction
        .and_then(parse_decimal)
        .filter(|h| (-12.0..=14.0).contains(h))
        .map_or(0, |h| (h * 3600.0).round() as i32);
    Some(CatchUp {
        kind,
        days,
        source,
        correction_secs,
    })
}

/// `2.5` or `2,5`; finite values only.
fn parse_decimal(s: &str) -> Option<f64> {
    s.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests;
