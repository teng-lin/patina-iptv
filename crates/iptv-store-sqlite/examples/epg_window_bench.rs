//! Guide window read-back benchmark: what the app keeps in memory after a
//! guide is stored (`Store::load_epg`, as `iptv-app` `data/stored.rs` and
//! `data/pages.rs` call it), on a store an `epg_import_bench` run filled.
//! One read per process, so RSS is not muddied by an earlier one. Prints
//! the time, the guide's own heap estimate, and the process's RSS before,
//! at the peak and after (Linux `/proc`). Synthetic data only.
//!
//! ```sh
//! cargo run --release -p iptv-store-sqlite --example epg_window_bench -- <store dir> window 7
//! cargo run --release -p iptv-store-sqlite --example epg_window_bench -- <store dir> window 1
//! cargo run --release -p iptv-store-sqlite --example epg_window_bench -- <store dir> day 0
//! ```
//!
//! `window N`: every channel from N days before now to the end (the app
//! at start); `day K`: the K-th older day before the one-day window (a
//! page the guide reads in when browsing back).

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use iptv_epg::DAY;
use iptv_store_sqlite::{EpgWindow, Store};

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, mode, n] = args.as_slice() else {
        eprintln!("usage: epg_window_bench <store dir> window|day <days|index>");
        std::process::exit(2);
    };
    let n: i64 = n.parse().expect("a number");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let store = Store::open(PathBuf::from(dir).join("iptv.sqlite")).expect("store");
    let source = store.sources().expect("sources")[0].id;
    let window = match mode.as_str() {
        "window" => EpgWindow::from(now, n as u32),
        "day" => {
            // The pages of `iptv_guide`: whole days back from the hour now
            // falls in, less the one-day window.
            let loaded_from = now - now.rem_euclid(3600) - DAY;
            EpgWindow {
                from: loaded_from - (n + 1) * DAY,
                to: loaded_from - n * DAY,
                ..EpgWindow::from(now, 0)
            }
        }
        _ => panic!("window or day"),
    };
    let base = status_kib("VmRSS:");
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    let t = Instant::now();
    let guide = store.load_epg(source, &window).expect("load");
    let took = t.elapsed();
    let peak = status_kib("VmHWM:");
    let after = status_kib("VmRSS:");
    let programs: usize = guide.sources().iter().map(|s| s.program_count()).sum();
    println!(
        "{mode} {n}: {} channels, {programs} programs in {:.2}s; guide heap {:.1} MiB; \
         rss before {:.0} MiB, peak +{:.0} MiB, after +{:.0} MiB",
        guide.bound_count(),
        took.as_secs_f64(),
        guide.heap_bytes() as f64 / f64::from(1 << 20),
        mib(base),
        mib(peak.saturating_sub(base)),
        mib(after.saturating_sub(base)),
    );
}
