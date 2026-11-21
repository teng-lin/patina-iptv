//! Memory of the streaming guide import does not grow with the guide: the
//! Rust heap (every thread, counted by a global allocator) and SQLite's
//! own heap (`sqlite3_memory_highwater`) peak at the same level for a
//! guide eight times longer. The programs go to the stage in batches of
//! bounded size; only the channel lists stay in memory.
//!
//! Each case runs in a fresh process: the counters are process-wide.

use crate::common;
use crate::stream_support;

use crate::allocations::{LIVE, PEAK};
use std::sync::atomic::Ordering;

use iptv_epg::{FeedId, Overrides, StoreOptions};
use iptv_import::{CancelToken, EpgSourceConfig, NoProgress, import_epg};
use iptv_providers::SecretUrl;
use iptv_store_sqlite::{EpgFeedMeta, EpgStage, Store};
use stream_support::{Feeds, GenShape, Serve, block_on};

const NOW: i64 = 1_800_000_000;
const CHANNELS: usize = 500;

/// Peak Rust heap above the start, SQLite heap high-water while staging
/// and in all (bytes), programs stored.
fn import(days: i64) -> (usize, usize, usize, u64) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("iptv.sqlite")).unwrap();
    let source = store.add_source(&common::m3u_source("Synthetic")).unwrap();
    let chans = stream_support::generated_playlist(CHANNELS);
    let shape = GenShape {
        channels: CHANNELS,
        days,
        from: NOW - days * 12 * 3600,
    };
    let fetch = Feeds::new().serve("g.xml", Serve::Generated(shape));
    let mut config = EpgSourceConfig::new(
        FeedId(1),
        SecretUrl::new(format!("{}g.xml", stream_support::BASE)),
    );
    config.store = StoreOptions::default();
    let configs = [config];

    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    // SAFETY: plain counter reads of the bundled SQLite.
    // SAFETY: SQLite memory counters take no pointers and are safe to query here.
    unsafe { rusqlite::ffi::sqlite3_memory_highwater(1) };
    let mut stage = EpgStage::create_in(dir.path()).unwrap();
    let epg = block_on(import_epg(
        &fetch,
        &configs,
        &chans,
        &Overrides::new(),
        NOW,
        &mut stage,
        &mut NoProgress,
        &CancelToken::new(),
    ))
    .unwrap();
    // SAFETY: SQLite memory counters take no pointers and are safe to query here.
    let staging = unsafe { rusqlite::ffi::sqlite3_memory_highwater(0) } as usize;
    let feeds = EpgFeedMeta::from_import(&epg, &configs, NOW);
    let counts = store
        .replace_epg(source, &mut stage, &epg, &feeds, 0)
        .unwrap();
    drop(stage);
    let peak = (PEAK.load(Ordering::Relaxed) - base).max(0) as usize;
    // SAFETY: SQLite memory counters take no pointers and are safe to query here.
    let sqlite = unsafe { rusqlite::ffi::sqlite3_memory_highwater(0) } as usize;
    (peak, staging, sqlite, counts.programs)
}

#[test]
fn import_memory_does_not_grow_with_the_guide() {
    crate::isolated(
        "epg_stream_memory::import_memory_does_not_grow_with_the_guide",
        || {
            let mib = |b: usize| b as f64 / f64::from(1 << 20);
            let (short_heap, short_staging, short_sqlite, short_n) = import(1);
            let (long_heap, long_staging, long_sqlite, long_n) = import(8);
            eprintln!(
                "{short_n} programs: heap {:.1} MiB, SQLite {:.1} MiB staging, {:.1} MiB in all; \
         {long_n} programs: heap {:.1} MiB, SQLite {:.1} MiB staging, {:.1} MiB in all",
                mib(short_heap),
                mib(short_staging),
                mib(short_sqlite),
                mib(long_heap),
                mib(long_staging),
                mib(long_sqlite)
            );
            assert_eq!(long_n, 8 * short_n);
            assert_eq!(short_n, CHANNELS as u64 * 24);
            // Batches, parser buffers and the channel lists: a few MiB at most,
            // whatever the guide's length.
            assert!(long_heap < 12 << 20, "{long_heap}");
            assert!(
                long_heap < short_heap + (1 << 20),
                "{short_heap} -> {long_heap}"
            );
            // Page caches bound SQLite: 4 MiB for the stage while downloading,
            // 32 MiB for the store while copying.
            assert!(long_staging < 8 << 20, "{long_staging}");
            assert!(long_sqlite < 48 << 20, "{long_sqlite}");
        },
    );
}
