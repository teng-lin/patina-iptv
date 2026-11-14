//! A replay's keys on a TV, run headless with the TV idiom: the card
//! shows the replay's seek bar as its one time line (no second bar, no
//! action cards), Left/Right (`catchup.seek_keys`' default) choose a seek
//! shown on it, OK opens the archive there at once, and OK then plays and
//! pauses.
//!
//! Synthetic data only: a playlist picked from a file, channel one with a
//! day of catch-up, its guide a local XMLTV file.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);
/// Programmes of 45 minutes, "Show <n>" counting from the one on now (0).
const LEN: i64 = 45 * 60;
const CARD: &str = "id:player-card";
const LINE: &str = "id:player-time-line";

fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv><channel id=\"one\"><display-name>One</display-name></channel>",
    );
    let first = now - 86_400 + 20 * 60;
    let live = (now - first) / LEN;
    let mut k = 0;
    while first + k * LEN < now + 6 * 3_600 {
        let (start, stop) = (first + k * LEN, first + (k + 1) * LEN);
        xml.push_str(&format!(
            "<programme channel=\"one\" start=\"{}\" stop=\"{}\"><title>Show {}</title></programme>",
            format_xmltv_time(start),
            format_xmltv_time(stop),
            k - live
        ));
        k += 1;
    }
    xml + "</tv>"
}

#[test]
fn a_replays_card_has_its_seek_bar_and_ok_seeks_then_pauses() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"1\",Test Channel One\nhttp://stream.example.test/one.m3u8\n",
        guide_path.display()
    );
    let config = EngineConfig {
        viewport: Size::new(px(960.0), px(540.0)),
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
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));

    // Two programmes back, OK replays it full screen: the card alone, its
    // time line the seek bar (position and length), no action cards.
    step(app.press_key("home"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Show -2"));
    step(app.press_key("enter"));
    step(app.wait_for(CARD));
    step(app.wait_for("text:CATCH-UP"));
    assert_eq!(app.screen().count(LINE).unwrap(), 1, "one time line");
    assert!(!app.screen().matches("id:player-actions").unwrap());
    step(app.wait_for(&format!("{LINE} >> text:45:00")));
    step(app.wait_for("text:/◀ ▶ seek/"));

    // Right chooses a seek a time-bar step on (5 min), shown on the bar;
    // OK opens the archive there at once.
    step(app.press_key("right"));
    step(app.wait_for(&format!("{LINE} >> text:⇢ 5:00")));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:/^⇢/", Duration::from_secs(2)));
    step(app.wait_for(&format!("{LINE} >> text:5:00")));

    // Then OK pauses and plays.
    step(app.press_key("enter"));
    step(app.wait_for("text:PAUSED"));
    step(app.press_key("enter"));
    step(app.wait_for("text:CATCH-UP"));
    assert_eq!(app.screen().count(LINE).unwrap(), 1);
}
