//! Foreground refresh integration with synthetic local files; no network/provider data.
use crate::headless::Headless;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::{SourceId, SourcePolicy, Store};
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[track_caller]
fn step(result: Result<(), String>) {
    if let Err(error) = result {
        panic!("{error}")
    }
}
fn playlist(label: &str, count: usize) -> String {
    let mut out = String::from("#EXTM3U\n");
    for i in 0..count {
        out.push_str(&format!(
            "#EXTINF:-1 group-title=\"News\",{label} {i}\nhttp://stream.example.test/{label}/{i}\n"
        ));
    }
    out
}
fn add(store: &mut Store, dir: &Path, label: &str, policy: SourcePolicy) -> (SourceId, PathBuf) {
    let file = dir.join(format!("{label}.m3u"));
    let text = playlist(label, 1);
    std::fs::write(&file, &text).unwrap();
    let id = store
        .add_source(&Source {
            name: format!("Synthetic {label}"),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(file.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let m = iptv_providers::m3u::parse_m3u(&text);
    store
        .replace_catalog(id, &m.groups, &m.channels.items, &[], &[])
        .unwrap();
    store.set_source_policy(id, policy).unwrap();
    (id, file)
}
fn until(app: &mut Headless, what: &str, timeout: Duration, mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + crate::isolated::scaled_duration(timeout);
    while !ready() {
        assert!(Instant::now() < end, "timed out: {what}");
        step(app.wait(Duration::from_millis(50)));
    }
}
#[test]
fn startup_and_periodic_refresh_skip_disabled_sources_and_failures_keep_the_cache() {
    let Some(home) = isolated!() else { return };
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let (a, file_a) = add(
        &mut store,
        home.path(),
        "A",
        SourcePolicy {
            refresh_on_start: false,
            ..Default::default()
        },
    );
    let (b, file_b) = add(&mut store, home.path(), "B", SourcePolicy::default());
    let (c, file_c) = add(
        &mut store,
        home.path(),
        "C",
        SourcePolicy {
            enabled: false,
            refresh_seconds: 1,
            ..Default::default()
        },
    );
    store.set_active_source(a).unwrap();
    drop(store);
    std::fs::write(&file_a, playlist("A", 2)).unwrap();
    std::fs::write(&file_b, playlist("B", 2)).unwrap();
    std::fs::write(&file_c, playlist("C", 2)).unwrap();
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:A 0 · Synthetic A", Duration::from_secs(20)));
    step(app.press("text:☰  All playlists"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Playlists >> nth=0"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:Synthetic A"));
    step(app.press("text:Synthetic A"));
    step(app.wait_for("text:Show this playlist"));
    step(app.press("text:Show this playlist"));
    step(app.wait_for_within("text:A 0", Duration::from_secs(20)));
    until(
        &mut app,
        "background startup refresh",
        Duration::from_secs(10),
        || {
            crate::isolated::open_database(&path)
                .unwrap()
                .import_status(b)
                .unwrap()
                .counts
                .channels
                == 2
        },
    );
    let mut store = crate::isolated::open_database(&path).unwrap();
    assert_eq!(
        store.import_status(a).unwrap().counts.channels,
        1,
        "startup refresh is off; open the cached source"
    );
    assert_eq!(
        store.import_status(c).unwrap().counts.channels,
        1,
        "disabled source is not refreshed on startup"
    );
    std::fs::write(&file_a, playlist("A", 3)).unwrap();
    store
        .set_source_policy(
            a,
            SourcePolicy {
                refresh_seconds: 1,
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    // Exercise the production 60-second foreground timer, not a copied scheduling formula.
    until(
        &mut app,
        "periodic refresh",
        Duration::from_secs(75),
        || {
            crate::isolated::open_database(&path)
                .unwrap()
                .import_status(a)
                .unwrap()
                .counts
                .channels
                == 3
        },
    );
    step(app.wait_for("text:A 2")); // Store commit precedes application of the refreshed UI.
    let mut store = crate::isolated::open_database(&path).unwrap();
    assert_eq!(store.import_status(c).unwrap().counts.channels, 1);
    store
        .set_source_policy(
            a,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    // A failed manual update keeps the successful catalogue on screen and on disk.
    std::fs::write(&file_a, "#EXTM3U\n").unwrap();
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Playlists >> nth=0"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:Synthetic A"));
    step(app.press("text:Synthetic A"));
    step(app.wait_for("text:Refresh now"));
    step(app.press("text:Refresh now"));
    until(
        &mut app,
        "failed import status",
        Duration::from_secs(10),
        || {
            crate::isolated::open_database(&path)
                .unwrap()
                .import_status(a)
                .unwrap()
                .last_error
                .is_some()
        },
    );
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .import_status(a)
            .unwrap()
            .counts
            .channels,
        3
    );
    // Manual refresh of a disabled source remains possible without enabling/showing it.
    step(app.press("text:Synthetic C"));
    step(app.wait_for("text:Refresh now"));
    step(app.press("text:Refresh now"));
    until(
        &mut app,
        "explicit refresh of disabled source",
        Duration::from_secs(10),
        || {
            crate::isolated::open_database(&path)
                .unwrap()
                .import_status(c)
                .unwrap()
                .counts
                .channels
                == 2
        },
    );
    assert!(
        !crate::isolated::open_database(&path)
            .unwrap()
            .source_policy(c)
            .unwrap()
            .enabled
    );
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .active_source()
            .unwrap(),
        Some(a)
    );
    step(app.press("text:Synthetic A"));
    step(app.wait_for("text:Playlist enabled: Yes"));
    step(app.press("text:Playlist enabled: Yes"));
    until(
        &mut app,
        "fallback to an enabled source",
        Duration::from_secs(10),
        || {
            crate::isolated::open_database(&path)
                .unwrap()
                .active_source()
                .unwrap()
                == Some(b)
        },
    );
    step(app.press("text:Close"));
    step(app.wait_for("text:B 0"));
}
