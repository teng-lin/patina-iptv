//! The app driven by keys alone, run headless: a desktop keyboard in the
//! guide (arrows, Page Up/Down, Ch+/Ch-, Tab, Menu, Info), the search
//! screen (typing, digits, the tabs, a live result, an upcoming one's
//! details, a past one replayed, the recent searches, and the screen
//! coming back after watching with `search.stay_on_search`), the player's
//! channel list over and beside the video (its groups column, paging,
//! zapping, an empty Favourites list), and multiview on a TV (the arrows
//! between tiles, its menu's Add screen, Change channel, Enlarge, Full
//! screen and back, Remove; Back leaving to the active tile, a zap or the
//! guide ending a detour, the key hint timing out).
//!
//! Synthetic data only: a four-channel playlist picked from a file, its
//! guide a local XMLTV file around now.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use iptv_settings::{Settings, Value};
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(3);
const SLOT: i64 = 3600;
const FIELD: &str = "id:search-field";
const TILE: &str = "id:multiview-tile";
const MENU: &str = "id:multiview-menu";
/// A programme's channel and times, as the details popover (and a search
/// result) shows them.
// A time on another local day carries its weekday and date ("Thu 12 · 00:22"):
// late in the evening the programme two hours on ends, or starts, tomorrow.
const TIMES: &str = "text:/^Test Channel One · ([A-Z][a-z]{2} [0-9]{1,2} · )?[0-9:]+ [AP]M–([A-Z][a-z]{2} [0-9]{1,2} · )?[0-9:]+ [AP]M$/";

/// How many lines of channel one's programme times show: the details
/// popover adds one (it stays mounted, empty, while closed).
fn times(app: &Headless) -> usize {
    app.screen().count(TIMES).unwrap()
}

/// Waits until `n` such lines show.
fn wait_times(app: &mut Headless, n: usize) {
    step(
        app.wait_until(&format!("{n} time lines"), Duration::from_secs(3), |s| {
            s.count(TIMES) == Ok(n)
        }),
    );
}

/// Channel one's programmes are "Show <n>" and channel two's "Other <n>",
/// `n` counting hours from the one on air (0), with a few named ones:
/// on now "Morning Talk" and "Other Now", two hours back "Recital
/// Replay" (channel one replays a day) and "Old Concert" (channel two
/// replays nothing), two hours on "Gala Evening".
fn guide(now: i64) -> String {
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for (id, name) in [("one", "One"), ("two", "Two")] {
        xml.push_str(&format!(
            "<channel id=\"{id}\"><display-name>{name}</display-name></channel>"
        ));
    }
    for k in -20i64..20 {
        let start = now - 25 * 60 + k * SLOT;
        for (channel, title) in [("one", one(k)), ("two", two(k))] {
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
                format_xmltv_time(start),
                format_xmltv_time(start + SLOT),
            ));
        }
    }
    xml + "</tv>"
}

fn one(k: i64) -> String {
    match k {
        0 => "Morning Talk".into(),
        -2 => "Recital Replay".into(),
        2 => "Gala Evening".into(),
        _ => format!("Show {k}"),
    }
}

fn two(k: i64) -> String {
    match k {
        0 => "Other Now".into(),
        -2 => "Old Concert".into(),
        _ => format!("Other {k}"),
    }
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

/// Whether `title` has the focus: the header repeats it over its cell.
fn focused(app: &Headless, title: &str) -> bool {
    app.screen().texts().iter().filter(|t| **t == title).count() >= 2
}

fn step(r: Result<(), String>) {
    r.unwrap_or_else(|e| panic!("{e}"));
}

fn gone(app: &mut Headless, selector: &str) {
    step(app.wait_for_gone(selector, Duration::from_secs(3)));
}

/// Back until the search screen closes (the first may only take the
/// keyboard down).
fn close_search(app: &mut Headless) {
    for _ in 0..3 {
        if !app.screen().matches(FIELD).unwrap() {
            break;
        }
        step(app.back());
        step(app.wait(Duration::from_millis(100)));
    }
    gone(app, FIELD);
}

/// Waits until `title` has the focus.
fn wait_focused(app: &mut Headless, title: &str) {
    let t = title.to_owned();
    step(
        app.wait_until(&format!("{title} focused"), Duration::from_secs(3), |s| {
            s.texts().iter().filter(|x| **x == t).count() >= 2
        }),
    );
}

#[test]
fn keys_drive_the_guide_search_the_channel_list_and_multiview() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = playlist(&guide_path.display().to_string());
    // Search comes back after watching a result.
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
        .set("search.stay_on_search", Value::Bool(true))
        .unwrap();
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();

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

    guide_keys(&mut app);
    search_keys(&mut app);
    channel_list_keys(&mut app);
    multiview_keys(&mut app);
}

/// The guide by keyboard: the first key after the pointer starts from a
/// row on screen; arrows, pages, Ch+/Ch-, Info and Menu.
fn guide_keys(app: &mut Headless) {
    // Down from channel one: channel two's programme on now.
    step(app.press_key("down"));
    wait_focused(app, "Other Now");
    // Page Down: by the rows on screen, at most to the last channel;
    // Page Up back to the top.
    step(app.press_key("pagedown"));
    step(
        app.wait_until("off channel two", Duration::from_secs(3), |s| {
            s.texts().iter().filter(|x| **x == "Other Now").count() < 2
        }),
    );
    step(app.press_key("pageup"));
    wait_focused(app, "Morning Talk");
    // Ch- / Ch+ page the guide's rows too.
    step(app.press_key("-"));
    step(app.press_key("="));
    wait_focused(app, "Morning Talk");
    // Info is not the guide's: nothing changes.
    step(app.press_key("i"));
    assert!(focused(app, "Morning Talk"));
    // OK on a programme still to come does nothing (no reminders yet).
    step(app.press_key("right"));
    wait_focused(app, "Show 1");
    step(app.press_key("enter"));
    step(app.wait(Duration::from_millis(200)));
    assert!(focused(app, "Show 1") && app.shows("Nothing playing"));
    step(app.press_key("home"));
    wait_focused(app, "Morning Talk");
    // Menu: the programme's menu; Menu again closes it.
    step(app.press_key("m"));
    step(app.wait_for("text:Details"));
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("m"));
    gone(app, "text:Details");
    // Menu, then OK on its first item: the search screen.
    step(app.press_key("m"));
    step(app.wait_for("text:Details"));
    step(app.press_key("enter"));
    step(app.wait_for(FIELD));
    gone(app, "text:Details");
    close_search(app);
    // Its Details (tapped): the popover, which Menu closes; Back too.
    step(app.press_key("m"));
    step(app.wait_for("text:Details"));
    step(app.press("text:Details"));
    wait_times(app, 1);
    step(app.press_key("down"));
    step(app.press_key("m"));
    wait_times(app, 0);
    step(app.press_key("m"));
    step(app.wait_for("text:Details"));
    step(app.press("text:Details"));
    wait_times(app, 1);
    step(app.press_key("backspace"));
    wait_times(app, 0);
    wait_focused(app, "Morning Talk");
    // G (the guide key) keeps the guide.
    step(app.press_key("g"));
    wait_focused(app, "Morning Talk");

    // Tab: the groups; Menu there adds a playlist (Back closes the wizard).
    step(app.press_key("tab"));
    step(app.press_key("m"));
    step(app.wait_for("text:Playlist file"));
    step(app.back());
    gone(app, "text:Playlist file");
    // Down: the News group shows; OK takes it, with the keys' hint card,
    // which goes by itself (6 s): then Back goes to the groups at once,
    // Back again to the rail on TV, Up to Search, OK searches.
    step(app.press_key("down"));
    step(app.wait_for("text:☰  News"));
    step(app.press_key("enter"));
    step(app.wait(Duration::from_millis(6500)));
    step(app.press_key("backspace"));
    step(app.press_key("backspace"));
    // The rail (on TV): Menu does nothing there; Up to Search, OK opens it
    // (were the hint still up, the focus would be in the groups and Up
    // would show All channels).
    step(app.press_key("m"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for(FIELD));
    assert!(app.screen().matches("text:☰  News").unwrap());
    close_search(app);
    // From Search down past TV (the disabled items are skipped) to
    // Playlists, then Settings: each opens its screen.
    const PLAYLISTS: &str = "text:▲▼ choose   OK actions   Back close";
    const SETTINGS: &str = "text:▲▼ page   ▶ OK its settings   Back close";
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for(PLAYLISTS));
    // The screen takes the keys: OK opens the playlist's actions (their
    // rows the kit's), Back closes them, then the screen.
    const ACTIONS: &str = "text:▲▼ choose   OK do it   Back close the sheet";
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for(ACTIONS));
    step(app.wait_for("text:Refresh now"));
    step(app.press_key("down"));
    step(app.back());
    gone(app, ACTIONS);
    step(app.back());
    gone(app, PLAYLISTS);
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for(SETTINGS));
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.back());
    gone(app, SETTINGS);
    // Up to TV, OK: the groups; Up shows All channels, OK takes it (the
    // hint again, which the next key but Back dismisses).
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.press_key("up"));
    step(app.wait_for("text:☰  All channels"));
    step(app.press_key("enter"));
    step(app.press_key("right"));
    step(app.press_key("home"));
    step(app.press_key("pageup"));
    wait_focused(app, "Morning Talk");

    // The menu's Manage favourites: the editor takes the keys. OK adds the
    // focused channel, OK again removes it; Left (remove all) with none
    // changes nothing; Right adds all, Left removes them; Back closes.
    const STAR: &str = "text:★ Test Channel Four";
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favourites…"));
    step(app.press("text:Manage favourites…"));
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("text:★ Test Channel One"));
    step(app.press_key("enter"));
    gone(app, "text:★ Test Channel One");
    step(app.press_key("left"));
    step(app.press_key("right"));
    step(app.wait_for(STAR));
    step(app.press_key("left"));
    gone(app, STAR);
    step(app.press_key("backspace"));
    wait_focused(app, "Morning Talk");
}

/// Search by keys, with `search.stay_on_search`: watching a result keeps
/// the screen, which comes back when full screen is left.
fn search_keys(app: &mut Headless) {
    step(app.press_key("/"));
    step(app.wait_for(FIELD));
    step(app.wait_for("text:No recent searches yet"));
    // An upcoming programme (the only result: its tab shows): Back takes
    // the keyboard down, Down to the tabs, Right (no other tab with
    // results), Down to it, OK: its details.
    step(app.type_text(FIELD, "Gala"));
    step(app.wait_for("text:/^Programmes · 1$/"));
    step(app.back());
    step(app.press_key("down"));
    step(app.press_key("right"));
    step(app.press_key("down"));
    assert_eq!(times(app), 1, "the result's line");
    step(app.press_key("enter"));
    wait_times(app, 2);
    step(app.press_key("backspace"));
    wait_times(app, 1);
    assert!(app.screen().matches(FIELD).unwrap(), "search stays");
    // A digit pressed on a result goes into the query: nothing found.
    step(app.press_key("1"));
    step(app.wait_for("text:Nothing found for “Gala1”"));
    // A past programme with replay: played from its start, full screen;
    // leaving full screen brings the screen back with its query.
    step(app.clear(FIELD));
    step(app.type_text(FIELD, "Recital"));
    step(app.wait_for("text:Earlier · ↺ replay"));
    step(app.back());
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    gone(app, FIELD);
    step(app.press_key("o"));
    step(app.wait_for(FIELD));
    step(app.wait_for("text:Recital Replay"));
    // A channel by its number (programmes match it too; the programmes'
    // tab stays chosen): Right and Left switch the tabs; the channel plays
    // live. Search over the video brings the waiting screen back at once.
    step(app.clear(FIELD));
    step(app.type_text(FIELD, "3"));
    step(app.wait_for("text:/^Channels · 1$/"));
    step(app.wait_for("text:Show 3"));
    step(app.back());
    step(app.press_key("down"));
    step(app.press_key("right"));
    step(app.wait_for("text:3   Test Channel Three"));
    step(app.press_key("left"));
    step(app.wait_for("text:Show 3"));
    step(app.press_key("right"));
    step(app.wait_for("text:3   Test Channel Three"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    gone(app, FIELD);
    step(app.press_key("/"));
    step(app.wait_for("text:3   Test Channel Three"));
    step(app.wait_for("text:Playing"));
    // Back: the focus to the field, then the screen closes; the searches
    // are kept (`search_history.rs` picks them again).
    close_search(app);
    step(app.press_key("/"));
    step(app.wait_for("text:Recent searches"));
    step(app.wait_for("text:Recital"));
    step(app.press("text:Close"));
    gone(app, FIELD);
}

/// The channel list over full-screen video, by keys.
fn channel_list_keys(app: &mut Headless) {
    const HEAD: &str = "text:◂  All channels";
    // Channel one plays; F: full screen; Left: the list over the video,
    // on the playing channel.
    step(app.press_key("home"));
    step(app.press_key("pageup"));
    wait_focused(app, "Morning Talk");
    step(app.press_key("enter"));
    step(app.wait_for("text:/Test Channel One  ▶$/"));
    step(app.press_key("f"));
    step(app.press_key("left"));
    wait_list(app, true);
    assert!(
        lit(app.screen(), "1  Test Channel One  ▶"),
        "on the playing row"
    );
    // The pages (clamped), Down: OK on channel two plays it; the list stays.
    step(app.press_key("pagedown"));
    step(app.wait_until("the last row", WAIT, |s| lit(s, "4  Test Channel Four")));
    step(app.press_key("pageup"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:2  Test Channel Two  ▶"));
    assert!(list_open(app.screen()));
    // Ch+ zaps back to channel one, the list following.
    step(app.press_key("="));
    step(app.wait_until("following", WAIT, |s| lit(s, "1  Test Channel One  ▶")));
    // Space pauses and plays.
    step(app.press_key("space"));
    step(app.press_key("space"));
    // OK on the playing channel closes the list.
    step(app.press_key("enter"));
    wait_list(app, false);

    // Left again, Left: the groups beside it, the heading without its ◂.
    step(app.press_key("left"));
    wait_list(app, true);
    step(app.press_key("left"));
    gone(app, HEAD);
    step(app.wait_until("All channels", WAIT, |s| lit(s, "All channels")));
    // Up twice: Recently watched, then Favourites, empty; Right cannot
    // enter an empty list.
    step(app.press_key("up"));
    step(app.wait_until("recent", WAIT, |s| lit(s, "Recently watched")));
    step(app.press_key("up"));
    step(app.wait_for("text:No favourites yet"));
    step(app.press_key("right"));
    assert!(lit(app.screen(), "Favourites"), "still in the groups");
    // Down to the News group (entry 3); OK enters it.
    for _ in 0..3 {
        step(app.press_key("down"));
    }
    step(app.wait_until("news", WAIT, |s| lit(s, "News")));
    step(app.press_key("enter"));
    step(app.wait_for("text:◂  News"));
    step(app.wait_until("in the list", WAIT, |s| {
        lit(s, "1  Test Channel One  ▶") || lit(s, "2  Test Channel Two")
    }));
    // Back closes the list.
    step(app.press_key("backspace"));
    wait_list(app, false);
    // All channels again, from its groups.
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for(HEAD));
    step(app.press_key("backspace"));
    wait_list(app, false);

    // Shift + Left (Long Left): the list beside the video, shrunk to a
    // preview, with the focused channel's programme under it.
    step(app.press_key("shift+left"));
    wait_list(app, true);
    step(app.wait_for("text:1  Test Channel One"));
    // Leaving full screen closes it.
    step(app.press_key("o"));
    wait_list(app, false);
    step(app.press_key("f"));

    // Shift + Right (Long Right): the history chooser, which takes the
    // keys; Menu closes it; OK picks its focused channel and closes it.
    // (Its rows: the channels, indented where a choice has its ✓.)
    let open = |open: bool| {
        move |s: &nsh_testing::Screen| {
            s.texts()
                .iter()
                .any(|t| t.starts_with("    ") && t.contains("Test Channel"))
                == open
        }
    };
    step(app.press_key("shift+right"));
    step(app.wait_until("the history", WAIT, open(true)));
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("m"));
    step(app.wait_until("closed", WAIT, open(false)));
    step(app.press_key("shift+right"));
    step(app.wait_until("the history", WAIT, open(true)));
    step(app.press_key("enter"));
    step(app.wait_until("picked", WAIT, open(false)));
}

/// Whether the channel list's row or groups entry `text` shows in the
/// focus pill's dark colour.
fn lit(screen: &nsh_testing::Screen, text: &str) -> bool {
    let dark = nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff);
    [
        "id:channel-list",
        "id:channel-list-groups",
        "id:multiview-picker",
    ]
    .iter()
    .any(|scope| {
        screen
            .span_style(&format!("{scope} >> text:{text}"))
            .is_ok_and(|s| s.color == dark)
    })
}

/// Whether the channel list shows: one of its rows or entries has the
/// focus pill (the layer stays mounted, unseen, while closed).
fn list_open(screen: &nsh_testing::Screen) -> bool {
    if screen.matches("id:multiview-picker").unwrap_or(false) {
        return true;
    }
    let entries = [
        "Favourites",
        "Recently watched",
        "All channels",
        "News",
        "Sport",
    ];
    screen
        .texts()
        .iter()
        .filter(|t| t.contains("  Test Channel") || entries.contains(t))
        .any(|t| lit(screen, t))
}

fn wait_list(app: &mut Headless, open: bool) {
    step(
        app.wait_until(&format!("the list open: {open}"), WAIT, |s| {
            list_open(s) == open
        }),
    );
}

/// The text of a tile's channel line.
fn tile_channel(name: &str) -> String {
    format!("{TILE} >> text:/{name}$/")
}

/// The open tile menu's labels, top to bottom.
fn menu_labels(app: &Headless) -> Vec<String> {
    let nodes = app.screen().layout_nodes();
    let Some(panel) = nodes
        .iter()
        .find(|n| n.test_id.as_deref() == Some("multiview-menu"))
        .map(|n| n.id)
    else {
        return Vec::new();
    };
    let parent: std::collections::HashMap<_, _> = nodes.iter().map(|n| (n.id, n.parent)).collect();
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
    found.into_iter().map(|t| t.0).collect()
}

/// Opens the active tile's menu and chooses `label` by keys.
fn choose(app: &mut Headless, label: &str) {
    step(app.press_key("enter"));
    step(app.wait_for(MENU));
    let at = menu_labels(app)
        .iter()
        .position(|l| l == label)
        .unwrap_or_else(|| panic!("{label} in {:?}", menu_labels(app)));
    for _ in 0..at {
        step(app.press_key("down"));
    }
    step(app.press_key("enter"));
    gone(app, MENU);
}

/// Multiview on a TV, by keys.
fn multiview_keys(app: &mut Headless) {
    const HINT: &str = "text:OK: menu · Back: full screen";
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));
    step(app.set_viewport(Size::new(px(960.0), px(540.0))));
    // A TV has no play bar: a tap on the video shows the info card, a
    // second hides it.
    // (A first tap may only hide what the last pick showed.)
    step(app.press("label:Video"));
    if app
        .wait_for_within("id:player-card", Duration::from_secs(1))
        .is_err()
    {
        step(app.press("label:Video"));
        step(app.wait_for("id:player-card"));
    }
    step(app.press("label:Video"));
    gone(app, "id:player-card");
    // The control strip's Multiview (Menu): channel one the first tile,
    // the list picking the next.
    step(app.press_key("m"));
    step(app.wait_for("text:Multiview"));
    step(app.press("text:Multiview"));
    step(app.wait_for(TILE));
    // (The tap left the keys' pill off: the first key brings it back.)
    step(app.press_key("down"));
    step(app.wait_until("picking two", WAIT, |s| lit(s, "2  Test Channel Two")));
    step(app.press_key("enter"));
    step(app.wait_for(&tile_channel("Test Channel Two")));
    assert_eq!(app.screen().count(TILE).unwrap(), 2);
    wait_list(app, false);
    // The key hint, which goes by itself.
    step(app.wait_for(HINT));
    step(app.wait_for_gone(HINT, Duration::from_secs(8)));

    // Menu (the key) opens the tile's menu too; Back closes it.
    step(app.press_key("m"));
    step(app.wait_for(MENU));
    step(app.press_key("backspace"));
    gone(app, MENU);
    assert_eq!(app.screen().count(TILE).unwrap(), 2);

    // Add screen: the list again, from the active tile's channel (two);
    // channel three the third tile.
    choose(app, "Add screen");
    wait_list(app, true);
    assert!(lit(app.screen(), "2  Test Channel Two"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for(&tile_channel("Test Channel Three")));
    // Enlarge, then Reduce.
    choose(app, "Enlarge screen");
    step(app.press_key("enter"));
    step(app.wait_for(&format!("{MENU} >> text:Reduce screen")));
    step(app.press_key("backspace"));
    gone(app, MENU);
    choose(app, "Reduce screen");
    // Change channel: channel three's tile becomes channel four's.
    choose(app, "Change channel");
    wait_list(app, true);
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for(&tile_channel("Test Channel Four")));
    gone(app, &tile_channel("Test Channel Three"));
    // A fourth tile: the device's limit; the menu offers no more.
    choose(app, "Add screen");
    wait_list(app, true);
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for(&tile_channel("Test Channel Three")));
    assert_eq!(app.screen().count(TILE).unwrap(), 4);
    step(app.press_key("enter"));
    step(app.wait_for(MENU));
    let labels = menu_labels(app);
    assert!(!labels.iter().any(|l| l == "Add screen"), "{labels:?}");
    step(app.press_key("backspace"));
    gone(app, MENU);
    // The arrows walk the two by two grid.
    for key in ["up", "left", "down", "right"] {
        step(app.press_key(key));
    }

    // Full screen: a detour; Back brings the grid back as it was.
    choose(app, "Full screen");
    gone(app, TILE);
    step(app.press_key("backspace"));
    step(app.wait_for(TILE));
    assert_eq!(app.screen().count(TILE).unwrap(), 4);
    // Remove screen: three left.
    choose(app, "Remove screen");
    step(app.wait_until("three tiles", Duration::from_secs(3), |s| {
        s.count(TILE) == Ok(3)
    }));
    // Full screen again, then a zap: the session ends there.
    choose(app, "Full screen");
    gone(app, TILE);
    step(app.press_key("="));
    step(app.wait(Duration::from_millis(300)));
    step(app.press_key("m"));
    step(app.wait_for("text:Multiview"));
    assert!(!app.screen().matches("text:Back to multiview").unwrap());

    // Again from the strip; Back on the grid leaves to the active tile.
    step(app.press("text:Multiview"));
    step(app.wait_for(TILE));
    step(app.press_key("up"));
    wait_list(app, true);
    step(app.press_key("backspace"));
    wait_list(app, false);
    assert_eq!(app.screen().count(TILE).unwrap(), 1);
    step(app.press_key("backspace"));
    gone(app, TILE);
    // Leaving full screen from the grid (O) keeps its tile playing.
    step(app.press_key("m"));
    step(app.wait_for("text:Multiview"));
    step(app.press("text:Multiview"));
    step(app.wait_for(TILE));
    step(app.press_key("up"));
    step(app.press_key("backspace"));
    wait_list(app, false);
    step(app.press_key("o"));
    gone(app, TILE);
    step(app.press_key("f"));
    // Stop ends it all: nothing plays, the guide.
    step(app.press_key("m"));
    step(app.wait_for("text:Multiview"));
    step(app.press("text:Multiview"));
    step(app.wait_for(TILE));
    step(app.press_key("up"));
    step(app.press_key("backspace"));
    wait_list(app, false);
    step(app.press_key("stop"));
    gone(app, TILE);
    step(app.wait_for("text:Nothing playing"));
}
