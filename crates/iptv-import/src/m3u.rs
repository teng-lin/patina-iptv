//! M3U import: streamed download (or text in memory) → [`M3uParser`].
//!
//! Gzip-compressed files (`.m3u.gz`, served without `Content-Encoding`) are
//! detected by their magic bytes. Classification after parsing keeps
//! failures apart from empty playlists: an HLS manifest, HTML/JSON error
//! pages and all-malformed files are [`ParseProblem`]s; only a well-formed
//! playlist without entries is [`ImportError::Empty`].

use iptv_core::import_limits::{DecodeBuffer, ImportLimits, LimitExceeded, LimitKind};
use std::{collections::HashSet, io::Write};

use flate2::write::MultiGzDecoder;
use iptv_providers::m3u::{M3uParser, M3uPlaylist};
use iptv_providers::model::{Channel, ContentKind, M3uSourceConfig, StreamLocator};
use iptv_providers::{GroupId, HttpRequest};

use crate::cancel::{CancelToken, check};
use crate::catalog::{Catalog, SourceMeta};
use crate::ctx::Ctx;
use crate::error::{ImportError, ParseProblem, Step};
use crate::fetch::Fetch;
use crate::progress::{Phase, Progress, ProgressSink, REPORT_EVERY_BYTES};

/// Leading decoded bytes kept to recognize HTML/JSON bodies.
const SNIFF: usize = 512;

/// Decodes (optionally gzip) and parses a playlist fed in chunks.
pub(crate) struct M3uFeeder {
    parser: M3uParser,
    head: Vec<u8>,
    gzip: Option<Box<MultiGzDecoder<DecodeBuffer>>>,
    decided: bool,
    sniff: Vec<u8>,
    /// Channels moved out of the parser as they complete (progress counts).
    taken: Vec<Channel>,
    limits: ImportLimits,
    bytes: u64,
    parsed: u64,
    unusable_streams: usize,
    usable_groups: HashSet<GroupId>,
}

impl M3uFeeder {
    pub fn new(limits: ImportLimits) -> Self {
        Self {
            parser: M3uParser::with_limits(limits),
            limits,
            bytes: 0,
            parsed: 0,
            unusable_streams: 0,
            usable_groups: HashSet::new(),
            head: Vec::new(),
            gzip: None,
            decided: false,
            sniff: Vec::new(),
            taken: Vec::new(),
        }
    }

    /// Feeds raw bytes; returns the number of channels parsed so far.
    pub fn push(&mut self, bytes: &[u8]) -> Result<u64, ImportError> {
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        LimitExceeded::check(LimitKind::InputBytes, self.bytes, self.limits.input_bytes)
            .map_err(limit_error)?;
        if self.decided {
            self.decoded(bytes)?;
        } else {
            self.head.extend_from_slice(bytes);
            if self.head.len() >= 2 {
                self.decide();
                let head = std::mem::take(&mut self.head);
                self.decoded(&head)?;
            }
        }
        let mut completed = self.parser.take_channels();
        self.unusable_streams += retain_usable_streams(&mut completed, &mut self.usable_groups);
        self.parsed += completed.len() as u64;
        self.taken.append(&mut completed);
        Ok(self.parsed)
    }

    fn decide(&mut self) {
        self.decided = true;
        if self.head.starts_with(&[0x1f, 0x8b]) {
            self.gzip = Some(Box::new(MultiGzDecoder::new(DecodeBuffer::new(
                self.limits.decoded_bytes,
            ))));
        }
    }

    fn decoded(&mut self, bytes: &[u8]) -> Result<(), ImportError> {
        match &mut self.gzip {
            None => text(&mut self.sniff, &mut self.parser, bytes)?,
            Some(gz) => {
                // Small slices bound the inflated buffer.
                for piece in bytes.chunks(16 * 1024) {
                    gz.write_all(piece).map_err(|e| {
                        gz.get_ref()
                            .limit_error()
                            .map_or_else(|| gzip_error(e), limit_error)
                    })?;
                    let out = gz.get_mut().take();
                    text(&mut self.sniff, &mut self.parser, &out)?;
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(M3uPlaylist, Vec<u8>, usize), ImportError> {
        if !self.decided {
            self.decide();
            let head = std::mem::take(&mut self.head);
            self.decoded(&head)?;
        }
        if let Some(mut gz) = self.gzip.take() {
            gz.try_finish().map_err(|e| {
                gz.get_ref()
                    .limit_error()
                    .map_or_else(|| gzip_error(e), limit_error)
            })?;
            let out = gz.get_mut().take();
            text(&mut self.sniff, &mut self.parser, &out)?;
        }
        let mut playlist = self.parser.finish();
        if let Some(error) = playlist.error {
            return Err(limit_error(error));
        }
        self.unusable_streams +=
            retain_usable_streams(&mut playlist.channels.items, &mut self.usable_groups);
        playlist
            .groups
            .retain(|group| self.usable_groups.contains(&group.id));
        self.taken.append(&mut playlist.channels.items);
        playlist.channels.items = self.taken;
        Ok((playlist, self.sniff, self.unusable_streams))
    }
}

/// Discard unsupported protocols and malformed remote addresses before they
/// reach a private stage. Explicit local destinations still reach the existing
/// fatal destination-policy checks, as do private HTTP(S) hosts.
fn retain_usable_streams(
    channels: &mut Vec<Channel>,
    usable_groups: &mut HashSet<GroupId>,
) -> usize {
    let before = channels.len();
    channels.retain(|channel| {
        let StreamLocator::Url { url, .. } = &channel.stream else {
            return true;
        };
        let value = url.trim();
        let bytes = value.as_bytes();
        let drive_path = bytes.first().is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(1) == Some(&b':')
            && bytes.get(2).is_some_and(|b| matches!(b, b'/' | b'\\'));
        let file_url = value
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"));
        let relative_path = ["./", "../", ".\\", "..\\"]
            .iter()
            .any(|prefix| value.starts_with(prefix));
        if value.starts_with(['/', '\\']) || drive_path || file_url || relative_path {
            return true;
        }
        url::Url::parse(value).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
    });
    for group in channels.iter().flat_map(|channel| &channel.groups) {
        if !usable_groups.contains(group) {
            usable_groups.insert(group.clone());
        }
    }
    before - channels.len()
}

/// Decoded playlist text: keep the first bytes for sniffing, then parse.
fn limit_error(error: LimitExceeded) -> ImportError {
    ImportError::Limit {
        step: Step::Playlist,
        error,
    }
}

fn text(sniff: &mut Vec<u8>, parser: &mut M3uParser, bytes: &[u8]) -> Result<(), ImportError> {
    if sniff.len() < SNIFF {
        let n = (SNIFF - sniff.len()).min(bytes.len());
        sniff.extend_from_slice(&bytes[..n]);
    }
    parser.try_feed(bytes).map_err(limit_error)
}

fn gzip_error(e: std::io::Error) -> ImportError {
    ImportError::parse(
        Step::Playlist,
        ParseProblem::Decode(format!("corrupt gzip playlist: {e}")),
    )
}

/// Turns a parsed playlist into a catalog, or explains why it is not one.
fn into_catalog(
    mut playlist: M3uPlaylist,
    sniff: &[u8],
    stats: crate::catalog::ImportStats,
    allow_private_fixture: bool,
    emitted: u64,
    unusable_streams: usize,
) -> Result<Catalog, ImportError> {
    let fail = |p| Err(ImportError::parse(Step::Playlist, p));
    let head = String::from_utf8_lossy(sniff);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    let dropped_guides = crate::catalog::limit_header_guides(&mut playlist.header);
    let header = &playlist.header;
    if header.looks_like_hls {
        return fail(ParseProblem::HlsManifest);
    }
    if !header.declared {
        if head.is_empty() {
            return Err(ImportError::Empty {
                step: Step::Playlist,
            });
        }
        // Without `#EXTM3U` every non-comment line is read as a URL, so an
        // error page would otherwise "parse".
        let any_url = playlist.channels.items.iter().any(|c| match &c.stream {
            StreamLocator::Url { url, .. } => url.contains("://"),
            _ => false,
        });
        if head.starts_with(['<', '{', '[']) || (!any_url && emitted == 0) {
            return fail(ParseProblem::NotAPlaylist);
        }
    }
    let channels = &playlist.channels;
    if channels.items.is_empty() && emitted == 0 {
        let skipped = channels.skipped + unusable_streams;
        return if skipped > 0 {
            fail(ParseProblem::NoUsableEntries { skipped })
        } else {
            Err(ImportError::Empty {
                step: Step::Playlist,
            })
        };
    }
    if !allow_private_fixture {
        for url in &playlist.header.epg_urls {
            iptv_providers::content_url::validate(url).map_err(|_| {
                ImportError::InvalidConfig(
                    "playlist guide URL must name a public HTTP(S) host".into(),
                )
            })?;
        }
        for channel in &playlist.channels.items {
            if let StreamLocator::Url { url, .. } = &channel.stream {
                iptv_providers::content_url::validate(url).map_err(|_| {
                    ImportError::InvalidConfig(
                        "playlist stream URL must name a public HTTP(S) host".into(),
                    )
                })?;
            }
        }
    }
    let mut catalog = Catalog::new(SourceMeta::M3u {
        header: playlist.header,
    });
    catalog.warn_guide_limit(dropped_guides);
    catalog.stats = stats;
    if unusable_streams > 0 {
        catalog.stats.skipped += unusable_streams;
        catalog.warn(
            Step::Playlist,
            format!("skipped {unusable_streams} malformed or unsupported stream URLs"),
        );
    }
    catalog.stats.content_items = emitted + playlist.channels.items.len() as u64;
    catalog.groups = playlist.groups;
    catalog.channels = catalog.absorb(Step::Playlist, None, playlist.channels);
    Ok(catalog)
}

/// Imports an M3U playlist from its URL (or a `file://`/absolute path the
/// adapter can read). The configured User-Agent and Referer are sent.
pub(crate) async fn import_m3u<F: Fetch, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    config: &M3uSourceConfig,
) -> Result<Catalog, ImportError> {
    let location = config.location.expose().trim();
    if location.is_empty() {
        return Err(ImportError::InvalidConfig(
            "playlist address is required".into(),
        ));
    }
    let mut req = HttpRequest::get(location);
    req.picked_file = true;
    if let Some(ua) = config
        .user_agent
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        req = req.with_header("User-Agent", ua);
    }
    if let Some(r) = config
        .referrer
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        req = req.with_header("Referer", r);
    }
    ctx.phase(Phase::Playlist);
    let mut feeder = M3uFeeder::new(ctx.limits);
    let mut sink = ctx.catalog_sink.take();
    if let Some(s) = &mut sink {
        s.begin_step()
            .map_err(|e| ImportError::Storage(e.to_string()))?;
    }
    #[cfg(not(debug_assertions))]
    let allow_private_fixture = false;
    #[cfg(debug_assertions)]
    let allow_private_fixture = ctx.allow_private_fixture;
    let mut emitted = 0;
    ctx.stream(Step::Playlist, Phase::Playlist, req, |chunk| {
        let count = feeder.push(chunk)?;
        if let Some(s) = &mut sink {
            for channel in std::mem::take(&mut feeder.taken) {
                if !allow_private_fixture {
                    crate::validate_stream(&channel.stream)?;
                }
                s.push(crate::CatalogRow::Channel(channel))
                    .map_err(|e| ImportError::Storage(e.to_string()))?;
                emitted += 1;
            }
        }
        Ok(count)
    })
    .await?;
    let (playlist, sniff, unusable_streams) = feeder.finish()?;
    let mut catalog = into_catalog(
        playlist,
        &sniff,
        ctx.stats.clone(),
        allow_private_fixture,
        emitted,
        unusable_streams,
    )?;
    if let Some(s) = &mut sink {
        for group in std::mem::take(&mut catalog.groups) {
            s.push(crate::CatalogRow::Group(group))
                .map_err(|e| ImportError::Storage(e.to_string()))?;
        }
        for channel in std::mem::take(&mut catalog.channels) {
            s.push(crate::CatalogRow::Channel(channel))
                .map_err(|e| ImportError::Storage(e.to_string()))?;
        }
        s.end_step(true)
            .map_err(|e| ImportError::Storage(e.to_string()))?;
    }
    ctx.catalog_sink = sink;
    report_done(ctx.progress, catalog.stats.content_items as usize);
    Ok(catalog)
}

fn report_done(progress: &mut impl ProgressSink, channels: usize) {
    let mut p = Progress::phase(Phase::Items(ContentKind::Live));
    p.items = channels as u64;
    progress.report(&p);
    progress.report(&Progress::phase(Phase::Done));
}

/// Imports a playlist already in memory (a picked file, the clipboard).
/// Synchronous; checks `cancel` every 256 KiB.
pub fn import_m3u_text(
    bytes: &[u8],
    progress: &mut impl ProgressSink,
    cancel: &CancelToken,
) -> Result<Catalog, ImportError> {
    let mut feeder = M3uFeeder::new(ImportLimits::default());
    let mut p = Progress::phase(Phase::Playlist);
    p.total_bytes = Some(bytes.len() as u64);
    for chunk in bytes.chunks(REPORT_EVERY_BYTES as usize) {
        check(cancel)?;
        p.items = feeder.push(chunk)?;
        p.bytes += chunk.len() as u64;
        progress.report(&p);
    }
    let (playlist, sniff, unusable_streams) = feeder.finish()?;
    let stats = crate::catalog::ImportStats {
        bytes: bytes.len() as u64,
        ..Default::default()
    };
    let catalog = into_catalog(playlist, &sniff, stats, false, 0, unusable_streams)?;
    report_done(progress, catalog.channels.len());
    Ok(catalog)
}
