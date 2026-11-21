//! Guide import benchmark: what the app does when a playlist's guide is
//! downloaded (`iptv-app` `data/stored.rs`), headless: the playlist is
//! imported and stored, then the guide is imported, stored and its window
//! read back. Prints the time of each step and the process's peak RSS
//! during the guide import (Linux `/proc`). Synthetic data only.
//!
//! ```sh
//! python3 -I tools/make-iptv-fixtures.py --channels 10000 --groups 40 \
//!   --days-back 7 --days-ahead 2 --host 127.0.0.1:8123 --out fixtures/big
//! sed -i 's#/iptv/guide.xml.gz#/big/guide.xml.gz#' fixtures/big/playlist.m3u
//! (cd fixtures && python3 -I -m http.server 8123 --bind 127.0.0.1) &
//! cargo run --release -p iptv-store-sqlite --example epg_import_bench -- \
//!   http://127.0.0.1:8123/big/playlist.m3u /tmp/bench-dir
//! ```

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use iptv_epg::{DAY, Overrides, StoreOptions};
use iptv_import::{CancelToken, ImportOptions, NoProgress, import_epg, import_source};
use iptv_net::NetClient;
use iptv_providers::SecretUrl;
use iptv_providers::model::{M3uSourceConfig, Source, SourceConfig};
use iptv_store_sqlite::{EpgFeedMeta, EpgStage, EpgWindow, Store};

/// A `/proc/self/status` field in KiB.
fn status_kib(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

fn mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

/// Resets the peak RSS (`VmHWM`) to the current RSS.
fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, dir] = args.as_slice() else {
        eprintln!("usage: epg_import_bench <playlist url> <empty dir for the store>");
        std::process::exit(2);
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("store dir");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let net = NetClient::with_defaults().expect("client");
    let cancel = CancelToken::new();
    let config = SourceConfig::M3u(M3uSourceConfig {
        location: SecretUrl::new(url.as_str()),
        user_agent: None,
        referrer: None,
    });
    let t = Instant::now();
    let catalog = import_source(
        &net,
        &config,
        &ImportOptions::new(now),
        &mut NoProgress,
        &cancel,
    )
    .await
    .expect("playlist");
    let mut store = Store::open(dir.join("iptv.sqlite")).expect("store");
    let source = store
        .add_source(&Source {
            name: "Bench".into(),
            config,
        })
        .expect("source");
    store
        .replace_catalog(
            source,
            &catalog.groups,
            &catalog.channels,
            &catalog.vod,
            &catalog.series,
        )
        .expect("catalog");
    println!(
        "playlist: {} channels stored in {:.2?}",
        catalog.channels.len(),
        t.elapsed()
    );

    // The app's defaults: a week of finished programmes, descriptions on.
    let opts = StoreOptions {
        store_descriptions: std::env::var_os("BENCH_NO_DESC").is_none(),
        past_days_to_keep: Some(7),
    };
    let mut configs = catalog
        .epg_sources(1)
        .expect("bounded header guide identities fit from 1");
    for c in &mut configs {
        c.store = opts;
    }
    let overrides = Overrides::new();
    let fingerprint = iptv_import::matching_fingerprint(&catalog.channels, &overrides);
    let base = status_kib("VmRSS:");
    reset_peak();
    let t = Instant::now();
    // As the app: the download is staged next to the store.
    let mut stage = EpgStage::create_in(&dir).expect("stage");
    let epg = import_epg(
        &net,
        &configs,
        &catalog.channels,
        &overrides,
        now,
        &mut stage,
        &mut NoProgress,
        &cancel,
    )
    .await
    .expect("guide");
    let downloaded = t.elapsed();
    let staged = std::fs::metadata(stage.path()).map_or(0, |m| m.len());
    let feeds = EpgFeedMeta::from_import(&epg, &configs, now);
    let counts = store
        .replace_epg(source, &mut stage, &epg, &feeds, fingerprint)
        .expect("replace");
    let replaced = t.elapsed();
    drop(stage);
    let results: Vec<_> = epg.sources.iter().map(|o| (o.id, None)).collect();
    store
        .record_epg_feed_results(source, &results)
        .expect("results");
    let pruned = store.prune_epg(now - 7 * DAY).expect("prune");
    drop(epg);
    let stored = t.elapsed();
    let import_peak = status_kib("VmHWM:");
    let guide = store
        .load_epg(source, &EpgWindow::from(now, 7))
        .expect("load");
    let total = t.elapsed();
    let peak = status_kib("VmHWM:");
    println!(
        "guide: {} channels, {} programs, {} descriptions; stage file {:.0} MiB",
        counts.bindings,
        counts.programs,
        counts.descriptions,
        staged as f64 / f64::from(1 << 20)
    );
    println!(
        "time: download+parse {:.2}s, replaced at {:.2}s, stored at {:.2}s ({pruned} pruned), window loaded at {:.2}s",
        downloaded.as_secs_f64(),
        replaced.as_secs_f64(),
        stored.as_secs_f64(),
        total.as_secs_f64()
    );
    println!(
        "rss: before {:.0} MiB, peak {:.0} MiB (+{:.0} MiB; +{:.0} MiB until stored), window held {:.0} MiB, loaded guide bound {}",
        mib(base),
        mib(peak),
        mib(peak.saturating_sub(base)),
        mib(import_peak.saturating_sub(base)),
        guide.heap_bytes() as f64 / f64::from(1 << 20),
        guide.bound_count()
    );
}
