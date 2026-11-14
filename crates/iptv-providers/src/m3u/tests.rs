use super::*;
use crate::model::WarningLocation;

const SAMPLE: &str = "\u{feff}#EXTM3U url-tvg=\"http://epg.example.com/a.xml,http://epg.example.com/b.xml\" x-tvg-url=\"http://epg.example.com/a.xml\"\r\n\
\r\n\
#EXTINF:-1 tvg-id=\"one.example\" tvg-name=\"One\" tvg-logo=\"http://img.example.com/1.png\" tvg-chno=\"101\" group-title=\"News;Local\",Channel One\r\n\
#EXTVLCOPT:http-user-agent=SyntheticAgent/1.0\r\n\
#EXTVLCOPT:http-referrer=http://ref.example.com/\r\n\
http://live.example.com/one.m3u8\r\n\
#KODIPROP:inputstream.adaptive.license_type=clearkey\r\n\
#EXTINF:-1 group-title=\"Movies\" radio=\"true\",Radio Two\r\n\
http://live.example.com/two.mp3|User-Agent=PipeAgent&Referer=http://pipe.example.com/\r\n\
#EXTINF:0,Orphan without URL\r\n\
#EXTINF:-1,Three\r\n\
#EXTGRP:Grouped\r\n\
http://live.example.com/three.ts\r\n\
http://live.example.com/bare/four.ts\r\n";

#[test]
fn parses_sample_playlist() {
    let pl = parse_m3u(SAMPLE);
    assert!(pl.header.declared);
    assert_eq!(
        pl.header.epg_urls,
        vec![
            "http://epg.example.com/a.xml",
            "http://epg.example.com/b.xml"
        ]
    );
    let ch = &pl.channels.items;
    assert_eq!(ch.len(), 4);

    let one = &ch[0];
    assert_eq!(one.name, "Channel One");
    assert_eq!(one.epg_id.as_deref(), Some("one.example"));
    assert_eq!(one.epg_name.as_deref(), Some("One"));
    assert_eq!(one.number, Some(101));
    assert_eq!(one.logo.as_deref(), Some("http://img.example.com/1.png"));
    assert_eq!(one.groups.len(), 2);
    let StreamLocator::Url { url, headers, .. } = &one.stream else {
        panic!()
    };
    assert_eq!(url, "http://live.example.com/one.m3u8");
    assert!(headers.contains(&("User-Agent".into(), "SyntheticAgent/1.0".into())));
    assert!(headers.contains(&("Referer".into(), "http://ref.example.com/".into())));

    let two = &ch[1];
    assert!(two.radio);
    let StreamLocator::Url {
        url,
        headers,
        properties,
    } = &two.stream
    else {
        panic!()
    };
    assert_eq!(url, "http://live.example.com/two.mp3");
    assert!(headers.contains(&("User-Agent".into(), "PipeAgent".into())));
    assert!(headers.contains(&("Referer".into(), "http://pipe.example.com/".into())));
    // KODIPROP before #EXTINF belongs to the next entry.
    assert_eq!(
        properties,
        &vec![(
            "inputstream.adaptive.license_type".to_owned(),
            "clearkey".to_owned()
        )]
    );

    // The orphan was skipped with a warning at its line.
    assert_eq!(pl.channels.skipped, 1);
    assert_eq!(pl.channels.warnings[0].location, WarningLocation::Line(10));

    // #EXTGRP between #EXTINF and the URL applies.
    let three = &ch[2];
    assert_eq!(three.name, "Three");
    let grouped = pl.groups.iter().find(|g| g.name == "Grouped").unwrap();
    assert_eq!(three.groups, vec![grouped.id.clone()]);

    // Bare URL entry named after its file.
    assert_eq!(ch[3].name, "four");
    assert!(ch[3].groups.is_empty());

    let names: Vec<&str> = pl.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, vec!["News", "Local", "Movies", "Grouped"]);
}

#[test]
fn chunking_does_not_change_the_result() {
    let whole = parse_m3u(SAMPLE);
    for size in [1, 2, 3, 7, 64] {
        let mut p = M3uParser::new();
        for chunk in SAMPLE.as_bytes().chunks(size) {
            p.feed(chunk);
        }
        assert_eq!(p.finish(), whole, "chunk size {size}");
    }
}

#[test]
fn tolerates_missing_header_bare_cr_and_invalid_utf8() {
    let mut bytes = b"#EXTINF:-1,Caf\xE9\r#EXTINF:-1,Second\rhttp://live.example.com/2\r".to_vec();
    bytes.extend_from_slice(b"#EXTINF:-1,Third\nhttp://live.example.com/3");
    let mut p = M3uParser::new();
    p.feed(&bytes);
    let pl = p.finish();
    assert!(!pl.header.declared);
    // "Caf\xE9" had no URL; the next two are fine, last line has no newline.
    assert_eq!(
        pl.channels
            .items
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Second", "Third"]
    );
    assert_eq!(pl.channels.skipped, 1);
}

#[test]
fn stable_ids_and_duplicates() {
    let text = "#EXTINF:-1,Dup\nhttp://live.example.com/d.ts\n#EXTINF:-1,Dup\nhttp://live.example.com/d.ts\n#EXTINF:-1,Other\nhttp://live.example.com/o.ts\n";
    let a = parse_m3u(text).channels.items;
    assert_eq!(a[1].id.as_str(), format!("{}~2", a[0].id));
    // A refresh with a rotated token, different order and a new entry keeps ids.
    let refreshed = "#EXTINF:-1,New\nhttp://live.example.com/n.ts\n#EXTINF:-1,Other\nhttps://live.example.com/o.ts?token=zzz\n#EXTINF:-1,Dup\nhttp://live.example.com/d.ts\n";
    let b = parse_m3u(refreshed).channels.items;
    assert_eq!(b[1].id, a[2].id);
    assert_eq!(b[2].id, a[0].id);
}

#[test]
fn extinf_title_fallbacks_and_attribute_headers() {
    let text = "#EXTINF:-1 tvg-name=\"From Tvg\" user-agent=\"AttrAgent\" tvg-shift=\"-1.5\",\nhttp://live.example.com/x\n";
    let ch = &parse_m3u(text).channels.items[0];
    assert_eq!(ch.name, "From Tvg");
    assert_eq!(ch.epg_shift_minutes, Some(-90));
    let StreamLocator::Url { headers, .. } = &ch.stream else {
        panic!()
    };
    assert_eq!(
        headers,
        &vec![("User-Agent".to_owned(), "AttrAgent".to_owned())]
    );
}

#[test]
fn kodiprop_stream_headers_become_headers() {
    let text = "#EXTINF:-1,K\n#KODIPROP:inputstream.adaptive.stream_headers=User-Agent=KAgent&Referer=http%3A%2F%2Fr.example.com%2F\nhttp://live.example.com/k.mpd\n";
    let ch = &parse_m3u(text).channels.items[0];
    let StreamLocator::Url {
        headers,
        properties,
        ..
    } = &ch.stream
    else {
        panic!()
    };
    assert!(headers.contains(&("User-Agent".into(), "KAgent".into())));
    assert!(headers.contains(&("Referer".into(), "http://r.example.com/".into())));
    assert_eq!(properties.len(), 1);
}

#[test]
fn flags_hls_manifests() {
    let pl = parse_m3u("#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg1.ts\n");
    assert!(pl.header.looks_like_hls);
}

#[test]
fn very_long_urls_are_accepted() {
    let long = format!("http://live.example.com/play?jwt={}", "a".repeat(5000));
    let pl = parse_m3u(&format!(
        "#EXTINF:-1,Long\n{long}\n#EXTINF:-1,Next\nhttp://live.example.com/n\n"
    ));
    assert_eq!(pl.channels.items.len(), 2);
}

/// Generates `n` synthetic entries (~200 bytes each).
fn synthetic_playlist(n: usize) -> String {
    let mut s = String::with_capacity(n * 220);
    s.push_str("#EXTM3U url-tvg=\"http://epg.example.com/guide.xml\"\n");
    for i in 0..n {
        s.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"ch{i}.example\" tvg-name=\"Channel {i}\" \
             tvg-logo=\"http://img.example.com/{i}.png\" tvg-chno=\"{i}\" \
             group-title=\"Group {}\" catchup=\"shift\" catchup-days=\"3\",Channel {i}\n\
             http://live.example.com/stream/{i}.m3u8?token=t{i}\n",
            i % 50
        ));
    }
    s
}

#[test]
fn parses_100k_entries_quickly_and_linearly() {
    let n = 100_000;
    let text = synthetic_playlist(n);
    let t0 = std::time::Instant::now();
    let mut p = M3uParser::new();
    let mut total = 0;
    for chunk in text.as_bytes().chunks(64 * 1024) {
        p.feed(chunk);
        total += p.take_channels().len();
    }
    let pl = p.finish();
    total += pl.channels.items.len();
    let elapsed = t0.elapsed();
    eprintln!(
        "parsed {n} entries ({} MiB) in {elapsed:?}",
        text.len() / (1024 * 1024)
    );
    assert_eq!(total, n);
    assert_eq!(pl.groups.len(), 50);
    assert_eq!(pl.channels.warning_count(), 0);
    // Generous for unoptimised debug builds on slow CI; release is ~10x faster.
    assert!(elapsed.as_secs() < 20, "took {elapsed:?}");

    // Linearity: a 4x larger input must not take ~16x as long.
    let small = synthetic_playlist(n / 4);
    let t1 = std::time::Instant::now();
    let _ = parse_m3u(&small);
    let small_elapsed = t1.elapsed().max(std::time::Duration::from_millis(1));
    assert!(
        elapsed < small_elapsed * 10,
        "not linear: {elapsed:?} vs {small_elapsed:?} for n/4"
    );
}

fn first_catch_up(text: &str) -> Option<CatchUp> {
    parse_m3u(text).channels.items.remove(0).catch_up
}

#[test]
fn catch_up_without_days_keeps_an_unknown_window() {
    let cu = first_catch_up("#EXTINF:-1 catchup=\"xc\",A\nhttp://l.example.com/a\n").unwrap();
    assert_eq!((&cu.kind, cu.days), (&CatchUpKind::Xtream, None));
    assert_eq!(cu.window_days(7), 7);
    // Header type without days applies to every entry.
    let cu = first_catch_up("#EXTM3U catchup=\"shift\"\n#EXTINF:-1,A\nhttp://l.example.com/a\n")
        .unwrap();
    assert_eq!((cu.kind, cu.days), (CatchUpKind::Shift, None));
    // A source without a type is Kodi's `default`.
    let cu = first_catch_up(
        "#EXTINF:-1 catchup-source=\"http://arc.example.com/{utc}\",A\nhttp://l.example.com/a\n",
    )
    .unwrap();
    assert_eq!((cu.kind, cu.days), (CatchUpKind::Default, None));
    // Declared days win over the default; "0" is not a window.
    let cu = first_catch_up(
        "#EXTINF:-1 catchup=\"shift\" catchup-days=\"2.5\",A\nhttp://l.example.com/a\n",
    )
    .unwrap();
    assert_eq!((cu.days, cu.window_days(7)), (Some(3), 3));
    let cu = first_catch_up(
        "#EXTINF:-1 catchup=\"shift\" catchup-days=\"0\",A\nhttp://l.example.com/a\n",
    )
    .unwrap();
    assert_eq!(cu.days, None);
    // Nothing declared: no catch-up.
    assert_eq!(
        first_catch_up("#EXTINF:-1,A\nhttp://l.example.com/a\n"),
        None
    );
    assert_eq!(
        first_catch_up("#EXTINF:-1 tvg-rec=\"0\",A\nhttp://l.example.com/a\n"),
        None
    );
}

#[test]
fn parses_vod_type_and_catchup_correction() {
    let text = "#EXTM3U catchup-correction=\"-2.5\"\n\
                #EXTINF:-1 catchup=\"vod\" catchup-days=\"3\",A\nhttp://l.example.com/a\n\
                #EXTINF:-1 catchup=\"append\" catchup-correction=\"1,25\",B\nhttp://l.example.com/b\n\
                #EXTINF:-1 catchup=\"shift\" catchup-correction=\"40\",C\nhttp://l.example.com/c\n";
    let items = parse_m3u(text).channels.items;
    let cu: Vec<&CatchUp> = items.iter().map(|c| c.catch_up.as_ref().unwrap()).collect();
    assert_eq!(
        (&cu[0].kind, cu[0].correction_secs),
        (&CatchUpKind::Vod, -9000)
    );
    assert_eq!(
        cu[1].correction_secs, 4500,
        "per-channel overrides the header"
    );
    assert_eq!(cu[2].correction_secs, 0, "outside −12…+14 h is ignored");
}
