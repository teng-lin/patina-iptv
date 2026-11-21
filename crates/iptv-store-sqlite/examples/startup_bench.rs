//! Cold-start store reads benchmark: what `iptv-app` reads from the store
//! before its guide is on screen (`data.rs`, `data/stored.rs`), each step
//! timed, on a store an `epg_import_bench` run filled. One pass per
//! process (run it a few times: the first one also reads the file into
//! the page cache). Synthetic data only.
//!
//! ```sh
//! cargo run --release -p iptv-store-sqlite --example startup_bench -- <store dir>
//! ```
//!
//! Steps: open the store (each `open_store` of the app), the active source,
//! its channels in provider order, the stored guide's state, the first
//! screen's programmes and the guide window the app holds (an hour-aligned
//! day back to the end), as the app reads them.

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use iptv_epg::DAY;
use iptv_providers::model::ContentKind;
use iptv_store_sqlite::{ChannelOrder, ChannelQuery, EpgWindow, Store};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir] = args.as_slice() else {
        eprintln!("usage: startup_bench <store dir>");
        std::process::exit(2);
    };
    let path = PathBuf::from(dir).join("iptv.sqlite");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let store = Store::open(&path).expect("store");
    println!("open: {:.1} ms", ms(t));
    let t = Instant::now();
    let _again = Store::open(&path).expect("store");
    println!("open again: {:.1} ms", ms(t));

    let t = Instant::now();
    let source = match store.active_source().expect("active") {
        Some(id) => id,
        None => {
            store
                .sources()
                .expect("sources")
                .last()
                .expect("a source")
                .id
        }
    };
    let _ = store.source(source).expect("source");
    println!("source: {:.1} ms", ms(t));

    let t = Instant::now();
    let groups = store
        .groups(source, Some(ContentKind::Live))
        .expect("groups");
    let query = ChannelQuery {
        include_hidden: true,
        order: ChannelOrder::Provider,
        ..ChannelQuery::default()
    };
    let channels = store.load_channels(source, &query).expect("channels");
    println!(
        "catalog: {} groups, {} channels in {:.1} ms",
        groups.len(),
        channels.len(),
        ms(t)
    );

    let t = Instant::now();
    let _ = store.epg_guide(source).expect("guide state");
    println!("guide state: {:.1} ms", ms(t));

    // The first screen (`iptv-app` `data/opening.rs`): the hour before
    // now's to six hours after it.
    let t = Instant::now();
    let hour = now - now.rem_euclid(3600);
    let first = EpgWindow {
        from: hour - 3600,
        to: hour + 6 * 3600,
        ..EpgWindow::from(now, 0)
    };
    let guide = store.load_epg(source, &first).expect("first screen");
    let programs: usize = guide.sources().iter().map(|s| s.program_count()).sum();
    println!(
        "first screen: {} channels, {programs} programs, {} KiB in {:.1} ms",
        guide.bound_count(),
        guide.heap_bytes() / 1024,
        ms(t)
    );

    let t = Instant::now();
    let window = EpgWindow {
        from: now - now.rem_euclid(3600) - DAY,
        ..EpgWindow::from(now, 0)
    };
    let guide = store.load_epg(source, &window).expect("window");
    let programs: usize = guide.sources().iter().map(|s| s.program_count()).sum();
    println!(
        "guide window: {} channels, {programs} programs, {} KiB in {:.1} ms",
        guide.bound_count(),
        guide.heap_bytes() / 1024,
        ms(t)
    );

    let t = Instant::now();
    let first = store.epg_first_start(source).expect("first start");
    println!("first start: {first:?} in {:.1} ms", ms(t));
}
