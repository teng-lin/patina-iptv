//! The empty Movies and Shows catalogue in a compact window (#130): its
//! message and action are the one pane (`iptv_core::vod::Screen::one_pane`).
//! At 400×600 with an M3U-only library the categories give way, the
//! message keeps a readable width, Add playlist and Close stay inside the
//! window for a pointer or a finger, and the keyboard still moves between
//! the rail and the action — at Medium and at Extra large text.
//!
//! Synthetic data only: the personalization playlist (M3U).

use std::time::Duration;

use crate::personalization::step;
use iptv_settings::{Settings, Value};
use nsh_core::{Size, prelude::*};

const WINDOW: (f32, f32) = (400.0, 600.0);
const M3U_ONLY: &str = "Movies and shows come from Xtream Codes and Stalker playlists. M3U playlists have live channels only.";

fn bounds(app: &crate::Headless, matches: impl Fn(&nsh_testing::LayoutNode) -> bool) -> [f32; 4] {
    let node = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| !n.hidden && matches(n))
        .expect("laid out");
    let b = node.bounds;
    [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
}

fn by_id(id: &'static str) -> impl Fn(&nsh_testing::LayoutNode) -> bool {
    move |n| n.test_id.as_deref() == Some(id)
}

#[track_caller]
fn inside_the_window(what: &str, b: [f32; 4]) {
    let (w, h) = WINDOW;
    assert!(
        b[2] > 0.0
            && b[3] > 0.0
            && b[0] >= -0.5
            && b[1] >= -0.5
            && b[0] + b[2] <= w + 0.5
            && b[1] + b[3] <= h + 0.5,
        "{what} {b:?} inside the {w}x{h} window"
    );
}

/// The one-pane empty catalogue: no categories, a message as wide as most
/// of the window, its action and Close inside it.
#[track_caller]
fn one_pane(app: &mut crate::Headless) {
    step(app.wait_for(&format!("text:{M3U_ONLY}")));
    step(app.wait_for("id:vod-empty >> label:Add playlist"));
    for id in ["id:vod-category", "id:vod-continue"] {
        assert_eq!(app.screen().count(id).unwrap(), 0, "no categories");
    }
    let (w, _) = WINDOW;
    let pane = bounds(app, by_id("vod-empty"));
    assert!(
        pane[2] >= w * 0.6,
        "the message pane {pane:?} gets the window's width"
    );
    let message = bounds(app, |n| {
        n.text.as_ref().is_some_and(|t| t.text.text == M3U_ONLY)
    });
    inside_the_window("the message", message);
    assert!(
        message[2] >= w * 0.5,
        "the message {message:?} has a readable width"
    );
    let action = bounds(app, by_id("vod-empty-action"));
    inside_the_window("Add playlist", action);
    assert!(
        action[1] >= message[1] + message[3],
        "the action follows the message"
    );
    let close = app.screen().id("id:vod-screen >> label:Close").unwrap();
    let close = bounds(app, |n| n.id == close);
    inside_the_window("Close", close);
}

fn empty_catalogue_in_a_small_window(home: crate::isolated::Home, font: &str) {
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings
        .set("appearance.font_size", Value::choice(font))
        .unwrap();
    let mut fixture = crate::personalization::start(home, settings, true);
    let app = &mut fixture.app;
    step(app.set_viewport(Size::new(px(WINDOW.0), px(WINDOW.1))));
    app.settle();
    // A pointer: Movies, then Close on the rail beside the message.
    step(app.wait_for("id:open-movies"));
    step(app.press("id:open-movies"));
    one_pane(app);
    step(app.press("id:vod-screen >> label:Close"));
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(2)));
    // Shows by pointer, then the keyboard: Left reaches the rail, Right
    // the action again (the categories never return), Enter adds a playlist.
    step(app.wait_for("id:open-shows"));
    step(app.press("id:open-shows"));
    one_pane(app);
    step(app.press_key("left"));
    step(app.press_key("right"));
    one_pane(app);
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(2)));
    step(app.wait_for("text:Xtream Codes login"));
}

#[test]
fn the_empty_catalogue_takes_a_small_window_at_medium_text() {
    let Some(home) = isolated!() else { return };
    empty_catalogue_in_a_small_window(home, "medium");
}

#[test]
fn the_empty_catalogue_takes_a_small_window_at_extra_large_text() {
    let Some(home) = isolated!() else { return };
    empty_catalogue_in_a_small_window(home, "extra_large");
}
