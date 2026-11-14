//! Incremental XMLTV parser: feed it chunks, receive typed records.
//!
//! Memory is bounded by the largest single `<channel>`/`<programme>` element
//! (capped by [`ParseOptions::max_element_bytes`]) plus one input chunk: only
//! the unfinished tail of the input is kept between calls.

use iptv_core::import_limits::{ImportLimits, LimitExceeded, LimitKind};
use quick_xml::Reader;
use quick_xml::events::{BytesRef, BytesStart, Event};

use crate::decode::{Compression, DecodeError, GZIP_SLICE, InputDecoder};
use crate::time::{TimePolicy, parse_xmltv_time};
use crate::xmltv::{EpisodeNum, Rating, XmltvChannel, XmltvItem, XmltvProgramme};

/// Parser settings, usually one per EPG source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseOptions {
    pub limits: ImportLimits,
    pub time: TimePolicy,
    pub compression: Compression,
    /// Keep `<desc>` text. Off saves memory when descriptions are not stored.
    pub descriptions: bool,
    /// An element larger than this is skipped (counted as `oversized`).
    pub max_element_bytes: usize,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            limits: ImportLimits::default(),
            time: TimePolicy::UTC,
            compression: Compression::Auto,
            descriptions: true,
            max_element_bytes: 1 << 20,
        }
    }
}

/// Entries skipped or repaired. Parsing never aborts on these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseWarnings {
    /// XML syntax errors; parsing resumed at the next element.
    pub malformed_xml: u64,
    /// An element was still open at end of input.
    pub truncated: u64,
    /// Elements over `max_element_bytes`.
    pub oversized: u64,
    pub channel_without_id: u64,
    pub programme_without_channel: u64,
    /// Missing or unparsable `start`: programme skipped.
    pub bad_start: u64,
    /// Unparsable `stop`: kept, the stop is inferred later.
    pub bad_stop: u64,
    pub programme_without_title: u64,
    /// Invalid UTF-8 sequences replaced with U+FFFD.
    pub invalid_utf8: u64,
    /// The declared encoding is not supported; decoded as UTF-8.
    pub unknown_encoding: u64,
}

impl ParseWarnings {
    pub fn total(&self) -> u64 {
        self.malformed_xml
            + self.truncated
            + self.oversized
            + self.channel_without_id
            + self.programme_without_channel
            + self.bad_start
            + self.bad_stop
            + self.programme_without_title
            + self.invalid_utf8
            + self.unknown_encoding
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseStats {
    pub channels: u64,
    pub programmes: u64,
    pub warnings: ParseWarnings,
    /// Bytes passed to `feed`, before decompression.
    pub bytes_in: u64,
    /// Largest amount of decoded input held between events.
    pub peak_buffer_bytes: usize,
}

/// Streaming XMLTV parser. Sans-IO: the caller supplies bytes (plain or
/// gzip) in any chunking and receives records through a callback.
///
/// ```
/// use iptv_epg::{ParseOptions, XmltvItem, XmltvParser};
/// let xml = br#"<tv><channel id="one.example"><display-name>One</display-name></channel>
///   <programme channel="one.example" start="20260301120000 +0000" stop="20260301130000 +0000">
///   <title>Noon</title></programme></tv>"#;
/// let mut parser = XmltvParser::new(ParseOptions::default());
/// let mut titles = Vec::new();
/// let mut sink = |item| {
///     if let XmltvItem::Programme(p) = item {
///         titles.push(p.title)
///     }
/// };
/// for chunk in xml.chunks(16) {
///     parser.feed(chunk, &mut sink).unwrap();
/// }
/// let stats = parser.finish(&mut sink).unwrap();
/// assert_eq!(titles, ["Noon"]);
/// assert_eq!((stats.channels, stats.programmes, stats.warnings.total()), (1, 1, 0));
/// ```
pub struct XmltvParser {
    opts: ParseOptions,
    decoder: InputDecoder,
    buf: Vec<u8>,
    stats: ParseStats,
    done: bool,
    record_error: Option<LimitExceeded>,
    /// Set when the last pass stopped inside unfinished markup.
    stall: Option<Stall>,
}

/// Where a pass stopped waiting for more input. Re-running the reader
/// before a possible closing tag arrives would only re-parse the same
/// prefix, which is quadratic for small chunks.
#[derive(Clone, Copy)]
struct Stall {
    scanned: usize,
    in_element: bool,
}

/// Plain input is processed in slices of this size so the decoded
/// buffer stays small whatever the caller's chunk size.
const PLAIN_SLICE: usize = 64 * 1024;

impl XmltvParser {
    pub fn new(opts: ParseOptions) -> Self {
        XmltvParser {
            decoder: InputDecoder::with_limits(opts.compression, opts.limits),
            opts,
            buf: Vec::new(),
            stats: ParseStats::default(),
            done: false,
            record_error: None,
            stall: None,
        }
    }

    /// Feeds the next chunk. Complete records are passed to `sink` before
    /// this returns. An error means the byte stream itself is unusable
    /// (corrupt gzip, UTF-16); later calls are then no-ops.
    pub fn feed(
        &mut self,
        bytes: &[u8],
        sink: &mut impl FnMut(XmltvItem),
    ) -> Result<(), DecodeError> {
        self.check_limits()?;
        if self.done {
            return Ok(());
        }
        self.stats.bytes_in = self.stats.bytes_in.saturating_add(bytes.len() as u64);
        LimitExceeded::check(
            LimitKind::InputBytes,
            self.stats.bytes_in,
            self.opts.limits.input_bytes,
        )
        .map_err(DecodeError::Limit)?;
        let mut rest = bytes;
        while !rest.is_empty() {
            // Until the stream is known to be plain, assume it may inflate.
            let n = if self.decoder.is_plain() {
                PLAIN_SLICE
            } else {
                GZIP_SLICE
            };
            let (piece, tail) = rest.split_at(n.min(rest.len()));
            rest = tail;
            let r = self.decoder.push(piece, &mut self.buf);
            self.process(false, sink);
            self.check_limits()?;
            if let Err(e) = r {
                self.done = true;
                self.sync_decoder_stats();
                return Err(e);
            }
            while self.decoder.drain(&mut self.buf)? {
                self.process(false, sink);
                self.check_limits()?;
            }
        }
        Ok(())
    }

    /// Signals end of input, flushes the remaining records and returns the
    /// totals.
    pub fn finish(&mut self, sink: &mut impl FnMut(XmltvItem)) -> Result<ParseStats, DecodeError> {
        self.check_limits()?;
        if !self.done {
            self.done = true;
            let r = self.decoder.finish_input(&mut self.buf);
            self.process(false, sink);
            self.check_limits()?;
            r?;
            while self.decoder.drain(&mut self.buf)? {
                self.process(false, sink);
                self.check_limits()?;
            }
            let r = self.decoder.finish_text(&mut self.buf);
            self.process(true, sink);
            self.check_limits()?;
            self.sync_decoder_stats();
            r?;
        }
        Ok(self.stats)
    }

    fn check_limits(&self) -> Result<(), DecodeError> {
        if let Some(error) = self.record_error {
            return Err(DecodeError::Limit(error));
        }
        LimitExceeded::check(
            LimitKind::Items,
            self.stats.channels.saturating_add(self.stats.programmes),
            self.opts.limits.items,
        )
        .map_err(DecodeError::Limit)?;
        LimitExceeded::check(
            LimitKind::RecordBytes,
            self.buf.len() as u64,
            self.opts.limits.record_bytes as u64,
        )
        .map_err(DecodeError::Limit)
    }

    pub fn stats(&self) -> ParseStats {
        let mut s = self.stats;
        s.warnings.invalid_utf8 = self.decoder.invalid_utf8;
        s
    }

    /// Decoded bytes currently held for an unfinished element.
    pub fn buffered_bytes(&self) -> usize {
        self.buf.len()
    }

    fn sync_decoder_stats(&mut self) {
        self.stats.warnings.invalid_utf8 = self.decoder.invalid_utf8;
        self.stats.warnings.unknown_encoding = u64::from(self.decoder.unknown_charset);
    }

    /// Whether bytes appended since a stall could complete the markup.
    fn may_progress(&self, st: Stall) -> bool {
        let new = &self.buf[st.scanned.saturating_sub(12).min(self.buf.len())..];
        if st.in_element {
            self.buf.len() > self.opts.max_element_bytes
                || memchr::memmem::find(new, b"</programme").is_some()
                || memchr::memmem::find(new, b"</channel").is_some()
                || memchr::memmem::find(new, b"/>").is_some()
        } else {
            memchr::memchr(b'>', new).is_some()
        }
    }

    fn process(&mut self, at_end: bool, sink: &mut dyn FnMut(XmltvItem)) {
        self.stats.peak_buffer_bytes = self.stats.peak_buffer_bytes.max(self.buf.len());
        if let Some(st) = self.stall.filter(|&st| !at_end && !self.may_progress(st)) {
            self.stall = Some(Stall {
                scanned: self.buf.len(),
                ..st
            });
            return;
        }
        let len = self.buf.len();
        let mut stalled = None;
        let mut base = 0;
        let mut committed = 0;
        'restart: loop {
            let mut reader = Reader::from_reader(&self.buf[base..]);
            let cfg = reader.config_mut();
            cfg.allow_dangling_amp = true;
            cfg.allow_unmatched_ends = true;
            cfg.check_end_names = false;
            cfg.check_comments = false;
            let mut el: Option<Element> = None;
            loop {
                let ev_start = base + reader.buffer_position() as usize;
                let event = reader.read_event();
                let pos = base + reader.buffer_position() as usize;
                if el
                    .as_ref()
                    .is_some_and(|e| pos.saturating_sub(e.start) > self.opts.limits.record_bytes)
                {
                    self.record_error = Some(LimitExceeded {
                        kind: LimitKind::RecordBytes,
                        maximum: self.opts.limits.record_bytes as u64,
                    });
                    return;
                }
                match event {
                    Ok(Event::Eof) => break,
                    Ok(ev) => {
                        let finished = match el.as_mut() {
                            Some(e) => e.event(ev, &self.opts),
                            None => {
                                el = Element::begin(&ev, ev_start, &self.opts);
                                el.is_none() || matches!(ev, Event::Empty(_))
                            }
                        };
                        if finished {
                            if let Some(e) = el.take() {
                                if pos.saturating_sub(e.start) > self.opts.limits.record_bytes {
                                    self.record_error = Some(LimitExceeded {
                                        kind: LimitKind::RecordBytes,
                                        maximum: self.opts.limits.record_bytes as u64,
                                    });
                                    return;
                                }
                                e.finish(&mut self.stats, sink);
                            }
                            committed = pos;
                        }
                    }
                    Err(err) => {
                        if !at_end && incomplete(&err, pos, len) {
                            stalled = Some(el.is_some());
                            break; // Markup continues in the next chunk.
                        }
                        self.stats.warnings.malformed_xml += 1;
                        let from = el.take().map_or(ev_start, |e| e.start) + 1;
                        match next_element_start(&self.buf, from) {
                            Some(p) => {
                                base = p;
                                committed = p;
                                continue 'restart;
                            }
                            None => {
                                committed = keep_tail(&self.buf, from);
                                break 'restart;
                            }
                        }
                    }
                }
            }
            if let Some(e) = el {
                if at_end {
                    self.stats.warnings.truncated += 1;
                } else if len - e.start > self.opts.max_element_bytes {
                    self.stats.warnings.oversized += 1;
                    match next_element_start(&self.buf, e.start + 1) {
                        Some(p) => {
                            base = p;
                            committed = p;
                            continue 'restart;
                        }
                        None => committed = keep_tail(&self.buf, e.start + 1),
                    }
                } else {
                    stalled = Some(true);
                }
            }
            break;
        }
        if at_end {
            committed = len;
        }
        self.buf.drain(..committed);
        self.stall = stalled.filter(|_| !at_end).map(|in_element| Stall {
            scanned: self.buf.len(),
            in_element,
        });
    }
}

/// Parses a complete document held in memory.
pub fn parse_xmltv(bytes: &[u8], opts: ParseOptions) -> Result<ParsedXmltv, DecodeError> {
    let mut out = ParsedXmltv::default();
    let mut sink = |item| match item {
        XmltvItem::Channel(c) => out.channels.push(c),
        XmltvItem::Programme(p) => out.programmes.push(p),
    };
    let mut parser = XmltvParser::new(opts);
    parser.feed(bytes, &mut sink)?;
    let stats = parser.finish(&mut sink)?;
    out.stats = stats;
    Ok(out)
}

/// Result of [`parse_xmltv`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedXmltv {
    pub channels: Vec<XmltvChannel>,
    pub programmes: Vec<XmltvProgramme>,
    pub stats: ParseStats,
}

/// Whether a reader error only means the buffer ends inside markup. The
/// reader sees the whole buffer, so an "unclosed" error can only come from
/// running out of input.
fn incomplete(err: &quick_xml::Error, pos: usize, len: usize) -> bool {
    use quick_xml::errors::SyntaxError;
    match err {
        quick_xml::Error::Syntax(SyntaxError::InvalidBangMarkup) => pos + 16 >= len,
        quick_xml::Error::Syntax(_) => true,
        _ => pos + 16 >= len,
    }
}

/// Offset of the next `<channel` or `<programme` tag at or after `from`.
fn next_element_start(buf: &[u8], from: usize) -> Option<usize> {
    let mut from = from.min(buf.len());
    loop {
        let rest = &buf[from..];
        let p = memchr::memmem::find(rest, b"<programme");
        let c = memchr::memmem::find(rest, b"<channel");
        let (at, tag) = match (p, c) {
            (Some(p), Some(c)) if c < p => (c, 8),
            (Some(p), _) => (p, 10),
            (None, Some(c)) => (c, 8),
            (None, None) => return None,
        };
        match rest.get(at + tag) {
            Some(b) if b.is_ascii_whitespace() || *b == b'>' || *b == b'/' => {
                return Some(from + at);
            }
            // The tag name may continue in the next chunk: wait for it.
            None => return None,
            Some(_) => from += at + 1,
        }
    }
}

/// Where to cut when skipping garbage: keep a short tail that may be the
/// start of a tag split across chunks, aligned to a UTF-8 boundary.
fn keep_tail(buf: &[u8], from: usize) -> usize {
    let mut cut = buf.len().saturating_sub(16).max(from.min(buf.len()));
    while cut < buf.len() && (buf[cut] & 0xC0) == 0x80 {
        cut += 1;
    }
    cut
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    None,
    DisplayName,
    Title,
    SubTitle,
    Desc,
    Category,
    EpisodeNum,
    Rating,
    RatingValue,
}

enum Body {
    Channel(XmltvChannel),
    Programme {
        p: XmltvProgramme,
        start_ok: bool,
        bad_stop: bool,
    },
}

/// One `<channel>` or `<programme>` being assembled.
struct Element {
    start: usize,
    depth: u32,
    field: Field,
    field_depth: u32,
    text: String,
    system: Option<String>,
    body: Body,
}

impl Element {
    /// Starts an element for a top-level `<channel>`/`<programme>` event.
    fn begin(ev: &Event, start: usize, opts: &ParseOptions) -> Option<Element> {
        let e = match ev {
            Event::Start(e) | Event::Empty(e) => e,
            _ => return None,
        };
        let body = match e.name().as_ref() {
            "channel" => Body::Channel(XmltvChannel {
                id: attr(e, "id").unwrap_or_default().trim().to_string(),
                ..XmltvChannel::default()
            }),
            "programme" => {
                let start_t = attr(e, "start").and_then(|s| parse_xmltv_time(&s, opts.time));
                let stop_raw = attr(e, "stop").filter(|s| !s.trim().is_empty());
                let stop = stop_raw
                    .as_deref()
                    .and_then(|s| parse_xmltv_time(s, opts.time));
                Body::Programme {
                    p: XmltvProgramme {
                        channel: attr(e, "channel").unwrap_or_default().trim().to_string(),
                        start: start_t.unwrap_or_default(),
                        stop,
                        catchup_id: attr(e, "catchup-id").filter(|s| !s.is_empty()),
                        ..XmltvProgramme::default()
                    },
                    start_ok: start_t.is_some(),
                    bad_stop: stop_raw.is_some() && stop.is_none(),
                }
            }
            _ => return None,
        };
        Some(Element {
            start,
            depth: 0,
            field: Field::None,
            field_depth: 0,
            text: String::new(),
            system: None,
            body,
        })
    }

    /// Handles an event inside the element; `true` when the element closed.
    fn event(&mut self, ev: Event, opts: &ParseOptions) -> bool {
        match ev {
            Event::Start(e) => {
                self.child(&e, opts);
                self.depth += 1;
            }
            Event::Empty(e) => {
                if self.depth == 0 && e.name().as_ref() == "icon" {
                    self.icon(&e);
                }
            }
            Event::End(_) => {
                if self.depth == 0 {
                    return true;
                }
                if self.depth == self.field_depth {
                    self.close_field();
                }
                self.depth -= 1;
            }
            Event::Text(t) if self.capturing() => self.text.push_str(&t.xml10_content()),
            Event::CData(t) if self.capturing() => self.text.push_str(&t),
            Event::GeneralRef(r) if self.capturing() => push_ref(&mut self.text, &r),
            _ => {}
        }
        false
    }

    fn capturing(&self) -> bool {
        self.field != Field::None && self.field != Field::Rating && self.depth == self.field_depth
    }

    fn child(&mut self, e: &BytesStart, opts: &ParseOptions) {
        let name = e.name();
        let name = name.as_ref();
        if self.depth == 1 && self.field == Field::Rating && name == "value" {
            self.field = Field::RatingValue;
            self.field_depth = 2;
            self.text.clear();
            return;
        }
        if self.depth != 0 {
            return;
        }
        let channel = matches!(self.body, Body::Channel(_));
        self.field = match name {
            "display-name" if channel => Field::DisplayName,
            "title" if !channel => Field::Title,
            "sub-title" if !channel => Field::SubTitle,
            "desc" if !channel && opts.descriptions => Field::Desc,
            "category" if !channel => Field::Category,
            "episode-num" if !channel => Field::EpisodeNum,
            "rating" if !channel => Field::Rating,
            "icon" => {
                self.icon(e);
                Field::None
            }
            _ => Field::None,
        };
        self.field_depth = 1;
        self.text.clear();
        if matches!(self.field, Field::EpisodeNum | Field::Rating) {
            self.system = attr(e, "system").filter(|s| !s.is_empty());
        }
    }

    fn icon(&mut self, e: &BytesStart) {
        let slot = match &mut self.body {
            Body::Channel(c) => &mut c.icon,
            Body::Programme { p, .. } => &mut p.icon,
        };
        if slot.is_none() {
            *slot = attr(e, "src")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
    }

    fn close_field(&mut self) {
        let field = std::mem::replace(&mut self.field, Field::None);
        if field == Field::RatingValue {
            // Back inside <rating>, which closes one level up.
            self.field = Field::Rating;
            self.field_depth = 1;
        }
        let value = self.text.trim();
        if value.is_empty() || field == Field::Rating {
            return;
        }
        let value = value.to_string();
        match &mut self.body {
            Body::Channel(c) => {
                if field == Field::DisplayName {
                    c.display_names.push(value);
                }
            }
            Body::Programme { p, .. } => match field {
                // Superscript badges ("ᴸᶦᵛᵉ") read as words: `fold_superscripts`.
                Field::Title if p.title.is_empty() => p.title = folded(value),
                Field::SubTitle if p.sub_title.is_none() => p.sub_title = Some(folded(value)),
                Field::Desc if p.desc.is_none() => p.desc = Some(value),
                Field::Category => p.categories.push(value),
                Field::EpisodeNum => p.episode_num.push(EpisodeNum {
                    system: self.system.clone(),
                    value,
                }),
                Field::RatingValue if p.rating.is_none() => {
                    p.rating = Some(Rating {
                        system: self.system.clone(),
                        value,
                    })
                }
                _ => {}
            },
        }
    }

    fn finish(self, stats: &mut ParseStats, sink: &mut dyn FnMut(XmltvItem)) {
        let w = &mut stats.warnings;
        match self.body {
            Body::Channel(c) => {
                if c.id.is_empty() {
                    w.channel_without_id += 1;
                } else {
                    stats.channels += 1;
                    sink(XmltvItem::Channel(c));
                }
            }
            Body::Programme {
                p,
                start_ok,
                bad_stop,
            } => {
                if p.channel.is_empty() {
                    w.programme_without_channel += 1;
                } else if !start_ok {
                    w.bad_start += 1;
                } else if p.title.is_empty() {
                    w.programme_without_title += 1;
                } else {
                    w.bad_stop += u64::from(bad_stop);
                    stats.programmes += 1;
                    sink(XmltvItem::Programme(p));
                }
            }
        }
    }
}

fn attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| unescape_lenient(&a.value))
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

fn push_ref(text: &mut String, r: &BytesRef) {
    match entity(r) {
        Some(c) => text.push(c),
        None => {
            text.push('&');
            text.push_str(r);
            text.push(';');
        }
    }
}

/// Resolves known entity references; leaves stray `&` and unknown names as
/// they are (attribute values such as URLs often contain a bare `&`).
fn unescape_lenient(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let resolved = rest[1..]
            .find(';')
            .filter(|&n| n <= 10)
            .and_then(|n| entity(&rest[1..1 + n]).map(|c| (c, n + 2)));
        match resolved {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `value` with its superscript badges folded ([`fold_superscripts`]).
fn folded(value: String) -> String {
    match crate::fold_superscripts(&value) {
        std::borrow::Cow::Borrowed(_) => value,
        std::borrow::Cow::Owned(f) => f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lenient_unescape() {
        assert_eq!(unescape_lenient("a&amp;b"), "a&b");
        assert_eq!(
            unescape_lenient("http://x.example/?a=1&b=2"),
            "http://x.example/?a=1&b=2"
        );
        assert_eq!(
            unescape_lenient("&#233;&#xE9;&bogus;&"),
            "\u{e9}\u{e9}&bogus;&"
        );
    }

    #[test]
    fn element_start_search() {
        assert_eq!(next_element_start(b"xx<programme a", 0), Some(2));
        assert_eq!(next_element_start(b"<programmes><channel>", 0), Some(12));
        assert_eq!(next_element_start(b"<channel", 0), None);
        assert_eq!(next_element_start(b"nothing", 0), None);
    }
}
