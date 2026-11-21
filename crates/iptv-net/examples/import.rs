//! Runs a real import (source + its EPG) and prints counts. Development aid
//! for the synthetic mock servers; prints no URLs or credentials.
//!
//! ```sh
//! python3 -I tools/make-iptv-fixtures.py --host 127.0.0.1:8089 && tools/mock-server.py 8089 &
//! cargo run -p iptv-net --example import -- m3u http://127.0.0.1:8089/iptv/playlist.m3u
//! python3 -I tools/mock-xtream.py --public 127.0.0.1:8090 &
//! IPTV_PASSWORD=demo cargo run -p iptv-net --example import -- xtream http://127.0.0.1:8090 demo --vod
//! ```
//!
//! The Xtream/Stalker secret comes from `IPTV_PASSWORD` / the MAC from
//! `IPTV_MAC`, never from the command line.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use iptv_epg::Overrides;
use iptv_import::{CancelToken, ImportOptions, MemoryGuide, Progress, import_epg, import_source};
use iptv_net::NetClient;
use iptv_providers::model::{
    M3uSourceConfig, MacAddress, SourceConfig, StalkerCredentials, XtreamCredentials,
};
use iptv_providers::{Secret, SecretUrl};

fn usage() -> ! {
    eprintln!(
        "usage: import m3u <url|path> | xtream <server> <user> [--vod] | stalker <portal> [--vod]"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vod = args.iter().any(|a| a == "--vod");
    let pos: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let secret = || Secret::new(std::env::var("IPTV_PASSWORD").unwrap_or_else(|_| "demo".into()));
    let config = match pos.as_slice() {
        ["m3u", location] => SourceConfig::M3u(M3uSourceConfig {
            location: SecretUrl::new(*location),
            user_agent: None,
            referrer: None,
        }),
        ["xtream", server, user] => SourceConfig::Xtream(XtreamCredentials {
            server_url: (*server).into(),
            username: Secret::new(*user),
            password: secret(),
            user_agent: None,
        }),
        ["stalker", portal] => SourceConfig::Stalker(StalkerCredentials {
            portal_url: (*portal).into(),
            mac: MacAddress::parse(&std::env::var("IPTV_MAC").unwrap_or_default())
                .unwrap_or_else(|| usage()),
            serial: None,
            device_id: None,
            device_id2: None,
            signature: None,
            signature2: None,
            login: None,
            password: None,
        }),
        _ => usage(),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let net = NetClient::with_defaults().expect("client");
    let cancel = CancelToken::new();
    let mut last = None;
    let mut progress = |p: &Progress| {
        if last != Some(p.phase) {
            eprintln!("  {:?}", p.phase);
            last = Some(p.phase);
        }
    };

    let t = Instant::now();
    let opts = ImportOptions::new(now).with_vod(vod, vod);
    let catalog = match import_source(&net, &config, &opts, &mut progress, &cancel).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("import failed ({:?}): {e}", e.kind());
            std::process::exit(1);
        }
    };
    println!(
        "catalog: {} channels, {} groups, {} vod, {} series, {} warnings, {} requests, {} KiB in {:.2?}",
        catalog.channels.len(),
        catalog.groups.len(),
        catalog.vod.len(),
        catalog.series.len(),
        catalog.warning_count(),
        catalog.stats.requests,
        catalog.stats.bytes / 1024,
        t.elapsed()
    );

    let sources = catalog
        .epg_sources(1)
        .expect("bounded header guide identities fit from 1");
    if sources.is_empty() {
        println!("no EPG source declared");
        return;
    }
    let t = Instant::now();
    let mut progress = |_: &Progress| {};
    // In memory: a development aid (the app stages the guide on disk).
    let mut sink = MemoryGuide::new();
    match import_epg(
        &net,
        &sources,
        &catalog.channels,
        &Overrides::new(),
        now,
        &mut sink,
        &mut progress,
        &cancel,
    )
    .await
    {
        Ok(epg) => {
            let guide = sink.into_guide(&epg);
            for s in &epg.sources {
                match &s.result {
                    Ok(st) => println!(
                        "epg source {}: {} channels, {} programs kept, {} KiB",
                        s.id.0,
                        st.parse.channels,
                        st.ingest.kept,
                        st.bytes / 1024
                    ),
                    Err(e) => println!("epg source {} failed ({:?}): {e}", s.id.0, e.kind()),
                }
            }
            println!(
                "epg: matched {} of {} channels ({} by tvg-id, {} by name), {} MiB heap, in {:.2?}",
                epg.matches.matched(),
                epg.matches.channels,
                epg.matches.by_tvg_id,
                epg.matches.by_name,
                guide.heap_bytes() / (1024 * 1024),
                t.elapsed()
            );
        }
        Err(e) => println!("epg failed ({:?}): {e}", e.kind()),
    }
}
