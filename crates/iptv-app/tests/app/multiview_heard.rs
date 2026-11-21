//! Which player is heard (#135, `docs/interaction.md`, "Which player is
//! heard"): exactly one, chosen with a visible Listen to this, never moved
//! by focus. In multiview: focus moves (keys, taps, menus, enlarging)
//! leave the sound alone, Listen to this (the tile's menu, the pointer
//! chrome's button) moves it, the heard tile shows a speaker and the
//! others a muted one, removing the heard tile hands it back to the one
//! heard before, and a detour, enlarging, reducing and resizing keep it
//! with no stream opened. On the desktop: the main player and two player
//! windows (one compact), exactly one heard, each showing it, Listen to
//! this in any of them, and closing the heard one handing it back.
//!
//! Synthetic data only: the generated desktop fixture of
//! `desktop_workflows`.

use std::time::Duration;

use crate::desktop_workflows::{Observed, channel, show_controls, start};
use crate::headless::Headless;
use crate::personalization::step;
use nsh_core::{Size, prelude::*};
use serde_json::json;

const PICKER: &str = "id:multiview-picker";
const MENU: &str = "id:multiview-menu";

/// The tile showing `n`'s channel.
fn tile(n: usize) -> String {
    format!("id:multiview-ui >> role:button[/^{}/]", channel(n))
}

/// Which surfaces are muted, surface 0 first.
fn muted(seen: &Observed, surfaces: usize) -> Vec<bool> {
    (0..surfaces)
        .map(|s| seen.prop(s, "muted") == json!(true))
        .collect()
}

/// Every surface's stream opens so far.
fn opens(surfaces: usize) -> Vec<u64> {
    (0..surfaces).map(iptv_app::player_opens).collect()
}

/// Exactly one surface heard, `heard`; the others muted.
fn only_heard(app: &mut Headless, seen: &Observed, surfaces: usize, heard: usize, what: &str) {
    app.settle();
    let expected: Vec<bool> = (0..surfaces).map(|s| s != heard).collect();
    assert_eq!(muted(seen, surfaces), expected, "{what}");
}

/// The badges: one speaker (on `n`'s tile), the others muted.
fn badges(app: &Headless, tiles: usize, n: usize) {
    let screen = app.screen();
    assert_eq!(screen.count("id:multiview-heard").unwrap(), 1);
    assert_eq!(screen.count("id:multiview-muted").unwrap(), tiles - 1);
    assert!(
        screen
            .matches(&format!("{} >> id:multiview-heard", tile(n)))
            .unwrap(),
        "the speaker on {}",
        channel(n)
    );
}

/// Picks `n` in the open multiview picker.
fn pick(app: &mut Headless, n: usize) {
    let row = format!("{PICKER} >> text:/  {}$/", channel(n));
    step(app.wait_for(&row));
    step(app.press(&row));
    step(app.wait_for_gone(PICKER, Duration::from_secs(2)));
    step(app.wait_for(&tile(n)));
}

/// Tile `n`'s menu item `item`: a tap focuses the tile, the next opens
/// its menu.
fn menu(app: &mut Headless, n: usize, item: &str) {
    for _ in 0..2 {
        if app.screen().matches(MENU).unwrap() {
            break;
        }
        step(app.press(&tile(n)));
    }
    let item = format!("{MENU} >> text:{item}");
    step(app.wait_for(&item));
    step(app.press(&item));
    step(app.wait_for_gone(MENU, Duration::from_secs(2)));
}

/// Whether tile `n`'s menu offers `item` (the menu closes again).
fn offers(app: &mut Headless, n: usize, item: &str) -> bool {
    for _ in 0..2 {
        if app.screen().matches(MENU).unwrap() {
            break;
        }
        step(app.press(&tile(n)));
    }
    step(app.wait_for(MENU));
    let offered = app
        .screen()
        .matches(&format!("{MENU} >> text:{item}"))
        .unwrap();
    step(app.back());
    step(app.wait_for_gone(MENU, Duration::from_secs(2)));
    offered
}

#[test]
fn multiview_focus_never_moves_the_sound_and_listen_to_this_does() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    let seen = fixture.seen.clone();

    // Desk 03 full screen, then multiview: it is the first tile, Desk 05
    // and Desk 07 are added; the sound stays on Desk 03.
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    step(
        app.wait_until("Desk 03 plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    show_controls(app, "id:app-ui >> label:Multiview");
    step(app.press("id:app-ui >> label:Multiview"));
    pick(app, 5);
    menu(app, 5, "Add screen");
    pick(app, 7);
    step(
        app.wait_until("three tiles play", Duration::from_secs(5), |_| {
            (0..3).all(iptv_app::player_has_source)
        }),
    );
    only_heard(app, &seen, 3, 0, "adding tiles takes no sound");
    badges(app, 3, 3);
    assert!(
        app.screen()
            .matches("id:multiview-ui >> role:button[/^Desk 05 · muted$/]")
            .unwrap(),
        "a screen reader hears which tiles are muted"
    );
    let opened = opens(3);

    // Focus moves: the arrows, taps, a menu opened and closed, enlarging.
    for key in ["left", "right", "up", "down", "right"] {
        step(app.press_key(key));
        only_heard(app, &seen, 3, 0, key);
    }
    step(app.press(&tile(7)));
    only_heard(app, &seen, 3, 0, "a tap");
    assert!(
        offers(app, 7, "Listen to this"),
        "a tile not heard offers Listen to this"
    );
    assert!(!offers(app, 3, "Listen to this"), "the heard tile does not");
    menu(app, 7, "Enlarge screen");
    only_heard(app, &seen, 3, 0, "enlarging");
    badges(app, 3, 3);

    // Listen to this on Desk 07 (surface 2): the sound moves there only.
    menu(app, 7, "Listen to this");
    only_heard(app, &seen, 3, 2, "Listen to this");
    badges(app, 3, 7);
    // Working on another tile keeps it there.
    step(app.press(&tile(5)));
    step(app.press_key("left"));
    menu(app, 5, "Enlarge screen");
    only_heard(app, &seen, 3, 2, "another tile enlarged");
    menu(app, 5, "Reduce screen");
    for (w, h) in [(1920.0, 1080.0), (800.0, 600.0), (1280.0, 720.0)] {
        step(app.set_viewport(Size::new(px(w), px(h))));
        only_heard(app, &seen, 3, 2, &format!("{w}x{h}"));
    }
    badges(app, 3, 7);
    assert_eq!(
        opens(3),
        opened,
        "focus, enlarging and resizing open nothing"
    );

    // A detour: Desk 05 full screen on surface 0 is heard meanwhile; back
    // on the grid Desk 07 is heard again and nothing more opened.
    menu(app, 5, "Full screen");
    step(app.wait_for_gone("id:multiview-tile", Duration::from_secs(2)));
    only_heard(app, &seen, 3, 0, "the detour's full-screen tile");
    let detour = opens(3);
    step(app.back());
    step(app.wait_for(&tile(7)));
    only_heard(app, &seen, 3, 2, "back on the grid");
    badges(app, 3, 7);
    assert_eq!(opens(3), detour, "the way back opens nothing");

    // Removing the heard tile (its ✕): the sound goes back to Desk 03,
    // heard before it (now on surface 1, traded for the detour).
    step(app.hover(&tile(7)));
    let remove = format!("label:Remove screen: {}", channel(7));
    if !app.screen().matches(&remove).unwrap() {
        step(app.press(&tile(7)));
    }
    step(app.wait_for(&remove));
    step(app.press(&remove));
    step(app.wait_for_gone(&tile(7), Duration::from_secs(2)));
    only_heard(app, &seen, 2, 1, "the heard tile removed");
    badges(app, 2, 3);

    // The pointer chrome's speaker button on Desk 05 moves it there.
    step(app.hover(&tile(5)));
    let listen = format!("label:Listen to this: {}", channel(5));
    if !app.screen().matches(&listen).unwrap() {
        step(app.press(&tile(5)));
    }
    step(app.wait_for(&listen));
    step(app.press(&listen));
    only_heard(app, &seen, 2, 0, "the pointer's Listen to this");
    badges(app, 2, 5);

    // By keys alone, as a remote: focus Desk 03, OK opens its menu, Down
    // to Listen to this (after Add screen, Search and add, Change channel
    // and Pause), OK.
    step(app.press(&tile(3)));
    step(app.press_key("enter"));
    step(app.wait_for(&format!("{MENU} >> text:Listen to this")));
    for _ in 0..4 {
        step(app.press_key("down"));
    }
    step(app.press_key("enter"));
    step(app.wait_for_gone(MENU, Duration::from_secs(2)));
    only_heard(app, &seen, 2, 1, "the remote's Listen to this");
    badges(app, 2, 3);
}

// Player windows ("Watch in new window") exist on Linux and macOS only.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn one_of_the_main_player_and_its_windows_is_heard_and_each_shows_it() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    let seen = fixture.seen.clone();
    crate::desktop_workflows::play_then_guide(app, 3);

    // Two player windows: neither takes the sound.
    for n in [5, 6] {
        step(app.right_click(&format!("id:guide-ui >> text:{}", channel(n))));
        step(app.wait_for("id:menu-panel >> text:Watch in new window"));
        step(app.press("id:menu-panel >> text:Watch in new window"));
    }
    step(
        app.wait_until("both windows play", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(1) && iptv_app::player_has_source(2)
        }),
    );
    only_heard(app, &seen, 3, 0, "opening windows takes no sound");
    assert_eq!(seen.title("player-window-1"), "Desk 05 · muted");
    assert_eq!(seen.title("player-window-2"), "Desk 06 · muted");
    let opened = opens(3);

    // The second window compact, the main player in picture in picture:
    // their titles are fixed, so each shows a badge in its corner.
    let aux = |n: u64| format!("id:auxiliary-player-{n}");
    step(app.hover(&format!("{} >> label:Video", aux(2))));
    let pip = format!("{} >> label:Picture in picture", aux(2));
    if !app.screen().matches(&pip).unwrap() {
        step(app.press(&format!("{} >> label:Video", aux(2))));
    }
    step(app.wait_for(&pip));
    step(app.press(&pip));
    step(app.wait_for(&format!("{} >> id:pip-muted", aux(2))));
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    show_controls(app, "id:app-ui >> label:Picture in picture");
    step(app.press("id:app-ui >> label:Picture in picture"));
    step(app.wait_for("id:primary-pip >> id:pip-heard"));
    only_heard(
        app,
        &seen,
        3,
        0,
        "compact and picture in picture move nothing",
    );

    // Listen to this in the first (normal) window's play bar.
    step(app.hover(&format!("{} >> label:Video", aux(1))));
    let listen = format!("{} >> label:Listen to this", aux(1));
    if !app.screen().matches(&listen).unwrap() {
        step(app.press(&format!("{} >> label:Video", aux(1))));
    }
    step(app.wait_for(&listen));
    step(app.press(&listen));
    only_heard(app, &seen, 3, 1, "the window's Listen to this");
    assert_eq!(seen.title("player-window-1"), "Desk 05");
    step(app.wait_for("id:primary-pip >> id:pip-muted"));
    assert!(
        app.screen()
            .matches(&format!("{} >> id:pip-muted", aux(2)))
            .unwrap()
    );
    assert!(
        !app.screen().matches(&listen).unwrap(),
        "the heard window offers no Listen to this"
    );
    // That window to the operating system's full screen and back (a
    // double click on its video): the sound stays.
    use nsh_core::desktop::WindowMode;
    let mode = |app: &mut Headless| app.window_state("player-window-1").unwrap().unwrap().mode;
    step(app.press(&format!("{} >> label:Video", aux(1))));
    step(app.press(&format!("{} >> label:Video", aux(1))));
    app.settle();
    assert_eq!(mode(app), WindowMode::FullScreen);
    only_heard(app, &seen, 3, 1, "a window's full screen");
    // Escape (its panels first, if a press opened them) leaves it.
    for _ in 0..3 {
        if mode(app) == WindowMode::Normal {
            break;
        }
        step(app.press_key("escape"));
        app.settle();
    }
    assert_eq!(mode(app), WindowMode::Normal);
    only_heard(app, &seen, 3, 1, "back from a window's full screen");

    // Listen to this among the compact window's controls.
    step(app.hover(&format!("{} >> id:pip-video", aux(2))));
    step(app.wait_for(&format!("{} >> id:pip-listen", aux(2))));
    step(app.press(&format!("{} >> id:pip-listen", aux(2))));
    only_heard(app, &seen, 3, 2, "the compact window's Listen to this");
    step(app.wait_for(&format!("{} >> id:pip-heard", aux(2))));
    assert_eq!(seen.title("player-window-1"), "Desk 05 · muted");

    // Closing the heard window: back to the one heard before it.
    step(app.press_window_close("player-window-2"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-2").unwrap().is_none());
    only_heard(app, &seen, 2, 1, "the heard window closed");
    assert_eq!(seen.title("player-window-1"), "Desk 05");

    // Listen to this on the main player's picture-in-picture window.
    step(app.hover("id:primary-pip >> id:pip-video"));
    step(app.wait_for("id:primary-pip >> id:pip-listen"));
    step(app.press("id:primary-pip >> id:pip-listen"));
    only_heard(app, &seen, 2, 0, "the main player's Listen to this");
    step(app.wait_for("id:primary-pip >> id:pip-heard"));
    assert_eq!(seen.title("player-window-1"), "Desk 05 · muted");

    // Back in the app, the main play bar's Listen to this, while the
    // window has the sound. (A press on the video also opens or closes
    // the window's panels, over its play bar.)
    for _ in 0..3 {
        step(app.hover(&format!("{} >> label:Video", aux(1))));
        app.settle();
        if app.screen().matches(&listen).unwrap() {
            break;
        }
        step(app.press(&format!("{} >> label:Video", aux(1))));
        app.settle();
    }
    step(app.wait_for(&listen));
    step(app.press(&listen));
    only_heard(app, &seen, 2, 1, "the window again");
    step(app.hover("id:primary-pip >> id:pip-video"));
    step(app.wait_for("id:primary-pip >> id:pip-back"));
    step(app.press("id:primary-pip >> id:pip-back"));
    app.settle();
    let main_listen = "id:app-ui >> id:play-bar-listen";
    show_controls(app, main_listen);
    step(app.press(main_listen));
    only_heard(app, &seen, 2, 0, "the main play bar's Listen to this");
    assert_eq!(seen.title("player-window-1"), "Desk 05 · muted");
    assert_eq!(opens(2), opened[..2], "moving the sound opens nothing");
}
