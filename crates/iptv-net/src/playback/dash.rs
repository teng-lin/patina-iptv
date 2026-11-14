//! Rewrite DASH resource locations without changing segment templates/timing.
//!
//! The manifest is read here as UTF-8, whatever it declares. A player's XML
//! parser follows the declaration: under ISO-2022-JP, say, an escape
//! sequence turns the bytes after it into other characters, and a
//! `SegmentURL` that is comment or character data here is an element there.
//! So only a manifest that every parser reads as the same text is
//! rewritten ([`same_text_for_every_parser`]).
use super::{MAX_MANIFEST, invalid, resolve};
use crate::FetchError;
use quick_xml::{
    Reader, Writer,
    events::{BytesDecl, BytesStart, BytesText, Event},
};
use reqwest::Url;

/// Refuses a manifest whose bytes a parser that honours the XML declaration
/// could read as another text than this rewriter does: one that is not
/// valid UTF-8, or that holds a C0 control character other than tab, line
/// feed and carriage return. XML 1.0 allows none of those, and the escape
/// among them is what switches character sets in the ISO-2022 encodings.
/// (A declared encoding is checked where the declaration is read.)
fn same_text_for_every_parser(body: &[u8]) -> Result<(), FetchError> {
    if std::str::from_utf8(body).is_err() {
        return Err(invalid("DASH manifest is not UTF-8"));
    }
    if body
        .iter()
        .any(|byte| *byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r'))
    {
        return Err(invalid("DASH manifest holds control characters"));
    }
    Ok(())
}

/// Whether the XML declaration names an encoding that reads these bytes as
/// UTF-8 does: UTF-8 itself, or US-ASCII for a body that is ASCII.
fn declares_this_text(declaration: &BytesDecl, body: &[u8]) -> bool {
    match declaration.encoding() {
        None => true,
        Some(Ok(name)) if name.eq_ignore_ascii_case("utf-8") => true,
        Some(Ok(name)) if name.eq_ignore_ascii_case("us-ascii") => body.is_ascii(),
        Some(_) => false,
    }
}

pub(super) fn rewrite_dash(body: &[u8], base: &Url, prefix: &str) -> Result<Vec<u8>, FetchError> {
    same_text_for_every_parser(body)?;
    let mut reader = Reader::from_reader(body);
    let mut first = true;
    let mut writer = Writer::new(Vec::new());
    let mut bases = vec![base.clone()];
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut root_seen = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| invalid("Invalid DASH manifest"))?;
        let leading = std::mem::take(&mut first);
        match event {
            // A declaration is one only at the very start; a parser that
            // took a later one for it would read the rest differently.
            Event::Decl(ref declaration) => {
                if !leading || !declares_this_text(declaration, body) {
                    return Err(invalid("Unsupported DASH encoding"));
                }
                writer
                    .write_event(event)
                    .map_err(|_| invalid("Invalid DASH output"))?;
            }
            Event::Start(ref start)
                if matches!(start.local_name().as_ref(), "BaseURL" | "Location") =>
            {
                let raw = reader
                    .read_text(start.name())
                    .map_err(|_| invalid("Invalid DASH URL"))?;
                let value =
                    quick_xml::escape::unescape(&raw).map_err(|_| invalid("Invalid DASH URL"))?;
                let resolved = bases
                    .last()
                    .expect("root base")
                    .join(value.trim())
                    .map_err(|_| invalid("Invalid DASH URL"))?;
                let rewritten = resolve(prefix, bases.last().expect("root base"), value.trim())?;
                if start.local_name().as_ref() == "BaseURL" {
                    *bases.last_mut().expect("root base") = resolved;
                }
                writer
                    .write_event(Event::Start(start.clone()))
                    .map_err(|_| invalid("Invalid DASH output"))?;
                writer
                    .write_event(Event::Text(BytesText::new(&rewritten)))
                    .map_err(|_| invalid("Invalid DASH output"))?;
                writer
                    .write_event(Event::End(start.to_end()))
                    .map_err(|_| invalid("Invalid DASH output"))?;
            }
            Event::Start(ref start) | Event::Empty(ref start) => {
                // HTTP time synchronization is a separate fetch path in
                // native DASH players. Refuse it until explicitly proxied.
                if matches!(start.local_name().as_ref(), "UTCTiming" | "ContentSteering") {
                    return Err(invalid("DASH remote clock sources are unsupported"));
                }
                if !root_seen {
                    if start.local_name().as_ref() != "MPD" {
                        return Err(invalid("Unsupported XML playlist"));
                    }
                    root_seen = true;
                }

                let mut changed = BytesStart::new(start.name().as_ref().to_owned());
                let inherited = bases.last().expect("root base").clone();
                for attr in start.attributes() {
                    let attr = attr.map_err(|_| invalid("Invalid DASH attribute"))?;
                    let key = attr.key.as_ref();
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| invalid("Invalid DASH attribute"))?;
                    let rewritten;
                    let resource_attribute = (start.local_name().as_ref() == "SegmentTemplate"
                        && key == "bitstreamSwitching")
                        || matches!(
                            key,
                            "media"
                                | "initialization"
                                | "index"
                                | "sourceURL"
                                | "href"
                                | "xlink:href"
                        );
                    let value = if resource_attribute
                        && !value.is_empty()
                        && value != "urn:mpeg:dash:resolve-to-zero:2013"
                    {
                        rewritten = resolve(prefix, &inherited, &value)?;
                        rewritten.as_str()
                    } else {
                        &value
                    };
                    changed.push_attribute((key, value));
                }
                if matches!(event, Event::Start(_)) {
                    bases.push(inherited);
                    names.push(start.local_name().as_ref().as_bytes().to_vec());
                    writer
                        .write_event(Event::Start(changed))
                        .map_err(|_| invalid("Invalid DASH output"))?;
                } else {
                    writer
                        .write_event(Event::Empty(changed))
                        .map_err(|_| invalid("Invalid DASH output"))?;
                }
            }
            Event::End(_) => {
                if names.pop().is_none() {
                    return Err(invalid("Invalid DASH nesting"));
                }
                bases.pop();
                writer
                    .write_event(event)
                    .map_err(|_| invalid("Invalid DASH output"))?;
            }
            Event::Eof => break,
            Event::DocType(_) => return Err(invalid("DASH document types are unsupported")),
            Event::GeneralRef(ref value) => {
                if !matches!(value.as_ref(), "amp" | "lt" | "gt" | "quot" | "apos")
                    && !value.as_ref().starts_with('#')
                {
                    return Err(invalid("DASH custom entities are unsupported"));
                }
                writer
                    .write_event(event)
                    .map_err(|_| invalid("Invalid DASH output"))?;
            }
            _ => {
                writer
                    .write_event(event)
                    .map_err(|_| invalid("Invalid DASH output"))?;
            }
        }
        if writer.get_ref().len() > MAX_MANIFEST * 8 || bases.len() > 128 {
            return Err(invalid("DASH manifest exceeds limit"));
        }
    }
    if !root_seen || !names.is_empty() {
        return Err(invalid("Unclosed DASH element"));
    }
    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_bases_templates_and_absolute_segments_stay_on_relay() {
        let base = Url::parse("https://origin.example.test/manifest/main.mpd").unwrap();
        let body = b"<MPD><BaseURL>../video/</BaseURL><Period><AdaptationSet>\
<Representation><SegmentTemplate initialization=\"init.mp4\" \
media=\"part-$Number$.m4s\" index=\"https://cdn.example.test/index\"/>\
<SegmentList><SegmentURL media=\"https://cdn.example.test/one.m4s\"/>\
</SegmentList></Representation></AdaptationSet></Period></MPD>";
        let out = String::from_utf8(rewrite_dash(body, &base, "http://127.0.0.1:1/cap/").unwrap())
            .unwrap();
        assert!(
            out.contains(
                "http://127.0.0.1:1/cap/https://origin.example.test/video/part-$Number$.m4s"
            ),
            "{out}"
        );
        assert!(out.contains("http://127.0.0.1:1/cap/https://cdn.example.test/one.m4s"));
        assert!(out.contains("index=\"http://127.0.0.1:1/cap/https://cdn.example.test/index\""));
        assert!(rewrite_dash(b"<!DOCTYPE MPD><MPD/>", &base, "http://local/").is_err());
        assert!(rewrite_dash(br#"<MPD><UTCTiming schemeIdUri="urn:mpeg:dash:utc:http-iso:2014" value="http://127.0.0.1/private"/></MPD>"#, &base, "http://local/").is_err());
    }
    const HIDDEN: &str = "http://192.0.2.7/direct.m4s";

    fn refused(body: &[u8]) {
        let base = Url::parse("https://origin.example.test/main.mpd").unwrap();
        assert!(
            rewrite_dash(body, &base, "http://local/").is_err(),
            "accepted: {:?}",
            String::from_utf8_lossy(body)
        );
    }

    /// The reported confusion: under ISO-2022-JP the bytes after an escape
    /// sequence are two-byte characters, among them the `--` that would
    /// end this comment, so the element after it is real for a parser that
    /// follows the declaration and commented out here.
    #[test]
    fn a_declared_encoding_other_than_utf8_or_ascii_is_refused() {
        refused(
            format!(
                "<?xml version=\"1.0\" encoding=\"ISO-2022-JP\"?><MPD><Period><!-- \x1b$B--\x1b(B>\
                 <SegmentList><SegmentURL media=\"{HIDDEN}\"/></SegmentList><!-- --></Period></MPD>"
            )
            .as_bytes(),
        );
        // The declaration alone refuses it, without the escape bytes.
        for encoding in [
            "ISO-2022-JP",
            "iso-8859-1",
            "UTF-16",
            "UTF-7",
            "Shift_JIS",
            "utf8",
            "",
        ] {
            refused(format!("<?xml version=\"1.0\" encoding=\"{encoding}\"?><MPD/>").as_bytes());
        }
        // Declared ASCII with bytes that are not: two readings again.
        refused("<?xml version=\"1.0\" encoding=\"US-ASCII\"?><MPD><!-- é --></MPD>".as_bytes());
        // Only the first thing in a document is its declaration.
        refused(b"<MPD><?xml version=\"1.0\" encoding=\"UTF-8\"?></MPD>");
        refused(b"<!-- --><?xml version=\"1.0\"?><MPD/>");
    }

    #[test]
    fn an_escape_byte_or_another_control_character_is_refused() {
        // No declaration: a parser may still be told the encoding (or
        // guess it), and XML allows the byte in no encoding.
        refused(
            format!(
                "<MPD><Period><!-- \x1b$B--\x1b(B><SegmentList><SegmentURL media=\"{HIDDEN}\"/>\
                 </SegmentList><!-- --></Period></MPD>"
            )
            .as_bytes(),
        );
        for control in [0x00, 0x01, 0x08, 0x0b, 0x0c, 0x0e, 0x0f, 0x1b, 0x1f] {
            let mut body = b"<MPD><!-- ".to_vec();
            body.push(control);
            body.extend_from_slice(b" --></MPD>");
            refused(&body);
        }
        // Not UTF-8 at all.
        refused(b"<MPD><!-- \xe9 --></MPD>");
        refused(b"<MPD><!-- \xff\xfe --></MPD>");
    }

    #[test]
    fn utf8_and_ascii_declarations_and_ordinary_white_space_remain_valid() {
        let base = Url::parse("https://origin.example.test/main.mpd").unwrap();
        for declaration in [
            "<?xml version=\"1.0\"?>",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>",
            "<?xml version='1.0' encoding='US-ASCII'?>",
            "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        ] {
            let body = format!(
                "{declaration}\r\n<MPD>\n\t<Period>\r\n<SegmentList>\
                 <SegmentURL media=\"one.m4s\"/></SegmentList></Period>\n</MPD>\n"
            );
            let out = String::from_utf8(
                rewrite_dash(body.as_bytes(), &base, "http://127.0.0.1:1/cap/").unwrap(),
            )
            .unwrap();
            assert!(
                out.contains(
                    "media=\"http://127.0.0.1:1/cap/https://origin.example.test/one.m4s\""
                ),
                "{declaration}: {out}"
            );
        }
        // Text outside ASCII is fine where UTF-8 is declared or implied.
        let body = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><MPD><!-- Cañal --></MPD>";
        assert!(rewrite_dash(body.as_bytes(), &base, "http://local/").is_ok());
    }

    #[test]
    fn standard_entities_and_resolve_to_zero_remain_valid() {
        let base = Url::parse("https://origin.example.test/main.mpd").unwrap();
        let body = br#"<MPD><ProgramInformation><Title>A &amp; B &#65;</Title></ProgramInformation><Period xlink:href="urn:mpeg:dash:resolve-to-zero:2013"/></MPD>"#;
        let out = String::from_utf8(rewrite_dash(body, &base, "http://local/").unwrap()).unwrap();
        assert!(out.contains("A &amp; B &#65;"));
        assert!(out.contains("xlink:href=\"urn:mpeg:dash:resolve-to-zero:2013\""));
        assert!(rewrite_dash(b"<MPD>&custom;</MPD>", &base, "http://local/").is_err());
    }
}
