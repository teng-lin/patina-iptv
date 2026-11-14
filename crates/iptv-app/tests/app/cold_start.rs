//! A cold start with a stored playlist, headless: the guide's shell comes
//! first (never the synthetic guide), the last channel opens once, before
//! the guide is read, and stays open when the guide arrives (its channel
//! found by key, not opened again); the first screen's programmes, then the
//! whole stored window, reach the guide.
//!
//! Synthetic data only.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use iptv_store_sqlite::ChannelQuery;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);

/// Hour-long programmes from a day back to two days ahead: "Now Show" on
/// air now, "Late Show" ten hours on (past the first screen).
fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv>\
         <channel id=\"one\"><display-name>One</display-name></channel>\
         <channel id=\"two\"><display-name>Two</display-name></channel>",
    );
    let hour = now - now.rem_euclid(3600);
    for channel in ["one", "two"] {
        let mut t = hour - 86_400;
        while t < hour + 2 * 86_400 {
            let title = match t - hour {
                0 => "Now Show",
                36_000 => "Late Show",
                _ => "Other Show",
            };
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
                format_xmltv_time(t),
                format_xmltv_time(t + 3600)
            ));
            t += 3600;
        }
    }
    xml + "</tv>"
}

fn app() -> Headless {
    let config = EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    Headless::start(config).unwrap()
}

#[test]
fn the_last_channel_opens_once_before_the_guide() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One\nhttp://stream.example.test/one.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/two.m3u8\n",
        guide_path.display()
    );
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));

    // First run: the playlist is added; nothing plays.
    let mut first = app();
    first.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    step(first.wait_for("text:No playlists yet"));
    step(first.press("text:Add playlist"));
    step(first.wait_for("text:Playlist file"));
    step(first.press("text:Playlist file"));
    step(first.wait_for("text:Choose a file…"));
    // The form focuses its file field by identity. Activate that field through
    // the remote key path, without depending on a newly mounted row's layout.
    step(first.press_key("enter"));
    step(first.wait_for("text:synthetic.m3u"));
    step(first.press("text:Add playlist"));
    step(first.wait_for_within("text:Playlist added", IMPORT));
    step(first.press("text:Watch"));
    step(first.wait_for_within("text:Now Show", IMPORT));
    drop(first);

    // Channel two played last.
    let data = home.data_dir();
    let store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let source = store.sources().unwrap()[0].id;
    let two = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap()
        .into_iter()
        .find(|row| row.name == "Test Channel Two")
        .unwrap();
    drop(store);
    std::fs::write(
        data.join(format!("last_channel-{}", source.0)),
        two.channel.id.as_str(),
    )
    .unwrap();

    // Cold start: the shell, never the synthetic guide; channel two opens
    // at once, before the guide is read.
    let opens = iptv_app::player_opens(0);
    let mut app = app();
    assert!(!app.shows("text:Channel 1"), "the synthetic guide showed");
    step(app.wait_for_within("text:Test Channel Two", IMPORT));
    assert_eq!(iptv_app::player_opens(0), opens + 1, "opened once");
    // The guide arrives (first screen, then the whole window) and the
    // playing channel is found in it: back to the guide shows it, with
    // programmes past the first screen. Its first screen is on the tree
    // under the video once "Now Show" is; the whole window follows from
    // the store without a text of its own, so the app is left to finish
    // rendering before the count is read.
    step(app.wait_for_within("text:Now Show", IMPORT));
    step(app.idle(Duration::from_millis(500)).map(|_| ()));
    step(app.back());
    step(app.wait_for_within("text:Now Show", IMPORT));
    assert_eq!(
        iptv_app::player_opens(0),
        opens + 1,
        "the guide's arrival opened it again"
    );
}
