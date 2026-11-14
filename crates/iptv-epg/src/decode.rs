//! Input decoding in front of the XML reader: optional gzip, then charset to
//! UTF-8. Sans-IO: bytes in, UTF-8 bytes appended to the caller's buffer.

use std::fmt;
use std::io::Write;

use flate2::write::MultiGzDecoder;
use iptv_core::import_limits::{DecodeBuffer, ImportLimits, LimitExceeded, LimitKind};

/// Fatal problems with the byte stream itself (not with XMLTV content).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Limit(LimitExceeded),
    /// The gzip stream is corrupt or truncated.
    Gzip(String),
    /// UTF-16/UTF-32 input (detected by its byte-order mark).
    UnsupportedEncoding(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Limit(error) => error.fmt(f),
            DecodeError::Gzip(e) => write!(f, "corrupt gzip stream: {e}"),
            DecodeError::UnsupportedEncoding(e) => write!(f, "unsupported encoding: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Whether the input is gzip-compressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression {
    /// Detect from the gzip magic bytes (`1f 8b`).
    #[default]
    Auto,
    Plain,
    Gzip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Charset {
    Utf8,
    Latin1,
    Latin9,
    Windows1252,
}

enum Stage {
    /// Waiting for two bytes to look for the gzip magic.
    Detect,
    Plain,
    Gzip(Box<MultiGzDecoder<DecodeBuffer>>),
    Failed,
}

/// Size of the compressed slices handed to the inflater; bounds how much
/// decompressed data one step can produce.
pub(crate) const GZIP_SLICE: usize = 8 * 1024;
/// How many leading bytes may hold the XML declaration.
const DECL_PROBE: usize = 512;

pub(crate) struct InputDecoder {
    stage: Stage,
    nested: Option<Box<InputDecoder>>,
    allow_nested: bool,
    limits: ImportLimits,
    decoded: u64,
    mode: Compression,
    head: Vec<u8>,
    /// `None` until the XML declaration (or its absence) is seen.
    charset: Option<Charset>,
    probe: Vec<u8>,
    /// Incomplete UTF-8 sequence carried to the next chunk.
    utf8_tail: Vec<u8>,
    scratch: Vec<u8>,
    pub(crate) invalid_utf8: u64,
    pub(crate) unknown_charset: bool,
}

impl InputDecoder {
    #[cfg(test)]
    pub(crate) fn new(mode: Compression) -> Self {
        Self::with_limits(mode, ImportLimits::default())
    }

    pub(crate) fn with_limits(mode: Compression, limits: ImportLimits) -> Self {
        InputDecoder {
            stage: Stage::Detect,
            nested: None,
            allow_nested: true,
            limits,
            decoded: 0,
            mode,
            head: Vec::new(),
            charset: None,
            probe: Vec::new(),
            utf8_tail: Vec::new(),
            scratch: Vec::new(),
            invalid_utf8: 0,
            unknown_charset: false,
        }
    }

    /// Whether the stream is known to be uncompressed.
    pub(crate) fn is_plain(&self) -> bool {
        matches!(self.stage, Stage::Plain)
    }

    #[cfg(test)]
    fn is_gzip(&self) -> bool {
        matches!(self.stage, Stage::Gzip(_))
    }

    /// Decodes `input` and appends UTF-8 to `out`. For gzip input the caller
    /// should pass slices of at most [`GZIP_SLICE`] bytes.
    pub(crate) fn push(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), DecodeError> {
        if let Stage::Detect = self.stage {
            self.head.extend_from_slice(input);
            if self.head.len() < 2 {
                return Ok(());
            }
            self.start_stage();
            let head = std::mem::take(&mut self.head);
            return self.push(&head, out);
        }
        match &mut self.stage {
            Stage::Detect => unreachable!(),
            Stage::Failed => Ok(()),
            Stage::Plain => self.text(input, out),
            Stage::Gzip(gz) => {
                if let Err(e) = gz.write_all(input).and_then(|()| gz.flush()) {
                    let error = gz
                        .get_ref()
                        .limit_error()
                        .map_or_else(|| DecodeError::Gzip(e.to_string()), DecodeError::Limit);
                    self.stage = Stage::Failed;
                    return Err(error);
                }
                let decoded = gz.get_mut().take();
                self.inflated(&decoded, out)
            }
        }
    }

    /// Flushes everything held back. Call once at end of input.
    pub(crate) fn finish(&mut self, out: &mut Vec<u8>) -> Result<(), DecodeError> {
        if let Stage::Detect = self.stage {
            self.start_stage();
            let head = std::mem::take(&mut self.head);
            if !head.is_empty() {
                self.push(&head, out)?;
            }
        }
        if let Stage::Gzip(gz) = &mut self.stage {
            if let Err(e) = gz.try_finish() {
                let error = gz
                    .get_ref()
                    .limit_error()
                    .map_or_else(|| DecodeError::Gzip(e.to_string()), DecodeError::Limit);
                self.stage = Stage::Failed;
                return Err(error);
            }
            let decoded = gz.get_mut().take();
            self.inflated(&decoded, out)?;
        }
        if let Some(nested) = &mut self.nested {
            nested.finish(out)?;
            self.invalid_utf8 = nested.invalid_utf8;
            self.unknown_charset = nested.unknown_charset;
            return Ok(());
        }
        if self.charset.is_none() {
            self.charset = Some(Charset::Utf8);
            let probe = std::mem::take(&mut self.probe);
            self.emit(&probe, out);
        }
        if !self.utf8_tail.is_empty() {
            self.utf8_tail.clear();
            self.invalid_utf8 += 1;
            out.extend_from_slice("\u{FFFD}".as_bytes());
        }
        Ok(())
    }

    fn start_stage(&mut self) {
        let gzip = match self.mode {
            Compression::Auto => self.head.starts_with(&[0x1f, 0x8b]),
            Compression::Plain => false,
            Compression::Gzip => true,
        };
        if gzip && self.allow_nested {
            // HTTP gzip can wrap a gzip file. Bound decoding to these two
            // layers, with the decoded-byte budget enforced on each layer.
            let mut nested = Self::with_limits(Compression::Auto, self.limits);
            nested.allow_nested = false;
            self.nested = Some(Box::new(nested));
        }
        self.stage = if gzip {
            Stage::Gzip(Box::new(MultiGzDecoder::new(DecodeBuffer::new(
                self.limits.decoded_bytes,
            ))))
        } else {
            Stage::Plain
        };
    }

    fn inflated(&mut self, bytes: &[u8], out: &mut Vec<u8>) -> Result<(), DecodeError> {
        if let Some(nested) = &mut self.nested {
            for chunk in bytes.chunks(GZIP_SLICE) {
                nested.push(chunk, out)?;
            }
            self.invalid_utf8 = nested.invalid_utf8;
            self.unknown_charset = nested.unknown_charset;
            Ok(())
        } else {
            self.text(bytes, out)
        }
    }

    /// Decompressed bytes: settle the charset, then convert.
    fn text(&mut self, bytes: &[u8], out: &mut Vec<u8>) -> Result<(), DecodeError> {
        self.decoded = self.decoded.saturating_add(bytes.len() as u64);
        LimitExceeded::check(
            LimitKind::DecodedBytes,
            self.decoded,
            self.limits.decoded_bytes,
        )
        .map_err(DecodeError::Limit)?;
        if self.charset.is_some() {
            self.emit(bytes, out);
            return Ok(());
        }
        self.probe.extend_from_slice(bytes);
        if let Some(name) = utf16_bom(&self.probe) {
            self.stage = Stage::Failed;
            self.probe.clear();
            return Err(DecodeError::UnsupportedEncoding(name));
        }
        let body = self
            .probe
            .strip_prefix(b"\xEF\xBB\xBF")
            .unwrap_or(&self.probe);
        let lead = body
            .iter()
            .position(|c| !c.is_ascii_whitespace())
            .unwrap_or(body.len());
        let body = &body[lead..];
        let n = body.len().min(5);
        let may_be_decl = body[..n] == b"<?xml"[..n];
        let decl_end = memchr::memmem::find(body, b"?>");
        if may_be_decl && decl_end.is_none() && self.probe.len() < DECL_PROBE {
            return Ok(());
        }
        let mut probe = std::mem::take(&mut self.probe);
        if probe.starts_with(b"\xEF\xBB\xBF") {
            probe.drain(..3);
        }
        self.charset = Some(match declared_encoding(&probe) {
            None => Charset::Utf8,
            Some(name) => match name.to_ascii_lowercase().as_str() {
                "utf-8" | "utf8" | "us-ascii" | "ascii" => Charset::Utf8,
                "iso-8859-1" | "iso8859-1" | "latin1" | "latin-1" | "l1" => Charset::Latin1,
                "iso-8859-15" | "iso8859-15" | "latin9" | "latin-9" => Charset::Latin9,
                "windows-1252" | "cp1252" | "x-cp1252" => Charset::Windows1252,
                _ => {
                    self.unknown_charset = true;
                    Charset::Utf8
                }
            },
        });
        self.emit(&probe, out);
        Ok(())
    }

    fn emit(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        match self.charset.unwrap_or(Charset::Utf8) {
            Charset::Utf8 => self.utf8(bytes, out),
            single => {
                for &b in bytes {
                    if b < 0x80 {
                        out.push(b);
                    } else {
                        let c = single_byte_char(single, b);
                        let mut tmp = [0; 4];
                        out.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                    }
                }
            }
        }
    }

    /// Validates UTF-8, replacing invalid sequences with U+FFFD and carrying
    /// an incomplete trailing sequence to the next chunk.
    fn utf8(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        let input: &[u8] = if self.utf8_tail.is_empty() {
            bytes
        } else {
            self.scratch.clear();
            self.scratch.append(&mut self.utf8_tail);
            self.scratch.extend_from_slice(bytes);
            &self.scratch
        };
        let mut rest = input;
        loop {
            match std::str::from_utf8(rest) {
                Ok(_) => {
                    out.extend_from_slice(rest);
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.extend_from_slice(&rest[..valid]);
                    match e.error_len() {
                        Some(n) => {
                            self.invalid_utf8 += 1;
                            out.extend_from_slice("\u{FFFD}".as_bytes());
                            rest = &rest[valid + n..];
                        }
                        None => {
                            self.utf8_tail.extend_from_slice(&rest[valid..]);
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn utf16_bom(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        Some("UTF-16")
    } else {
        None
    }
}

/// The `encoding="..."` of a leading `<?xml ...?>` declaration.
fn declared_encoding(b: &[u8]) -> Option<String> {
    let start = b.iter().position(|c| !c.is_ascii_whitespace())?;
    let b = &b[start..];
    if !b.starts_with(b"<?xml") {
        return None;
    }
    let end = memchr::memmem::find(b, b"?>")?;
    let decl = std::str::from_utf8(&b[..end]).ok()?;
    let at = decl.find("encoding")?;
    let rest = decl[at + "encoding".len()..]
        .trim_start()
        .strip_prefix('=')?;
    let rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    Some(rest[..rest.find(quote)?].to_string())
}

fn single_byte_char(charset: Charset, b: u8) -> char {
    match charset {
        Charset::Windows1252 if (0x80..0xA0).contains(&b) => {
            const CP1252: [char; 32] = [
                '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
                '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}',
                '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
                '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
                '\u{0153}', '\u{9D}', '\u{017E}', '\u{0178}',
            ];
            CP1252[usize::from(b - 0x80)]
        }
        Charset::Latin9 => match b {
            0xA4 => '\u{20AC}',
            0xA6 => '\u{0160}',
            0xA8 => '\u{0161}',
            0xB4 => '\u{017D}',
            0xB8 => '\u{017E}',
            0xBC => '\u{0152}',
            0xBD => '\u{0153}',
            0xBE => '\u{0178}',
            _ => char::from(b),
        },
        _ => char::from(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]], mode: Compression) -> (String, InputDecoder) {
        let mut d = InputDecoder::new(mode);
        let mut out = Vec::new();
        for c in chunks {
            d.push(c, &mut out).unwrap();
        }
        d.finish(&mut out).unwrap();
        (String::from_utf8(out).unwrap(), d)
    }

    #[test]
    fn utf8_split_across_chunks() {
        let s = "<tv>caf\u{e9} \u{1F4FA}</tv>".as_bytes();
        let chunks: Vec<&[u8]> = s.chunks(1).collect();
        let (out, d) = run(&chunks, Compression::Auto);
        assert_eq!(out.as_bytes(), s);
        assert_eq!(d.invalid_utf8, 0);
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let (out, d) = run(&[b"<tv>a\xFFb\xE2\x82</tv>"], Compression::Plain);
        assert_eq!(out, "<tv>a\u{FFFD}b\u{FFFD}</tv>");
        assert_eq!(d.invalid_utf8, 2);
    }

    #[test]
    fn declared_latin1_and_cp1252() {
        let (out, _) = run(
            &[b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>Caf\xE9</t>"],
            Compression::Auto,
        );
        assert!(out.ends_with("<t>Caf\u{e9}</t>"));
        let (out, _) = run(
            &[b"<?xml version='1.0' encoding='windows-1252'?><t>\x80 \x93x\x94</t>"],
            Compression::Auto,
        );
        assert!(out.ends_with("<t>\u{20AC} \u{201C}x\u{201D}</t>"));
    }

    #[test]
    fn gzip_auto_detected_and_multi_member() {
        use flate2::write::GzEncoder;
        let gz = |s: &[u8]| {
            let mut e = GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(s).unwrap();
            e.finish().unwrap()
        };
        let mut data = gz(b"<tv>one");
        data.extend(gz(b" two</tv>"));
        let chunks: Vec<&[u8]> = data.chunks(3).collect();
        let (out, d) = run(&chunks, Compression::Auto);
        assert_eq!(out, "<tv>one two</tv>");
        assert!(d.is_gzip());
    }

    #[test]
    fn http_gzip_wrapping_gzip_file_is_bounded_and_chunk_safe() {
        let gzip = |bytes: &[u8]| {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        };
        let xml = b"<?xml encoding='iso-8859-1'?><tv>Caf\xe9</tv>";
        let bytes = gzip(&gzip(xml));
        let chunks: Vec<_> = bytes.chunks(1).collect();
        let (out, _) = run(&chunks, Compression::Auto);
        assert!(out.ends_with("<tv>Café</tv>"));
        let bomb = gzip(&gzip(&vec![b'x'; 100_000]));
        let mut decoder = InputDecoder::with_limits(
            Compression::Auto,
            ImportLimits {
                decoded_bytes: 1024,
                ..Default::default()
            },
        );
        let mut out = Vec::new();
        assert!(matches!(
            decoder.push(&bomb, &mut out),
            Err(DecodeError::Limit(_))
        ));
        assert!(out.len() <= 1024);
    }

    #[test]
    fn corrupt_gzip_is_an_error() {
        let mut d = InputDecoder::new(Compression::Auto);
        let mut out = Vec::new();
        let r = d
            .push(
                &[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 0xff, 0xff, 0xff, 0xff],
                &mut out,
            )
            .and_then(|()| d.finish(&mut out));
        assert!(matches!(r, Err(DecodeError::Gzip(_))));
    }
}

#[cfg(test)]
mod limits_tests {
    use super::*;
    #[test]
    fn gzip_budget_fails_before_expansion_is_retained() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&vec![b'x'; 100_000]).unwrap();
        let bytes = encoder.finish().unwrap();
        let mut decoder = InputDecoder::with_limits(
            Compression::Auto,
            ImportLimits {
                decoded_bytes: 1024,
                ..Default::default()
            },
        );
        let mut out = Vec::new();
        assert!(matches!(
            decoder.push(&bytes, &mut out),
            Err(DecodeError::Limit(LimitExceeded {
                kind: LimitKind::DecodedBytes,
                ..
            }))
        ));
        assert!(out.len() <= 1024);
    }
}
