//! Combined browsing keeps identical provider keys distinct and routes writes
//! and playback to their owning source. Only generated, local playlists.
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::headless::{Headless, KeyPhase};
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

#[track_caller]
fn wait_for_requests(
    app: &mut Headless,
    what: &str,
    timeout: Duration,
    requests: &std::sync::atomic::AtomicUsize,
    expected: usize,
) {
    use std::sync::atomic::Ordering;
    let deadline = Instant::now() + crate::isolated::scaled_duration(timeout);
    // A held HTTP request updates only the mock server's counter. Waiting
    // for a UI batch can sleep until the deadline after the request arrived.
    while requests.load(Ordering::SeqCst) < expected {
        assert!(Instant::now() < deadline, "{what} did not arrive");
        step(app.wait(Duration::from_millis(10)));
    }
    assert_eq!(requests.load(Ordering::SeqCst), expected, "{what}");
}

fn seed(store: &mut Store, home: &Path, name: &str) -> SourceId {
    seed_with_channels(store, home, name, "")
}

fn seed_with_channels(store: &mut Store, home: &Path, name: &str, channels: &str) -> SourceId {
    let playlist = format!(
        "#EXTM3U\n\
         #EXTINF:-1 tvg-id=\"shared\" group-title=\"News\",Shared\nhttp://stream.example.test/shared\n\
         #EXTINF:-1 group-title=\"News\",Only {name}\nhttp://stream.example.test/{name}\n{channels}"
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
             <desc>{name} synthetic program details.</desc></programme></tv>",
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

/// Settings → Playlists.
fn playlists(app: &mut Headless) {
    step(app.wait_for("id:guide-timeline"));
    step(app.open_settings_playlists());
}

/// The settings row of playlist `name`.
fn row(name: &str) -> String {
    format!("id:settings-content >> role:checkbox[/^{name} · /]")
}

/// Playlist `name`'s actions: a long press on its settings row.
fn actions(app: &mut Headless, name: &str) {
    step(app.open_playlist_actions(name));
}

#[test]
fn recycled_playlist_and_guide_rows_keep_their_content_and_actions() {
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[("IPTV_TEST_DEVICE_SIGNALS", "1")],
    ) else {
        return;
    };
    let directory = home.data_dir();
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let mut last = SourceId(0);
    for index in 0..36 {
        last = seed(&mut store, home.path(), &format!("Playlist {index:02}"));
        store.set_guide_sources(last, &[]).unwrap();
    }
    let guide = std::fs::read(home.path().join("Playlist 35.xml")).unwrap();
    let feeds: Vec<_> = (0..20)
        .map(|index| {
            let file = home.path().join(format!("feed-{index:02}.xml"));
            std::fs::write(&file, &guide).unwrap();
            GuideSource {
                feed: iptv_epg::FeedId(2000 + index),
                provider_index: None,
                url: SecretUrl::new(file.to_string_lossy()),
                name: Some(format!("Feed {index:02}")),
                enabled: true,
                shift_minutes: 0,
            }
        })
        .collect();
    store.set_guide_sources(last, &feeds).unwrap();
    store.set_all_playlists(false).unwrap();
    drop(store);
    let mut settings = Settings::new();
    settings
        .set("appearance.font_size", Value::choice("extra_large"))
        .unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(960.0), px(540.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for("id:guide-timeline"));
    crate::device_settings::report(&mut app, "tv", false, true);
    playlists(&mut app);
    // Into the rows, down past recycled rows to the last playlist; Menu
    // opens its actions.
    step(app.press_key("right"));
    for _ in 0..35 {
        step(app.press_key("down"));
    }
    step(app.wait_for(&row("Playlist 35")));
    step(app.press_remote(nsh_tv::RemoteKey::Menu));
    step(app.wait_for("id:playlist-actions-list"));
    // Scroll down and back to the destructive action after cells are recycled.
    for _ in 0..30 {
        step(app.press_key("down"));
    }
    for _ in 0..25 {
        if app
            .screen()
            .matches("id:playlist-actions-list >> text:Delete")
            .unwrap()
        {
            break;
        }
        step(app.press_key("up"));
    }
    step(app.press("id:playlist-actions-list >> text:Delete"));
    step(app.wait_for("text:Delete “Playlist 35”?"));
    step(app.press("text:Keep it"));
    step(app.back());
    step(app.open_playlist_actions("Playlist 35"));
    for _ in 0..25 {
        if app
            .screen()
            .matches("id:playlist-actions-list >> text:Guide data")
            .unwrap()
        {
            break;
        }
        step(app.press_key("down"));
    }
    step(app.press("id:playlist-actions-list >> text:Guide data"));
    step(app.wait_for("text:Guide data · Playlist 35"));
    for _ in 0..19 {
        step(app.press_key("down"));
    }
    step(app.wait_for("id:guide-data-list >> text:20.  Feed 19"));
    step(app.press("id:guide-data-list >> text:20.  Feed 19"));
    step(app.wait_for("id:guide-source-actions >> text:Feed 19"));
    step(app.press("id:guide-source-actions >> text:Switch off"));
    step(app.wait_until(
        "only the selected feed is switched off",
        Duration::from_secs(5),
        |_| {
            let feeds = crate::isolated::open_database(&path)
                .unwrap()
                .guide_sources(last)
                .unwrap();
            feeds.len() == 20
                && feeds
                    .iter()
                    .all(|feed| feed.enabled == (feed.feed != iptv_epg::FeedId(2019)))
        },
    ));
    step(app.back());
    for _ in 0..10 {
        step(app.press_key("down"));
    }
    step(app.press("id:guide-data-list >> text:Delete stored guide data"));
    step(app.wait_for("text:Delete guide data for “Playlist 35”?"));
    step(app.press("text:Cancel"));
}

#[test]
fn failed_guide_source_actions_follow_focus_inside_large_text_tv() {
    let Some(home) = isolated!() else { return };
    let directory = home.data_dir();
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    seed(&mut store, home.path(), "Generated guide actions");
    drop(store);
    let guide = home.path().join("Generated guide actions.xml");
    let xml = std::fs::read(&guide).unwrap();
    std::fs::remove_file(&guide).unwrap();
    let mut settings = Settings::new();
    settings
        .set("epg.update", Value::choice("on_request"))
        .unwrap();
    settings
        .set("appearance.font_size", Value::choice("extra_large"))
        .unwrap();
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
    let mut config = EngineConfig {
        viewport: Size::new(px(960.0), px(540.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = nsh_core::platform::Idiom::Tv;
    config.environment.font_scale = 1.3;
    let mut app = Headless::start(config).unwrap();
    playlists(&mut app);
    actions(&mut app, "Generated guide actions");
    for _ in 0..30 {
        step(app.press_key("down"));
    }
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.press("id:playlist-actions-list >> text:Guide data"));
    if !app
        .screen()
        .matches("id:guide-data-list >> text:Retry")
        .unwrap()
    {
        step(app.press("id:guide-data-list >> text:Update the guide now"));
    }
    step(app.wait_for("id:guide-data-list >> text:Retry"));
    step(app.press("id:guide-data-list >> text:1.  Generated guide actions guide"));
    step(app.wait_for("id:guide-source-actions >> text:Retry"));
    step(app.press_key("up"));
    let inside = |screen: &nsh_testing::Screen, selector: &str| {
        let nodes = screen.layout_nodes();
        let bounds = |selector| {
            let id = screen.id(selector).unwrap();
            nodes.iter().find(|n| n.id == id).unwrap().bounds
        };
        let card = bounds("id:guide-source-actions");
        let viewport = bounds("id:guide-source-actions-scroll");
        let row = bounds(selector);
        card.origin.y.0 >= 0.0
            && card.origin.y.0 + card.size.height.0 <= 540.0
            && row.origin.x.0 >= viewport.origin.x.0 - 1.0
            && row.origin.x.0 + row.size.width.0
                <= viewport.origin.x.0 + viewport.size.width.0 + 1.0
            && row.origin.y.0 >= viewport.origin.y.0 - 1.0
            && row.origin.y.0 + row.size.height.0
                <= viewport.origin.y.0 + viewport.size.height.0 + 1.0
    };
    let check_focus = |app: &mut Headless, label: &str| {
        let selector = format!("id:guide-source-actions >> label:{label}");
        step(app.wait_until(
            "focused action is visible",
            Duration::from_secs(5),
            |screen| inside(screen, &selector),
        ));
        assert_eq!(
            app.screen()
                .span_style(&format!("{selector} >> text:{label}"))
                .unwrap()
                .color,
            nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff),
            "D-pad focus reached {label}"
        );
    };
    let labels = [
        "Retry",
        "Switch off",
        "Move up (used first)",
        "Move down",
        "No time shift",
        "Name",
        "Change the link",
        "Remove",
    ];
    for label in labels {
        check_focus(&mut app, label);
        if label != "Remove" {
            step(app.press_key("down"));
        }
    }
    step(app.press_key("enter"));
    step(app.wait_for("text:Remove “Generated guide actions guide”?"));
    // Keep it is the default remote action; returning rebuilds the scroll
    // at its initial offset while logical focus remains on Remove.
    step(app.press_key("enter"));
    check_focus(&mut app, "Remove");
    for label in labels[..7].iter().rev() {
        step(app.press_key("up"));
        check_focus(&mut app, label);
    }
    std::fs::write(&guide, xml).unwrap();
    step(app.press_key("enter"));
    step(app.wait_for_gone(
        "id:guide-source-actions >> text:Retry",
        Duration::from_secs(5),
    ));
    check_focus(&mut app, "Switch off");
}

fn favorite_names(path: &Path, source: SourceId) -> Vec<String> {
    crate::isolated::open_database(path)
        .unwrap()
        .load_channels(
            source,
            &ChannelQuery {
                favorites_only: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|row| row.name)
        .collect()
}

#[test]
fn all_playlists_row_identity_follows_playlist_deletion() {
    let Some(home) = isolated!() else { return };
    let directory = home.data_dir();
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    seed(&mut store, home.path(), "Alpha");
    seed(&mut store, home.path(), "Beta");
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    playlists(&mut app);
    actions(&mut app, "Beta");
    for _ in 0..30 {
        step(app.press_key("down"));
    }
    step(app.press("id:playlist-actions-list >> text:Delete"));
    step(app.wait_for("text:Delete “Beta”?"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for_gone(&row("Beta"), Duration::from_secs(10)));
    // The playlist that stays keeps its one row; the commands follow it.
    assert_eq!(
        app.screen()
            .count("id:settings-content >> role:checkbox")
            .unwrap(),
        1
    );
    step(app.wait_for(&row("Alpha")));
    step(app.wait_for("id:settings-content >> text:Add a playlist"));
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
    step(app.wait_for_within("text:Only Beta", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.wait_for("id:guide-groups >> role:tab[Beta] >> text:▶"));
    let expansion =
        |app: &Headless, tab: &str| app.screen().accessibility_state(tab).unwrap().expanded;
    assert_eq!(
        expansion(&app, "id:guide-groups >> role:tab[Alpha]"),
        Some(true)
    );
    assert_eq!(
        expansion(&app, "id:guide-groups >> role:tab[Beta]"),
        Some(false)
    );
    assert_eq!(expansion(&app, "id:guide-groups >> role:tab[News]"), None);
    step(app.press("id:guide-groups >> text:    News"));
    step(app.press_key("right"));
    step(app.wait_for("role:grid[News]"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    assert!(
        !app.screen()
            .matches("id:guide-row >> text:/^Only Beta$/")
            .unwrap()
    );

    step(app.press("text:Only Alpha"));
    step(app.wait_for("text:▶"));
    step(app.press("text:Only Alpha"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    let picker = "id:multiview-picker";
    step(app.wait_for(picker));
    // The picker's expansion is temporary. In particular, folding Alpha must
    // not remove the guide's selected News category from its own catalog.
    step(app.press(&format!("{picker} >> role:tab[Alpha] >> text:▼")));
    step(app.wait_for(&format!("{picker} >> role:tab[Alpha] >> text:▶")));
    assert_eq!(
        expansion(&app, &format!("{picker} >> role:tab[Alpha]")),
        Some(false)
    );
    step(app.press(&format!("{picker} >> role:tab[Alpha] >> text:▶")));
    step(app.wait_for(&format!("{picker} >> role:tab[Alpha] >> text:▼")));
    step(app.press(&format!("{picker} >> role:tab[Alpha] >> text:▼")));
    step(app.wait_for(&format!("{picker} >> role:tab[Alpha] >> text:▶")));
    step(app.press(&format!("{picker} >> role:tab[Beta] >> text:▶")));
    step(app.wait_for(&format!("{picker} >> role:tab[Beta] >> text:▼")));
    assert_eq!(
        expansion(&app, &format!("{picker} >> role:tab[Beta]")),
        Some(true)
    );
    // Give any erroneously queued preference writes or catalog rebuilds a
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
    step(app.wait_for("role:grid[News]"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    step(app.wait_for("text:Only Alpha"));
    assert!(
        !app.screen()
            .matches("id:guide-row >> text:/^Only Beta$/")
            .unwrap()
    );
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.wait_for("id:guide-groups >> role:tab[Beta] >> text:▶"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        1,
        "the guide keeps its own expanded playlist and selected category"
    );
}

/// A local M3U server that answers only once released.
fn held_playlist(playlist: &'static str) -> (String, Arc<(Mutex<bool>, std::sync::Condvar)>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/synthetic.m3u", listener.local_addr().unwrap());
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let held = release.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let held = held.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let (released, signal) = &*held;
                drop(
                    signal
                        .wait_while(released.lock().unwrap(), |released| !*released)
                        .unwrap(),
                );
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/x-mpegurl\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{playlist}",
                    playlist.len()
                );
            });
        }
    });
    (url, release)
}

/// The combined guide with its second playlist, Beta, on screen while the
/// first, Alpha, is still downloading. Setting the returned flag lets Alpha in.
fn second_playlist_first(
    home: &crate::isolated::Home,
) -> (Headless, Arc<(Mutex<bool>, std::sync::Condvar)>) {
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(dir.join("iptv.sqlite")).unwrap();
    let (url, release) = held_playlist(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",Only Alpha\n\
         http://stream.example.test/Alpha\n",
    );
    store
        .add_source(&Source {
            name: "Alpha".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(url),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    seed(&mut store, home.path(), "Beta");
    store.set_all_playlists(true).unwrap();
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("id:guide-row >> text:Only Beta", Duration::from_secs(20)));
    step(app.wait_for("id:guide-header-details >> text:Beta · News"));
    assert!(!app.shows("id:guide-row >> text:Only Alpha"));
    (app, release)
}

fn release_playlist(release: &(Mutex<bool>, std::sync::Condvar)) {
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
}

#[test]
fn an_untouched_combined_guide_stays_at_the_top_as_an_earlier_playlist_loads() {
    let Some(home) = isolated!() else { return };
    let (mut app, release) = second_playlist_first(&home);
    release_playlist(&release);
    step(app.wait_for_within("id:guide-row >> text:Only Alpha", Duration::from_secs(20)));
    step(app.wait_for("id:guide-header-details >> text:Alpha · News"));
    step(app.wait_for("id:guide-header-details >> text:Only Alpha"));
}

#[test]
fn a_chosen_combined_guide_row_is_followed_as_an_earlier_playlist_loads() {
    let Some(home) = isolated!() else { return };
    let (mut app, release) = second_playlist_first(&home);
    // Away and back: the same first row, now the viewer's choice.
    step(app.press_key("right"));
    step(app.press_key("down"));
    step(app.wait_for("id:guide-header-details >> text:Only Beta"));
    step(app.press_key("up"));
    step(app.wait_for("id:guide-header-details >> text:Shared"));
    release_playlist(&release);
    step(app.wait_for_within("id:guide-row >> text:Only Alpha", Duration::from_secs(20)));
    step(app.wait_for("id:guide-header-details >> text:Shared"));
    step(app.wait_for("id:guide-header-details >> text:Beta · News"));
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
    let beta = seed_with_channels(
        &mut store,
        home.path(),
        "Beta",
        "#EXTINF:-1 group-title=\"Sports\",Beta Sports\nhttp://stream.example.test/beta-sports\n",
    );
    store.set_active_source(alpha).unwrap();
    drop(store);

    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    // Multiple enabled sources are the default, without a visit to management.
    step(app.wait_for_within("id:guide-row >> text:Shared", Duration::from_secs(20)));
    step(app.wait_for_within("text:Alpha bulletin", Duration::from_secs(20)));
    step(app.wait_for_within("text:Beta bulletin", Duration::from_secs(20)));
    step(app.wait_for("id:guide-row >> text:Beta Sports"));
    assert_eq!(
        app.screen().count("id:guide-row >> text:Shared").unwrap(),
        2
    );
    assert!(
        !app.screen()
            .matches("id:guide-row >> text:/ · (Alpha|Beta)/")
            .unwrap()
    );
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 5);

    step(app.wait_for("id:guide-header-details >> text:Shared"));
    step(app.wait_for("id:guide-header-details >> text:Alpha · News"));

    // Cross-playlist favorite management belongs to the guide's program menu.
    step(app.long_press("id:guide-ui >> role:cell[Alpha bulletin]"));
    step(app.press("id:menu-panel >> text:More options"));
    step(app.wait_for("id:menu-panel >> text:Manage favorites…"));
    step(app.back());
    step(app.back());
    // Copying from either News group stays in its owning source.
    step(app.open_guide_groups());
    step(app.press("id:guide-groups >> text:    News >> nth=1"));
    step(app.press_key("right"));
    step(app.wait_for("role:grid[News]"));
    crate::personalization::open_program_menu(&mut app);
    step(app.press("id:menu-panel >> text:Copy channels to a group…"));
    step(app.wait_for("text:0 selected · 2 matching · 2 total"));
    step(app.wait_for("id:custom-groups-screen >> text:Only Beta"));
    assert!(!app.shows("id:custom-groups-screen >> text:Only Alpha"));
    assert!(!app.shows("id:custom-groups-screen >> text:Beta Sports"));
    step(app.back());
    step(app.back());
    step(app.wait_for_gone("id:custom-groups-screen", Duration::from_secs(3)));
    step(app.open_guide_groups());
    step(app.press("id:guide-groups >> text:All playlists"));
    step(app.press_key("right"));
    step(app.wait_for("role:grid[All playlists]"));

    // These shared rows have the same original provider key. The header
    // follows Beta's source, and editing Beta preserves Alpha's membership.
    step(app.press("id:guide-row >> text:Shared >> nth=1"));
    step(app.wait_for("id:guide-header-details >> text:Shared"));
    step(app.wait_for("id:guide-header-details >> text:Beta · News"));
    step(app.press_key("m"));
    step(app.wait_for("text:Add to favorites"));
    step(app.press("text:Add to favorites"));
    step(crate::isolated::wait_outside(
        &mut app,
        "owning source favorite saved",
        Duration::from_secs(5),
        || favorite_names(&path, beta) == ["Shared"],
    ));
    assert!(favorite_names(&path, alpha).is_empty());
    step(app.wait_for("id:guide-header-details >> text:★"));
    let nodes = app.screen().layout_nodes();
    let bounds = |id| {
        nodes
            .iter()
            .find(|n| n.test_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("missing {id}"))
            .bounds
    };
    let star = bounds("guide-header-favorite");
    let source = bounds("guide-header-source");
    let channel = bounds("guide-header-channel");
    let details = bounds("guide-header-details");
    assert!((star.origin.y.0 - channel.origin.y.0).abs() < 1.0);
    let right = |b: nsh_core::Bounds<nsh_core::Points>| b.origin.x.0 + b.size.width.0;
    assert!((right(star) - right(details)).abs() < 1.0);
    assert!((right(source) - right(star)).abs() < 1.0);
    assert!(source.origin.y.0 >= star.origin.y.0 + star.size.height.0 - 0.5);

    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let observed = opened.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(source) = value.as_ref().map(crate::support::upstream_source)
                        && let Some(url) = source["url"].as_str()
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
    step(app.open_guide_groups());
    let nodes = app.screen().layout_nodes();
    let playing = nodes
        .iter()
        .filter(|node| node.test_id.as_deref() == Some("guide-playing-marker"))
        .filter_map(|node| node.text.as_ref())
        .find(|text| text.text.text == "▶")
        .expect("the playing channel has a marker");
    let disclosures: Vec<_> = nodes
        .iter()
        .filter(|node| node.test_id.as_deref() == Some("playlist-disclosure"))
        .collect();
    assert_eq!(disclosures.len(), 2);
    assert!((right(disclosures[0].bounds) - right(disclosures[1].bounds)).abs() < 1.0);
    for node in disclosures {
        let text = node.text.as_ref().unwrap();
        assert_eq!(
            text.text.spans[0].style.font_size,
            playing.text.spans[0].style.font_size
        );
        let row = nodes.iter().find(|row| row.id == node.parent).unwrap();
        let inset = right(row.bounds) - right(node.bounds);
        assert!((inset - 14.0).abs() < 1.0, "right inset was {inset}");
    }

    step(app.press_key("/"));
    step(app.wait_for("id:search-field"));
    step(app.type_text("id:search-field", "shared"));
    step(app.wait_until(
        "both Shared channels in search results",
        Duration::from_secs(10),
        |screen| screen.count("id:search-result >> text:/Shared$/").unwrap() == 2,
    ));
    assert!(
        !app.screen()
            .matches("id:search-result >> text:/ · (Alpha|Beta)/")
            .unwrap()
    );
    step(app.clear("id:search-field"));
    step(app.type_text("id:search-field", "bulletin"));
    step(app.wait_for("text:/^Programs/"));
    step(app.press("text:/^Programs/"));
    step(app.wait_for_within("text:/Alpha bulletin/", Duration::from_secs(10)));
    step(app.wait_for("text:/Beta bulletin/"));
    step(app.press("text:Close"));

    // Switching back to a source restores its ordinary names and does not
    // leak the other source's favorite, then All playlists remains available.
    playlists(&mut app);
    actions(&mut app, "Alpha");
    step(app.wait_for("text:Show this playlist"));
    step(app.press("text:Show this playlist"));
    step(app.wait_for_within("text:Only Alpha", Duration::from_secs(20)));
    assert!(!app.screen().matches("text:/^Only Beta/").unwrap());
    assert!(!app.screen().matches("text:/^★ Shared/").unwrap());

    playlists(&mut app);
    step(app.press("id:settings-content >> text:Show all playlists"));
    step(app.wait_for_within("text:/^Only Beta/", Duration::from_secs(20)));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .all_playlists()
            .unwrap()
    );
    playlists(&mut app);
    // Its check box hides it.
    step(app.press(&row("Beta")));
    step(app.wait_for("id:settings-content >> role:checkbox[/^Beta · .* · Disabled/]"));
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
fn playlist_popup_hides_and_settings_restores_the_original_playlist() {
    use nsh_core::Checked;
    use nsh_core::platform::{Idiom, Os};
    use nsh_tv::RemoteKey;

    let Some(home) = isolated!() else { return };
    iptv_app::use_remote_key_phases();
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let alpha = seed(&mut store, home.path(), "Alpha");
    let beta = seed(&mut store, home.path(), "Beta");
    let gamma = seed(&mut store, home.path(), "Gamma");
    store
        .set_source_policy(
            gamma,
            SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    store.set_all_playlists(true).unwrap();
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    let alpha_heading = "id:guide-groups >> role:tab[Alpha]";
    let beta_heading = "id:guide-groups >> role:tab[Beta]";
    let checkbox = |name| format!("id:menu-panel >> role:checkbox[{name}]");
    let delete = "id:playlist-menu-panel >> text:Delete playlist";
    let confirmation = "id:playlist-delete-confirmation";
    let check = |app: &mut Headless, name, on| {
        step(app.wait_until(
            &format!("playlist checkbox {name} is {on}"),
            Duration::from_secs(8),
            |screen| {
                screen
                    .accessibility_state(&checkbox(name))
                    .is_ok_and(|state| {
                        state.checked == Some(if on { Checked::True } else { Checked::False })
                    })
            },
        ));
    };
    let load = || crate::isolated::open_database(&path).unwrap();

    let hide = "id:playlist-menu-panel >> text:Hide playlist";
    let hidden = |app: &mut Headless| {
        step(app.wait_for_gone(hide, Duration::from_secs(5)));
        assert!(!app.shows("id:playlist-menu-panel >> text:Show"));
    };
    let interval_off = "id:playlist-menu-panel >> text:Automatic refresh: Off";
    let interval_hour = "id:playlist-menu-panel >> text:Automatic refresh: every hour";
    let startup = "id:playlist-menu-panel >> text:Refresh on startup: Yes";
    let no_startup = "id:playlist-menu-panel >> text:Refresh on startup: No";
    step(app.wait_for("text:Only Beta"));
    step(app.open_guide_groups());
    step(app.long_press(alpha_heading));
    step(app.wait_for(hide));
    for unexpected in [
        "role:checkbox",
        "text:Beta",
        "text:Gamma",
        "text:Playlist settings",
        "text:Reorder channel",
        "text:Show all playlists",
    ] {
        assert!(
            !app.screen()
                .matches(&format!("id:playlist-menu-panel >> {unexpected}"))
                .unwrap()
        );
    }
    step(app.back());
    step(app.set_environment("desktop", |env| {
        env.idiom = Idiom::Desktop;
        env.os = Os::Linux;
    }));
    step(app.right_click(beta_heading));
    step(app.wait_for("id:playlist-menu-panel >> role:header[Beta]"));
    step(app.press(interval_off));
    step(app.press(no_startup));
    step(app.wait_for(interval_hour));
    step(app.wait_for(startup));
    assert_eq!(load().source_policy(beta).unwrap().refresh_seconds, 3600);
    assert!(load().source_policy(beta).unwrap().refresh_on_start);
    assert_eq!(
        load().source_policy(alpha).unwrap(),
        SourcePolicy {
            refresh_on_start: false,
            ..Default::default()
        }
    );
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .pragma_update(None, "key", format!("x'{}'", "54".repeat(32)))
        .unwrap();
    writer
        .execute_batch(
            "CREATE TRIGGER fail_visibility BEFORE UPDATE OF enabled ON source_policy
             BEGIN SELECT RAISE(ABORT,'synthetic visibility failure'); END;
             CREATE TRIGGER fail_deletion BEFORE DELETE ON sources
             BEGIN SELECT RAISE(ABORT,'synthetic deletion failure'); END;",
        )
        .unwrap();
    let failure =
        "id:app-notice >> text:The change could not be saved. Reopen the list and try again.";
    step(app.press(hide));
    step(app.wait_for(failure));
    step(app.wait_for(hide));
    assert!(load().source_policy(beta).unwrap().enabled);
    writer
        .execute_batch("DROP TRIGGER fail_visibility")
        .unwrap();
    step(app.wait_for_gone(failure, Duration::from_secs(5)));
    step(app.press(hide));
    hidden(&mut app);
    assert!(!load().source_policy(beta).unwrap().enabled);
    assert!(load().source(beta).unwrap().is_some());
    assert!(!load().source_policy(gamma).unwrap().enabled);
    step(app.press(delete));
    step(app.wait_for("id:playlist-delete-confirmation >> text:Delete “Beta”?"));
    step(app.press(&format!("{confirmation} >> text:Keep it")));
    hidden(&mut app);
    assert!(load().source(beta).unwrap().is_some());
    step(app.press(delete));
    step(app.press(&format!("{confirmation} >> text:Delete")));
    step(app.wait_for(failure));
    hidden(&mut app);
    assert!(load().source(beta).unwrap().is_some());
    writer.execute_batch("DROP TRIGGER fail_deletion").unwrap();
    step(app.press(delete));
    step(app.press(&format!("{confirmation} >> text:Delete")));
    step(app.wait_for_gone("id:playlist-menu-panel", Duration::from_secs(8)));
    assert!(load().source(beta).unwrap().is_none());
    assert!(load().source(alpha).unwrap().is_some());
    assert!(load().source(gamma).unwrap().is_some());

    // Holding OK opens Alpha's menu without a second action on key-up.
    step(app.open_guide_groups());
    step(app.press(alpha_heading));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.wait(Duration::from_millis(700)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.wait_for(hide));
    for _ in 0..2 {
        step(app.remote_tap(RemoteKey::Down));
    }
    step(app.remote_tap(RemoteKey::Ok));
    hidden(&mut app);
    assert!(load().all_playlists().unwrap());
    step(app.back());
    step(app.wait_for("text:This group has no channels."));

    // The aggregate reports hidden sources; Settings restores them by checkbox.
    step(app.open_guide_groups());
    step(app.right_click("id:guide-groups >> role:tab[All playlists]"));
    check(&mut app, "Alpha", false);
    check(&mut app, "Gamma", false);
    assert!(
        app.screen()
            .accessibility_state(&checkbox("Alpha"))
            .unwrap()
            .disabled
    );
    step(app.back());
    playlists(&mut app);
    step(app.press(&row("Alpha")));
    step(app.wait_for("id:settings-content >> role:checkbox[/^Alpha · /]"));
    step(app.wait_until(
        "Alpha restored from Settings",
        Duration::from_secs(5),
        |_| load().source_policy(alpha).unwrap().enabled,
    ));
    step(app.press("id:settings-content >> role:button[Close]"));
    step(app.wait_for("text:Only Alpha"));
    assert!(!load().source_policy(gamma).unwrap().enabled);

    // Settings exposes a direct delete control for hidden sources, too.
    let hide = "id:playlist-menu-panel >> text:Hide";
    let hidden = |app: &mut Headless| {
        step(app.wait_for_gone(hide, Duration::from_secs(5)));
        assert!(!app.shows("id:playlist-menu-panel >> text:Show"));
    };
    playlists(&mut app);
    step(app.remote_tap(RemoteKey::Right));
    step(app.wait_for("text:▲▼ move   OK show or hide   Menu its settings   ◀ Back pages"));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.wait(Duration::from_millis(700)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.wait_for(hide));
    step(app.back());
    step(app.long_press(&row("Gamma")));
    step(app.wait_for("id:playlist-menu-panel >> role:header[Gamma]"));
    hidden(&mut app);
    step(app.back());
    step(app.press(&row("Gamma")));
    step(app.wait_until(
        "Gamma restored from Settings",
        Duration::from_secs(5),
        |_| load().source_policy(gamma).unwrap().enabled,
    ));
    step(app.long_press(&row("Gamma")));
    step(app.press(hide));
    hidden(&mut app);
    step(app.wait_until(
        "Gamma hidden in the saved playlist policy",
        Duration::from_secs(5),
        |_| !load().source_policy(gamma).unwrap().enabled,
    ));
    step(app.back());
    let settings_delete = "id:settings-content >> role:button[Delete “Gamma”]";
    step(app.press(settings_delete));
    step(app.wait_for("id:playlist-delete-confirmation >> text:Delete “Gamma”?"));
    step(app.press(&format!("{confirmation} >> text:Keep it")));
    assert!(load().source(gamma).unwrap().is_some());
    step(app.press(settings_delete));
    step(app.press(&format!("{confirmation} >> text:Delete")));
    step(app.wait_for_gone(settings_delete, Duration::from_secs(8)));
    step(app.wait_for_gone(&row("Gamma"), Duration::from_secs(8)));
    assert!(!app.screen().matches(&row("Beta")).unwrap());
    step(app.wait_for(&row("Alpha")));
    assert!(load().source(gamma).unwrap().is_none());
    assert_eq!(load().sources().unwrap()[0].id, alpha);
    step(app.press("id:settings-content >> role:button[Close]"));
    step(app.open_guide_groups());
    step(app.right_click("id:guide-groups >> role:tab[All playlists]"));
    check(&mut app, "Alpha", true);
    assert_eq!(
        app.screen()
            .count("id:menu-panel >> role:checkbox")
            .unwrap(),
        1
    );
}

#[test]
fn playlist_order_options_apply_distinct_guide_orders_and_keep_the_custom_order() {
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let zulu = seed(&mut store, home.path(), "Zulu");
    let alpha = seed(&mut store, home.path(), "Alpha");
    let mike = seed(&mut store, home.path(), "Mike");
    store.set_source_order(&[mike, zulu, alpha]).unwrap();
    store
        .set_collapsed_playlists(&[zulu.0, alpha.0, mike.0])
        .unwrap();
    store.set_all_playlists(true).unwrap();
    drop(store);
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    let mut app = Headless::start(config).unwrap();
    step(app.set_environment("desktop", |env| env.os = nsh_core::platform::Os::Linux));
    step(app.open_guide_groups());
    let mut method = "Custom order";
    for (next, order) in [
        ("By date added", ["Zulu", "Alpha", "Mike"]),
        ("By name", ["Alpha", "Mike", "Zulu"]),
        ("Custom order", ["Mike", "Zulu", "Alpha"]),
    ] {
        step(app.right_click("id:guide-groups >> role:tab[Zulu]"));
        step(app.press(&format!(
            "id:playlist-menu-panel >> text:Playlist order: {method}"
        )));
        step(app.press(&format!("id:menu-panel >> text:{next}")));
        crate::personalization_order::wait_visible_order(&mut app, &order);
        method = next;
    }
    let store = crate::isolated::open_database(&path).unwrap();
    assert_eq!(
        store
            .sources()
            .unwrap()
            .iter()
            .map(|source| source.id)
            .collect::<Vec<_>>(),
        [mike, zulu, alpha]
    );
}

#[test]
fn saving_a_playlist_order_draft_rechecks_settings_access() {
    use nsh_core::{
        EventKind, EventPayload,
        platform::{Idiom, Os},
    };

    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let alpha = seed(&mut store, home.path(), "Alpha");
    let zulu = seed(&mut store, home.path(), "Zulu");
    store.set_all_playlists(true).unwrap();
    store
        .set_parental_credential(&iptv_store_sqlite::parental::ParentalCredential {
            salt: [0x63; 16],
            verifier: [
                108, 253, 222, 238, 40, 38, 77, 8, 100, 135, 138, 42, 133, 48, 171, 236, 123, 169,
                134, 85, 199, 28, 212, 46, 152, 192, 13, 230, 58, 195, 156, 119,
            ],
            recovery_verifier: [0; 32],
            failures: 0,
            retry_after: 0,
        })
        .unwrap();
    drop(store);
    let mut settings = Settings::new();
    for (key, value) in [
        ("playlists.sort", "name"),
        ("parental.require_pin_for", "blocked_content_and_settings"),
        ("parental.unlock_window", "every_time"),
    ] {
        settings.set(key, Value::choice(value)).unwrap();
    }
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();

    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(config).unwrap();
    step(app.set_environment("desktop", |env| env.os = Os::Linux));
    step(app.open_guide_groups());
    let unlock = |app: &mut Headless| {
        step(app.wait_for("id:parental-continue"));
        // This synthetic verifier is for 2468.
        for digit in ["2", "4", "6", "8"] {
            step(app.press_key(digit));
        }
        step(app.press("id:parental-continue"));
        step(app.wait_for_gone("id:parental-continue", Duration::from_secs(10)));
    };
    step(app.right_click("id:guide-groups >> role:tab[Zulu]"));
    unlock(&mut app);
    step(app.press("id:playlist-menu-panel >> text:Reorder playlist"));
    step(app.wait_for("id:organization-row-1 >> text:Zulu"));
    step(app.press_key("enter"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("id:organization-row-0 >> text:Zulu"));
    let surface = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("video-player"))
        .unwrap()
        .id;
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "test.parental.lock".into(),
            data: serde_json::Value::Null,
        },
    );
    app.settle();
    let saved = || {
        crate::isolated::open_database(&path)
            .unwrap()
            .sources()
            .unwrap()
            .into_iter()
            .map(|source| source.id)
            .collect::<Vec<_>>()
    };
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for("id:parental-continue"));
    assert_eq!(saved(), [alpha, zulu]);
    unlock(&mut app);
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    assert_eq!(saved(), [zulu, alpha]);
    crate::personalization::saved_settings(&mut app, &dir.join("settings.json"), |settings| {
        settings.choice(
            iptv_settings::keys::PLAYLIST_SORT,
            &iptv_settings::Target::Global,
        ) == "manual"
    });
}

#[test]
fn playlist_reorder_from_settings_and_guide_restores_expansion_and_visibility() {
    use nsh_core::geometry::point;
    use nsh_core::platform::Idiom;

    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let load = || crate::isolated::open_database(&path).unwrap();
    let mut store = load();
    let alpha = seed(&mut store, home.path(), "Alpha");
    let beta = seed(&mut store, home.path(), "Beta");
    let gamma = seed(&mut store, home.path(), "Gamma");
    store
        .set_source_policy(
            gamma,
            SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    store.set_collapsed_playlists(&[beta.0]).unwrap();
    store.set_all_playlists(true).unwrap();
    drop(store);
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(config).unwrap();
    step(app.set_environment("desktop", |env| {
        env.idiom = Idiom::Desktop;
        env.os = nsh_core::platform::Os::Linux;
    }));
    let persisted = || {
        load()
            .sources()
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect::<Vec<_>>()
    };
    let restored = |app: &mut Headless| {
        step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
        step(app.wait_for("id:guide-groups >> role:tab[Beta] >> text:▶"));
        assert_eq!(
            app.screen()
                .count("id:guide-groups >> text:    News")
                .unwrap(),
            1
        );
        assert!(
            !app.screen()
                .matches("id:guide-groups >> role:tab[Gamma]")
                .unwrap()
        );
        assert_eq!(load().collapsed_playlists().unwrap(), [beta.0]);
        assert!(!load().source_policy(gamma).unwrap().enabled);
    };
    let order_screen = |app: &mut Headless| {
        step(app.wait_for("id:organization-screen >> text:Reorder playlists"));
        assert!(!app.screen().matches("id:settings-screen").unwrap());
        assert!(!app.screen().matches("id:guide-groups").unwrap());
        assert_eq!(
            app.screen()
                .count("id:organization-screen >> role:row")
                .unwrap(),
            3
        );
        for index in 0..3 {
            step(app.wait_for(&format!(
                "id:organization-row-{index} >> id:reorder-handle-{index}"
            )));
        }
        assert_eq!(load().collapsed_playlists().unwrap(), [beta.0]);
    };
    step(app.wait_for("text:Only Beta"));
    step(app.open_guide_groups());
    restored(&mut app);
    let command = "id:playlist-menu-panel >> text:Reorder playlist";
    step(app.right_click("id:guide-groups >> role:tab[Beta]"));
    assert_eq!(
        app.panel_texts("playlist-actions-list"),
        [
            "Rename playlist",
            "Delete playlist",
            "Hide playlist",
            "Refresh Now",
            "Automatic refresh: Off",
            "Refresh on startup: No",
            "Reorder playlist",
            "Playlist order: Custom order",
            "Reorder group",
            "Group order: Custom order",
        ]
    );
    step(app.press("id:playlist-menu-panel >> text:Reorder group"));
    step(app.wait_for("id:organization-screen >> text:Reorder groups"));
    step(app.wait_for("id:organization-row-0 >> text:News"));
    step(app.press("id:organization-screen >> text:Cancel"));
    step(app.open_guide_groups());
    step(app.right_click("id:guide-groups >> role:tab[Beta]"));
    step(app.press("id:playlist-menu-panel >> text:Group order: Custom order"));
    step(app.press("id:organization-sort >> text:Name (A–Z)"));
    step(app.wait_for_gone("id:organization-sort", Duration::from_secs(5)));
    step(app.open_guide_groups());
    step(app.right_click("id:guide-groups >> role:tab[Beta]"));
    step(app.wait_for("id:playlist-menu-panel >> text:Group order: Name (A–Z)"));
    step(app.back());
    playlists(&mut app);
    step(app.press("id:settings-content >> role:button[More options for Beta]"));
    step(app.wait_for(command));
    assert_eq!(
        app.panel_texts("playlist-actions-list"),
        [
            "Rename",
            "Delete",
            "Refresh now",
            "Hide",
            "Automatic refresh: Off",
            "Refresh on startup: No",
            "Reorder playlist",
        ]
    );
    step(app.press(command));
    order_screen(&mut app);
    // The heading chosen in Settings is focused. OK picks it up, Up moves it.
    step(app.press_key("enter"));
    step(app.press_key("up"));
    step(app.wait_for("id:organization-row-0 >> text:Beta"));
    step(app.press_key("enter"));
    step(app.back());
    step(app.press("text:Keep editing"));
    step(app.wait_for("id:organization-row-0 >> text:Beta"));
    step(app.back());
    step(app.press("text:Discard changes"));
    restored(&mut app);
    assert_eq!(persisted(), [alpha, beta, gamma]);

    // The guide offers reorder even while the list is sorted by name.
    step(app.right_click("id:guide-groups >> role:tab[Alpha]"));
    step(app.press("id:playlist-menu-panel >> text:Playlist order: Custom order"));
    step(app.wait_for("id:menu-panel >> text:Custom order"));
    step(app.press("id:menu-panel >> text:By name"));
    playlists(&mut app);
    assert!(!app.shows("id:settings-content >> text:Reorder playlists"));
    step(app.press("id:settings-content >> role:button[Close]"));
    step(app.open_guide_groups());

    // A failed save from the guide keeps the manual order draft available.
    step(app.right_click("id:guide-groups >> role:tab[Alpha]"));
    step(app.wait_for("id:playlist-menu-panel >> text:Playlist order: By name"));
    step(app.press(command));
    order_screen(&mut app);
    step(app.drag("id:reorder-handle-1", point(px(0.0), px(-55.0))));
    step(app.wait_for("id:organization-row-0 >> text:Beta"));
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .pragma_update(None, "key", format!("x'{}'", "54".repeat(32)))
        .unwrap();
    writer.execute_batch("CREATE TRIGGER fail_reorder BEFORE UPDATE OF position ON sources BEGIN SELECT RAISE(ABORT,'synthetic order failure'); END;").unwrap();
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for(
        "id:app-notice >> text:The change could not be saved. Reopen the list and try again.",
    ));
    assert_eq!(persisted(), [alpha, beta, gamma]);
    step(app.wait_for("id:organization-row-0 >> text:Beta"));
    writer.execute_batch("DROP TRIGGER fail_reorder").unwrap();

    let settings_path = dir.join("settings.json");
    let sort_is = |method| {
        move |settings: &iptv_settings::Settings| {
            settings.choice(
                iptv_settings::keys::PLAYLIST_SORT,
                &iptv_settings::Target::Global,
            ) == method
        }
    };
    crate::personalization::saved_settings(&mut app, &settings_path, sort_is("name"));
    let blocked_settings = settings_path.with_extension("json.tmp");
    std::fs::create_dir(&blocked_settings).unwrap();
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for(
        "id:app-notice >> text:Playlist positions saved. Could not switch to Custom order. Try saving again.",
    ));
    assert_eq!(persisted(), [beta, alpha, gamma]);
    step(app.wait_for("id:organization-row-0 >> text:Beta"));
    crate::personalization::saved_settings(&mut app, &settings_path, sort_is("name"));

    // Retrying only needs the settings write. A new position write would fail.
    writer.execute_batch("CREATE TRIGGER fail_reorder BEFORE UPDATE OF position ON sources BEGIN SELECT RAISE(ABORT,'synthetic order failure'); END;").unwrap();
    std::fs::remove_dir(&blocked_settings).unwrap();
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    writer.execute_batch("DROP TRIGGER fail_reorder").unwrap();
    crate::personalization::saved_settings(&mut app, &settings_path, sort_is("manual"));
    restored(&mut app);
    assert_eq!(persisted(), [beta, alpha, gamma]);
    step(app.right_click("id:guide-groups >> role:tab[Alpha]"));
    step(app.wait_for("id:playlist-menu-panel >> role:header[Alpha]"));
    step(app.wait_for("id:playlist-menu-panel >> text:Playlist order: Custom order"));
    step(app.back());
    step(app.right_click("id:guide-groups >> role:tab[All playlists]"));
    let position = |app: &Headless, name| {
        app.panel_texts("menu-panel")
            .iter()
            .position(|text| text == name)
            .unwrap()
    };
    assert!(position(&app, "Beta") < position(&app, "Alpha"));
    step(app.back());

    let alpha_heading = "id:guide-groups >> role:tab[Alpha]";
    step(app.right_click(alpha_heading));
    step(app.press("id:playlist-menu-panel >> text:Playlist order: Custom order"));
    step(app.press("id:menu-panel >> text:By date added"));
    step(app.right_click(alpha_heading));
    step(app.press("id:playlist-menu-panel >> text:Playlist order: By date added"));
    step(app.back());
    step(app.right_click("id:guide-groups >> role:tab[All playlists]"));
    assert!(position(&app, "Alpha") < position(&app, "Beta"));
    assert_eq!(persisted(), [beta, alpha, gamma]);
    step(app.back());
    step(app.right_click(alpha_heading));
    step(app.press("id:playlist-menu-panel >> text:Playlist order: By date added"));
    step(app.press("id:menu-panel >> text:Custom order"));
    restored(&mut app);

    // A TV remote can pick any heading even when it has no pointer.
    step(app.set_environment("tv", |env| env.idiom = Idiom::Tv));
    playlists(&mut app);
    step(app.press("id:settings-content >> text:Reorder playlists"));
    order_screen(&mut app);
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.back());
    step(app.wait_for("text:Save the new order?"));
    step(app.press_key("enter"));
    restored(&mut app);
    assert_eq!(persisted(), [alpha, beta, gamma]);
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
    step(app.wait_for_within("role:grid[All playlists]", Duration::from_secs(15)));
    step(app.wait_for("text:This group has no channels."));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 0);
    playlists(&mut app);
    // Its check box shows it again.
    step(app.press(&row("Alpha")));
    step(app.wait_for_gone("role:checkbox[/Disabled/]", Duration::from_secs(5)));
    step(app.back());
    step(app.back());
    step(app.wait_for_within("text:Only Alpha", Duration::from_secs(15)));
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
    step(app.open_guide_groups());
    let heading = "id:guide-groups >> role:tab[Alpha] >> text:▼";
    let delete = "id:playlist-menu-panel >> text:Delete playlist";
    let confirmation = "id:playlist-delete-confirmation";
    step(app.wait_for(heading));
    step(app.long_press(heading));
    step(app.press("id:playlist-menu-panel >> text:Reorder playlist"));
    step(app.wait_for("id:organization-row-0 >> id:reorder-handle-0"));
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for(heading));
    step(app.long_press(heading));
    step(app.press(delete));
    step(app.wait_for("id:playlist-delete-confirmation >> text:Delete “Alpha”?"));
    step(app.press_key("enter"));
    step(app.wait_for_gone(confirmation, Duration::from_secs(3)));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .source(source)
            .unwrap()
            .is_some()
    );
    step(app.press(delete));
    step(app.press("id:playlist-delete-confirmation >> text:Delete"));
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
    step(app.wait_for("id:settings-content >> text:Add a playlist"));
    assert_eq!(
        app.screen()
            .count("id:settings-content >> role:checkbox")
            .unwrap(),
        0
    );
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
    step(app.wait_for_within("text:Only Beta", Duration::from_secs(20)));
    playlists(&mut app);
    actions(&mut app, "Alpha");
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
                        && let Some(source) = value.as_ref().map(crate::support::upstream_source)
                        && let Some(url) = source["url"].as_str()
                    {
                        observed.lock().unwrap().push(url.into());
                    }
                }
            }
        }
    });
    playlists(&mut app);
    step(app.press("id:settings-content >> text:Show all playlists"));
    step(app.wait_for_within("text:Only Beta", Duration::from_secs(20)));
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
    step(app.wait_for_within("text:Only Beta", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.wait_for("id:guide-groups >> role:tab[Beta] >> text:▼"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        2
    );
    assert!(!app.shows("id:guide-groups >> role:tab[Disabled]"));
    // A parent previews only that playlist, and a tap folds its children.
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▶"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        1
    );
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    step(app.wait_for("text:Only Alpha"));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .collapsed_playlists()
            .unwrap(),
        [alpha.0]
    );
    // OK expands in place; Right enters that source's complete channel list.
    step(app.press_key("enter"));
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.press_key("right"));
    step(app.wait_for_gone("id:guide-groups", Duration::from_secs(3)));
    step(app.wait_for("role:grid[Alpha]"));
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> text:    News >> nth=1"));
    step(app.press("id:guide-groups >> text:    News >> nth=1"));
    step(app.wait_for("text:Only Beta"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    // Fold Alpha again, then move down: Beta's header is the very next row.
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.press_key("down"));
    step(app.press_key("right"));
    step(app.wait_for("role:grid[Beta]"));
    step(app.wait_for("text:Only Beta"));
    drop(app);
    // Expansion survives a fresh application, while all channels stay available.
    let mut app = start();
    step(app.wait_for_within("text:Only Beta", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▶"));
    step(app.wait_for("id:guide-groups >> role:tab[Beta] >> text:▼"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 4);
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        1
    );
}

/// A fold changes the next remote row before its preference can be saved.
/// Keeping a real writer transaction open makes the old asynchronous window
/// deterministic: Alpha's hidden child must never receive the following Down.
#[test]
fn playlist_section_navigation_does_not_wait_for_the_expansion_writer() {
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let alpha = seed(&mut store, home.path(), "Alpha");
    seed(&mut store, home.path(), "Beta");
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:Only Beta", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.wait_for("id:guide-groups >> role:tab[Beta] >> text:▼"));

    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .pragma_update(None, "key", format!("x'{}'", "54".repeat(32)))
        .unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    // These keys immediately follow the tap, just as on a fast remote.
    step(app.press_key("down"));
    step(app.press_key("right"));
    step(app.wait_for("role:grid[Beta]"));
    step(app.wait_for("text:Only Beta"));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    // More folds queue before any save can finish; each uses the immediately
    // preceding projection, and the final fold is the one persisted.
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▶"));
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▶"));
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.press_key("down"));
    step(app.press_key("right"));
    step(app.wait_for("role:grid[Beta]"));
    let saved: i64 = writer
        .query_row(
            "SELECT count(*) FROM store_meta WHERE key='collapsed_playlists'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        saved, 0,
        "navigation completed while the preference was unsaved"
    );
    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);
    step(crate::isolated::wait_outside(
        &mut app,
        "the expansion is saved after the writer is released",
        Duration::from_secs(10),
        || {
            crate::isolated::open_database(&path)
                .unwrap()
                .collapsed_playlists()
                .unwrap()
                == [alpha.0]
        },
    ));
    // The later save/rebuild must preserve Beta by identity, too.
    step(app.wait_for("role:grid[Beta]"));
    step(app.wait_for("text:Only Beta"));
}

#[test]
fn a_failed_expansion_save_preserves_a_newer_choice_and_recovers_the_last_failure() {
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    seed(&mut store, home.path(), "Alpha");
    seed(&mut store, home.path(), "Beta");
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:Only Beta", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .pragma_update(None, "key", format!("x'{}'", "54".repeat(32)))
        .unwrap();
    // Reject folds, but allow the newer expansion that follows one to save.
    writer
        .execute_batch(
            "CREATE TRIGGER generated_fold_failure BEFORE INSERT ON store_meta \
             WHEN NEW.key='collapsed_playlists' AND NEW.value<>'[]' \
             BEGIN SELECT RAISE(FAIL,'synthetic expansion failure'); END; \
             BEGIN IMMEDIATE;",
        )
        .unwrap();
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▶"));
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    writer.execute_batch("ROLLBACK").unwrap();
    step(crate::isolated::wait_outside(
        &mut app,
        "the newer expansion is saved despite the older fold failure",
        Duration::from_secs(10),
        || {
            writer
                .query_row(
                    "SELECT value FROM store_meta WHERE key='collapsed_playlists'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .is_ok_and(|saved| saved == "[]")
        },
    ));
    step(app.wait_for("text:The change could not be saved. Reopen the list and try again."));
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.wait_for("id:guide-groups >> role:tab[Beta] >> text:▼"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        2
    );

    // A last failed choice rolls back to persisted state, with no newer
    // choice for its recovery to overwrite.
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    step(app.press("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▶"));
    writer.execute_batch("ROLLBACK").unwrap();
    step(app.wait_for("id:guide-groups >> role:tab[Alpha] >> text:▼"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> text:    News")
            .unwrap(),
        2
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
    app.answer_picks_myself();
    step(app.wait_for_within("text:Only Alpha", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.press_key("m"));
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    // The form focuses its file field by identity. The request can arrive
    // after the key press settles; wait_for does not answer late picks.
    step(app.press_key("enter"));
    step(app.wait_until(
        "the second playlist file picker",
        Duration::from_secs(5),
        |screen| !screen.picks().is_empty(),
    ));
    step(app.answer_pick(vec![PickedFile::new(
        "Beta.m3u",
        "audio/x-mpegurl",
        ImageData::new(
            b"#EXTM3U\n#EXTINF:-1 group-title=\"Sport\",Added Beta\nhttp://stream.example.test/added-beta\n"
                .to_vec(),
        ),
    )]));
    step(app.wait_for("text:Beta.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Only Alpha", Duration::from_secs(20)));
    step(app.wait_for("text:Added Beta"));
    step(app.wait_for("role:grid[All playlists]"));
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
    crate::browse_fixture::open_add_playlist(&mut app);
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Xtream Codes login"));
    step(app.press("text:Xtream Codes login"));
    for (field, empty, value) in [
        ("Server", "http://server:port", server.as_str()),
        ("Username", "Required", "viewer"),
        ("Password", "Required", "secret"),
    ] {
        step(app.press(&format!("role:button[/^{field}$/]")));
        step(app.type_text(&format!("text:{empty}"), value));
        step(app.submit(&format!("text:{value}")));
    }
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("role:grid[All playlists]", Duration::from_secs(20)));
    step(app.wait_for("text:Imported Beta Two"));
    wait_for_requests(
        &mut app,
        "original guide download waiting",
        Duration::from_secs(10),
        &guides,
        1,
    );
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
                        && let Some(source) = value.as_ref().map(crate::support::upstream_source)
                        && let Some(url) = source["url"].as_str()
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
    step(app.press("text:Imported Beta One"));
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
    step(app.press("text:Imported Beta One"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    // Playlist parents in the picker expand their categories without closing
    // it or changing the guide's list. The source filter survives both toggles.
    let picker = "id:multiview-picker";
    step(app.press(&format!("{picker} >> role:tab[Alpha] >> text:▼")));
    step(app.wait_for(&format!("{picker} >> role:tab[Alpha] >> text:▶")));
    step(app.press(&format!("{picker} >> role:tab[Alpha] >> text:▶")));
    step(app.wait_for(&format!("{picker} >> role:tab[Alpha] >> text:▼")));
    step(app.press(&format!("{picker} >> text:/^    News$/ >> nth=0")));
    step(app.type_text("id:multiview-channel-search", "Imported Beta"));
    step(app.wait_for("text:No matching channels. Clear the search or choose another group."));
    step(app.press(&format!("{picker} >> text:All playlists")));
    step(app.wait_for(&format!("{picker} >> text:4  Imported Beta Two")));
    step(app.press(&format!("{picker} >> text:4  Imported Beta Two")));
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
    wait_for_requests(
        &mut app,
        "previous startup account request held",
        Duration::from_secs(10),
        &accounts,
        1,
    );
    crate::browse_fixture::open_add_playlist(&mut app);
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:Alpha.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("role:grid[All playlists]", Duration::from_secs(20)));
    step(app.wait_for_within("text:Imported Beta Two", Duration::from_secs(20)));
    assert_eq!(
        accounts.load(Ordering::SeqCst),
        2,
        "an unfinished previous import must be replaced, not retained as provisional cached rows"
    );
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    step(app.wait_for_within("text:Delayed Beta guide 1", Duration::from_secs(20)));
    step(app.wait_for("text:Added Alpha"));
    assert!(
        !app.screen()
            .matches("id:guide-row >> text:/^Cached Beta$/")
            .unwrap()
    );
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
        .set("epg.update", Value::choice("on_request"))
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
        step(app.open_guide_groups());
        step(app.press_key("left"));
        step(app.press_key("left"));
        step(app.wait_for("text:Settings >> nth=0"));
        step(app.press("text:Settings >> nth=0"));
        step(app.wait_for("text:TV guide data"));
        step(app.press("text:TV guide data"));
        step(app.press("text:Guide sources"));
        step(app.wait_for("text:Update the guide now"));
        step(app.press("text:Update the guide now"));
        wait_for_requests(
            &mut app,
            "explicit previous guide update held",
            Duration::from_secs(10),
            &feed_requests,
            2,
        );
        step(app.back());
        step(app.back());
        step(app.back());
    } else {
        playlists(&mut app);
        actions(&mut app, "Previous");
        step(app.press("text:Refresh now"));
        wait_for_requests(
            &mut app,
            "previous guide refresh held after playlist",
            Duration::from_secs(10),
            &feed_requests,
            2,
        );
        // RUNNING clears at Playlist Done, before this guide response is released.
        actions(&mut app, "Previous");
        step(app.wait_for_within("text:Refresh now", Duration::from_secs(10)));
        step(app.back());
        step(app.wait_for_gone("id:playlists-task", Duration::from_secs(5)));
        step(app.press("id:settings-content >> text:Close"));
    }
    crate::browse_fixture::open_add_playlist(&mut app);
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:Second.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for_within("role:grid[All playlists]", Duration::from_secs(20)));
    step(app.wait_for("text:Previous original guide"));
    step(app.wait_for("text:Second channel"));
    step(app.wait(Duration::from_millis(300)));
    assert_eq!(
        feed_requests.load(Ordering::SeqCst),
        2,
        "handoff must not start a competing EPG fetch"
    );
    let deleted = matches!(action, PreviousGuideAction::DeletePending);
    if deleted {
        playlists(&mut app);
        actions(&mut app, "Previous");
        step(app.press("text:Guide data"));
        step(app.wait_for("text:Delete stored guide data"));
        step(app.press("text:Delete stored guide data"));
        step(app.wait_for("text:/^Delete guide data for /"));
        step(app.press("id:guide-sources-screen >> text:Delete"));
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
    while app
        .screen()
        .count("id:guide-row >> text:Cached 39")
        .unwrap()
        == 0
        && std::time::Instant::now() < deadline
    {
        step(app.press_key("down"));
        app.idle(Duration::from_millis(10)).unwrap();
    }
    assert!(
        app.screen()
            .count("id:guide-row >> text:Cached 39")
            .unwrap()
            > 0,
        "the final cached source must hydrate while both refresh workers are stalled"
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let requests = server.join().unwrap();
    assert!(requests <= 2, "cached-only sources must not make requests");
}
