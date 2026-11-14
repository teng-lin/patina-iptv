//! XMLTV parser behaviour on synthetic documents.

use std::io::Write;

use iptv_epg::*;

fn parse_chunked(xml: &[u8], chunk: usize, opts: ParseOptions) -> ParsedXmltv {
    let mut out = ParsedXmltv::default();
    let mut sink = |item| match item {
        XmltvItem::Channel(c) => out.channels.push(c),
        XmltvItem::Programme(p) => out.programmes.push(p),
    };
    let mut parser = XmltvParser::new(opts);
    for c in xml.chunks(chunk.max(1)) {
        parser.feed(c, &mut sink).unwrap();
    }
    out.stats = parser.finish(&mut sink).unwrap();
    out
}

fn t(s: &str) -> i64 {
    parse_xmltv_time(s, TimePolicy::UTC).unwrap()
}

const FULL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="synthetic">
  <!-- a comment with <programme> inside must not confuse anything -->
  <channel id="alpha.example">
    <display-name lang="en">Alpha One</display-name>
    <display-name>Alpha 1 HD</display-name>
    <display-name></display-name>
    <icon src="https://logos.example.com/alpha.png?size=1&amp;fmt=png" />
    <url>https://alpha.example.com</url>
  </channel>
  <channel id="beta.example"><display-name>B&#233;ta &amp; Co</display-name><icon src="https://logos.example.com/b.png?a=1&b=2"/></channel>
  <programme start="20260301120000 +0000" stop="20260301130000 +0000" channel="alpha.example" catchup-id="cu-123">
    <title lang="en">Morning &amp; Noon</title>
    <title lang="xx">Ignored second title</title>
    <sub-title>Part <![CDATA[<One>]]></sub-title>
    <desc lang="en">Line one
line two &#x2014; &unknown; & more</desc>
    <credits><director>Not A Field</director></credits>
    <category>News</category>
    <category lang="en">Talk</category>
    <episode-num system="xmltv_ns">1.4.</episode-num>
    <episode-num system="onscreen">S02E05</episode-num>
    <icon src="https://img.example.com/p1.jpg"/>
    <rating system="SYN"><value>12</value><icon src="https://img.example.com/r.png"/></rating>
    <star-rating><value>3/5</value></star-rating>
  </programme>
  <programme start="20260301130000" channel="alpha.example"><title>No stop, no offset</title></programme>
</tv>
"#;

#[test]
fn parses_every_supported_field() {
    let r = parse_chunked(FULL.as_bytes(), 1 << 20, ParseOptions::default());
    assert_eq!(r.stats.warnings, ParseWarnings::default());
    assert_eq!((r.stats.channels, r.stats.programmes), (2, 2));

    let a = &r.channels[0];
    assert_eq!(a.id, "alpha.example");
    assert_eq!(a.display_names, ["Alpha One", "Alpha 1 HD"]);
    assert_eq!(
        a.icon.as_deref(),
        Some("https://logos.example.com/alpha.png?size=1&fmt=png")
    );
    let b = &r.channels[1];
    assert_eq!(b.display_names, ["B\u{e9}ta & Co"]);
    assert_eq!(
        b.icon.as_deref(),
        Some("https://logos.example.com/b.png?a=1&b=2")
    );

    let p = &r.programmes[0];
    assert_eq!(p.channel, "alpha.example");
    assert_eq!(p.start, t("20260301120000 +0000"));
    assert_eq!(p.stop, Some(t("20260301130000 +0000")));
    assert_eq!(p.title, "Morning & Noon");
    assert_eq!(p.sub_title.as_deref(), Some("Part <One>"));
    assert_eq!(
        p.desc.as_deref(),
        Some("Line one\nline two \u{2014} &unknown; & more")
    );
    assert_eq!(p.categories, ["News", "Talk"]);
    assert_eq!(p.episode_num.len(), 2);
    assert_eq!(p.episode_num[0].season_episode(), Some((Some(2), Some(5))));
    assert_eq!(p.episode_num[1].system.as_deref(), Some("onscreen"));
    assert_eq!(p.icon.as_deref(), Some("https://img.example.com/p1.jpg"));
    assert_eq!(
        p.rating,
        Some(Rating {
            system: Some("SYN".into()),
            value: "12".into()
        })
    );
    assert_eq!(p.catchup_id.as_deref(), Some("cu-123"));

    let q = &r.programmes[1];
    assert_eq!(q.stop, None);
    assert_eq!(q.start, t("20260301130000 +0000"));
}

#[test]
fn result_does_not_depend_on_chunking() {
    let whole = parse_chunked(FULL.as_bytes(), 1 << 20, ParseOptions::default());
    for n in [1, 2, 3, 5, 7, 13, 64, 333] {
        let r = parse_chunked(FULL.as_bytes(), n, ParseOptions::default());
        assert_eq!(r.channels, whole.channels, "chunk {n}");
        assert_eq!(r.programmes, whole.programmes, "chunk {n}");
        assert_eq!(r.stats.warnings, whole.stats.warnings, "chunk {n}");
    }
}

#[test]
fn time_policy_applies_assumed_offset_and_shift() {
    let opts = ParseOptions {
        time: TimePolicy {
            assumed_offset_minutes: 120,
            shift_minutes: 30,
        },
        ..ParseOptions::default()
    };
    let r = parse_chunked(FULL.as_bytes(), 100, opts);
    // Explicit +0000 keeps its offset, then the shift applies.
    assert_eq!(r.programmes[0].start, t("20260301123000 +0000"));
    // No offset: assumed +0200, then the shift.
    assert_eq!(r.programmes[1].start, t("20260301113000 +0000"));
}

#[test]
fn descriptions_can_be_dropped_at_parse_time() {
    let opts = ParseOptions {
        descriptions: false,
        ..ParseOptions::default()
    };
    let r = parse_chunked(FULL.as_bytes(), 50, opts);
    assert!(r.programmes.iter().all(|p| p.desc.is_none()));
    assert_eq!(r.programmes[0].title, "Morning & Noon");
}

const BROKEN: &str = r#"<tv>
<channel><display-name>No id</display-name></channel>
<programme start="20260301120000 +0000" stop="20260301130000 +0000"><title>No channel</title></programme>
<programme channel="c.example" start="tomorrow" stop="20260301130000 +0000"><title>Bad start</title></programme>
<programme channel="c.example" stop="20260301130000 +0000"><title>No start</title></programme>
<programme channel="c.example" start="20260301120000 +0000" stop="soon"><title>Bad stop kept</title></programme>
<programme channel="c.example" start="20260301120000 +0000"><title>  </title></programme>
<programme channel="c.example" start="20260301120000 +0000"/>
<programme channel="c.example" start="20260301120000 +0000"><title>Broken <!x markup</title></programme>
<channel id="ok.example"><display-name>Survivor</display-name></channel>
<programme channel="ok.example" start="20260301140000 +0000" stop="20260301150000 +0000"><title>After the damage</title></programme>
<programme channel="ok.example" start="20260301150000 +0000"><title>Cut off"#;

#[test]
fn malformed_entries_are_counted_and_skipped() {
    for n in [1, 4, 17, 1 << 20] {
        let r = parse_chunked(BROKEN.as_bytes(), n, ParseOptions::default());
        let w = r.stats.warnings;
        assert_eq!(w.channel_without_id, 1, "chunk {n}");
        assert_eq!(w.programme_without_channel, 1, "chunk {n}");
        assert_eq!(w.bad_start, 2, "chunk {n}");
        assert_eq!(w.bad_stop, 1, "chunk {n}");
        assert_eq!(w.programme_without_title, 2, "chunk {n}");
        assert_eq!(w.malformed_xml, 1, "chunk {n}");
        assert_eq!(w.truncated, 1, "chunk {n}");
        let titles: Vec<_> = r.programmes.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["Bad stop kept", "After the damage"], "chunk {n}");
        assert_eq!(r.channels.len(), 1);
        assert_eq!(r.channels[0].id, "ok.example");
    }
}

#[test]
fn oversized_elements_are_skipped() {
    let big = "x".repeat(10_000);
    let xml = format!(
        r#"<tv><programme channel="a.example" start="20260301120000 +0000"><title>Big</title><desc>{big}</desc></programme>
<programme channel="a.example" start="20260301130000 +0000"><title>Small</title></programme></tv>"#
    );
    let opts = ParseOptions {
        max_element_bytes: 1000,
        ..ParseOptions::default()
    };
    let r = parse_chunked(xml.as_bytes(), 256, opts);
    assert_eq!(r.stats.warnings.oversized, 1);
    assert_eq!(r.programmes.len(), 1);
    assert_eq!(r.programmes[0].title, "Small");
    assert!(
        r.stats.peak_buffer_bytes < 2000,
        "{}",
        r.stats.peak_buffer_bytes
    );
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[test]
fn gzip_input_is_detected_and_decoded() {
    let whole = parse_chunked(FULL.as_bytes(), 1 << 20, ParseOptions::default());
    let gz = gzip(FULL.as_bytes());
    for n in [1, 10, 1 << 20] {
        let r = parse_chunked(&gz, n, ParseOptions::default());
        assert_eq!(r.programmes, whole.programmes);
        assert_eq!(r.channels, whole.channels);
        assert_eq!(r.stats.bytes_in, gz.len() as u64);
    }
    // Forcing plain on gzip bytes yields garbage, not a panic.
    let forced = ParseOptions {
        compression: Compression::Plain,
        ..ParseOptions::default()
    };
    let r = parse_chunked(&gz, 100, forced);
    assert!(r.programmes.is_empty());
}

#[test]
fn corrupt_gzip_reports_an_error() {
    let mut gz = gzip(FULL.as_bytes());
    let n = gz.len();
    gz.truncate(n / 2);
    gz.extend_from_slice(&[0xff; 64]);
    let mut parser = XmltvParser::new(ParseOptions::default());
    let mut sink = |_| {};
    let r = parser
        .feed(&gz, &mut sink)
        .and_then(|()| parser.finish(&mut sink).map(|_| ()));
    assert!(matches!(r, Err(DecodeError::Gzip(_))), "{r:?}");
}

/// HTTP gzip around a `.gz` file whose inner layer is corrupt beyond its
/// first decoding slice: the error comes while the parser drains the inner
/// layer between records, and ends the parse like any other decode error.
#[test]
fn a_late_inner_gzip_error_ends_the_parse() {
    // Small identical members keep the inner stream valid for several
    // slices and let the outer layer deliver all of it in one step.
    let mut inner = gzip(b"<tv>");
    let filler = gzip(b" ");
    for _ in 0..1000 {
        inner.extend_from_slice(&filler);
    }
    // An invalid byte: its warning must survive the failure too.
    inner.extend(gzip(
        b"<channel id=\"early.example\"><display-name>Early\xFF</display-name></channel>",
    ));
    for _ in 0..1000 {
        inner.extend_from_slice(&filler);
    }
    let slice = 8 * 1024;
    assert!(inner.len() > 3 * slice, "{}", inner.len());
    inner.extend_from_slice(&[0xff; 64]);
    let outer = gzip(&inner);
    assert!(outer.len() < slice, "one outer step: {}", outer.len());

    let mut channels = Vec::new();
    let mut sink = |item| {
        if let XmltvItem::Channel(c) = item {
            channels.push(c.id);
        }
    };
    let mut parser = XmltvParser::new(ParseOptions::default());
    let r = parser.feed(&outer, &mut sink);
    assert!(matches!(r, Err(DecodeError::Gzip(_))), "{r:?}");
    let failed = parser.stats();
    assert_eq!(failed.bytes_in, outer.len() as u64);
    assert_eq!(failed.warnings.invalid_utf8, 1);
    // Later calls are no-ops: nothing more is counted, decoded or emitted.
    parser
        .feed(b"<channel id=\"late.example\"/>", &mut sink)
        .unwrap();
    assert_eq!(parser.stats(), failed);
    assert_eq!(parser.finish(&mut sink).unwrap(), failed);
    assert_eq!(channels, ["early.example"]);
}

#[test]
fn latin1_documents_are_transcoded() {
    let mut xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<tv><channel id=\"c.example\"><display-name>Cin\xE9ma</display-name></channel>".to_vec();
    xml.extend_from_slice(b"<programme channel=\"c.example\" start=\"20260301120000 +0000\"><title>D\xE9j\xE0 vu</title></programme></tv>");
    let r = parse_chunked(&xml, 7, ParseOptions::default());
    assert_eq!(r.channels[0].display_names, ["Cin\u{e9}ma"]);
    assert_eq!(r.programmes[0].title, "D\u{e9}j\u{e0} vu");
    assert_eq!(r.stats.warnings.total(), 0);
}

#[test]
fn invalid_utf8_is_replaced_and_counted() {
    let xml = b"<tv><programme channel=\"c.example\" start=\"20260301120000 +0000\"><title>Bad \xFF byte</title></programme></tv>";
    let r = parse_chunked(xml, 3, ParseOptions::default());
    assert_eq!(r.programmes[0].title, "Bad \u{FFFD} byte");
    assert_eq!(r.stats.warnings.invalid_utf8, 1);
}

#[test]
fn utf16_is_rejected() {
    let mut parser = XmltvParser::new(ParseOptions::default());
    let r = parser.feed(&[0xFF, 0xFE, b'<', 0, b't', 0], &mut |_| {});
    assert_eq!(r, Err(DecodeError::UnsupportedEncoding("UTF-16")));
}

#[test]
fn bom_and_missing_declaration_are_fine() {
    let mut xml = b"\xEF\xBB\xBF".to_vec();
    xml.extend_from_slice(
        b"<tv><channel id=\"c.example\"><display-name>C</display-name></channel></tv>",
    );
    let r = parse_chunked(&xml, 2, ParseOptions::default());
    assert_eq!(r.channels.len(), 1);
    assert_eq!(r.stats.warnings.total(), 0);
}

#[test]
fn tiny_chunks_stay_linear() {
    // One large element fed byte by byte must not be re-parsed per byte.
    let desc = "word ".repeat(40_000);
    let xml = format!(
        r#"<tv><programme channel="a.example" start="20260301120000 +0000"><title>Big</title><desc>{desc}</desc></programme>
<programme channel="a.example" start="20260301130000 +0000"><title>Next</title></programme></tv>"#
    );
    let started = std::time::Instant::now();
    let r = parse_chunked(xml.as_bytes(), 1, ParseOptions::default());
    assert_eq!(r.programmes.len(), 2);
    assert_eq!(r.programmes[0].desc.as_deref(), Some(desc.trim()));
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
}

#[test]
fn superscript_badges_in_titles_are_folded_at_import() {
    let xml = r#"<tv><channel id="a"><display-name>A</display-name></channel>
<programme start="20260301120000 +0000" stop="20260301130000 +0000" channel="a"><title>Cup Finalᴸᶦᵛᵉ</title><sub-title>ᴺᵉʷ Episode</sub-title></programme>
<programme start="20260301130000 +0000" stop="20260301140000 +0000" channel="a"><title>H₂O Facts</title></programme>
</tv>"#;
    // Fed a byte at a time: a badge split across chunks folds the same.
    let r = parse_chunked(xml.as_bytes(), 1, ParseOptions::default());
    assert_eq!(r.programmes[0].title, "Cup Final Live");
    assert_eq!(r.programmes[0].sub_title.as_deref(), Some("New Episode"));
    assert_eq!(r.programmes[1].title, "H₂O Facts");
}
