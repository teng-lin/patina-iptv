//! The guide holds the stored guide's last day; older days are read in
//! when the user browses back to them, and dropped back at now. Run
//! headless: a playlist picked from a file (its channel replays three
//! days, so the guide browses back that far) whose guide is a local XMLTV
//! file covering three days back.
//!
//! Synthetic data only.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);
/// Programmes of 45 minutes, from three days back to a day ahead.
const LEN: i64 = 45 * 60;

/// "Archive Show" before `now` − 25 h (only in the days paged in), "Recent
/// Show" after.
fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv><channel id=\"one\"><display-name>One</display-name></channel>",
    );
    let mut t = now - 3 * 86_400;
    while t < now + 86_400 {
        let title = if t + LEN <= now - 25 * 3600 {
            "Archive Show"
        } else {
            "Recent Show"
        };
        xml.push_str(&format!(
            "<programme channel=\"one\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
            format_xmltv_time(t),
            format_xmltv_time(t + LEN)
        ));
        t += LEN;
    }
    xml + "</tv>"
}

#[test]
fn older_days_are_read_in_when_browsing_back() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n#EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"3\",Test Channel One\nhttp://stream.example.test/one.m3u8\n",
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

    // Now: the day held with the guide.
    step(app.wait_for_within("text:Recent Show", IMPORT));
    assert!(!app.shows("text:Archive Show"));

    // Back a day and more (half an hour per step): that day is read in.
    let back = |app: &mut Headless| {
        for _ in 0..54 {
            step(app.press("text:‹"));
        }
    };
    back(&mut app);
    step(app.wait_for_within("text:Archive Show", Duration::from_secs(5)));
    assert!(!app.shows("text:No information"));

    // And forward to now again.
    for _ in 0..54 {
        step(app.press("text:›"));
    }
    step(app.wait_for_gone("text:Archive Show", Duration::from_secs(2)));
    step(app.wait_for("text:Recent Show"));
    // Back again: the day (dropped meanwhile) is read in again.
    back(&mut app);
    step(app.wait_for_within("text:Archive Show", Duration::from_secs(5)));
}
