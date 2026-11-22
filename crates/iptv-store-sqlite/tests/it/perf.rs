//! Large-catalog timings. Run in release:
//!
//! ```sh
//! cargo test --release -p iptv-store-sqlite --test it perf -- --ignored --nocapture
//! ```
//!
//! Budget (release, desktop-class CPU): replacing 50k channels in one
//! transaction well under 3 s. Debug builds compile SQLite unoptimized, so
//! the budget is only asserted in release.

use crate::common;

use std::time::{Duration, Instant};

use common::*;
use iptv_store_sqlite::{ChannelQuery, ChannelScope, OrderScope, Store};

const CHANNELS: usize = 50_000;
const GROUPS: usize = 250;
const BUDGET: Duration = Duration::from_secs(3);

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    println!("{label:<44} {:>9.1} ms", took.as_secs_f64() * 1000.0);
    (out, took)
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn replace_50k_channel_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("bench.db")).unwrap();
    let src = s.add_source(&xtream_source("Bench")).unwrap();
    let first = catalog(CHANNELS, GROUPS, XT_PASSWORD);
    let rotated = catalog(CHANNELS, GROUPS, "rotated-pass");
    println!("{CHANNELS} channels, {GROUPS} groups, file-backed WAL store");

    let (_, fresh) = timed("replace_catalog (empty store)", || {
        replace(&mut s, src, &first)
    });
    // A realistic amount of customization.
    timed("1000 favorites + 500 renames + 500 hides", || {
        for i in 0..1000 {
            s.set_favorite(src, &first.channels[i * 37].id, true)
                .unwrap();
        }
        for i in 0..500 {
            let id = &first.channels[i * 91].id;
            s.rename_channel(src, id, Some(&format!("Custom {i}")))
                .unwrap();
            s.set_hidden(src, &first.channels[i * 53 + 1].id, true)
                .unwrap();
        }
    });
    let group = OrderScope::Group(group(7).id);
    timed("move_channel in a 200-channel group", || {
        s.move_channel(src, &group, &first.channels[7 + GROUPS * 100].id, 0)
            .unwrap()
    });
    let (_, refresh) = timed("replace_catalog (refresh, rotated URLs)", || {
        replace(&mut s, src, &rotated)
    });

    let all = ChannelQuery::default();
    let (count, _) = timed("channel_count (all visible)", || {
        s.channel_count(src, &all).unwrap()
    });
    assert_eq!(count, (CHANNELS - 500) as u64);
    timed("load_channels first page of 50 (all)", || {
        s.load_channels(
            src,
            &ChannelQuery {
                limit: Some(50),
                ..Default::default()
            },
        )
        .unwrap()
    });
    timed("load_channels page 500 of 50 (all)", || {
        s.load_channels(
            src,
            &ChannelQuery {
                offset: 25_000,
                limit: Some(50),
                ..Default::default()
            },
        )
        .unwrap()
    });
    let (rows, _) = timed("load_channels one group, manual order", || {
        s.load_channels(
            src,
            &ChannelQuery {
                scope: ChannelScope::Group(group_id(7)),
                ..Default::default()
            },
        )
        .unwrap()
    });
    assert_eq!(rows[0].channel.id, first.channels[7 + GROUPS * 100].id);
    timed("load_channels favorites", || {
        s.load_channels(
            src,
            &ChannelQuery {
                favorites_only: true,
                ..Default::default()
            },
        )
        .unwrap()
    });
    let (all_rows, _) = timed("load_channels all 50k decoded", || {
        s.load_channels(src, &all).unwrap()
    });
    assert_eq!(all_rows.len() as u64, count);
    let (hits, _) = timed("search_channels \"channel 4999\" (FTS5)", || {
        s.search_channels(Some(src), "channel 4999", 50).unwrap()
    });
    assert!(!hits.is_empty());
    timed("search_channels \"custom 4\" (custom names)", || {
        s.search_channels(Some(src), "custom 4", 50).unwrap()
    });

    if !cfg!(debug_assertions) {
        assert!(fresh < BUDGET, "fresh replace took {fresh:?}");
        assert!(refresh < BUDGET, "refresh took {refresh:?}");
    }
}

fn group_id(i: usize) -> iptv_providers::GroupId {
    group(i).id
}

/// A guide of about 250,000 programs replacing one of the same size, in
/// one call and in 400-row slices (the browser's), with the longest slice.
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn replace_250k_program_guide() {
    use crate::{epg_synth, staged};
    use iptv_epg::StoreOptions;
    use iptv_import::EpgSourceConfig;
    use iptv_store_sqlite::{EpgFeedMeta, EpgReplacement, Step};

    let shape = epg_synth::Shape {
        channels: 3000,
        days_back: 2,
        days_ahead: 1,
    };
    let options = StoreOptions {
        store_descriptions: false,
        past_days_to_keep: None,
    };
    let (guide, programs) = epg_synth::import(shape, options);
    let feeds = [EpgFeedMeta {
        config: EpgSourceConfig::new(
            iptv_epg::FeedId(1),
            iptv_providers::SecretUrl::new("http://epg.example.com/bench.xml"),
        ),
        fetched_at: epg_synth::NOW,
        etag: None,
        last_modified: None,
        bytes: 0,
    }];
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("bench.db")).unwrap();
    let src = s.add_source(&xtream_source("Bench")).unwrap();
    println!("{programs} programs on {} channels", shape.channels);
    timed("replace_epg (empty store)", || {
        staged::replace_epg(&mut s, src, &guide, &feeds, 1).unwrap()
    });
    let (mut stage, import, _stage_dir) = staged::stage(&s, &guide);
    timed("replace_epg (refresh, one call)", || {
        s.replace_epg(src, &mut stage, &import, &feeds, 2).unwrap()
    });
    let (mut stage, import, _stage_dir) = staged::stage(&s, &guide);
    let (mut slices, mut longest) = (0, Duration::ZERO);
    // The total includes making the job (flushing the stage), as the
    // one-call timing does.
    let (_, sliced) = timed("EpgReplacement in 400-row slices", || {
        let mut job = EpgReplacement::new(
            &mut s,
            src,
            &mut stage,
            std::borrow::Cow::Borrowed(&import),
            &feeds[..],
            3,
        )
        .unwrap();
        loop {
            slices += 1;
            let start = Instant::now();
            let step = job.step(400).unwrap();
            longest = longest.max(start.elapsed());
            if let Step::Done(counts) = step {
                break counts;
            }
        }
    });
    println!(
        "{slices} slices, longest {:.2} ms, {:.0} ms in all",
        longest.as_secs_f64() * 1000.0,
        sliced.as_secs_f64() * 1000.0
    );
}
