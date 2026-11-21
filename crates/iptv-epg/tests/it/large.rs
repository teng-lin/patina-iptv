//! Scale test: ~2,000 channels x 7 days generated on the fly and fed in
//! chunks, so the whole document never exists in memory.
//!
//! `cargo test -p iptv-epg --release --test it large -- --nocapture` prints
//! throughput and memory figures.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Instant;

use iptv_epg::*;

const CHANNELS: usize = 2_000;
const DAYS: i64 = 7;
const T0: i64 = 1_772_323_200; // 2026-03-01T00:00:00Z
const CHUNK: usize = 64 * 1024;
const DURATIONS_MIN: [i64; 6] = [30, 60, 45, 90, 30, 45];
const GENRES: [&str; 5] = ["News", "Film", "Sport", "Kids", "Documentary"];

/// Writes the synthetic document piecewise into `emit`.
fn generate(channels: usize, mut emit: impl FnMut(&[u8])) -> u64 {
    let mut buf = String::with_capacity(2 * CHUNK);
    let mut programs = 0u64;
    let mut flush = |buf: &mut String, force: bool| {
        if force || buf.len() >= CHUNK {
            emit(buf.as_bytes());
            buf.clear();
        }
    };
    buf.push_str(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv generator-info-name=\"synthetic\">\n",
    );
    for c in 0..channels {
        let _ = writeln!(
            buf,
            r#"<channel id="ch{c}.example"><display-name>Synthetic {c} HD</display-name><icon src="https://logos.example.com/{c}.png"/></channel>"#
        );
        flush(&mut buf, false);
    }
    // Interleaved by day, like many real feeds: the store has to bucket.
    for day in 0..DAYS {
        for c in 0..channels {
            let mut t = T0 + day * DAY;
            let end = t + DAY;
            let mut i = c;
            while t < end {
                let dur = DURATIONS_MIN[i % DURATIONS_MIN.len()] * 60;
                let stop = (t + dur).min(end);
                let _ = writeln!(
                    buf,
                    concat!(
                        r#"<programme start="{}" stop="{}" channel="ch{c}.example">"#,
                        r#"<title>Show {} &amp; friends</title>"#,
                        r#"<desc>Episode {} of a synthetic series about item {}. Nothing here is real."#,
                        r#"</desc><category>{}</category>"#,
                        r#"<episode-num system="xmltv_ns">{}.{}.</episode-num></programme>"#,
                    ),
                    format_xmltv_time(t),
                    format_xmltv_time(stop),
                    i % 97,
                    i % 13,
                    i % 251,
                    GENRES[i % GENRES.len()],
                    i % 5,
                    i % 20,
                    // Named here: a format string built by `concat!` cannot
                    // capture variables by itself.
                    c = c,
                );
                programs += 1;
                t = stop;
                i += 1;
                flush(&mut buf, false);
            }
        }
    }
    buf.push_str("</tv>\n");
    flush(&mut buf, true);
    programs
}

#[test]
fn two_thousand_channels_for_seven_days() {
    let now = T0 + 3 * DAY + 1234;
    let opts = StoreOptions {
        store_descriptions: true,
        past_days_to_keep: None,
    };
    let mut builder = SourceBuilder::new(FeedId(1), 0, opts, now);
    let mut parser = XmltvParser::new(ParseOptions::default());
    let mut bytes = 0u64;
    let started = Instant::now();
    let generated = generate(CHANNELS, |chunk| {
        bytes += chunk.len() as u64;
        parser.feed(chunk, &mut |item| builder.push(item)).unwrap();
    });
    let stats = parser.finish(&mut |item| builder.push(item)).unwrap();
    let parsed_in = started.elapsed();
    let finishing = Instant::now();
    let src = builder.finish();
    let finished_in = finishing.elapsed();

    assert_eq!(stats.warnings, ParseWarnings::default());
    assert_eq!(stats.channels, CHANNELS as u64);
    assert_eq!(stats.programs, generated);
    assert_eq!(src.program_count() as u64, generated);
    assert_eq!(src.report().overlaps_trimmed, 0);
    // Bounded: one chunk plus at most one element, whatever the input size.
    assert!(
        stats.peak_buffer_bytes < 2 * CHUNK + 4096,
        "peak buffer {}",
        stats.peak_buffer_bytes
    );

    let mut store = GuideStore::new();
    store.replace_source(src);
    let mut index = MatchIndex::new();
    let src = &store.sources()[0];
    index.add_source(src.id(), src.priority(), src.channels());
    let keys: Vec<ChannelKey> = (0..CHANNELS)
        .map(|c| ChannelKey(format!("pl-{c}")))
        .collect();
    let names: Vec<String> = (0..CHANNELS).map(|c| format!("SYNTHETIC {c}")).collect();
    let matching = Instant::now();
    let result = index.match_channels(
        keys.iter().zip(&names).map(|(key, name)| PlaylistChannel {
            key,
            tvg_id: None,
            name,
            tvg_name: None,
        }),
        &Overrides::new(),
    );
    let matched_in = matching.elapsed();
    assert_eq!(result.report.by_name, CHANNELS);
    store.set_matches(result.matches);

    // Guide-shaped queries: now/next for every channel and a 3-hour window
    // for a screenful.
    let querying = Instant::now();
    let nn = store.now_next(&keys, now);
    assert!(nn.iter().all(|n| n.now.is_some() && n.next.is_some()));
    let mut cells = 0;
    for k in keys.iter().take(12) {
        cells += store.programs(k, now, now + 3 * 3600).count();
    }
    assert!(cells >= 12 * 2);
    let queried_in = querying.elapsed();

    let secs = parsed_in.as_secs_f64();
    eprintln!(
        "large: {CHANNELS} channels x {DAYS} days = {generated} programmes, {:.1} MB XML",
        bytes as f64 / 1e6
    );
    eprintln!(
        "large: parse+ingest {:.2}s ({:.1} MB/s, {:.0}k programmes/s), finish {:.0} ms, match {:.0} ms, now/next x{CHANNELS} + 12 windows {:.2} ms",
        secs,
        bytes as f64 / 1e6 / secs,
        generated as f64 / 1e3 / secs,
        finished_in.as_secs_f64() * 1e3,
        matched_in.as_secs_f64() * 1e3,
        queried_in.as_secs_f64() * 1e3,
    );
    eprintln!(
        "large: peak parser buffer {} KiB, store heap ~{:.1} MiB ({:.0} B/programme), {} distinct strings",
        stats.peak_buffer_bytes / 1024,
        store.heap_bytes() as f64 / (1 << 20) as f64,
        store.heap_bytes() as f64 / generated as f64,
        store.sources()[0].string_count(),
    );
    // Generous: debug builds on slow CI machines.
    assert!(secs < 180.0, "parse took {secs:.1}s");
}

#[test]
fn gzip_large_input_in_small_chunks() {
    let mut xml = Vec::new();
    let generated = generate(150, |c| xml.extend_from_slice(c));
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(&xml).unwrap();
    let gz = e.finish().unwrap();
    drop(xml);

    let mut count = 0u64;
    let mut parser = XmltvParser::new(ParseOptions::default());
    for chunk in gz.chunks(4096) {
        parser
            .feed(chunk, &mut |item| {
                count += u64::from(matches!(item, XmltvItem::Program(_)))
            })
            .unwrap();
    }
    let stats = parser.finish(&mut |_| {}).unwrap();
    assert_eq!((count, stats.programs), (generated, generated));
    assert_eq!(stats.warnings, ParseWarnings::default());
    // A gzip slice inflates to more than its size, but stays small.
    assert!(
        stats.peak_buffer_bytes < 1 << 20,
        "{}",
        stats.peak_buffer_bytes
    );
}
