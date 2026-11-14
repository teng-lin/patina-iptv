//! Rewrite DASH resource locations without changing segment templates/timing.
use super::{MAX_MANIFEST, invalid, resolve};
use crate::FetchError;
use quick_xml::{
    Reader, Writer,
    events::{BytesStart, BytesText, Event},
};
use reqwest::Url;

pub(super) fn rewrite_dash(body: &[u8], base: &Url, prefix: &str) -> Result<Vec<u8>, FetchError> {
    let mut reader = Reader::from_reader(body);
    let mut writer = Writer::new(Vec::new());
    let mut bases = vec![base.clone()];
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut root_seen = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| invalid("Invalid DASH manifest"))?;
        match event {
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
