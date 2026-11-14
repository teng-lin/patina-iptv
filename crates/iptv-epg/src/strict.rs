//! Strict, bounded document validation for proposed guide sources.
//! Ordinary imports keep their recovery parser; inspection evidence requires a
//! complete XMLTV document and rejects entities/DTDs and malformed structure.
use crate::decode::{GZIP_SLICE, InputDecoder};
use crate::{Compression, DecodeError, ParseOptions, ParseStats, XmltvItem, XmltvParser};
use quick_xml::{Reader, events::Event};

#[derive(Debug)]
pub enum StrictError {
    Decode(DecodeError),
    InvalidXml,
}
impl From<DecodeError> for StrictError {
    fn from(value: DecodeError) -> Self {
        Self::Decode(value)
    }
}

pub struct StrictXmltvParser {
    decoder: InputDecoder,
    parser: XmltvParser,
    document: Document,
    input: u64,
    limit: u64,
}
impl StrictXmltvParser {
    pub fn new(mut options: ParseOptions) -> Self {
        let decoder = InputDecoder::with_limits(options.compression, options.limits);
        let limit = options.limits.input_bytes;
        options.compression = Compression::Plain;
        // The outer decoder owns the input budget; plain bytes are expanded.
        options.limits.input_bytes = options.limits.decoded_bytes;
        Self {
            decoder,
            parser: XmltvParser::new(options),
            document: Document::default(),
            input: 0,
            limit,
        }
    }
    fn process(
        &mut self,
        bytes: Vec<u8>,
        end: bool,
        sink: &mut impl FnMut(XmltvItem),
    ) -> Result<(), StrictError> {
        self.document.feed(&bytes, end)?;
        self.parser.feed_decoded(&bytes, sink)?;
        if self.decoder.invalid_utf8 != 0 || self.decoder.unknown_charset {
            return Err(StrictError::InvalidXml);
        }
        Ok(())
    }
    pub fn feed(
        &mut self,
        bytes: &[u8],
        sink: &mut impl FnMut(XmltvItem),
    ) -> Result<(), StrictError> {
        self.input = self.input.saturating_add(bytes.len() as u64);
        if self.input > self.limit {
            return Err(StrictError::Decode(DecodeError::Limit(
                iptv_core::import_limits::LimitExceeded {
                    kind: iptv_core::import_limits::LimitKind::InputBytes,
                    maximum: self.limit,
                },
            )));
        }
        for chunk in bytes.chunks(GZIP_SLICE) {
            let mut decoded = Vec::new();
            self.decoder.push(chunk, &mut decoded)?;
            self.process(decoded, false, sink)?;
            loop {
                let mut decoded = Vec::new();
                let more = self.decoder.drain(&mut decoded)?;
                self.process(decoded, false, sink)?;
                if !more {
                    break;
                }
            }
        }
        Ok(())
    }
    pub fn finish(&mut self, sink: &mut impl FnMut(XmltvItem)) -> Result<ParseStats, StrictError> {
        let mut decoded = Vec::new();
        self.decoder.finish_input(&mut decoded)?;
        self.process(decoded, false, sink)?;
        loop {
            let mut decoded = Vec::new();
            let more = self.decoder.drain(&mut decoded)?;
            self.process(decoded, false, sink)?;
            if !more {
                break;
            }
        }
        let mut decoded = Vec::new();
        self.decoder.finish_text(&mut decoded)?;
        self.process(decoded, true, sink)?;
        let mut stats = self.parser.finish_decoded(sink)?;
        let warnings = stats.warnings;
        if warnings.malformed_xml != 0
            || warnings.truncated != 0
            || warnings.oversized != 0
            || warnings.channel_without_id != 0
            || warnings.programme_without_channel != 0
        {
            return Err(StrictError::InvalidXml);
        }
        stats.bytes_in = self.input;
        Ok(stats)
    }
}

#[derive(Default)]
struct Document {
    tail: Vec<u8>,
    stack: Vec<String>,
    seen: bool,
    closed: bool,
}
impl Document {
    fn feed(&mut self, bytes: &[u8], end: bool) -> Result<(), StrictError> {
        if std::str::from_utf8(bytes).map_or(true, |text| {
            text.chars().any(|ch| !valid_xml_char(ch as u32))
        }) {
            return Err(StrictError::InvalidXml);
        }
        self.tail.extend_from_slice(bytes);
        // Wait for complete markup and preserve unfinished text/entity tokens.
        let length = if end {
            self.tail.len()
        } else {
            complete_prefix(&self.tail)
        };
        if self.tail.len() > 2 * 1024 * 1024 {
            return Err(StrictError::InvalidXml);
        }
        let mut reader = Reader::from_reader(&self.tail[..length]);
        reader.config_mut().check_end_names = false;
        reader.config_mut().allow_unmatched_ends = true;
        reader.config_mut().check_comments = true;
        loop {
            match reader.read_event().map_err(|_| StrictError::InvalidXml)? {
                Event::Eof => break,
                Event::DocType(_) => return Err(StrictError::InvalidXml),
                Event::Start(e) | Event::Empty(e) => {
                    let empty = self.tail.get(reader.buffer_position() as usize - 2) == Some(&b'/');
                    let name = e.name().as_ref().to_owned();
                    for attribute in e.attributes() {
                        let attribute = attribute.map_err(|_| StrictError::InvalidXml)?;
                        if attribute.value.contains('<') || !valid_entities(&attribute.value) {
                            return Err(StrictError::InvalidXml);
                        }
                    }
                    if self.closed || (self.stack.is_empty() && (self.seen || name != "tv")) {
                        return Err(StrictError::InvalidXml);
                    }
                    if self.stack.len() == 1 && name != "channel" && name != "programme" {
                        return Err(StrictError::InvalidXml);
                    }
                    if self.stack.is_empty() {
                        self.seen = true;
                    }
                    if !empty {
                        self.stack.push(name);
                    } else if self.stack.is_empty() {
                        self.closed = true;
                    }
                    if self.stack.len() > 32 {
                        return Err(StrictError::InvalidXml);
                    }
                }
                Event::End(e) => {
                    if self.stack.pop().as_deref() != Some(e.name().as_ref()) {
                        return Err(StrictError::InvalidXml);
                    }
                    if self.stack.is_empty() {
                        self.closed = true;
                    }
                }
                Event::Text(e) if self.stack.is_empty() => {
                    if !e.as_ref().bytes().all(|b| b.is_ascii_whitespace()) {
                        return Err(StrictError::InvalidXml);
                    }
                }
                Event::CData(_) if self.stack.is_empty() => return Err(StrictError::InvalidXml),
                Event::GeneralRef(e) => {
                    let value = e.as_ref();
                    if self.stack.is_empty() || !valid_entity(value) {
                        return Err(StrictError::InvalidXml);
                    }
                }
                Event::Decl(_) if self.seen => return Err(StrictError::InvalidXml),
                _ => {}
            }
        }
        self.tail.drain(..length);
        if end && (!self.seen || !self.closed || !self.stack.is_empty()) {
            return Err(StrictError::InvalidXml);
        }
        Ok(())
    }
}

fn valid_xml_char(value: u32) -> bool {
    matches!(value,0x9|0xa|0xd|0x20..=0xd7ff|0xe000..=0xfffd|0x10000..=0x10ffff)
}
fn valid_entity(value: &str) -> bool {
    if ["amp", "lt", "gt", "apos", "quot"].contains(&value) {
        return true;
    }
    value
        .strip_prefix('#')
        .and_then(|number| match number.strip_prefix('x') {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse().ok(),
        })
        .is_some_and(valid_xml_char)
}
fn valid_entities(mut text: &str) -> bool {
    while let Some(at) = text.find('&') {
        text = &text[at + 1..];
        let Some(end) = text.find(';') else {
            return false;
        };
        if !valid_entity(&text[..end]) {
            return false;
        }
        text = &text[end + 1..];
    }
    true
}

fn complete_prefix(bytes: &[u8]) -> usize {
    let mut position = 0;
    let mut complete = 0;
    while position < bytes.len() {
        let Some(start) = bytes[position..].iter().position(|&b| b == b'<') else {
            break;
        };
        let start = position + start;
        let rest = &bytes[start..];
        let marker = if rest.starts_with(b"<!--") {
            Some(b"-->".as_slice())
        } else if rest.starts_with(b"<![CDATA[") {
            Some(b"]]>".as_slice())
        } else if rest.starts_with(b"<?") {
            Some(b"?>".as_slice())
        } else {
            None
        };
        if let Some(marker) = marker {
            let Some(end) = rest.windows(marker.len()).position(|b| b == marker) else {
                break;
            };
            position = start + end + marker.len();
        } else {
            let mut quote = None;
            let mut end = None;
            for (at, &b) in rest.iter().enumerate().skip(1) {
                match (quote, b) {
                    (Some(q), v) if q == v => quote = None,
                    (None, b'\'' | b'"') => quote = Some(b),
                    (None, b'>') => {
                        end = Some(at);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                break;
            };
            position = start + end + 1;
        }
        complete = position;
    }
    complete
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(xml: &[u8], chunk: usize) -> bool {
        let mut parser = StrictXmltvParser::new(ParseOptions::default());
        for piece in xml.chunks(chunk) {
            if parser.feed(piece, &mut |_| {}).is_err() {
                return false;
            }
        }
        parser.finish(&mut |_| {}).is_ok()
    }
    #[test]
    fn requires_complete_xmltv_and_refuses_unsafe_or_ambiguous_documents() {
        for xml in [
            "",
            "garbage",
            "<html><channel id='a'/></html>",
            "<tv><channel id='a'>",
            "<tv></tv><tv/>",
            "<!DOCTYPE tv [<!ENTITY bad 'x'>]><tv/>",
            "<tv><channel id='a'><display-name>&bad;</display-name></channel></tv>",
            "<tv><channel id='a'></programme></tv>",
            "<tv><channel id='a' id='b'/></tv>",
            "<tv><channel id='&untrusted;'/></tv>",
            "<tv><channel id='a'><display-name>&#xD800;</display-name></channel></tv>",
            "<tv><channel id='a'><display-name>&#0;</display-name></channel></tv>",
        ] {
            assert!(!parse(xml.as_bytes(), 1), "{xml}");
        }
        let xml=b"<?xml version='1.0'?><tv><channel id='a'><display-name>Amber &amp; Birch</display-name></channel></tv>";
        for chunk in 1..=xml.len() {
            assert!(parse(xml, chunk), "chunk {chunk}");
        }
    }
    #[test]
    fn nesting_is_bounded() {
        let xml = format!(
            "<tv><channel id='a'>{}x{}</channel></tv>",
            "<x>".repeat(33),
            "</x>".repeat(33)
        );
        assert!(!parse(xml.as_bytes(), 5));
    }
    #[test]
    fn chunk_boundaries_preserve_quotes_cdata_comments_and_decoded_limits() {
        let xml=b"<tv><!-- x > y --><channel id='a>b'><display-name><![CDATA[Amber > Birch]]></display-name></channel></tv>";
        for chunk in 1..=xml.len() {
            assert!(parse(xml, chunk), "chunk {chunk}");
        }
        use std::io::Write;
        let xml = format!("<tv><channel id='a'/><!-- {} --></tv>", "x".repeat(16_384));
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(xml.as_bytes()).unwrap();
        let gzip = gzip.finish().unwrap();
        let mut options = ParseOptions::default();
        options.limits.decoded_bytes = 4096;
        let mut parser = StrictXmltvParser::new(options);
        assert!(parser.feed(&gzip, &mut |_| {}).is_err());
    }
    #[test]
    fn declared_single_byte_text_is_decoded_exactly_once() {
        let xml = b"<?xml version='1.0' encoding='ISO-8859-1'?><tv>\
            <channel id='caf\xe9.id'><display-name>Caf\xe9</display-name></channel>\
            <programme channel='caf\xe9.id' start='20261114120000 +0000' stop='20261114130000 +0000'>\
            <title>Synthetic Caf\xe9</title></programme></tv>";
        for size in [1, 7, xml.len()] {
            let mut parser = StrictXmltvParser::new(ParseOptions::default());
            let mut channels = Vec::new();
            let mut programmes = Vec::new();
            let mut collect = |item| match item {
                XmltvItem::Channel(channel) => channels.push(channel),
                XmltvItem::Programme(programme) => programmes.push(programme),
            };
            for piece in xml.chunks(size) {
                parser.feed(piece, &mut collect).unwrap();
            }
            let stats = parser.finish(&mut collect).unwrap();
            assert_eq!(stats.bytes_in, xml.len() as u64);
            assert_eq!(channels[0].id, "café.id", "chunk size {size}");
            assert_eq!(channels[0].display_names, ["Café"]);
            assert_eq!(programmes[0].channel, "café.id");
            assert_eq!(programmes[0].title, "Synthetic Café");
        }
    }
}
