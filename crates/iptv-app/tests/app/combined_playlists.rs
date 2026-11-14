//! Combined browsing keeps identical provider keys distinct and routes writes
//! and playback to their owning source. Only generated, local playlists.
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::headless::Headless;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::{ChannelQuery, GuideSource, SourceId, SourcePolicy, Store};
use nsh_core::{MountItem, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

#[track_caller]
fn step(result: Result<(), String>) {
    result.unwrap_or_else(|error| panic!("{error}"));
}

fn seed(store: &mut Store, home: &Path, name: &str) -> SourceId {
    let playlist = format!(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"shared\" group-title=\"News\",Shared\nhttp://stream.example.test/shared\n#EXTINF:-1 group-title=\"News\",Only {name}\nhttp://stream.example.test/{name}\n"
    );
    let file = home.join(format!("{name}.m3u"));
    std::fs::write(&file, &playlist).unwrap();
    let source = store
        .add_source(&Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(file.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(&playlist);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    // The same feed/channel IDs deliberately occur in both source namespaces.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide = home.join(format!("{name}.xml"));
    std::fs::write(
        &guide,
        format!(
            "<tv><channel id=\"shared\"><display-name>Shared</display-name></channel>\
             <programme channel=\"shared\" start=\"{}\" stop=\"{}\">\
             <title>{name} bulletin</title>\
             <desc>{name} synthetic programme details.</desc></programme></tv>",
            iptv_epg::format_xmltv_time(now - 1800),
            iptv_epg::format_xmltv_time(now + 1800)
        ),
    )
    .unwrap();
    store
        .set_guide_sources(
            source,
            &[GuideSource {
                feed: iptv_epg::FeedId(1001),
                provider_index: None,
                url: SecretUrl::new(guide.to_string_lossy()),
                name: Some(format!("{name} guide")),
                enabled: true,
                shift_minutes: 0,
            }],
        )
        .unwrap();
    source
}

fn playlists(app: &mut Headless) {
    step(app.wait_for("text:/^☰  /"));
    step(app.press("text:/^☰  /"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Playlists >> nth=0"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:Tap a playlist for what you can do with it"));
}

fn favorite_names(path: &Path, source: SourceId) -> Vec<String> {
    crate::isolated::open_database(path)
        .unwrap()
        .load_channels(
            source,
            &ChannelQuery {
                favourites_only: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|row| row.name)
        .collect()
}

#[test]
fn multiview_picker_expansion_preserves_the_guides_category_and_saved_expansion() {
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    seed(&mut store, home.path(), "Alpha");
    let beta = seed(&mut store, home.path(), "Beta");
    store.set_collapsed_playlists(&[beta.0]).unwrap();
    drop(store);
    let mut settings = Settings::new();
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    settings.set("guide.switch_delay", Value::secs(0)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        clock: nsh_core::time::Clock::manual(SystemTime::now()),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:/^Only Beta · Beta/", Duration::from_secs(20)));
    step(app.press("text:☰  All playlists"));
    step(app.wait_for("id:guide-groups >> text:▾  Alpha"));
    step(app.wait_for("id:guide-groups >> text:▸  Beta"));
    step(app.press("id:guide-groups >> text:    News"));
    step(app.press_key("right"));
    step(app.wait_for("text:☰  News"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    assert!(!app.screen().matches("text:/^Only Beta · Beta/").unwrap());

    step(app.press("text:/^Only Alpha · Alpha/"));
    step(app.wait_for("text:▶"));
    step(app.press("text:/^Only Alpha · Alpha/"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    let picker = "id:multiview-picker";
    step(app.wait_for(picker));
    // The picker's expansion is temporary. In particular, folding Alpha must
    // not remove the guide's selected News category from its own catalogue.
    step(app.press(&format!("{picker} >> text:▾  Alpha")));
    step(app.wait_for(&format!("{picker} >> text:▸  Alpha")));
    step(app.press(&format!("{picker} >> text:▸  Alpha")));
    step(app.wait_for(&format!("{picker} >> text:▾  Alpha")));
    step(app.press(&format!("{picker} >> text:▾  Alpha")));
    step(app.wait_for(&format!("{picker} >> text:▸  Alpha")));
    step(app.press(&format!("{picker} >> text:▸  Beta")));
    step(app.wait_for(&format!("{picker} >> text:▾  Beta")));
    // Give any erroneously queued preference writes or catalogue rebuilds a
    // chance to finish before checking that the stored guide choice is intact.
    step(app.run_for(Duration::from_secs(1)));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .collapsed_playlists()
            .unwrap(),
        [beta.0],
        "picker expansion must not overwrite the guide's saved expansion"
    );
    step(app.press(&format!("{picker} >> text:Close")));
    step(app.wait_for_gone(picker, Duration::from_secs(3)));
    assert_eq!(app.screen().count("id:multiview-tile").unwrap(), 1);
    step(app.press_key("backspace"));
    step(app.wait_for_gone("id:multiview-tile", Duration::from_secs(3)));
    step(app.press_key("f"));
    step(app.wait_for("text:☰  News"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    step(app.wait_for("text:/^Only Alpha · Alpha/"));
    assert!(!app.screen().matches("text:/^Only Beta · Beta/").unwrap());
    step(app.press("text:☰  News"));
    step(app.wait_for("id:guide-groups >> text:▾  Alpha"));
    step(app.wait_for("id:guide-groups >> text:▸  Beta"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        1,
        "the guide keeps its own expanded playlist and selected category"
    );
}

#[test]
fn combined_browsing_routes_duplicate_keys_favorites_search_and_playback() {
    let Some(home) = isolated!() else { return };
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let mut settings = Settings::new();
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    settings.set("guide.switch_delay", Value::secs(0)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let alpha = seed(&mut store, home.path(), "Alpha");
    let beta = seed(&mut store, home.path(), "Beta");
    store.set_active_source(alpha).unwrap();
    drop(store);

    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    // Multiple enabled sources are the default, without a visit to management.
    step(app.wait_for_within("text:/^Shared · Alpha/", Duration::from_secs(20)));
    step(app.wait_for("text:/^Shared · Beta/"));
    step(app.wait_for_within("text:Alpha bulletin", Duration::from_secs(20)));
    step(app.wait_for_within("text:Beta bulletin", Duration::from_secs(20)));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 4);

    // These shared rows have the same original provider key. Editing Beta
    // must not change Alpha's membership.
    step(app.press("text:/^Shared · Beta/"));
    step(app.press_key("m"));
    step(app.wait_for("text:Add to favourites"));
    step(app.press("text:Add to favourites"));
    step(crate::isolated::wait_outside(
        &mut app,
        "owning source favorite saved",
        Duration::from_secs(5),
        || favorite_names(&path, beta) == ["Shared"],
    ));
    assert!(favorite_names(&path, alpha).is_empty());

    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let observed = opened.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(url) = value.as_ref().and_then(|v| v["url"].as_str())
                    {
                        observed.lock().unwrap().push(url.into());
                    }
                }
            }
        }
    });
    step(app.press("text:/^Only Beta/"));
    step(
        app.wait_until("Beta stream opened", Duration::from_secs(5), |_| {
            opened
                .lock()
                .unwrap()
                .iter()
                .any(|url| url == "http://stream.example.test/Beta")
        }),
    );

    step(app.press_key("/"));
    step(app.wait_for("id:search-field"));
    step(app.type_text("id:search-field", "shared"));
    step(app.wait_for_within("text:/Shared · Alpha/", Duration::from_secs(10)));
    step(app.wait_for("text:/Shared · Beta/"));
    step(app.clear("id:search-field"));
    step(app.type_text("id:search-field", "bulletin"));
    step(app.wait_for("text:/^Programmes/"));
    step(app.press("text:/^Programmes/"));
    step(app.wait_for_within("text:/Alpha bulletin/", Duration::from_secs(10)));
    step(app.wait_for("text:/Beta bulletin/"));
    step(app.press("text:Close"));

    // Switching back to a source restores its ordinary names and does not
    // leak the other source's favorite, then All playlists remains available.
    playlists(&mut app);
    step(app.press("text:Alpha"));
    step(app.wait_for("text:Show this playlist"));
    step(app.press("text:Show this playlist"));
    step(app.wait_for_within("text:Only Alpha", Duration::from_secs(20)));
    assert!(!app.screen().matches("text:/^Only Beta/").unwrap());
    assert!(!app.screen().matches("text:/^★ Shared/").unwrap());

    playlists(&mut app);
    step(app.press("id:playlists-all"));
    step(app.wait_for_within("text:/^Only Beta/", Duration::from_secs(20)));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .all_playlists()
            .unwrap()
    );
    playlists(&mut app);
    step(app.press("text:Beta"));
    step(app.wait_for("text:Playlist enabled: Yes"));
    step(app.press("text:Playlist enabled: Yes"));
    assert!(
        !crate::isolated::open_database(&path)
            .unwrap()
            .source_policy(beta)
            .unwrap()
            .enabled
    );
    step(app.back());
    step(app.back());
    step(app.wait_for_gone("text:/^Only Beta/", Duration::from_secs(10)));
    step(app.wait_for("text:/^Only Alpha/"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    // Disabling keeps the underlying configuration and membership available
    // for re-enabling; the combined-mode choice stays persisted.
    assert_eq!(favorite_names(&path, beta), ["Shared"]);
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .all_playlists()
            .unwrap()
    );
}

#[test]
fn persisted_combined_mode_recovers_disabled_and_failed_sources_and_deletion() {
    let Some(home) = isolated!() else { return };
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = seed(&mut store, home.path(), "Alpha");
    store.set_all_playlists(true).unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    // The stored catalog remains usable even when a later refresh cannot
    // read the generated file. No network server or real source is needed.
    std::fs::remove_file(home.path().join("Alpha.m3u")).unwrap();
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    // Zero rows also holds before the initial mount. Wait for the persisted
    // combined mode and its real empty state before sending navigation input;
    // startup can otherwise replace the placeholder underneath those clicks.
    step(app.wait_for_within("text:☰  All playlists", Duration::from_secs(15)));
    step(app.wait_for("text:This group has no channels."));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 0);
    playlists(&mut app);
    step(app.press("text:Alpha"));
    step(app.wait_for("text:Playlist enabled: No"));
    step(app.press("text:Playlist enabled: No"));
    step(app.back());
    step(app.back());
    step(app.wait_for_within("text:/^Only Alpha · Alpha/", Duration::from_secs(15)));
    step(app.wait_until(
        "failed refresh recorded with cache retained",
        Duration::from_secs(15),
        |_| {
            crate::isolated::open_database(&path)
                .unwrap()
                .import_status(source)
                .unwrap()
                .last_error
                .is_some()
        },
    ));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    playlists(&mut app);
    step(app.press("text:Alpha"));
    step(app.wait_for("text:Delete"));
    step(app.press("text:Delete"));
    step(app.wait_for("text:Delete >> nth=0"));
    step(app.press("text:Delete >> nth=0"));
    step(app.back());
    step(app.wait_until(
        "deleted source removed from aggregate",
        Duration::from_secs(10),
        |app| app.count("id:guide-channel-name").unwrap() == 0,
    ));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .sources()
            .unwrap()
            .is_empty()
    );
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .all_playlists()
            .unwrap()
    );
    playlists(&mut app);
    step(app.wait_for("text:+ Add playlist"));
}

#[test]
fn leaving_combined_mode_cancels_a_missing_startup_channel_before_manual_return() {
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let alpha = seed(&mut store, home.path(), "Alpha");
    let beta = seed(&mut store, home.path(), "Beta");
    store.set_all_playlists(true).unwrap();
    store
        .set_combined_last_channel("source:99999:missing-generated-channel")
        .unwrap();
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:/^Only Beta · Beta/", Duration::from_secs(20)));
    playlists(&mut app);
    step(app.press("text:Alpha"));
    step(app.wait_for("text:Show this playlist"));
    step(app.press("text:Show this playlist"));
    step(app.wait_for_within("text:Only Alpha", Duration::from_secs(20)));
    let mut store = crate::isolated::open_database(&path).unwrap();
    assert_eq!(store.active_source().unwrap(), Some(alpha));
    let channel = store.load_channels(beta, &ChannelQuery::default()).unwrap()[0]
        .channel
        .id
        .clone();
    store
        .set_combined_last_channel(&format!("source:{}:{}", beta.0, channel.as_str()))
        .unwrap();
    drop(store);
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let observed = opened.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(url) = value.as_ref().and_then(|v| v["url"].as_str())
                    {
                        observed.lock().unwrap().push(url.into());
                    }
                }
            }
        }
    });
    playlists(&mut app);
    step(app.press("id:playlists-all"));
    step(app.wait_for_within("text:/^Only Beta · Beta/", Duration::from_secs(20)));
    step(app.wait_for_within("text:Beta bulletin", Duration::from_secs(20)));
    assert!(
        opened.lock().unwrap().is_empty(),
        "manual All playlists entry must not resume the old startup request"
    );
}

#[test]
fn guide_browses_playlist_sections_by_touch_and_remote_and_remembers_expansion() {
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let alpha = seed(&mut store, home.path(), "Alpha");
    seed(&mut store, home.path(), "Beta");
    let disabled = seed(&mut store, home.path(), "Disabled");
    store
        .set_source_policy(
            disabled,
            SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    // No aggregate preference is set: the enabled sources are visible together.
    assert!(!store.all_playlists().unwrap());
    drop(store);
    let start = || {
        Headless::start(EngineConfig {
            viewport: Size::new(px(1920.0), px(1080.0)),
            ..EngineConfig::new(Root::of::<iptv_app::App>())
        })
        .unwrap()
    };
    let mut app = start();
    step(app.wait_for_within("text:/^Only Beta · Beta/", Duration::from_secs(20)));
    step(app.press("text:☰  All playlists"));
    step(app.wait_for("id:guide-groups >> text:▾  Alpha"));
    step(app.wait_for("id:guide-groups >> text:▾  Beta"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        2
    );
    assert_eq!(app.screen().count("text:▾  Disabled").unwrap(), 0);
    // A parent previews only that playlist, and a tap folds its children.
    step(app.press("id:guide-groups >> text:▾  Alpha"));
    step(app.wait_for("id:guide-groups >> text:▸  Alpha"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        1
    );
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    step(app.wait_for("text:/^Only Alpha · Alpha/"));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .collapsed_playlists()
            .unwrap(),
        [alpha.0]
    );
    // OK expands in place; Right enters that source's complete channel list.
    step(app.press_key("enter"));
    step(app.wait_for("id:guide-groups >> text:▾  Alpha"));
    step(app.press_key("right"));
    step(app.wait_for_gone("id:guide-groups", Duration::from_secs(3)));
    step(app.wait_for("text:☰  Alpha"));
    step(app.press("text:☰  Alpha"));
    step(app.wait_for("id:guide-groups >> text:    News >> nth=1"));
    step(app.press("id:guide-groups >> text:    News >> nth=1"));
    step(app.wait_for("text:/^Only Beta · Beta/"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    // Fold Alpha again, then move down: Beta's header is the very next row.
    step(app.press("id:guide-groups >> text:▾  Alpha"));
    step(app.press_key("down"));
    step(app.press_key("right"));
    step(app.wait_for("text:☰  Beta"));
    step(app.wait_for("text:/^Only Beta · Beta/"));
    drop(app);
    // Expansion survives a fresh application, while all channels stay available.
    let mut app = start();
    step(app.wait_for_within("text:/^Only Beta · Beta/", Duration::from_secs(20)));
    step(app.press("text:☰  All playlists"));
    step(app.wait_for("id:guide-groups >> text:▸  Alpha"));
    step(app.wait_for("id:guide-groups >> text:▾  Beta"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 4);
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        1
    );
}

#[test]
fn adding_second_playlist_opens_combined_browser_without_management_switch() {
    use nsh_core::{ImageData, picker::PickedFile};
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = crate::isolated::open_database(dir.join("iptv.sqlite")).unwrap();
    seed(&mut store, home.path(), "Alpha");
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    app.on_pick(|_| vec![PickedFile::new("Beta.m3u", "audio/x-mpegurl", ImageData::new(
        b"#EXTM3U\n#EXTINF:-1 group-title=\"Sport\",Added Beta\nhttp://stream.example.test/added-beta\n".to_vec()
    ))]);
    step(app.wait_for_within("text:Only Alpha", Duration::from_secs(20)));
    step(app.press("text:☰  All channels"));
    step(app.press_key("m"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:Beta.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:/^Only Alpha · Alpha/", Duration::from_secs(20)));
    step(app.wait_for("text:/^Added Beta · /"));
    step(app.wait_for("text:☰  All playlists"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 3);
    assert!(
        crate::isolated::open_database(dir.join("iptv.sqlite"))
            .unwrap()
            .all_playlists()
            .unwrap()
    );
}

/// A generated account permits only TS and one connection. Its first guide is
/// held until the Add result has joined the browser; another account import is
/// rejected so retaining the successful result cannot depend on a second fetch.
type DelayedAccount = (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<(Mutex<bool>, std::sync::Condvar)>,
);

fn delayed_account(hold_first_account: bool) -> DelayedAccount {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accounts = Arc::new(AtomicUsize::new(0));
    let guides = Arc::new(AtomicUsize::new(0));
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let observed_accounts = accounts.clone();
    let observed_guides = guides.clone();
    let release_guide = release.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let accounts = observed_accounts.clone();
            let guides = observed_guides.clone();
            let release = release_guide.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                let _ = reader.read_line(&mut request);
                let mut header = String::new();
                while reader.read_line(&mut header).unwrap_or(0) > 2 {
                    header.clear();
                }
                let target = request.split_whitespace().nth(1).unwrap_or("/");
                let action = target
                    .split(['?', '&'])
                    .find_map(|p| p.strip_prefix("action="))
                    .unwrap_or("");
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                let (status, body) = if target.starts_with("/xmltv.php") {
                    let count = guides.fetch_add(1, Ordering::SeqCst) + 1;
                    let (ready, signal) = &*release;
                    let _ready = signal
                        .wait_while(ready.lock().unwrap(), |ready| !*ready)
                        .unwrap();
                    (
                        "200 OK",
                        format!(
                            "<tv><channel id=\"beta1\">\
                             <display-name>Imported Beta One</display-name></channel>\
                             <programme channel=\"beta1\" start=\"{}\" stop=\"{}\">\
                             <title>Delayed Beta guide {count}</title></programme></tv>",
                            iptv_epg::format_xmltv_time(now - 3600),
                            iptv_epg::format_xmltv_time(now + 24 * 3600),
                        ),
                    )
                } else if action.is_empty() || action == "get_account_info" {
                    let attempt = accounts.fetch_add(1, Ordering::SeqCst);
                    if hold_first_account && attempt == 0 {
                        let (ready, signal) = &*release;
                        drop(
                            signal
                                .wait_while(ready.lock().unwrap(), |ready| !*ready)
                                .unwrap(),
                        );
                    }
                    if attempt > 0 && !hold_first_account {
                        (
                            "503 Service Unavailable",
                            "Generated account retry rejected".into(),
                        )
                    } else {
                        (
                            "200 OK",
                            serde_json::json!({
                                "user_info": {"username":"viewer", "password":"secret", "auth":1,
                                    "status":"Active", "exp_date":(now+30*86400).to_string(),
                                    "max_connections":"1", "allowed_output_formats":["ts"]},
                                "server_info": {"url":"127.0.0.1", "port":port.to_string(),
                                    "server_protocol":"http", "timezone":"UTC", "timestamp_now":now}
                            })
                            .to_string(),
                        )
                    }
                } else {
                    let body = match action {
                        "get_live_categories" => serde_json::json!([
                            {"category_id":"1", "category_name":"News", "parent_id":0}]),
                        "get_live_streams" => serde_json::json!([
                            {"name":"Imported Beta One", "stream_id":1, "stream_type":"live", "category_id":"1", "epg_channel_id":"beta1", "tv_archive":0},
                            {"name":"Imported Beta Two", "stream_id":2, "stream_type":"live", "category_id":"1", "epg_channel_id":"beta2", "tv_archive":0}]),
                        _ => serde_json::json!([]),
                    };
                    ("200 OK", body.to_string())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    (
        format!("http://127.0.0.1:{port}"),
        accounts,
        guides,
        release,
    )
}

#[test]
fn automatic_join_keeps_account_format_limit_and_the_original_guide_job_and_timer() {
    use std::sync::atomic::Ordering;
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    seed(&mut store, home.path(), "Alpha");
    drop(store);
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    settings.set("guide.switch_delay", Value::secs(0)).unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let (server, accounts, guides, release) = delayed_account(false);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        clock: nsh_core::time::Clock::manual(SystemTime::now()),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:Alpha bulletin", Duration::from_secs(20)));
    step(app.press("text:+ Add playlist"));
    step(app.wait_for("text:Xtream Codes login"));
    step(app.press("text:Xtream Codes login"));
    for (field, empty, value) in [
        ("Server", "http://server:port", server.as_str()),
        ("Username", "Required", "viewer"),
        ("Password", "Required", "secret"),
    ] {
        step(app.press(&format!("text:{field}")));
        step(app.type_text(&format!("text:{empty}"), value));
        step(app.submit(&format!("text:{value}")));
    }
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:☰  All playlists", Duration::from_secs(20)));
    step(app.wait_for("text:/^Imported Beta Two · /"));
    step(app.wait_until(
        "original guide download waiting",
        Duration::from_secs(10),
        |_| guides.load(Ordering::SeqCst) == 1,
    ));
    assert_eq!(
        accounts.load(Ordering::SeqCst),
        1,
        "joining must reuse the completed account import"
    );
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    step(app.wait_for_within("text:Delayed Beta guide 1", Duration::from_secs(20)));
    // This is the original Add's post-playlist Guide message, not a reimport.
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let observed = opened.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(url) = value.as_ref().and_then(|v| v["url"].as_str())
                    {
                        observed.lock().unwrap().push(url.into());
                    }
                }
            }
        }
    });
    let beta = crate::isolated::open_database(&path)
        .unwrap()
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.name != "Alpha")
        .unwrap()
        .id;
    // Let NextCheck arrive, remove the stored guide, then run the original
    // Add's scheduled check. It must still be eligible in the combined mode.
    step(app.run_for(Duration::from_secs(1)));
    crate::isolated::open_database(&path)
        .unwrap()
        .delete_epg(beta)
        .unwrap();
    step(app.fast_forward(Duration::from_secs(7 * 3600)));
    step(app.wait_for_within("text:Delayed Beta guide 2", Duration::from_secs(20)));
    assert_eq!(accounts.load(Ordering::SeqCst), 1);
    step(app.press("text:/^Imported Beta One · /"));
    step(app.wait_until(
        "TS-only account stream opened",
        Duration::from_secs(5),
        |_| {
            opened
                .lock()
                .unwrap()
                .iter()
                .any(|url| url.ends_with("/1.ts"))
        },
    ));
    step(app.press("text:/^Imported Beta One · /"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    // Playlist parents in the picker expand their categories without closing
    // it or changing the guide's list. The source filter survives both toggles.
    let picker = "id:multiview-picker";
    step(app.press(&format!("{picker} >> text:▾  Alpha")));
    step(app.wait_for(&format!("{picker} >> text:▸  Alpha")));
    step(app.press(&format!("{picker} >> text:▸  Alpha")));
    step(app.wait_for(&format!("{picker} >> text:▾  Alpha")));
    step(app.press(&format!("{picker} >> text:/^    News$/ >> nth=0")));
    step(app.type_text("id:multiview-channel-search", "Imported Beta"));
    step(app.wait_for("text:No matching channels. Clear the search or choose another group."));
    step(app.press(&format!("{picker} >> text:All playlists")));
    step(app.wait_for(&format!("{picker} >> text:/^4  Imported Beta Two · /")));
    step(app.press(&format!("{picker} >> text:/^4  Imported Beta Two · /")));
    step(app.wait_for("text:Your account allows 1 stream at once"));
}

#[test]
fn automatic_join_reloads_previous_source_whose_account_refresh_is_still_pending() {
    use iptv_providers::{Secret, model::XtreamCredentials};
    use nsh_core::{ImageData, picker::PickedFile};
    use std::sync::atomic::Ordering;
    let Some(home) = isolated!() else { return };
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let (server, accounts, _, release) = delayed_account(true);
    let mut store = crate::isolated::open_database(dir.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Pending account".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: server,
                username: Secret::new("viewer"),
                password: Secret::new("secret"),
                user_agent: None,
            }),
        })
        .unwrap();
    let cached = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",Cached Beta\nhttp://stream.example.test/cached-beta\n",
    );
    store
        .replace_catalog(source, &cached.groups, &cached.channels.items, &[], &[])
        .unwrap();
    store.set_active_source(source).unwrap();
    let policy = store.source_policy(source).unwrap();
    assert!(policy.enabled && policy.refresh_on_start);
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    app.on_pick(|_| vec![PickedFile::new("Alpha.m3u", "audio/x-mpegurl", ImageData::new(
        b"#EXTM3U\n#EXTINF:-1 group-title=\"News\",Added Alpha\nhttp://stream.example.test/added-alpha\n".to_vec()
    ))]);
    step(app.wait_for_within("text:Cached Beta", Duration::from_secs(20)));
    // This waits for native client initialization and the first loopback
    // request, so use the same startup/import budget as the surrounding steps.
    // The cached guide is already visible; the request itself remains held.
    let held = app.wait_until(
        "previous startup account request held",
        Duration::from_secs(20),
        |_| accounts.load(Ordering::SeqCst) >= 1,
    );
    step(held.map_err(|error| {
        format!(
            "{error}; observed account requests: {}",
            accounts.load(Ordering::SeqCst)
        )
    }));
    assert_eq!(
        accounts.load(Ordering::SeqCst),
        1,
        "the first startup account request must remain held"
    );
    step(app.press("text:+ Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:Alpha.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:☰  All playlists", Duration::from_secs(20)));
    step(app.wait_for_within("text:/^Imported Beta Two · /", Duration::from_secs(20)));
    assert_eq!(
        accounts.load(Ordering::SeqCst),
        2,
        "an unfinished previous import must be replaced, not retained as provisional cached rows"
    );
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    step(app.wait_for_within("text:Delayed Beta guide 1", Duration::from_secs(20)));
    step(app.wait_for("text:/^Added Alpha · /"));
    assert!(!app.screen().matches("text:/^Cached Beta · /").unwrap());
}

enum PreviousGuideAction {
    PlaylistRefresh,
    ManualUpdate,
    DeletePending,
}

fn exercise_previous_guide_handoff(home: &super::isolated::Home, action: PreviousGuideAction) {
    let manual_guide = matches!(action, PreviousGuideAction::ManualUpdate);
    use nsh_core::{ImageData, picker::PickedFile};
    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let playlist = "#EXTM3U\n#EXTINF:-1 tvg-id=\"previous\" group-title=\"News\",Previous channel\nhttp://stream.example.test/previous\n".to_owned();
    let feed_requests = Arc::new(AtomicUsize::new(0));
    let playlist_requests = Arc::new(AtomicUsize::new(0));
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let observed_feeds = feed_requests.clone();
    let observed_playlists = playlist_requests.clone();
    let unblock = release.clone();
    let body = playlist.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let feeds = observed_feeds.clone();
            let playlists = observed_playlists.clone();
            let release = unblock.clone();
            let playlist = body.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                let _ = reader.read_line(&mut request);
                let mut header = String::new();
                while reader.read_line(&mut header).unwrap_or(0) > 2 {
                    header.clear();
                }
                let body = if request.contains("/playlist.m3u ") {
                    playlists.fetch_add(1, Ordering::SeqCst);
                    playlist
                } else {
                    let count = feeds.fetch_add(1, Ordering::SeqCst) + 1;
                    if count > 1 {
                        let (ready, signal) = &*release;
                        drop(
                            signal
                                .wait_while(ready.lock().unwrap(), |ready| !*ready)
                                .unwrap(),
                        );
                    }
                    let title = if count == 1 {
                        "Previous original guide"
                    } else {
                        "Previous completed guide"
                    };
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64;
                    format!(
                        "<tv><channel id=\"previous\">\
                         <display-name>Previous channel</display-name></channel>\
                         <programme channel=\"previous\" start=\"{}\" stop=\"{}\">\
                         <title>{title}</title></programme></tv>",
                        iptv_epg::format_xmltv_time(now - 3600),
                        iptv_epg::format_xmltv_time(now + 24 * 3600)
                    )
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings
        .set("epg.update_interval", Value::Duration(0))
        .unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(dir.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Previous".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(format!("{base}/playlist.m3u")),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(&playlist);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    store
        .set_guide_sources(
            source,
            &[GuideSource {
                feed: iptv_epg::FeedId(1001),
                provider_index: None,
                url: SecretUrl::new(format!("{base}/guide.xml")),
                name: None,
                enabled: true,
                shift_minutes: 0,
            }],
        )
        .unwrap();
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    app.on_pick(|_| vec![PickedFile::new("Second.m3u", "audio/x-mpegurl", ImageData::new(
        b"#EXTM3U\n#EXTINF:-1 group-title=\"News\",Second channel\nhttp://stream.example.test/second\n".to_vec()
    ))]);
    step(app.wait_for_within("text:Previous original guide", Duration::from_secs(20)));
    if manual_guide {
        step(app.press("text:/^☰  /"));
        step(app.press_key("left"));
        step(app.press_key("left"));
        step(app.wait_for("text:Settings >> nth=0"));
        step(app.press("text:Settings >> nth=0"));
        step(app.wait_for("text:TV guide data"));
        step(app.press("text:TV guide data"));
        step(app.wait_for("text:Update the guide now"));
        step(app.press("text:Update the guide now"));
        step(app.wait_until(
            "explicit previous guide update held",
            Duration::from_secs(10),
            |_| feed_requests.load(Ordering::SeqCst) == 2,
        ));
        step(app.back());
        step(app.back());
    } else {
        playlists(&mut app);
        step(app.press("text:Previous"));
        step(app.wait_for("text:Refresh now"));
        step(app.press("text:Refresh now"));
        step(app.wait_until(
            "previous guide refresh held after playlist",
            Duration::from_secs(10),
            |_| feed_requests.load(Ordering::SeqCst) == 2,
        ));
        // RUNNING clears at Playlist Done, before this guide response is released.
        step(app.press("text:Previous"));
        step(app.wait_for_within("text:Refresh now", Duration::from_secs(10)));
        step(app.back());
        step(app.wait_for("text:Close"));
        step(app.press("text:Close"));
    }
    step(app.press("text:+ Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:Second.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:☰  All playlists", Duration::from_secs(20)));
    step(app.wait_for("text:Previous original guide"));
    step(app.wait_for("text:/^Second channel · /"));
    step(app.wait(Duration::from_millis(300)));
    assert_eq!(
        feed_requests.load(Ordering::SeqCst),
        2,
        "handoff must not start a competing EPG fetch"
    );
    let deleted = matches!(action, PreviousGuideAction::DeletePending);
    if deleted {
        step(app.press("text:/^☰  /"));
        step(app.press_key("left"));
        step(app.press_key("left"));
        step(app.wait_for("text:Settings >> nth=0"));
        step(app.press("text:Settings >> nth=0"));
        step(app.wait_for("text:TV guide data"));
        step(app.press("text:TV guide data"));
        step(app.wait_for("text:Delete stored guide data"));
        step(app.press("text:Delete stored guide data"));
        step(app.wait_for("text:Delete stored guide data?"));
        step(app.press("text:Delete stored guide data >> nth=1"));
        step(app.wait_for_gone("text:Previous original guide", Duration::from_secs(5)));
    }
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    if deleted {
        step(app.wait(Duration::from_millis(300)));
        assert!(
            !app.screen()
                .matches("text:Previous completed guide")
                .unwrap()
        );
        assert!(
            crate::isolated::open_database(dir.join("iptv.sqlite"))
                .unwrap()
                .epg_guide(source)
                .unwrap()
                .is_none()
        );
    } else {
        step(app.wait_for_within("text:Previous completed guide", Duration::from_secs(20)));
    }
    assert_eq!(feed_requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        playlist_requests.load(Ordering::SeqCst),
        usize::from(!manual_guide),
        "retain the ready previous playlist without reimporting it"
    );
}

#[test]
fn automatic_join_receives_previous_inflight_guide_with_automatic_updates_disabled() {
    let Some(home) = isolated!() else { return };
    exercise_previous_guide_handoff(&home, PreviousGuideAction::PlaylistRefresh);
}

#[test]
fn automatic_join_receives_previous_manual_guide_update_with_automatic_updates_disabled() {
    let Some(home) = isolated!() else { return };
    exercise_previous_guide_handoff(&home, PreviousGuideAction::ManualUpdate);
}

#[test]
fn deleting_guide_supersedes_a_retained_previous_download() {
    let Some(home) = isolated!() else { return };
    exercise_previous_guide_handoff(&home, PreviousGuideAction::DeletePending);
}

#[test]
fn forty_cached_sources_open_while_both_network_workers_are_stalled() {
    let Some(home) = isolated!() else { return };
    let directory = home.data_dir();
    std::fs::create_dir_all(&directory).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/synthetic.m3u", listener.local_addr().unwrap());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_accepted = accepted.clone();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut connections = Vec::new();
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok((stream, _)) = listener.accept() {
                connections.push(stream);
                server_accepted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        connections.len()
    });
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
    for index in 0..40 {
        let name = format!("Cached {index:02}");
        let source = store
            .add_source(&Source {
                name: name.clone(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new(&url),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let parsed = iptv_providers::m3u::parse_m3u(&format!(
            "#EXTM3U\n#EXTINF:-1,{name}\nhttps://media.example.test/{index}\n"
        ));
        store
            .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
            .unwrap();
        store
            .set_source_policy(
                source,
                SourcePolicy {
                    refresh_on_start: index < 2,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    store.set_all_playlists(true).unwrap();
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:Cached 00", Duration::from_secs(10)));
    step(crate::isolated::wait_outside(
        &mut app,
        "both refresh workers have outstanding requests",
        Duration::from_secs(10),
        || accepted.load(std::sync::atomic::Ordering::Relaxed) == 2,
    ));
    // Every cached constituent must reach the combined guide independently
    // of the two blocked network requests, including sources beyond queue size.
    step(app.press_key("right"));
    let deadline =
        std::time::Instant::now() + crate::isolated::scaled_duration(Duration::from_secs(10));
    while app.screen().count("text:Cached 39 · Cached 39").unwrap() == 0
        && std::time::Instant::now() < deadline
    {
        step(app.press_key("down"));
        app.idle(Duration::from_millis(10)).unwrap();
    }
    assert!(
        app.screen().count("text:Cached 39 · Cached 39").unwrap() > 0,
        "the final cached source must hydrate while both refresh workers are stalled"
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let requests = server.join().unwrap();
    assert!(requests <= 2, "cached-only sources must not make requests");
}
