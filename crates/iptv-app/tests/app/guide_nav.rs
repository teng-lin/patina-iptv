//! Moving along the guide's timeline, run headless on a desktop: a finger
//! (or mouse) dragging the programmes sideways pans them and flings on, so
//! does a sideways wheel, a Now button takes them back, a tap (even with a
//! small wobble) still plays. The keys: Left walks back through the
//! programmes a channel can replay (channel one: a day of catch-up) and
//! from the oldest, or from now's programme on a channel without catch-up
//! (channel two), opens the groups; Left there focuses the rail's Search.
//! `/` searches; hour and day steps stop where Left does.
//!
//! Synthetic data only: a two-channel playlist picked from a file, its
//! guide a local XMLTV file from three days back to a day ahead.

use crate::browse_fixture;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Point, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);
/// Programmes of 45 minutes.
const LEN: i64 = 45 * 60;

/// Channel one's programmes are "Show <n>", channel two's "Other <n>": `n`
/// counts from the programme airing at `now` (0), negative before it.
/// Programme 0 began 25 minutes before `now`, so a day of archive starts
/// 20 minutes into "Show -32": "Show -31" is the oldest replay, with
/// minutes to spare either side.
fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv><channel id=\"one\"><display-name>One</display-name></channel>\
         <channel id=\"two\"><display-name>Two</display-name></channel>",
    );
    let first = now - 3 * 86_400 + 20 * 60;
    let live = (now - first) / LEN;
    let mut k = 0;
    while first + k * LEN < now + 86_400 {
        let (start, stop) = (first + k * LEN, first + (k + 1) * LEN);
        for (channel, name) in [("one", "Show"), ("two", "Other")] {
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{name} {}</title></programme>",
                format_xmltv_time(start),
                format_xmltv_time(stop),
                k - live
            ));
        }
        k += 1;
    }
    xml + "</tv>"
}

#[test]
fn the_timeline_pans_by_touch_and_left_goes_from_replays_to_groups_and_search() {
    let Some(home) = isolated!() else { return };
    browse_fixture::two_step_preview(home.path());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"1\",Test Channel One\nhttp://stream.example.test/one.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/two.m3u8\n",
        guide_path.display()
    );

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
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    let gone = |app: &mut Headless, s: &str| app.wait_for_gone(s, Duration::from_secs(3));
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
    step(app.wait_for_within("text:Show 0", IMPORT));
    assert!(!app.shows("text:Now"), "at now: no Now button");

    // A tap with a small wobble still plays (it is no drag).
    step(app.drag("text:Other 0", Point::new(px(4.0), px(3.0))));
    step(app.wait_for("text:▶"));

    // Dragging the programmes right shows earlier ones (channel one
    // replays a day): three drags and the fling the last release starts,
    // several hours back, but not before the archive.
    for _ in 0..3 {
        step(app.drag("text:Test Channel One", Point::new(px(500.0), px(12.0))));
    }
    step(app.wait(Duration::from_secs(3)));
    step(app.wait_for("text:Now"));
    let back = shown(&app);
    assert!(
        back.iter().all(|&n| (-32..0).contains(&n)) && back.iter().any(|&n| n <= -5),
        "hours back: {back:?}"
    );
    // A vertical drag is the channel list's: the timeline stays.
    step(app.drag("text:Test Channel One", Point::new(px(3.0), px(80.0))));
    step(app.wait(Duration::from_millis(500)));
    assert_eq!(shown(&app), back);

    // The Now button takes the view back and goes.
    step(app.press("text:Now"));
    step(app.wait_for("text:Show 0"));
    step(gone(&mut app, "text:Now"));

    // A wheel or touchpad scrolling sideways (Shift + wheel too) pans:
    // 1200 dp back is about four hours here.
    step(app.scroll("id:guide-timeline", px(-1200.0)));
    step(app.wait_for("text:Now"));
    let wheeled = shown(&app);
    assert!(wheeled.iter().all(|&n| n < -2), "wheeled back: {wheeled:?}");
    step(app.scroll("id:guide-timeline", px(1200.0)));
    step(app.wait_for("text:Show 0"));
    step(gone(&mut app, "text:Now"));

    // Left walks back through channel one's replays to the oldest, the
    // programme focused (its title in the header and its cell).
    // (Home first: the pans took the focus along into their windows; Up
    // from channel two, which the tap focused.)
    step(app.press_key("home"));
    step(app.press_key("up"));
    assert!(focused(&app, "Show 0"), "{:?}", app.screen().texts());
    for _ in 0..31 {
        step(app.press_key("left"));
    }
    step(app.wait_for("text:Show -31"));
    assert!(focused(&app, "Show -31"), "{:?}", app.screen().texts());
    // There Left opens the groups, Left again the rail on Search: OK
    // searches (were the focus still in the grid, OK would play).
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("enter"));
    step(app.wait_for("id:search-field"));
    step(app.back());
    step(gone(&mut app, "id:search-field"));

    // Home: back to now on the channel; a day back stops at the oldest
    // replay too, and a day on comes back past now.
    step(app.press_key("home"));
    step(app.wait_for("text:Show 0"));
    step(app.press_key("mod+left"));
    step(app.wait_for("text:Show -31"));
    assert!(focused(&app, "Show -31"));
    step(app.press_key("mod+right"));
    step(app.wait_for("text:Show 0"));
    step(app.press_key("home"));

    // Five back on channel one, then Down to channel two (no catch-up):
    // its programme on air, the view back at now.
    for _ in 0..5 {
        step(app.press_key("left"));
    }
    step(app.wait_for("text:Show -5"));
    step(app.press_key("down"));
    step(gone(&mut app, "text:Show -5"));
    assert!(focused(&app, "Other 0"), "{:?}", app.screen().texts());
    // Hours back do nothing there; Left at now opens the groups at once.
    step(app.press_key("shift+left"));
    step(app.press_key("mod+left"));
    step(app.wait(Duration::from_millis(300)));
    assert!(focused(&app, "Other 0"));
    assert!(!app.shows("text:Now"), "the view stays at now");
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("enter"));
    step(app.wait_for("id:search-field"));
    step(app.back());
    step(gone(&mut app, "id:search-field"));

    // `/` and Ctrl+F search from the guide.
    step(app.press_key("tab"));
    step(app.press_key("/"));
    step(app.wait_for("id:search-field"));
    step(app.back());
    step(gone(&mut app, "id:search-field"));
    step(app.press_key("mod+f"));
    step(app.wait_for("id:search-field"));
    step(app.back());
    step(gone(&mut app, "id:search-field"));

    // Hours on (channel one again), and Tab to the groups and back: Left
    // walks the replays again.
    step(app.press_key("home"));
    step(app.press_key("up"));
    step(app.press_key("shift+right"));
    step(app.press_key("shift+right"));
    step(app.wait_for("text:Now"));
    let later = shown(&app);
    assert!(
        later.iter().all(|&n| n >= 0) && later.iter().any(|&n| n >= 3),
        "two hours on: {later:?}"
    );
    step(app.press_key("home"));
    step(app.press_key("tab"));
    step(app.press_key("tab"));
    for _ in 0..8 {
        step(app.press_key("left"));
    }
    step(app.wait_for("text:Show -8"));
    assert!(focused(&app, "Show -8"));
}

/// Whether the programme titled `title` has the focus: its title shows in
/// the header as well as in its cell.
fn focused(app: &Headless, title: &str) -> bool {
    let texts = app.screen().texts();
    texts.iter().filter(|t| **t == title).count() >= 2
}

/// The numbers of channel one's programmes on screen ("Show <n>").
fn shown(app: &Headless) -> Vec<i32> {
    let mut found: Vec<i32> = app
        .screen()
        .texts()
        .into_iter()
        .filter_map(|t| t.strip_prefix("Show ")?.parse().ok())
        .collect();
    found.sort_unstable();
    found
}
