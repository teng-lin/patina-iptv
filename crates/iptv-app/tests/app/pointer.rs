//! What a click or a tap reaches, on the app run headless: the overlay
//! layers mounted while hidden (the channel list, the player panels, the
//! choosers) must leave the guide, the rail and the header to the
//! pointer. NSH's pointer events inherit as CSS's, so a view saying
//! `PointerEvents::Auto` inside a hidden layer's `None` takes presses
//! through it, and on the desktop a press goes to the topmost view under
//! the pointer; `Headless::press` fails when another view covers its
//! target there, as a real click would miss it.
//!
//! Synthetic data only: a three-channel playlist "picked" from a file.

use std::time::Duration;

use crate::headless::Headless;
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::{ImageData, Point, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const PLAYLIST: &str = "#EXTM3U
#EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One
http://stream.example.test/one.m3u8
#EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two
http://stream.example.test/two.m3u8
#EXTINF:-1 tvg-id=\"three\" group-title=\"Sport\",Test Channel Three
http://stream.example.test/three.m3u8
";

const IMPORT: Duration = Duration::from_secs(20);

#[test]
fn clicks_reach_the_guide_under_the_hidden_layers() {
    // The app keeps its files under $HOME: a fresh, empty one.
    let Some(_home) = isolated!() else { return };

    let config = EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(|_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(PLAYLIST.as_bytes().to_vec()),
        )]
    });
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));

    // First run: add the playlist from a file.
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

    // The guide: a channel's name plays it (the name column lies under
    // the hidden channel list's panel).
    step(app.wait_for("text:Test Channel Two"));
    step(app.press("text:Test Channel Two"));
    step(app.wait_for("text:▶"));

    // The playing channel's name again: full screen. A tap on the video
    // shows the play bar, whose buttons act; another tap hides it.
    step(app.press("text:Test Channel Two"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Mute"));
    step(app.press("label:Play or pause"));
    step(app.wait_for("label:Favourite"));
    step(app.press("label:Favourite"));
    step(app.wait_for("text:★ Test Channel Two"));
    step(app.press("label:Video"));
    step(app.wait_for_gone("label:Mute", Duration::from_secs(2)));
    // Every button of the bar is reachable on a narrow and a wide window,
    // and the popups it opens close on a second press or a press beside
    // them while the bar stays.
    for (w, h) in [(760.0, 480.0), (1600.0, 900.0)] {
        step(app.set_viewport(Size::new(px(w), px(h))));
        bar_buttons(&mut app, step);
    }
    step(app.set_viewport(Size::new(px(1280.0), px(720.0))));
    step(app.press("label:Video"));
    step(app.wait_for_gone("label:Mute", Duration::from_secs(2)));
    multiview(&mut app, step);

    // A TV has no pointer: a press there (an assistive one) shows the
    // remote's card instead, whose TV guide goes back to the guide.
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));
    step(app.press("label:Video"));
    step(app.wait_for("text:TV guide"));
    assert!(
        !app.screen().matches("label:Mute").unwrap(),
        "no play bar on a TV"
    );
    step(app.set_environment("a desktop", |e| e.idiom = Idiom::Desktop));
    step(app.press("text:TV guide"));

    // The list name in the header (under the hidden player panels' top
    // bar) opens the groups and the rail; a group shows its channels.
    step(app.press("text:☰  All channels"));
    step(app.wait_for("text:Sport"));
    step(app.press("text:Sport"));
    step(app.wait_for("text:Test Channel Three"));
    step(app.wait_for_gone("text:Test Channel One", Duration::from_secs(2)));

    // The header's add button (under the hidden player panels' top bar
    // too) opens the wizard.
    step(app.press("text:+ Add playlist"));
    step(app.wait_for("text:Playlist link"));
}

/// A mouse moving over full-screen video brings the play bar up, but not
/// while the channel list is open over the video: the list is what the
/// pointer is for then, and the bar would lie over it.
fn moving_mouse(app: &mut Headless, step: impl Fn(Result<(), String>)) {
    const LIST: &str = "id:channel-list";
    const BESIDE: &str = "id:channel-list-beside";
    // A move counts once the mouse went somewhere else, a quarter of a
    // second after the last one that counted (`desktop::pointer_moved`).
    let pause = Duration::from_millis(300);
    let no_bar = |app: &mut Headless, at: &str| {
        step(app.wait(pause));
        step(app.hover(at));
        step(app.wait(pause));
        assert!(
            !app.screen().matches("label:Mute").unwrap(),
            "the play bar came up over the channel list (mouse over {at})"
        );
    };

    step(app.hover("label:Video"));
    step(app.wait_for("label:Mute"));
    // A separate click, outside the desktop's double-click interval.
    step(app.wait(Duration::from_millis(600)));
    step(app.press("label:Video"));
    step(app.wait_for_gone("label:Mute", Duration::from_secs(2)));

    // The channel list, from the menu; the mouse over the picture beside
    // it, then over the list itself.
    step(app.press_key("m"));
    step(app.wait_for("text:Channels"));
    step(app.press("text:Channels"));
    no_bar(app, BESIDE);
    no_bar(app, LIST);

    // The list closed (a tap beside it): the mouse shows the bar again,
    // once it moves from where it last showed it (the moves over the list
    // did not count).
    step(app.press(BESIDE));
    step(app.wait(pause));
    step(app.hover(BESIDE));
    step(app.wait_for("label:Mute"));
    step(app.wait(Duration::from_millis(600)));
    step(app.press("label:Video"));
    step(app.wait_for_gone("label:Mute", Duration::from_secs(2)));
}

/// Presses every button of the play bar over full-screen video (shown by
/// a tap), checking what the popups do.
fn bar_buttons(app: &mut Headless, step: impl Fn(Result<(), String>)) {
    let gone = |app: &mut Headless, s: &str| app.wait_for_gone(s, Duration::from_secs(2));
    if !app.screen().matches("label:Mute").unwrap() {
        step(app.press("label:Video"));
    }
    step(app.wait_for("label:Mute"));
    for name in [
        "Mute",
        "Mute",
        "Rewind",
        "Play or pause",
        "Play or pause",
        "Fast-forward",
        "Favourite",
        "Favourite",
        "Subtitles and tracks",
        "Full screen",
        "Full screen",
    ] {
        step(app.press(&format!("label:{name}")));
    }
    // Programme details: a second press closes them.
    let details = "text:/^Test Channel Two · \\d/";
    step(app.press("label:Programme details"));
    step(app.wait_for(details));
    step(app.press("label:Programme details"));
    step(gone(app, details));
    // Picture size: a second press closes it, and so does a press beside
    // it; the bar stays meanwhile.
    step(app.press("label:Picture size"));
    step(app.wait_for("text:Stretch to fill"));
    step(app.press("label:Picture size"));
    step(gone(app, "text:Stretch to fill"));
    step(app.press("label:Picture size"));
    step(app.wait_for("text:Stretch to fill"));
    step(app.press("label:Close the chooser"));
    step(gone(app, "text:Stretch to fill"));
    assert!(app.screen().matches("label:Mute").unwrap(), "the bar stays");
    // Desktops have picture in picture: the bar has its button (touch
    // cannot reach the control strip's).
    assert!(
        app.screen().matches("label:Picture in picture").unwrap(),
        "the bar's picture-in-picture button"
    );
    // The channel list replaces the bar; Back closes it.
    step(app.press("label:Channel list"));
    step(gone(app, "label:Mute"));
    step(app.press_key("backspace"));
    // Multiview opens with the playing channel and the picker for the
    // next tile; the bar does not show over it. Back closes the picker
    // (one tile stays), Back again returns to full screen, where the bar
    // comes back.
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for(TILE));
    step(gone(app, "label:Mute"));
    step(app.press_key("backspace"));
    assert_eq!(tiles(app), 1, "Back closed the picker only");
    // Leaving with the tile surface 0 plays opens nothing again.
    let opens = iptv_app::player_opens(0);
    step(app.press_key("backspace"));
    step(gone(app, TILE));
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "surface 0 kept its stream"
    );
    step(app.press("label:Video"));
    step(app.wait_for("label:Mute"));
    step(app.press("label:Video"));
    // The guide leaves full screen; the playing channel's name comes back
    // (cut with two dots where the window is narrow: the harness still
    // reports the 760 pt window here).
    step(app.press("label:Video"));
    step(app.wait_for("label:TV guide"));
    step(app.press("label:TV guide"));
    step(gone(app, "label:Mute"));
    step(app.press("text:/^★ (Test Channel Two|T.*\\.\\.)$/"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Mute"));
}

const TILE: &str = "id:multiview-tile";
const MENU: &str = "id:multiview-menu";
/// Channel Three's row in the channel list (All channels). The list stays
/// mounted while hidden: what it did shows in the tiles.
const ROW_THREE: &str = "id:multiview-picker >> text:3  Test Channel Three";
const TILE_THREE: &str = "id:multiview-tile >> text:/Test Channel Three$/";
const TILE_TWO: &str = "id:multiview-tile >> text:/Test Channel Two$/";

fn tiles(app: &Headless) -> usize {
    app.screen().count(TILE).unwrap()
}

/// Multiview's pointer chrome on screen: a tap beside the tiles shows it.
fn chrome(app: &mut Headless, step: impl Fn(Result<(), String>)) {
    if !app.screen().matches("label:Exit multiview").unwrap() {
        step(app.press_at("label:Video", Point::new(px(20.0), px(20.0))));
    }
    step(app.wait_for("label:Exit multiview"));
}

/// Opens the menu of the tile showing Channel Three (active) and watches
/// it full screen: the grid goes, the session waits.
fn detour(app: &mut Headless, step: impl Fn(Result<(), String>)) {
    step(app.press(TILE_THREE));
    step(app.wait_for(&format!("{MENU} >> text:Full screen")));
    step(app.press(&format!("{MENU} >> text:Full screen")));
    step(app.wait_for_gone(TILE, Duration::from_secs(2)));
}

/// Multiview by pointer, over full-screen video with the bar hidden: the
/// tiles alone (no add cell), the toolbar's Add and Exit, a tile's
/// enlarge and remove buttons, a long press, the tile menu at the right
/// edge with no backdrop (the tiles still take presses), and Full screen
/// as a detour that Back (or the bar's button) undoes.
fn multiview(app: &mut Headless, step: impl Fn(Result<(), String>) + Copy) {
    let gone = |app: &mut Headless, s: &str| app.wait_for_gone(s, Duration::from_secs(2));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for(TILE));
    assert_eq!(tiles(app), 1, "the playing channel is the first tile");
    // Picking a channel adds its tile at once.
    step(app.press(ROW_THREE));
    step(app.wait_for(TILE_THREE));
    assert_eq!(tiles(app), 2);
    assert!(!app.screen().matches("text:Add a channel").unwrap());
    assert!(
        !app.screen().matches("label:Exit multiview").unwrap(),
        "no chrome until the pointer acts"
    );
    // The chrome: a tile's enlarge button and a long press toggle it.
    chrome(app, step);
    step(app.press("label:Enlarge screen: Test Channel Two"));
    step(app.wait_for("label:Reduce screen: Test Channel Two"));
    step(app.long_press(&format!("{TILE} >> nth=0")));
    step(app.wait_for("label:Enlarge screen: Test Channel Two"));
    // The active tile's menu: a panel at the right edge in the menu's
    // words, nothing dimming the tiles: a press on the other tile reaches
    // it and closes the menu, and so does a press beside the tiles.
    step(app.press(TILE_THREE));
    step(app.press(TILE_THREE));
    step(app.wait_for(&format!("{MENU} >> text:Remove screen")));
    let panel = menu_bounds(app);
    assert!(
        (1280.0 - (panel.0 + panel.2) - 10.0 * 0.875).abs() < 0.5,
        "at the right edge: {panel:?}"
    );
    assert!((panel.1 + panel.3 / 2.0 - 360.0).abs() < 1.0, "centred");
    for item in ["Add screen", "Search and add", "Change channel", "Pause"] {
        assert!(
            app.screen()
                .matches(&format!("{MENU} >> text:{item}"))
                .unwrap()
        );
    }
    step(app.press(TILE_TWO));
    step(gone(app, MENU));
    step(app.press(TILE_THREE));
    step(app.wait_for(MENU));
    step(app.press_at("label:Video", Point::new(px(20.0), px(20.0))));
    step(gone(app, MENU));
    // Its remove button: one tile left, without buttons.
    chrome(app, step);
    step(app.press("label:Remove screen: Test Channel Three"));
    assert_eq!(tiles(app), 1);
    assert!(!app.screen().matches("label:/^Remove /").unwrap());
    // The toolbar's Add opens the picker (the chrome goes); Back cancels
    // it, and only it.
    chrome(app, step);
    step(app.press("label:Add screen"));
    step(gone(app, "label:Exit multiview"));
    step(app.press_key("backspace"));
    assert_eq!(tiles(app), 1);
    // Again, picking Channel Three.
    chrome(app, step);
    step(app.press("label:Add screen"));
    step(app.wait_for(ROW_THREE));
    step(app.press(ROW_THREE));
    step(app.wait_for(TILE_THREE));
    assert_eq!(tiles(app), 2);
    // Full screen is a detour: the play bar's button returns to the
    // same grid…
    detour(app, step);
    step(app.wait_for("label:Back to multiview"));
    step(app.press("label:Back to multiview"));
    step(app.wait_for(TILE));
    assert_eq!(tiles(app), 2);
    step(app.wait_for(TILE_THREE));
    step(app.wait_for(TILE_TWO));
    // …and so does Back.
    detour(app, step);
    step(app.press_key("backspace"));
    step(app.wait_for(TILE));
    assert_eq!(tiles(app), 2);
    // The toolbar's Exit: the active tile full screen, the bar at hand.
    // Channel Two's tile plays on another surface (Channel Three took
    // surface 0 for its detour): leaving opens it there once.
    step(app.press(TILE_TWO));
    chrome(app, step);
    let opens = iptv_app::player_opens(0);
    step(app.press("label:Exit multiview"));
    step(gone(app, TILE));
    assert_eq!(
        iptv_app::player_opens(0),
        opens + 1,
        "one open on surface 0"
    );
    step(app.wait_for("label:Mute"));
    step(app.press("label:Video"));
    step(gone(app, "label:Mute"));
}

/// The tile menu's panel (x, y, width, height).
fn menu_bounds(app: &Headless) -> (f32, f32, f32, f32) {
    let b = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some("multiview-menu"))
        .expect("the tile menu")
        .bounds;
    (b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0)
}

/// Left mouse clicks and taps follow the same back-navigation chain.
#[test]
fn desktop_back_controls() {
    let Some(home) = isolated!() else { return };
    back_navigation(&home, Idiom::Desktop);
}

#[test]
fn tablet_back_controls() {
    let Some(home) = isolated!() else { return };
    back_navigation(&home, Idiom::Pad);
}

#[test]
fn tv_keeps_remote_navigation() {
    let Some(home) = isolated!() else { return };
    back_navigation(&home, Idiom::Tv);
}

fn back_navigation(home: &crate::isolated::Home, device: Idiom) {
    crate::browse_fixture::two_step_preview(home.path());
    let mut config = EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = device;
    config.environment.os = if device == Idiom::Desktop {
        nsh_core::platform::Os::Linux
    } else {
        nsh_core::platform::Os::Android
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(|_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(PLAYLIST.as_bytes().to_vec()),
        )]
    });
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    step(app.wait_for("text:No playlists yet"));
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    if device == Idiom::Tv {
        step(app.wait_for("text:Choose a file…"));
        assert!(!app.shows("text:‹ Back"));
    } else {
        step(app.wait_for("text:‹ Back"));
    }
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for("text:Test Channel One"));
    // A key must not remove the visible route back on desktop or tablet.
    step(app.press_key("down"));
    if device == Idiom::Tv {
        assert!(!app.shows("label:Groups") && !app.shows("label:Menu"));
        return;
    }
    step(app.wait_for("label:Groups"));
    step(app.press("label:Groups"));
    step(app.wait_for("label:Menu"));
    step(app.press("label:Menu"));
    step(app.wait_for_gone("label:Groups", Duration::from_secs(2)));
    step(app.wait_for_gone("label:Menu", Duration::from_secs(2)));
    step(app.press_key("right"));
    step(app.press_key("right"));
    step(app.wait_for("label:Groups"));

    // The full-screen chevron returns directly to the guide.
    step(app.press("text:Test Channel One"));
    step(app.wait_for("text:▶"));
    step(app.press("text:Test Channel One"));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Guide"));
    step(app.wait_for("label:Mute"));
    step(app.press_key("space"));
    step(app.wait_for_gone("label:Mute", Duration::from_secs(2)));
    step(app.wait_for_gone("label:Guide", Duration::from_secs(2)));
    if device == Idiom::Desktop {
        moving_mouse(&mut app, step);
    }
    // A separate tap, outside the desktop's double-click interval.
    step(app.wait(Duration::from_millis(600)));
    step(app.press("label:Video"));
    step(app.wait_for("label:Mute"));
    step(app.wait_for("label:Guide"));
    step(app.press("label:Guide"));
    step(app.wait_for("label:Groups"));
}
