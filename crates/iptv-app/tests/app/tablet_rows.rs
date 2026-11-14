//! What a first run on a tablet shows, headless: the wizard's playlist
//! link starts empty (no development address), the key-timing line shows
//! only in development, a five-digit row number fits its column, and a
//! superscript badge in a programme title reads as a word.
//!
//! Synthetic data only: a 10,000-channel playlist picked from a file, with
//! a local XMLTV guide for its first channel.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::{ApproxTextEngine, ImageData, Size, TextEngine, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;

const IMPORT: Duration = Duration::from_secs(60);
const CHANNELS: usize = 10_000;

fn playlist(guide: &str) -> String {
    let mut m3u = format!("#EXTM3U url-tvg=\"{guide}\"\n");
    for n in 1..=CHANNELS {
        let id = if n == 1 { " tvg-id=\"one\"" } else { "" };
        m3u.push_str(&format!(
            "#EXTINF:-1{id} group-title=\"All\",Test Channel {n:05}\nhttp://stream.example.test/{n}.m3u8\n"
        ));
    }
    m3u
}

/// One programme around now whose title ends in a superscript badge.
fn guide(now: i64) -> String {
    format!(
        "<?xml version=\"1.0\"?><tv><channel id=\"one\"><display-name>One</display-name></channel>\
         <programme channel=\"one\" start=\"{}\" stop=\"{}\"><title>Cup Finalᴸᶦᵛᵉ</title></programme></tv>",
        format_xmltv_time(now - 3600),
        format_xmltv_time(now + 3600)
    )
}

#[test]
fn a_tablet_first_run_shows_whole_numbers_and_no_development_defaults() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = playlist(&guide_path.display().to_string());

    let config = EngineConfig {
        viewport: Size::new(px(1280.0), px(800.0)),
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

    // The playlist link starts empty: its placeholder, no address.
    step(app.wait_for("text:No playlists yet"));
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Playlist link"));
    step(app.press("text:Playlist link"));
    if option_env!("IPTV_RS_DEV_PLAYLIST").is_none() {
        step(app.wait_for("text:http://…/playlist.m3u"));
        let texts = app.screen().texts().join("\n");
        assert!(
            !texts.contains("8089") && !texts.contains("10.0.2.2"),
            "no development address: {texts}"
        );
    }
    step(app.back());

    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for("text:Test Channel 00001"));

    // The badge reads as a word after the title.
    step(app.wait_for_within("text:Cup Final Live", IMPORT));
    assert!(app.screen().matches("role:cell[/Cup Final Live/]").unwrap());
    assert!(
        app.screen()
            .matches("role:button[/Test Channel 00001/]")
            .unwrap()
    );

    // Selection reflects content identity and does not change merely when focus or keys visibility changes.
    let cell_selector = "role:cell[/Cup Final Live/]";
    let channel_selector = "role:button[/Test Channel 00001/]";
    assert!(
        app.screen()
            .accessibility_state(cell_selector)
            .unwrap()
            .selected
    );
    assert!(
        app.screen()
            .accessibility_state(channel_selector)
            .unwrap()
            .selected
    );

    // Unselected channels are not selected.
    if app
        .screen()
        .matches("role:button[/Test Channel 00002/]")
        .unwrap()
    {
        assert!(
            !app.screen()
                .accessibility_state("role:button[/Test Channel 00002/]")
                .unwrap()
                .selected
        );
    }

    // Keys visibility changes (keys shown via navigation key). Selection does not change.
    step(app.press_key("home"));
    assert!(
        app.screen()
            .accessibility_state(cell_selector)
            .unwrap()
            .selected
    );
    assert!(
        app.screen()
            .accessibility_state(channel_selector)
            .unwrap()
            .selected
    );

    // Pointer/touch action (keys hidden). Selection does not change.
    step(app.press("text:Test Channel 00001"));
    assert!(
        app.screen()
            .accessibility_state(cell_selector)
            .unwrap()
            .selected
    );
    assert!(
        app.screen()
            .accessibility_state(channel_selector)
            .unwrap()
            .selected
    );

    // Moving focus to another pane (Groups column via Tab).
    // Programme cell and channel in the guide retain selection even when focus moves to groups pane.
    step(app.press_key("tab"));
    assert!(
        app.screen()
            .accessibility_state(cell_selector)
            .unwrap()
            .selected
    );
    assert!(
        app.screen()
            .accessibility_state(channel_selector)
            .unwrap()
            .selected
    );
    step(app.press_key("tab"));

    // The key-timing line: development builds only.
    let timing = app.screen().matches("text:/^keys \\d+ · \\d+ µs/").unwrap();
    assert_eq!(timing, iptv_app::debug_overlay());

    // Row numbers share one column, sized for the list's widest number
    // (10000): it fits there as the headless text engine measures it.
    widest_number_fits(&app);
    // Also at a larger system text size (a tablet's default 1.15), which
    // the platform applies to the numbers' text.
    step(app.set_environment("larger text", |e| e.font_scale = 1.3));
    widest_number_fits(&app);
}

fn widest_number_fits(app: &Headless) {
    let nodes = app.screen().layout_nodes();
    let first = nodes
        .iter()
        .filter_map(|n| n.text.as_ref())
        .find(|t| t.text.text == "1")
        .expect("the first row's number");
    let mut widest = (*first.text).clone();
    widest.text = CHANNELS.to_string();
    if let Some(span) = widest.spans.last_mut() {
        span.end = widest.text.len() as u32;
    }
    let engine = ApproxTextEngine;
    let needs = engine.measure(&engine.prepare(&widest), None).width;
    assert!(
        needs <= first.content.size.width,
        "{} needs {needs:?}, the column gives {:?}",
        widest.text,
        first.content.size.width
    );
}
