//! Heap of the guide, measured with a counting allocator (this thread's
//! Rust allocations; SQLite's own heap is reported separately).
//!
//! ```sh
//! cargo test --release -p iptv-store-sqlite --test memory epg_memory:: -- --nocapture
//! cargo test --release -p iptv-store-sqlite --test memory epg_memory:: -- --ignored --nocapture
//! ```
//!
//! The first runs the 1000-channel guide (~83 programmes per channel, as
//! `make-iptv-fixtures.py --channels 1000 --days-back 2 --days-ahead 1`
//! writes it); the second the script's default (5000 channels, 9 days,
//! 1.2M programmes).

use crate::epg_synth;
use crate::staged;

use crate::allocations::thread::{LIVE, PEAK};
use std::cell::Cell;
use std::time::Instant;

use epg_synth::{NOW, Shape};
use iptv_epg::{GuideStore, StoreOptions};
use iptv_guide::{EpgSchedule, GuideChannel};
use iptv_import::EpgSourceConfig;
use iptv_providers::SecretUrl;
use iptv_store_sqlite::{EpgFeedMeta, EpgWindow, Store};

/// Live and peak bytes allocated by `f` on this thread, and its result.
fn measure<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    let base = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(base));
    let out = f();
    let live = LIVE.with(Cell::get) - base;
    let peak = PEAK.with(Cell::get) - base;
    (out, live.max(0) as usize, peak.max(0) as usize)
}

fn mib(b: usize) -> f64 {
    b as f64 / f64::from(1 << 20)
}

fn schedule(shape: Shape, store: GuideStore) -> EpgSchedule {
    let channels = (0..shape.channels)
        .map(|n| GuideChannel {
            key: epg_synth::key(n),
            name: format!("Synth {n:04}"),
            number: n as u32 + 1,
            catchup_days: 0,
        })
        .collect();
    EpgSchedule::new(vec![("All".into(), channels)], store, epg_synth::NOW)
}

struct Report {
    programmes: u64,
    live: usize,
    peak: usize,
}

/// The import path as the app runs it today: everything in memory, with
/// descriptions.
fn in_memory_import(shape: Shape) -> Report {
    let ((sched, programmes), live, peak) = measure(|| {
        let (guide, programmes) = epg_synth::import(shape, StoreOptions::default());
        (schedule(shape, guide), programmes)
    });
    eprintln!(
        "epg memory {}ch: in-memory import of {programmes} programmes: live {:.1} MiB, peak {:.1} MiB (store estimate {:.1} MiB)",
        shape.channels,
        mib(live),
        mib(peak),
        mib(sched.store().heap_bytes()),
    );
    Report {
        programmes,
        live,
        peak,
    }
}

/// SQLite's own heap (page cache, statements), not counted above.
fn sqlite_heap() -> usize {
    // SAFETY: SQLite memory counters take no pointers and are safe to query here.
    usize::try_from(unsafe { rusqlite::ffi::sqlite3_memory_used() }).unwrap_or(0)
}

/// Days of finished programmes the guide keeps (`epg.keep_past` default).
const KEEP_PAST_DAYS: u32 = 2;

/// The stored path: import, write to a file-backed store, drop the import,
/// then load the guide from `now - keep_past` on without descriptions.
fn stored_load(shape: Shape) -> (Report, Report) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("guide.sqlite")).unwrap();
    let source = store
        .add_source(&iptv_providers::model::Source {
            name: "Synthetic".into(),
            config: iptv_providers::model::SourceConfig::M3u(
                iptv_providers::model::M3uSourceConfig {
                    location: SecretUrl::new("http://lists.example.com/synthetic.m3u"),
                    user_agent: None,
                    referrer: None,
                },
            ),
        })
        .unwrap();
    let (guide, programmes) = epg_synth::import(shape, StoreOptions::default());
    let feeds = [EpgFeedMeta {
        config: EpgSourceConfig::new(
            iptv_epg::FeedId(1),
            SecretUrl::new("http://epg.example.com/guide.xml.gz"),
        ),
        fetched_at: NOW,
        etag: None,
        last_modified: None,
        bytes: 0,
    }];
    let started = Instant::now();
    let counts = staged::replace_epg(&mut store, source, &guide, &feeds, 0).unwrap();
    let replaced_in = started.elapsed();
    assert_eq!(counts.programmes, programmes);
    drop(guide);
    let size = std::fs::metadata(dir.path().join("guide.sqlite")).map_or(0, |m| m.len());

    let mut out = Vec::new();
    for (label, window) in [
        (
            "window (keep_past 2 d)",
            EpgWindow::from(NOW, KEEP_PAST_DAYS),
        ),
        ("everything", EpgWindow::from(NOW, 365)),
    ] {
        let started = Instant::now();
        let ((sched, loaded), live, peak) = measure(|| {
            let guide = store.load_epg(source, &window).unwrap();
            let loaded = guide
                .sources()
                .iter()
                .map(|s| s.programme_count())
                .sum::<usize>();
            (schedule(shape, guide), loaded)
        });
        eprintln!(
            "epg memory {}ch: stored load, {label}, no descriptions: {loaded} programmes in {:.0} ms: live {:.1} MiB, peak {:.1} MiB (store estimate {:.1} MiB; SQLite heap {:.1} MiB)",
            shape.channels,
            started.elapsed().as_secs_f64() * 1e3,
            mib(live),
            mib(peak),
            mib(sched.store().heap_bytes()),
            mib(sqlite_heap()),
        );
        out.push(Report {
            programmes: loaded as u64,
            live,
            peak,
        });
        drop(sched);
    }
    eprintln!(
        "epg memory {}ch: replace {programmes} programmes in {:.2} s, file {:.1} MiB",
        shape.channels,
        replaced_in.as_secs_f64(),
        size as f64 / f64::from(1 << 20),
    );
    let all = out.pop().unwrap();
    (out.pop().unwrap(), all)
}

#[test]
fn thousand_channel_guide() {
    crate::isolated("epg_memory::thousand_channel_guide", || {
        let r = in_memory_import(Shape::THOUSAND);
        assert!(r.programmes > 80_000, "{}", r.programmes);
        assert!(r.peak >= r.live);
        let (window, all) = stored_load(Shape::THOUSAND);
        assert_eq!(all.programmes, r.programmes);
        // Budgets (measured: ~2-3 MiB live): the stored guide without
        // descriptions stays far below the in-memory import.
        assert!(window.live < 5 << 20, "{}", window.live);
        assert!(all.live < r.live / 2, "{} vs {}", all.live, r.live);
        // Settled rows go straight into their slots: reading back holds no
        // raw copy of the guide on the way (it used to peak ~2.5x).
        for w in [&window, &all] {
            assert!(
                w.peak < w.live + w.live / 4 + (64 << 10),
                "{} vs {}",
                w.peak,
                w.live
            );
        }
    });
}

#[test]
#[ignore = "large: run with --release --ignored"]
fn default_fixture_guide() {
    crate::isolated("epg_memory::default_fixture_guide", || {
        let r = in_memory_import(Shape::DEFAULT_FIXTURE);
        assert!(r.programmes > 1_000_000, "{}", r.programmes);
        let (window, _) = stored_load(Shape::DEFAULT_FIXTURE);
        assert!(window.live < 40 << 20, "{}", window.live);
    });
}
