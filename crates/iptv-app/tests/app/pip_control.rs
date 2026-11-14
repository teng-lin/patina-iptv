//! The control strip's Picture in picture, run headless as a desktop app
//! (keys as on a TV): Menu over full-screen video shows the button; OK on
//! it opens a second window titled "Picture-in-Picture" (`pip.rs`,
//! `desktop/pip_window.rs`) showing the same player (no stream opens for
//! it), while the main window stays as it was, usable, with a placeholder
//! where the video was. Back to the app (the placeholder's button, the
//! window's control, Escape in it, a double click on it) closes the window
//! and the player is full screen again; its ✕ or the system's close stops
//! playback. Android's system window is checked on a device.
//!
//! Synthetic data only: a two-channel playlist "picked" from a file.

use std::time::Duration;

use crate::headless::Headless;
use nsh_core::desktop::{DesktopCommand, OpenWindow};
use nsh_core::picker::PickedFile;
use nsh_core::platform::{Idiom, Os};
use nsh_core::{Command, ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const PLAYLIST: &str = "#EXTM3U
#EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One
http://stream.example.test/one.m3u8
#EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two
http://stream.example.test/two.m3u8
";

const STRIP: &str = "text:/M or Esc hide/";
const PIP: &str = "text:Picture in picture";
const PLACEHOLDER: &str = "text:Playing in picture-in-picture";

fn step(r: Result<(), String>) {
    r.unwrap_or_else(|e| panic!("{e}"));
}

/// Whether the picture-in-picture window is open.
fn pip_open(app: &mut Headless) -> bool {
    app.window_state("pip").unwrap().is_some()
}

/// The main window's size.
fn main_size(app: &mut Headless) -> (f32, f32) {
    let state = app.window_state("main").unwrap().expect("a desktop window");
    (state.bounds.size.width.0, state.bounds.size.height.0)
}

/// The last window the app opened.
fn opened(app: &Headless) -> OpenWindow {
    app.screen()
        .commands()
        .iter()
        .rev()
        .find_map(|(_, c)| match c {
            Command::Desktop(d) => match &**d {
                DesktopCommand::OpenWindow(open) => Some((**open).clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("a window opened")
}

/// Menu over full-screen video, then OK on the strip's Picture in
/// picture (Search, Channels, Recordings, Multiview, then it).
fn enter(app: &mut Headless) {
    step(app.press_key("m"));
    step(app.wait_for(STRIP));
    step(app.wait_for(PIP));
    for _ in 0..4 {
        step(app.press_key("right"));
    }
    step(app.press_key("enter"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(pip_open(app), "the picture-in-picture window opened");
    step(app.wait_for(PLACEHOLDER));
}

/// Back in the main window, the player full screen: the strip comes
/// again.
fn back_in_app(app: &mut Headless) {
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(!pip_open(app), "the window closed");
    assert!(!app.screen().shows("Playing in picture-in-picture"));
    assert!(iptv_app::player_has_source(0), "playing on");
}

#[test]
fn picture_in_picture_opens_a_window_of_its_own_on_the_same_player() {
    let Some(_home) = isolated!() else { return };
    let mut config = EngineConfig {
        viewport: Size::new(px(960.0), px(540.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    // A desktop app: it has windows of its own.
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
    step(app.wait_for("text:Test Channel One"));
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));

    // Full screen.
    // OK on a channel tunes it after the switch delay and goes full
    // screen (`guide.stay_on_guide` off by default).
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner"));
    let before = main_size(&mut app);
    let opens = iptv_app::player_opens(0);

    // A second window, titled as browsers title theirs, 600 wide in 16:9;
    // the main window as it was, a placeholder where the video was, and
    // no stream opened for the window.
    enter(&mut app);
    let open = opened(&app);
    assert_eq!(open.options.id, "pip");
    assert_eq!(
        open.options.attributes.title.as_deref(),
        Some("Picture-in-Picture")
    );
    assert_eq!(
        open.options.default_size,
        Some(Size::new(px(600.0), px(338.0)))
    );
    assert_eq!(main_size(&mut app), before, "the main window stays");
    assert_eq!(iptv_app::player_opens(0), opens, "no second stream");
    assert!(app.screen().matches("id:pip-video").unwrap());

    // The main window stays usable: Back goes to the guide, where the
    // placeholder takes the preview's place; the window plays on. Another
    // channel chosen there plays in the window (one open: its stream).
    step(app.back());
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(pip_open(&mut app));
    assert!(app.screen().shows("Playing in picture-in-picture"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    // It tunes after the switch delay.
    let tuned = std::time::Instant::now();
    while iptv_app::player_opens(0) == opens
        && tuned.elapsed() < crate::isolated::scaled_duration(Duration::from_secs(5))
    {
        app.idle(Duration::from_millis(100)).unwrap();
    }
    assert!(pip_open(&mut app), "the window shows the new channel");
    assert_eq!(iptv_app::player_opens(0), opens + 1, "the new channel only");
    let opens = opens + 1;

    // Its "Back to the app": the window closes, the player full screen.
    step(app.press("id:pip-return"));
    back_in_app(&mut app);
    assert_eq!(iptv_app::player_opens(0), opens, "the same stream");

    // The window's own controls show while the pointer moves over it.
    enter(&mut app);
    assert!(
        !app.screen().matches("id:pip-back").unwrap(),
        "hidden at rest"
    );
    step(app.hover("id:pip-video"));
    step(app.wait_for("id:pip-back"));
    step(app.press("id:pip-play"));
    step(app.wait_for("label:Play"));
    step(app.press("id:pip-play"));
    step(app.wait_for("label:Pause"));
    step(app.press("id:pip-back"));
    back_in_app(&mut app);

    // Escape in the window (a click gives it the keys).
    enter(&mut app);
    step(app.press("id:pip-video"));
    step(app.press_key("escape"));
    back_in_app(&mut app);

    // A double click on it.
    enter(&mut app);
    step(app.press("id:pip-video"));
    step(app.press("id:pip-video"));
    back_in_app(&mut app);
    assert_eq!(main_size(&mut app), before);
    assert_eq!(iptv_app::player_opens(0), opens, "never reopened");

    // Its ✕: closed, and playback stops.
    enter(&mut app);
    step(app.hover("id:pip-video"));
    step(app.press("id:pip-close"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(!pip_open(&mut app));
    assert!(!iptv_app::player_has_source(0), "playback stopped");

    // The system closing it (Super+W) does the same.
    // OK on a channel tunes it after the switch delay and goes full
    // screen (`guide.stay_on_guide` off by default).
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner"));
    enter(&mut app);
    step(app.press_window_close("pip"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(!pip_open(&mut app));
    assert!(!iptv_app::player_has_source(0), "playback stopped");
}
