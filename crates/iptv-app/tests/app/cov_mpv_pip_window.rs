//! The desktops' picture-in-picture window and the pointer, run headless
//! as a desktop app (`desktop/pip_window.rs`): its controls show while the
//! pointer moves over the video and hide when it leaves the window or
//! rests there; a single click is no double click, however long after it
//! the next one comes. `pip_control.rs` covers opening and leaving it.
//!
//! Synthetic data only: a two-channel playlist "picked" from a file.

use std::time::Duration;

use crate::headless::Headless;
use nsh_core::picker::PickedFile;
use nsh_core::platform::{Idiom, Os};
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const PLAYLIST: &str = "#EXTM3U
#EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Synthetic One
http://stream.example.test/one.m3u8
#EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Synthetic Two
http://stream.example.test/two.m3u8
";

const CONTROLS: &str = "id:pip-back";

fn step(r: Result<(), String>) {
    r.unwrap_or_else(|e| panic!("{e}"));
}

fn pip_open(app: &mut Headless) -> bool {
    app.window_state("pip").unwrap().is_some()
}

fn shows_controls(app: &Headless) -> bool {
    app.screen().matches(CONTROLS).unwrap()
}

#[test]
fn the_pip_windows_controls_follow_the_pointer() {
    let Some(_home) = isolated!() else { return };
    let mut config = EngineConfig {
        viewport: Size::new(px(960.0), px(540.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.os = Os::Linux;
    let mut app = Headless::start(config).unwrap();
    app.on_pick(|_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(PLAYLIST.as_bytes().to_vec()),
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
    step(app.wait_for_within("text:Playlist added", Duration::from_secs(20)));
    step(app.press("text:Watch"));
    step(app.wait_for("text:Synthetic One"));
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));

    // Full screen, then the control strip's Picture in picture.
    // OK on a channel tunes it after the switch delay and goes full
    // screen (`guide.stay_on_guide` off by default).
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner"));
    step(app.press_key("m"));
    step(app.wait_for("text:Picture in picture"));
    for _ in 0..4 {
        step(app.press_key("right"));
    }
    step(app.press_key("enter"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(pip_open(&mut app));
    assert!(!shows_controls(&app), "hidden at rest");

    // Over the video: the controls; the pointer resting (on one of them):
    // they hide by themselves.
    step(app.hover("id:pip-video"));
    step(app.wait_for(CONTROLS));
    step(app.hover(CONTROLS));
    step(app.wait_for_gone(CONTROLS, Duration::from_secs(20)));
    assert!(pip_open(&mut app));

    // Moving again shows them; out of the window (to the main window's
    // placeholder) they go at once.
    step(app.hover("id:pip-video"));
    step(app.wait_for(CONTROLS));
    step(app.hover("id:pip-return"));
    app.idle(Duration::from_millis(100)).unwrap();
    assert!(!shows_controls(&app), "the pointer left the window");

    // One click, and another well after it: two clicks, not a double
    // click (the window stays); the video drags the window again between.
    step(app.press("id:pip-video"));
    app.wait(Duration::from_millis(900)).unwrap();
    step(app.press("id:pip-video"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(pip_open(&mut app), "not a double click");
    app.wait(Duration::from_millis(900)).unwrap();
    assert!(pip_open(&mut app));
}
