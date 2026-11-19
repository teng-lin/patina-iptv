//! Moving along the guide's timeline, run headless on a desktop: a finger
//! (or mouse) dragging the programmes sideways pans them and flings on, so
//! does a sideways wheel, a Now button takes them back, a tap (even with a
//! small wobble) still plays. Left, day/hour keys and touch reach retained
//! past listings with or without catch-up. OK on a listing outside the
//! channel's archive opens its details without loading a stream.
//!
//! Synthetic data only: a local playlist and XMLTV guide from three days
//! back to a day ahead, including a channel without a guide binding.

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
const FIRST: i32 = -95;

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
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{name} {number}</title><desc>Generated details for {name} {number}</desc></programme>",
                format_xmltv_time(start),
                format_xmltv_time(stop),
                number = k - live
            ));
        }
        k += 1;
    }
    xml + "</tv>"
}

#[test]
fn the_timeline_pans_by_touch_and_left_reaches_retained_listings() {
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
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/two.m3u8\n\
         #EXTINF:-1 tvg-id=\"unmapped\" group-title=\"No listings\",Unmapped channel\nhttp://stream.example.test/unmapped.m3u8\n",
        guide_path.display()
    );

    let mut config = EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    let mut app = Headless::start(config).unwrap();
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
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
    assert!(
        !app.screen().matches("text:Now").unwrap(),
        "at now: no Now button"
    );

    step(app.press_key("down"));
    assert!(focused(&app, "Other 0"));

    // Dragging the programmes right shows earlier ones: three drags and
    // the final fling go several hours back, within retained listings.
    for _ in 0..3 {
        step(app.drag("text:Test Channel One", Point::new(px(500.0), px(12.0))));
    }
    step(app.wait(Duration::from_secs(3)));
    step(app.wait_for("text:Now"));
    let back = shown(&app);
    assert!(
        back.iter().all(|&n| (FIRST..0).contains(&n)) && back.iter().any(|&n| n <= -5),
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

    // An available replay remains marked by its action. The listing just
    // before the archive is reachable but offers only its details.
    step(app.press_key("home"));
    step(app.press_key("up"));
    assert!(focused(&app, "Show 0"), "{:?}", app.screen().texts());
    for _ in 0..31 {
        step(app.press_key("left"));
    }
    step(app.wait_for("text:Show -31"));
    assert!(focused(&app, "Show -31"));
    step(app.press_key("m"));
    step(app.wait_for("text:Play from the beginning"));
    step(app.back());

    step(app.press_key("left"));
    step(app.wait_for("text:Show -32"));
    assert!(focused(&app, "Show -32"));
    step(app.press_key("m"));
    step(app.wait_for("text:Details"));
    assert!(
        !app.screen()
            .matches("id:menu-panel >> text:Play from the beginning")
            .unwrap()
    );
    step(app.back());
    step(app.press_key("enter"));
    step(app.wait_for("id:description-layer >> text:Generated details for Show -32"));
    assert_eq!(iptv_app::player_opens(0), 0);
    step(app.back());

    // Day steps reach the oldest retained page. Instead of trying to
    // browse before it, Left opens the groups and then the rail.
    for _ in 0..3 {
        step(app.press_key("mod+left"));
    }
    step(app.wait_for("text:Show -95"));
    assert!(focused(&app, "Show -95"), "{:?}", app.screen().texts());
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("enter"));
    step(app.wait_for("id:search-field"));
    step(app.back());
    step(gone(&mut app, "id:search-field"));

    // A row without catch-up can inspect retained past listings too.
    step(app.press_key("home"));
    step(app.press_key("down"));
    step(app.wait_for("text:Other 0"));
    step(app.press_key("left"));
    step(app.wait_for("text:Other -1"));
    assert!(focused(&app, "Other -1"));
    step(app.press_key("enter"));
    step(app.wait_for("id:description-layer >> text:Generated details for Other -1"));
    assert_eq!(iptv_app::player_opens(0), 0);
    step(app.back());
    step(app.press_key("shift+left"));
    step(app.wait_for("text:Other -3"));
    assert!(focused(&app, "Other -3"));

    let old_unmapped_cell = "id:guide-ui >> role:cell[No information] >> nth=0";
    step(app.press(old_unmapped_cell));
    assert_eq!(iptv_app::player_opens(0), 0);
    step(app.right_click(old_unmapped_cell));
    assert!(
        !app.screen()
            .matches("id:menu-panel >> text:Details")
            .unwrap()
    );
    assert_eq!(iptv_app::player_opens(0), 0);

    // A row without a retained guide listing stays at now. Left can
    // immediately open Groups even with other channels' listings on screen.
    step(app.press_key("down"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("enter"));
    step(app.wait_for("id:search-field"));
    step(app.back());
    step(gone(&mut app, "id:search-field"));
    step(app.press_key("home"));
    step(app.press_key("up"));

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
    // walks the retained listings again.
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

    // Start playback after the zero-open checks so a simulated stream retry
    // cannot change their count.
    step(app.press_key("home"));
    step(app.drag("text:Other 0", Point::new(px(4.0), px(3.0))));
    step(app.wait_for("text:▶"));
}

#[track_caller]
fn step(result: Result<(), String>) {
    if let Err(error) = result {
        panic!("{error}");
    }
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
