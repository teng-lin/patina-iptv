//! Multiview's tile menu on a TV, run headless with the TV idiom (keys
//! stand in for the remote): OK opens a panel at the right edge,
//! vertically centered, 208 dp wide with 40 dp rows, with no backdrop over
//! the tiles; its items in the reference's order and words, the focus the
//! light pill with dark text. Pause holds that tile alone (⏸ on it) and
//! Play resumes it; Search and add opens the search screen over the grid,
//! whose result becomes a new tile, and Back from it adds nothing.
//!
//! Synthetic data only: a generated playlist "picked" from a file.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::headless::Headless;
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const PLAYLIST: &str = "#EXTM3U
#EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One
http://stream.example.test/one.m3u8
#EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two
http://stream.example.test/two.m3u8
#EXTINF:-1 tvg-id=\"three\" group-title=\"Sport\",Test Channel Three
http://stream.example.test/three.m3u8
#EXTINF:-1 tvg-id=\"four\" group-title=\"Cinema\",Test Channel Four
http://stream.example.test/four.m3u8
";

const IMPORT: Duration = Duration::from_secs(20);
const TILE: &str = "id:multiview-tile";
const MENU: &str = "id:multiview-menu";
const PAUSED: &str = "id:multiview-paused";
/// A TV's window (dp).
const W: f32 = 960.0;
const H: f32 = 540.0;

fn tiles(app: &Headless) -> usize {
    app.screen().count(TILE).unwrap()
}

/// The menu panel's bounds (x, y, width, height).
fn panel(app: &Headless) -> (f32, f32, f32, f32) {
    let node = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some("multiview-menu"))
        .expect("the menu panel");
    let b = node.bounds;
    (b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0)
}

/// The panel's texts, top to bottom, with their tops.
fn items(app: &Headless) -> Vec<(String, f32)> {
    let nodes = app.screen().layout_nodes();
    let panel = nodes
        .iter()
        .find(|n| n.test_id.as_deref() == Some("multiview-menu"))
        .expect("the menu panel")
        .id;
    let parent: HashMap<_, _> = nodes.iter().map(|n| (n.id, n.parent)).collect();
    let inside = |mut id| {
        while let Some(&up) = parent.get(&id) {
            if up == panel {
                return true;
            }
            id = up;
        }
        false
    };
    let mut found: Vec<(String, f32)> = nodes
        .iter()
        .filter(|n| inside(n.id))
        .filter_map(|n| Some((n.text.as_ref()?.text.text.to_string(), n.bounds.origin.y.0)))
        .collect();
    found.sort_by(|a, b| a.1.total_cmp(&b.1));
    found
}

fn labels(app: &Headless) -> Vec<String> {
    items(app).into_iter().map(|(t, _)| t).collect()
}

fn color(app: &Headless, label: &str) -> nsh_core::Color {
    app.screen()
        .span_style(&format!("{MENU} >> text:{label}"))
        .unwrap()
        .color
}

/// A recycled row can still be mounted above the viewport. Check the visible
/// text bounds inside the picker, rather than matching its label alone.
fn picker_channel_visible(app: &Headless, label: &str) -> bool {
    let nodes = app.screen().layout_nodes();
    let list = nodes
        .iter()
        .find(|n| n.test_id.as_deref() == Some("multiview-picker-channels"))
        .expect("the picker's channel list");
    let parents: HashMap<_, _> = nodes.iter().map(|n| (n.id, n.parent)).collect();
    nodes.iter().any(|node| {
        if !node.text.as_ref().is_some_and(|t| t.text.text == label) {
            return false;
        }
        let mut id = node.id;
        while let Some(&parent) = parents.get(&id) {
            if parent == list.id {
                let top = node.bounds.origin.y.0;
                let bottom = top + node.bounds.size.height.0;
                let list_top = list.bounds.origin.y.0;
                return top >= list_top && bottom <= list_top + list.bounds.size.height.0;
            }
            id = parent;
        }
        false
    })
}

fn wait_picker_channel_visible(app: &mut Headless, label: &str, reason: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !picker_channel_visible(app, label) {
        assert!(Instant::now() < deadline, "{reason}");
        app.wait(Duration::from_millis(16)).unwrap();
    }
}

#[test]
fn changing_picker_group_or_query_reveals_selection_after_touch_scrolling() {
    let Some(_home) = isolated!() else { return };
    let mut playlist = String::from("#EXTM3U\n");
    for group in ["Alpha", "Beta"] {
        for channel in 0..60 {
            playlist.push_str(&format!(
                "#EXTINF:-1 tvg-id=\"{group}-{channel}\" group-title=\"{group}\",Shared {group} {channel:03}\nhttp://stream.example.test/{group}/{channel}.m3u8\n"
            ));
        }
    }
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    step(app.wait_for("id:wizard-welcome"));
    step(app.press("id:wizard-welcome-add"));
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for("text:Shared Alpha 000"));
    step(app.press("text:Shared Alpha 000"));
    step(app.wait_for("text:▶"));
    step(app.press("text:Shared Alpha 000"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));

    let picker = "id:multiview-picker";
    let channels = "id:multiview-picker-channels";
    step(app.wait_for(channels));
    step(app.press(&format!("{picker} >> text:Alpha")));
    assert!(picker_channel_visible(&app, "1  Shared Alpha 000"));
    step(app.scroll(channels, px(800.0)));
    assert!(!picker_channel_visible(&app, "1  Shared Alpha 000"));

    // Both groups have 60 rows and resolve selection to row zero. A different
    // group must reveal its selected channel even though that index is equal.
    step(app.press(&format!("{picker} >> text:Beta")));
    wait_picker_channel_visible(
        &mut app,
        "1  Shared Beta 000",
        "changing group must reveal its selected channel",
    );

    // This query keeps the same results, selected key, and selected row. A new
    // filter still returns a touch-scrolled list to that selected result.
    step(app.scroll(channels, px(800.0)));
    assert!(!picker_channel_visible(&app, "1  Shared Beta 000"));
    step(app.type_text("id:multiview-channel-search", "Shared"));
    wait_picker_channel_visible(
        &mut app,
        "1  Shared Beta 000",
        "changing query must reveal selection even when key and row stay equal",
    );
    assert_eq!(tiles(&app), 1, "browsing does not open another stream");
}

#[test]
fn the_tile_menu_is_a_side_panel_with_pause_and_search_and_add() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    // Four generated streams are allowed for this tile-menu fixture.
    settings
        .set_in(
            "playlists.max_connections",
            &iptv_settings::Target::playlist("1"),
            iptv_settings::Value::Int(4),
        )
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let config = EngineConfig {
        viewport: Size::new(px(W), px(H)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(|_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(PLAYLIST.as_bytes().to_vec()),
        )]
    });
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    let gone = |app: &mut Headless, s: &str| app.wait_for_gone(s, Duration::from_secs(2));

    step(app.wait_for("id:wizard-welcome"));
    step(app.press("id:wizard-welcome-add"));
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for("text:Test Channel One"));
    let store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    assert_eq!(
        store.sources().unwrap()[0].id.0,
        1,
        "fixture allowance source"
    );
    drop(store);

    // Multiview from full-screen video (the play bar's button: the
    // harness starts as a desktop), Channel One the first tile; then a
    // TV, the channel list picking the next tile by keys.
    step(app.press("text:Test Channel One"));
    step(app.wait_for("text:▶"));
    step(app.press("text:Test Channel One"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for(TILE));
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));
    // The harness's window size, again after the environment changed.
    step(app.set_viewport(Size::new(px(W), px(H))));
    step(app.wait_for("text:3  Test Channel Three"));
    // Add screen exposes groups and a search scoped to the chosen group.
    let picker = "id:multiview-picker";
    let field = "id:multiview-channel-search";
    step(app.wait_for(field));
    step(app.press(&format!("{picker} >> text:News")));
    step(gone(
        &mut app,
        &format!("{picker} >> text:3  Test Channel Three"),
    ));
    step(app.type_text(field, "tHrEe"));
    step(app.wait_for("text:No matching channels. Clear the search or choose another group."));
    assert_eq!(tiles(&app), 1, "filtering does not start streams");
    // The query remains while switching groups, with the group's original number.
    step(app.press(&format!("{picker} >> text:Sport")));
    step(app.wait_for(&format!("{picker} >> text:1  Test Channel Three")));
    step(app.press(&format!("{picker} >> text:Clear")));
    // From the group column, Right enters the matching channels; OK adds.
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(gone(&mut app, picker));
    step(app.wait_for(&format!("{TILE} >> text:/Test Channel Three$/")));
    assert_eq!(tiles(&app), 2);
    assert!(
        app.screen()
            .matches("id:multiview-ui >> role:button[/^Test Channel One$/]")
            .unwrap()
    );
    assert!(
        app.screen()
            .matches("id:multiview-ui >> role:button[/^Test Channel Three · muted$/]")
            .unwrap()
    );
    assert!(
        app.screen()
            .accessibility_state("id:multiview-ui >> role:button[/^Test Channel Three · muted$/]")
            .unwrap()
            .selected
    );
    step(
        app.wait_until("both synthetic tile sources are published", IMPORT, |_| {
            iptv_app::player_has_source(0) && iptv_app::player_has_source(1)
        }),
    );

    // OK: the panel at the right edge, centered, with the reference's items.
    step(app.press_key("enter"));
    step(app.wait_for(MENU));
    let (x, y, w, h) = panel(&app);
    assert!((w - 208.0).abs() < 0.5, "208 dp wide: {w}");
    assert!((W - (x + w) - 8.0).abs() < 0.5, "8 dp from the edge: {x}");
    assert!((y + h / 2.0 - H / 2.0).abs() < 1.0, "centered: {y} + {h}");
    assert_eq!(
        labels(&app),
        [
            "Add screen",
            "Search and add",
            "Change channel",
            "Pause",
            "Listen to this",
            "Enlarge screen",
            "Full screen",
            "Remove screen"
        ]
    );
    let rows: Vec<f32> = items(&app).iter().map(|i| i.1).collect();
    for pair in rows.windows(2) {
        assert!(
            (pair[1] - pair[0] - 40.0).abs() < 0.5,
            "40 dp rows: {rows:?}"
        );
    }
    // The focus: the light pill's dark text; the others the lists' light.
    let dark = nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff);
    let light = nsh_core::Color((iptv_app::theme::LIST_TEXT << 8) | 0xff);
    assert_eq!(color(&app, "Add screen"), dark);
    assert_eq!(color(&app, "Pause"), light);
    // No backdrop: the tiles are the only full views under the panel.
    let covering = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| {
            n.test_id.is_none()
                && n.bounds.origin.x.0 <= 0.0
                && n.bounds.size.width.0 >= W
                && n.bounds.size.height.0 >= H
        })
        .count();
    step(app.press_key("backspace"));
    step(gone(&mut app, MENU));
    let without = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| {
            n.test_id.is_none()
                && n.bounds.origin.x.0 <= 0.0
                && n.bounds.size.width.0 >= W
                && n.bounds.size.height.0 >= H
        })
        .count();
    assert_eq!(covering, without, "the menu adds nothing over the screen");

    // Pause (the fourth item): that tile alone shows ⏸; Play resumes it.
    step(app.press_key("enter"));
    for _ in 0..3 {
        step(app.press_key("down"));
    }
    assert_eq!(color(&app, "Pause"), dark);
    step(app.press_key("enter"));
    step(gone(&mut app, MENU));
    step(app.wait_for(&format!("{TILE} >> {PAUSED}")));
    assert_eq!(app.screen().count(PAUSED).unwrap(), 1, "one tile paused");
    step(app.press_key("enter"));
    step(app.wait_for(&format!("{MENU} >> text:Play")));
    assert_eq!(labels(&app)[3], "Play");
    for _ in 0..3 {
        step(app.press_key("down"));
    }
    step(app.press_key("enter"));
    step(gone(&mut app, PAUSED));

    // Search and add: the search screen over the grid; a result becomes
    // a new tile and multiview is back.
    step(app.press_key("enter"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("id:search-field"));
    step(app.wait_for("text:Search and add"));
    step(app.type_text("id:search-field", "Two"));
    step(app.wait_for("text:2   Test Channel Two"));
    step(app.press("text:2   Test Channel Two"));
    step(gone(&mut app, "id:search-field"));
    step(app.wait_for(&format!("{TILE} >> text:/Test Channel Two$/")));
    assert_eq!(tiles(&app), 3);

    // Back from it adds nothing: the grid as it was.
    step(app.press_key("enter"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("id:search-field"));
    // The platform Back closes the search while its input has focus.
    step(app.back());
    step(gone(&mut app, "id:search-field"));
    assert_eq!(tiles(&app), 3);
    assert!(!app.screen().matches(MENU).unwrap());
    // A single tile's menu would be shorter; here three of four: Remove
    // screen is last, Full screen before it.
    step(app.press_key("enter"));
    step(app.wait_for(MENU));
    let l = labels(&app);
    assert_eq!(&l[l.len() - 2..], ["Full screen", "Remove screen"]);

    // Replacement uses the same group/search picker and keeps the tile count.
    step(app.press(&format!("{MENU} >> text:Change channel")));
    step(app.wait_for(field));
    step(app.type_text(field, "Four"));
    step(app.wait_for(&format!("{picker} >> text:4  Test Channel Four")));
    step(app.press(&format!("{picker} >> text:4  Test Channel Four")));
    step(gone(&mut app, picker));
    step(app.wait_for(&format!("{TILE} >> text:/Test Channel Four$/")));
    assert_eq!(tiles(&app), 3);

    // Reopening starts unfiltered; closing a replacement keeps the old stream.
    step(app.press_key("enter"));
    step(app.wait_for(&format!("{MENU} >> text:Change channel")));
    step(app.press(&format!("{MENU} >> text:Change channel")));
    step(app.wait_for(&format!("{picker} >> text:1  Test Channel One")));
    step(app.type_text(field, "missing"));
    step(app.wait_for(&format!("{picker} >> text:Close")));
    step(app.press(&format!("{picker} >> text:Close")));
    step(gone(&mut app, picker));
    assert_eq!(tiles(&app), 3);
    step(app.wait_for(&format!("{TILE} >> text:/Test Channel Four$/")));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn closing_the_main_window_stops_grid_streams_and_keeps_pending_recordings() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&iptv_providers::model::Source {
            name: "Generated recording grid".into(),
            config: iptv_providers::model::SourceConfig::M3u(
                iptv_providers::model::M3uSourceConfig {
                    location: iptv_providers::SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                },
            ),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated One\nhttps://example.test/one.ts\n#EXTINF:-1 group-title=\"Generated group\",Generated Two\nhttps://example.test/two.ts\n",
    );
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    store.set_active_source(source).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let id = iptv_core::recording::JobId(471);
    store
        .schedule_recording(
            id,
            &iptv_core::recording::Request {
                source: source.0,
                channel: parsed.channels.items[0].id.as_str().into(),
                channel_name: "Generated One".into(),
                title: "Generated future program".into(),
                start: now + 3600,
                end: now + 7200,
            },
            now,
        )
        .unwrap();
    drop(store);
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("guide.autoplay", iptv_settings::Value::Bool(false))
        .unwrap();
    settings
        .set(
            "general.resume_last_channel",
            iptv_settings::Value::Bool(false),
        )
        .unwrap();
    settings
        .set_in(
            "playlists.max_connections",
            &iptv_settings::Target::playlist(source.0.to_string()),
            iptv_settings::Value::Int(3),
        )
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let mut config = EngineConfig::new(Root::of::<iptv_app::App>());
    config.viewport = Size::new(px(1280.0), px(720.0));
    config.environment.os = if cfg!(target_os = "macos") {
        nsh_core::platform::Os::MacOs
    } else {
        nsh_core::platform::Os::Linux
    };
    let mut app = Headless::start(config).unwrap();
    let props: std::sync::Arc<
        std::sync::Mutex<HashMap<nsh_core::NodeId, HashMap<String, serde_json::Value>>>,
    > = Default::default();
    let observed = props.clone();
    app.observe(move |_, batch| {
        let mut observed = observed.lock().unwrap();
        for item in &batch.items {
            match item {
                nsh_core::MountItem::ComponentProps { id, props } => {
                    let entry = observed.entry(*id).or_default();
                    for (name, value) in props {
                        entry.insert(
                            name.to_string(),
                            value.clone().unwrap_or(serde_json::Value::Null),
                        );
                    }
                }
                nsh_core::MountItem::Delete { id } => {
                    observed.remove(id);
                }
                _ => {}
            }
        }
    });
    let active = || {
        props
            .lock()
            .unwrap()
            .values()
            .filter(|p| {
                p.contains_key("popupSurface")
                    && p.get("source").is_some_and(|source| !source.is_null())
            })
            .count()
    };

    let step = |result: Result<(), String>| result.unwrap_or_else(|error| panic!("{error}"));
    step(app.wait_for("text:Generated One"));
    step(app.press("text:Generated One"));
    step(app.wait_for("text:▶"));
    step(app.press("text:Generated One"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for("id:multiview-picker"));
    step(app.press("id:multiview-picker >> text:2  Generated Two"));
    step(app.wait_until(
        "both generated grid streams",
        Duration::from_secs(10),
        |_| active() == 2,
    ));
    let primary = *props
        .lock()
        .unwrap()
        .iter()
        .find(|(_, p)| p.get("popupSurface") == Some(&serde_json::json!(0)))
        .unwrap()
        .0;
    step(app.press_window_close("main"));
    step(app.wait_until(
        "hidden grid streams released",
        Duration::from_secs(5),
        |_| active() == 0,
    ));
    assert!(
        props.lock().unwrap().contains_key(&primary),
        "the primary player surface remains mounted when the whole window hides"
    );
    assert!(!app.window_state("main").unwrap().unwrap().visible);
    let store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    assert_eq!(
        store.recording_job(id).unwrap().unwrap().status,
        iptv_core::recording::Status::Scheduled
    );
}

/// Account-backed locators need the source context on every surface. Checking
/// tile labels alone cannot establish that the decoder received a stream.
#[test]
fn an_idle_m3u_playlist_does_not_block_a_second_xtream_screen() {
    let Some(home) = isolated!() else { return };
    account_streams(&home, AccountScenario::IdleM3u);
}

#[test]
fn adding_a_screen_from_another_xtream_playlist_uses_its_account() {
    let Some(home) = isolated!() else { return };
    account_streams(&home, AccountScenario::OtherXtream);
}

#[test]
fn a_real_connection_limit_does_not_claim_the_channel_has_no_address() {
    let Some(home) = isolated!() else { return };
    account_streams(&home, AccountScenario::LimitedHttp);
}

#[test]
fn a_native_provider_retry_preserves_the_tile_during_a_capacity_refusal() {
    let Some(home) = isolated!() else { return };
    account_streams(&home, AccountScenario::LimitedNative);
}

#[derive(Clone, Copy)]
enum AccountScenario {
    IdleM3u,
    OtherXtream,
    LimitedHttp,
    LimitedNative,
}

const NATIVE_STREAM: &str = "rtsp://stream.example.test/generated-native";

fn generated_account() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let observed = observed.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                while reader.read_line(&mut String::new()).unwrap_or(0) > 2 {}
                let action = path
                    .split(['?', '&'])
                    .find_map(|s| s.strip_prefix("action="))
                    .unwrap_or("");
                let (status, body) = if path.starts_with("/live/") {
                    observed.lock().unwrap().push(path);
                    ("404 Not Found", "Generated native fallback".to_owned())
                } else if path.starts_with("/xmltv.php") {
                    ("200 OK", "<tv/>".to_owned())
                } else {
                    let body = match action {
                        "" | "get_account_info" => serde_json::json!({
                            "user_info": {"auth":1,"status":"Active","max_connections":"2","allowed_output_formats":["ts"]},
                            "server_info": {"timezone":"UTC"}
                        }),
                        "get_live_categories" => {
                            serde_json::json!([{"category_id":"1","category_name":"News"}])
                        }
                        "get_live_streams" => serde_json::json!([
                            {"name":"Test Channel One","stream_id":1,"stream_type":"live","category_id":"1"},
                            {"name":"Test Channel Two","stream_id":2,"stream_type":"live","category_id":"1"}
                        ]),
                        _ => serde_json::json!([]),
                    };
                    ("200 OK", body.to_string())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    (url, requests)
}

fn account_streams(home: &crate::isolated::Home, scenario: AccountScenario) {
    let another_playlist = matches!(scenario, AccountScenario::OtherXtream);
    let limited = matches!(
        scenario,
        AccountScenario::LimitedHttp | AccountScenario::LimitedNative
    );
    let native_primary = matches!(scenario, AccountScenario::LimitedNative);
    use iptv_providers::{
        Secret,
        model::{Source, SourceConfig, XtreamCredentials},
    };
    crate::browse_fixture::two_step_preview(home.path());
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let mut store = crate::isolated::open_database(home.data_dir().join("iptv.sqlite")).unwrap();
    let (url, requests) = generated_account();
    let source = store
        .add_source(&Source {
            name: "Alpha".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: url,
                username: Secret::new("generated-viewer"),
                password: Secret::new("generated-password"),
                user_agent: None,
            }),
        })
        .unwrap();
    if limited {
        let path = home.data_dir().join("settings.json");
        let (mut settings, _) =
            iptv_settings::Settings::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        settings
            .set_in(
                "playlists.max_connections",
                &iptv_settings::Target::playlist(source.0.to_string()),
                iptv_settings::Value::Int(1),
            )
            .unwrap();
        std::fs::write(path, settings.to_json_string()).unwrap();
    }
    store.set_active_source(source).unwrap();
    if !another_playlist {
        let idle = store
            .add_source(&Source {
                name: if native_primary {
                    "Native"
                } else {
                    "Idle generated playlist"
                }
                .into(),
                config: SourceConfig::M3u(iptv_providers::model::M3uSourceConfig {
                    location: iptv_providers::SecretUrl::new("https://example.test/idle.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let playlist = if native_primary {
            format!("#EXTM3U\n#EXTINF:-1,Native Channel\n{NATIVE_STREAM}\n")
        } else {
            "#EXTM3U\n#EXTINF:-1,Idle generated channel\nhttps://example.test/idle.ts\n".into()
        };
        let parsed = iptv_providers::m3u::parse_m3u(&playlist);
        store
            .replace_catalog(idle, &parsed.groups, &parsed.channels.items, &[], &[])
            .unwrap();
        store
            .set_source_policy(
                idle,
                iptv_store_sqlite::SourcePolicy {
                    refresh_on_start: false,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let other_requests = if another_playlist {
        let (url, requests) = generated_account();
        store
            .add_source(&Source {
                name: "Beta".into(),
                config: SourceConfig::Xtream(XtreamCredentials {
                    server_url: url,
                    username: Secret::new("generated-viewer"),
                    password: Secret::new("generated-password"),
                    user_agent: None,
                }),
            })
            .unwrap();
        Some(requests)
    } else {
        None
    };
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::now()),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    // A directly published RTSP source proves the fourth scenario reaches the
    // NativeProvider replacement path, rather than an HTTP relay/probe retry.
    let native_published = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed_native = native_published.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let nsh_core::MountItem::ComponentProps { props, .. } = item {
                for (name, value) in props {
                    if name.as_ref() == "source"
                        && value.as_ref().is_some_and(|v| v["url"] == NATIVE_STREAM)
                    {
                        observed_native.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
        }
    });
    let step = |result: Result<(), String>| result.unwrap_or_else(|error| panic!("{error}"));
    let first = if native_primary {
        "id:guide-row >> text:/^Native Channel$/"
    } else {
        "id:guide-row >> text:/^Test Channel One$/ >> nth=0"
    };
    step(app.wait_for_within(first, IMPORT));
    step(app.press(first));
    step(
        app.wait_until("primary account stream and native delivery", IMPORT, |_| {
            // Source calculation precedes delivery of the mount batch to the test
            // observer. Both acknowledgements must arrive before checking ownership.
            iptv_app::player_has_source(0)
                && (!native_primary || native_published.load(std::sync::atomic::Ordering::SeqCst))
        }),
    );
    step(app.press(first));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for("id:multiview-picker"));
    if another_playlist {
        step(app.press("id:multiview-picker >> role:tab[Beta]"));
    }
    step(app.press("id:multiview-picker >> text:/Test Channel Two$/"));
    if limited {
        // Give each asynchronous refusal time to reach the health machine
        // between advancing retry deadlines. A genuine limit stays retryable.
        for _ in 0..20 {
            step(app.run_for(Duration::from_secs(1)));
            step(app.wait(Duration::from_millis(20)));
        }
        step(app.wait_for("text:The provider is refusing this stream for now."));
        assert!(
            !app.screen()
                .matches("text:This channel has no stream address.")
                .unwrap()
        );
        assert!(iptv_app::player_has_source(0));
        assert!(!iptv_app::player_has_source(1));
        // Release the admitted screen; the refused tile must recover through
        // its scheduled retry without pressing Try again.
        step(app.press("id:multiview-tile >> nth=0"));
        step(app.press(if native_primary {
            "label:Remove screen: Native Channel"
        } else {
            "label:Remove screen: Test Channel One"
        }));
        for _ in 0..35 {
            step(app.run_for(Duration::from_secs(1)));
            step(app.wait(Duration::from_millis(20)));
            if iptv_app::player_has_source(1) {
                break;
            }
        }
        assert!(
            iptv_app::player_has_source(1),
            "the retry recovers after capacity is released"
        );
        return;
    }

    step(app.wait_until("second account stream", IMPORT, |_| {
        iptv_app::player_has_source(1)
    }));
    assert!(
        iptv_app::player_has_source(0),
        "adding a screen preserves the primary stream"
    );
    assert_eq!(tiles(&app), 2);
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with("/1.ts")),
        "the first channel resolves through its account"
    );
    assert!(
        other_requests
            .as_ref()
            .unwrap_or(&requests)
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with("/2.ts")),
        "the second channel resolves through its account"
    );
    assert!(
        !app.screen()
            .matches("text:This channel has no stream address.")
            .unwrap()
    );
}
