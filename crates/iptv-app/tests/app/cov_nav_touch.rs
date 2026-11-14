//! The app by pointer and touch, run headless: the search screen tapped
//! (results by number and name, the tabs, Clear and Close, a programme on
//! now played, one later shown in detail, one past replayed, nothing found)
//! with the recent searches hidden (`search.show_history` off); the
//! player's channel list tapped (its heading opens the groups, an entry
//! shows its list, a row plays, a tap beside it closes it); and multiview
//! by mouse and on a tablet (the pointer chrome's Add screen, a tile's
//! enlarge and remove buttons, taps on tiles and the menu's items, a tap
//! beside the menu closing it, a long press enlarging, Search and add with
//! its programme results, a tile's own buffering and failure states and
//! Try again, the device's limit, Exit multiview), and on an account that
//! allows one stream, the warning before adding a tile.
//!
//! Synthetic data only: a four-channel playlist picked from a file, its
//! guide a local XMLTV file around now, and a local Xtream Codes server.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use iptv_settings::{Settings, Value};
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::{
    EventKind, EventPayload, ImageData, Point, Size, ViewKind, element::Root, prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::json;

const IMPORT: Duration = Duration::from_secs(20);
const SLOT: i64 = 3600;
const FIELD: &str = "id:search-field";
const TILE: &str = "id:multiview-tile";
const MENU: &str = "id:multiview-menu";
const PICKER: &str = "id:multiview-picker";
const PAUSED: &str = "id:multiview-paused";
const GROUPS: &str = "id:channel-list-groups";
const WAIT: Duration = Duration::from_secs(3);
/// "Gala Evening"'s channel and times: its result's second line, and the
/// details popover's (mounted, empty, while closed).
// A time on another local day carries its weekday and date ("Thu 12 · 00:22"):
// late in the evening the programme two hours on ends, or starts, tomorrow.
const GALA: &str = "text:/^Test Channel One · ([A-Z][a-z]{2} [0-9]{1,2} · )?[0-9:]+–([A-Z][a-z]{2} [0-9]{1,2} · )?[0-9:]+$/";

/// As in `cov_nav.rs`: "Morning Talk" on now on channel one, "Recital
/// Replay" two hours back (replayable), "Gala Evening" two hours on;
/// channel two's "Other Now" and "Old Concert" (no replay).
fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv><channel id=\"one\"><display-name>One</display-name></channel>\
         <channel id=\"two\"><display-name>Two</display-name></channel>",
    );
    for k in -20i64..20 {
        let start = now - 25 * 60 + k * SLOT;
        let one = match k {
            0 => "Morning Talk".to_owned(),
            -2 => "Recital Replay".to_owned(),
            2 => "Gala Evening".to_owned(),
            _ => format!("Show {k}"),
        };
        let two = match k {
            0 => "Other Now".to_owned(),
            -2 => "Old Concert".to_owned(),
            _ => format!("Other {k}"),
        };
        for (channel, title) in [("one", one), ("two", two)] {
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
                format_xmltv_time(start),
                format_xmltv_time(start + SLOT),
            ));
        }
    }
    xml + "</tv>"
}

fn playlist(guide: &str) -> String {
    format!(
        "#EXTM3U url-tvg=\"{guide}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"1\",Test Channel One\nhttp://stream.example.test/one.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/two.m3u8\n\
         #EXTINF:-1 tvg-id=\"three\" group-title=\"Sport\",Test Channel Three\nhttp://stream.example.test/three.m3u8\n\
         #EXTINF:-1 tvg-id=\"four\" group-title=\"Sport\",Test Channel Four\nhttp://stream.example.test/four.m3u8\n"
    )
}

fn step(r: Result<(), String>) {
    r.unwrap_or_else(|e| panic!("{e}"));
}

fn gone(app: &mut Headless, selector: &str) {
    step(app.wait_for_gone(selector, Duration::from_secs(3)));
}

fn tiles(app: &Headless) -> usize {
    app.screen().count(TILE).unwrap()
}

#[test]
fn pointer_and_touch_drive_search_the_channel_list_and_multiview() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = playlist(&guide_path.display().to_string());
    let mut settings = Settings::new();
    // These journeys exercise the two-step preview and list interaction;
    // dedicated personalization tests cover the default close/delay policy.
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    settings
        .set("guide.stay_on_list", Value::Bool(true))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("search.show_history", Value::Bool(false))
        .unwrap();
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    // A search made before, kept though the screen hides it.
    std::fs::write(data.join("search-history.json"), "[\"weather\"]").unwrap();

    let config = EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    step(app.wait_for("text:No playlists yet"));
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Morning Talk", IMPORT));

    search_by_touch(&mut app, &data);
    channel_list_by_touch(&mut app);
    multiview_by_pointer(&mut app);
    multiview_on_a_tablet(&mut app);
    multiview_past_the_account(&mut app, now);
}

/// A local Xtream Codes server whose account allows one stream at once:
/// two channels, no films or series, an empty guide. Its base URL.
fn xtream_server(now: i64) -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let target = first.split(' ').nth(1).unwrap_or("/");
            let action = target
                .split(['?', '&'])
                .find_map(|p| p.strip_prefix("action="))
                .unwrap_or("");
            let body = if target.starts_with("/xmltv.php") {
                "<?xml version=\"1.0\"?><tv></tv>".to_owned()
            } else {
                match action {
                    "" | "get_account_info" => format!(
                        concat!(
                            r#"{{"user_info":{{"username":"viewer","password":"secret","auth":1,"#,
                            r#""status":"Active","exp_date":"{}","is_trial":"0","active_cons":"0","#,
                            r#""max_connections":"1","allowed_output_formats":["ts"]}},"#,
                            r#""server_info":{{"url":"127.0.0.1","port":"{port}","server_protocol":"http","#,
                            r#""timezone":"UTC","timestamp_now":{now}}}}}"#,
                        ),
                        now + 30 * 86_400,
                        // Named here: a format string built by `concat!`
                        // cannot capture variables by itself.
                        port = port,
                        now = now,
                    ),
                    "get_live_categories" => {
                        r#"[{"category_id":"1","category_name":"Synthetic","parent_id":0}]"#
                            .to_owned()
                    }
                    "get_live_streams" => {
                        let items: Vec<String> = (1..=2)
                            .map(|i| {
                                format!(
                                    r#"{{"num":{i},"name":"Xtream Channel {i}","stream_type":"live","stream_id":{i},"category_id":"1","tv_archive":0}}"#
                                )
                            })
                            .collect();
                        format!("[{}]", items.join(","))
                    }
                    _ => "[]".to_owned(),
                }
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Multiview on an Xtream account that allows one stream: adding a tile
/// asks first (the warning, centred over a dimmed screen); Cancel by keys,
/// a tap beside it, and Add it anyway.
fn multiview_past_the_account(app: &mut Headless, now: i64) {
    const WARNING: &str = "text:Your account allows 1 stream at once";
    let server = xtream_server(now);
    step(app.set_environment("a desktop", |e| e.idiom = Idiom::Desktop));
    step(app.set_viewport(Size::new(px(1280.0), px(720.0))));
    // Out of full screen (Exit multiview left it there).
    step(app.press_key("f"));
    step(app.wait_for("text:+ Add playlist"));
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
        // Return in the sheet's field: Done.
        step(app.submit(&format!("text:{value}")));
    }
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:/^Xtream Channel 1 · /", IMPORT));
    // Explicitly open the account alone: this journey exercises its
    // single-account multiview picker and stream-limit warning.
    step(app.press("text:☰  All playlists"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Playlists >> nth=0"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:127.0.0.1"));
    step(app.press("text:127.0.0.1"));
    step(app.wait_for("text:Show this playlist"));
    step(app.press("text:Show this playlist"));
    step(app.wait_for_within("text:Xtream Channel 1", IMPORT));
    // Refresh the account in single-source mode so its session limit is
    // known before opening multiview (a cached mode switch has no account fetch).
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Playlists >> nth=0"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:127.0.0.1"));
    step(app.press("text:127.0.0.1"));
    step(app.wait_for("text:Refresh now"));
    step(app.press("text:Refresh now"));
    step(app.wait_for("text:127.0.0.1"));
    step(app.press("text:127.0.0.1"));
    step(app.wait_for_within("text:Refresh now", IMPORT));
    step(app.back());
    step(app.wait_for("text:Close"));
    step(app.press("text:Close"));
    // Channel 1 full screen; Multiview asks at once.
    step(app.press("text:Xtream Channel 1"));
    step(app.press("text:Xtream Channel 1"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for(WARNING));
    step(app.wait_for("text:Add it anyway"));
    // Keys: Left to Add it anyway, Right back to Cancel, OK cancels.
    step(app.press_key("left"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    gone(app, WARNING);
    assert_eq!(tiles(app), 1);
    // The menu's Add screen asks again; a tap beside the dialog closes it.
    step(app.press(&tile("Xtream Channel 1")));
    step(app.wait_for(&format!("{MENU} >> text:Add screen")));
    step(app.press(&format!("{MENU} >> text:Add screen")));
    step(app.wait_for(WARNING));
    step(app.press_at("label:Video", Point::new(px(4.0), px(4.0))));
    gone(app, WARNING);
    // Add it anyway: the list picks the second tile; no more asking.
    step(app.press(&tile("Xtream Channel 1")));
    step(app.wait_for(&format!("{MENU} >> text:Add screen")));
    step(app.press(&format!("{MENU} >> text:Add screen")));
    step(app.wait_for("text:Add it anyway"));
    step(app.press("text:Add it anyway"));
    gone(app, WARNING);
    step(app.press(&format!("{PICKER} >> text:/^2  Xtream Channel 2$/")));
    step(app.wait_for(&tile("Xtream Channel 2")));
    assert_eq!(tiles(app), 2);
}

fn open_search(app: &mut Headless) {
    step(app.press_key("/"));
    step(app.wait_for(FIELD));
}

fn search_by_touch(app: &mut Headless, data: &std::path::Path) {
    open_search(app);
    // The hidden history: no heading, no rows.
    assert!(!app.screen().matches("text:weather").unwrap());
    // By number: channel two, then its name; Clear empties the query.
    step(app.type_text(FIELD, "2"));
    step(app.wait_for("text:2   Test Channel Two"));
    step(app.press("text:Clear"));
    gone(app, "text:2   Test Channel Two");
    // Nothing found; the search submitted (Return) is remembered.
    step(app.type_text(FIELD, "Zebra"));
    step(app.wait_for("text:Nothing found for “Zebra”"));
    step(app.submit(FIELD));
    step(app.wait_for("text:Close"));
    step(app.press("text:Close"));
    gone(app, FIELD);
    // The new query went on top of the saved ones, which an older build's
    // file had held: they are in the store now, and the file is gone.
    assert!(!data.join("search-history.json").exists());
    let store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let deadline =
        std::time::Instant::now() + crate::isolated::scaled_duration(Duration::from_secs(10));
    loop {
        let saved = store.search_history().unwrap();
        if saved == ["Zebra", "weather"] {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "saved: {saved:?}");
        step(app.wait(Duration::from_millis(50)));
    }

    // A programme later (a channel matches too: the programmes' tab
    // tapped): its details over the screen; Back closes them.
    open_search(app);
    step(app.type_text(FIELD, "Gala"));
    step(app.wait_for("text:/^Programmes · 1$/"));
    step(app.press("text:/^Programmes · 1$/"));
    step(app.wait_for("text:Later"));
    step(app.press("text:Later"));
    step(app.wait_until("details", WAIT, |s| s.count(GALA) == Ok(2)));
    step(app.back());
    step(app.wait_until("details closed", WAIT, |s| s.count(GALA) == Ok(1)));
    step(app.press("text:Close"));
    gone(app, FIELD);

    // A programme on now: it plays full screen and the screen closes.
    open_search(app);
    step(app.type_text(FIELD, "Morning"));
    step(app.wait_for("text:On now · ↺ replay"));
    step(app.press("text:On now · ↺ replay"));
    gone(app, FIELD);
    // Its channel a favourite (the play bar's ☆).
    step(app.press("label:Video"));
    step(app.wait_for("label:Favourite"));
    step(app.press("label:Favourite"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    gone(app, "label:Mute");

    // A past programme with replay plays from its start.
    open_search(app);
    step(app.type_text(FIELD, "Recital"));
    step(app.wait_for("text:Earlier · ↺ replay"));
    step(app.press("text:Earlier · ↺ replay"));
    gone(app, FIELD);
    // Searching the playing channel: a favourite, "Playing" under it.
    open_search(app);
    step(app.type_text(FIELD, "Test Channel One"));
    step(app.wait_for("text:1   ★ Test Channel One"));
    step(app.wait_for("text:Playing"));
    step(app.press("text:Close"));
    gone(app, FIELD);
}

/// The list over full-screen video, tapped.
fn channel_list_by_touch(app: &mut Headless) {
    const BESIDE: &str = "id:channel-list-beside";
    // A replay plays (Left would seek): the control strip's Channels.
    step(app.press_key("f"));
    step(app.press_key("m"));
    step(app.wait_for("text:Channels"));
    step(app.press("text:Channels"));
    // Its heading opens the groups beside it; an entry shows its list.
    step(app.press("text:◂  All channels"));
    gone(app, "text:◂  All channels");
    step(app.press(&format!("{GROUPS} >> text:Sport")));
    step(app.wait_for("text:/^1  Test Channel Three$/"));
    // A row plays its channel; the list stays.
    step(app.press("text:/^2  Test Channel Four$/"));
    step(app.wait_for("text:2  Test Channel Four  ▶"));
    // All channels again, from the groups (the row's tap closed them).
    step(app.press("text:◂  Sport"));
    step(app.wait_for(&format!("{GROUPS} >> text:All channels")));
    step(app.press(&format!("{GROUPS} >> text:All channels")));
    step(app.wait_for("text:4  Test Channel Four  ▶"));
    // A tap beside the list closes it: Up then zaps (in the list it would
    // move its focus).
    step(app.press(BESIDE));
    step(app.press_key("up"));
    step(app.wait_for("text:3  Test Channel Three  ▶"));
}

/// The tile whose channel line ends in `name`.
fn tile(name: &str) -> String {
    format!("{TILE} >> text:/{name}$/")
}

/// Sends the player under the tile showing `name` a native event, as the
/// host's player does.
fn player_event(app: &mut Headless, name: &str, event: &str, data: serde_json::Value) {
    let nodes = app.screen().layout_nodes();
    let parent: std::collections::HashMap<_, _> = nodes.iter().map(|n| (n.id, n.parent)).collect();
    let within = |mut id, tile| {
        while let Some(&up) = parent.get(&id) {
            if up == tile {
                return true;
            }
            if up == id {
                return false;
            }
            id = up;
        }
        false
    };
    let tile = nodes
        .iter()
        .filter(|n| n.test_id.as_deref() == Some("multiview-tile"))
        .find(|t| {
            nodes.iter().any(|n| {
                n.text.as_ref().is_some_and(|x| x.text.text.ends_with(name)) && within(n.id, t.id)
            })
        })
        .unwrap_or_else(|| panic!("{name}'s tile"))
        .bounds;
    let player = nodes
        .iter()
        .find(|n| n.kind == ViewKind::Native && n.bounds == tile)
        .unwrap_or_else(|| panic!("the player under {name}'s tile"))
        .id;
    app.engine().control().dispatch_event(
        player,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: event.into(),
            data,
        },
    );
}

/// Multiview by mouse: the pointer chrome, taps on tiles and menu items,
/// a tile's own waiting and failure states.
fn multiview_by_pointer(app: &mut Headless) {
    // Channel three plays: the first tile; channel two the next.
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for(TILE));
    step(app.press(&format!("{PICKER} >> text:/^2  Test Channel Two$/")));
    step(app.wait_for(&tile("Test Channel Two")));
    assert_eq!(tiles(app), 2);
    // A tap on the other tile makes it active and shows the chrome: the
    // toolbar's Add screen adds one.
    step(app.press(&tile("Test Channel Three")));
    step(app.wait_for("label:Add screen"));
    step(app.press("label:Add screen"));
    step(app.wait_for(&format!("{PICKER} >> text:/^4  Test Channel Four$/")));
    step(app.press(&format!("{PICKER} >> text:/^4  Test Channel Four$/")));
    step(app.wait_for(&tile("Test Channel Four")));
    assert_eq!(tiles(app), 3);
    // A tile's enlarge button enlarges it, then reduces it.
    step(app.press(&tile("Test Channel Two")));
    step(app.wait_for("label:Enlarge screen: Test Channel Four"));
    step(app.press("label:Enlarge screen: Test Channel Four"));
    step(app.wait_for("label:Reduce screen: Test Channel Four"));
    step(app.press("label:Reduce screen: Test Channel Four"));
    step(app.wait_for("label:Enlarge screen: Test Channel Four"));
    // A long press does the same.
    step(app.long_press(&tile("Test Channel Three")));
    step(app.wait_for(&tile("Test Channel Two")));
    step(app.press(&tile("Test Channel Two")));
    step(app.wait_for("label:Reduce screen: Test Channel Three"));
    step(app.long_press(&tile("Test Channel Three")));
    step(app.wait_for(&tile("Test Channel Two")));
    step(app.press(&tile("Test Channel Two")));
    step(app.wait_for("label:Enlarge screen: Test Channel Three"));
    // A remove button removes its tile (two is active now).
    step(app.press("label:Remove screen: Test Channel Four"));
    step(app.wait_until("two tiles", WAIT, |s| s.count(TILE) == Ok(2)));

    // A tap on the active tile opens its menu; a tap on Pause pauses it;
    // a tap beside the menu closes it.
    step(app.press(&tile("Test Channel Two")));
    step(app.wait_for(MENU));
    step(app.press(&format!("{MENU} >> text:Pause")));
    step(app.wait_for(&format!("{TILE} >> {PAUSED}")));
    step(app.press(&tile("Test Channel Two")));
    step(app.wait_for(MENU));
    step(app.press_at("label:Video", Point::new(px(4.0), px(4.0))));
    gone(app, MENU);
    // Search and add by tap: the programmes are searched too; a channel
    // result becomes a tile.
    step(app.press(&tile("Test Channel Two")));
    step(app.wait_for(MENU));
    step(app.press(&format!("{MENU} >> text:Search and add")));
    step(app.wait_for(FIELD));
    step(app.type_text(FIELD, "Gala"));
    step(app.wait_for("text:/^Programmes · 1$/"));
    step(app.wait_for("text:Tap a result to add its channel to multiview"));
    step(app.clear(FIELD));
    step(app.type_text(FIELD, "Four"));
    step(app.wait_for("text:4   Test Channel Four"));
    step(app.press("text:4   Test Channel Four"));
    gone(app, FIELD);
    step(app.wait_for(&tile("Test Channel Four")));

    // A tile's player buffering: its own label; then a stream the provider
    // does not have: its message and Try again, which opens it again.
    // (Until their players report, the tiles say they are loading.)
    let loading = format!("{TILE} >> text:Loading…");
    assert_eq!(app.screen().count(&loading), Ok(3));
    player_event(
        app,
        "Test Channel Four",
        "state",
        json!({ "state": "ready" }),
    );
    step(app.wait_until("four plays", WAIT, |s| s.count(&loading) == Ok(2)));
    let buffering = format!("{TILE} >> text:Buffering…");
    player_event(
        app,
        "Test Channel Four",
        "state",
        json!({ "state": "buffering" }),
    );
    step(app.wait_for(&buffering));
    player_event(
        app,
        "Test Channel Four",
        "state",
        json!({ "state": "ready" }),
    );
    gone(app, &buffering);
    let missing = format!("{TILE} >> text:The provider doesn't have this stream.");
    let error = json!({ "code": "ERROR_CODE_IO_BAD_HTTP_STATUS", "http": 404 });
    player_event(app, "Test Channel Four", "error", error.clone());
    step(app.wait_for(&missing));
    step(app.press(&format!("{TILE} >> text:Try again")));
    gone(app, &missing);
    // Failed again: the tile's menu offers Try again first.
    player_event(app, "Test Channel Four", "error", error);
    step(app.wait_for(&missing));
    step(app.press(&tile("Test Channel Four")));
    step(app.wait_for(&format!("{MENU} >> text:Try again")));
    step(app.press(&format!("{MENU} >> text:Try again")));
    gone(app, &missing);

    // A fourth: the toolbar names the device's limit instead of Add screen.
    step(app.press(&tile("Test Channel Three")));
    step(app.wait_for("label:Add screen"));
    step(app.press("label:Add screen"));
    step(app.wait_for(&format!("{PICKER} >> text:/^1  Test Channel One$/")));
    step(app.press(&format!("{PICKER} >> text:/^1  Test Channel One$/")));
    step(app.wait_for(&tile("Test Channel One")));
    step(app.press(&tile("Test Channel Two")));
    step(app.wait_for("text:This device plays up to 4 at once"));
    assert!(!app.screen().matches("label:Add screen").unwrap());
    // Exit multiview: the active tile (two) full screen, alone.
    step(app.press("label:Exit multiview"));
    gone(app, TILE);
}

/// Multiview on a tablet: a finger's larger buttons.
fn multiview_on_a_tablet(app: &mut Headless) {
    step(app.set_environment("a tablet", |e| e.idiom = Idiom::Pad));
    step(app.set_viewport(Size::new(px(1280.0), px(800.0))));
    // A tap shows the play bar (unless it still showed: then a second).
    step(app.press("label:Video"));
    if app
        .screen()
        .press_target("label:Multiview")
        .ok()
        .flatten()
        .is_none()
    {
        step(app.press("label:Video"));
    }
    step(app.press("label:Multiview"));
    step(app.wait_for(TILE));
    step(app.press(&format!("{PICKER} >> text:/^3  Test Channel Three$/")));
    step(app.wait_for(&tile("Test Channel Three")));
    step(app.press(&tile("Test Channel Two")));
    const REMOVE: &str = "label:Remove screen: Test Channel Three";
    step(app.wait_for(REMOVE));
    let id = app.screen().press_target(REMOVE).unwrap().unwrap();
    let side = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.id == id)
        .map(|n| n.bounds.size.width.0)
        .unwrap();
    assert!((side - 44.0).abs() < 0.5, "a finger's target: {side}");
    step(app.press("label:Exit multiview"));
    gone(app, TILE);
}
