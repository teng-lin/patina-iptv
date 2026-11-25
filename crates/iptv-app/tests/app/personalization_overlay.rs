use crate::personalization;
use iptv_settings::{Settings, Value};
use nsh_core::ViewKind;
use personalization::*;
use std::time::Duration;

fn overlay(stay: Option<bool>) -> Settings {
    let mut s = Settings::new();
    s.set("guide.browse_mode", Value::choice("overlay"))
        .unwrap();
    // The retired switch: stored on, it reads as "Keep it open" (#281).
    if let Some(stay) = stay {
        s.set("guide.stay_on_guide", Value::Bool(stay)).unwrap();
    }
    s
}

fn player(app: &mut crate::headless::Headless) -> nsh_testing::LayoutNode {
    let players: Vec<_> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.kind == ViewKind::Native)
        .collect();
    assert_eq!(players.len(), 1, "one persistent player surface");
    players[0].clone()
}

#[test]
fn overlay_choosing_a_channel_watches_it_full_screen_by_default() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, overlay(None), true);
    let app = &mut f.app;
    // In the overlay layout, OK tunes after the switch delay and closes the guide.
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    step(app.wait_for("id:player-banner"));
    let id = player(app).id;
    assert_eq!(player(app).bounds.size.width.0, 1920.0);
    // Back returns to the guide over the same video.
    step(app.back());
    step(app.back());
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favorites…"));
    assert_eq!(player(app).id, id);
    assert_eq!(player(app).bounds.size.width.0, 1920.0);
}

#[test]
fn overlay_keeps_the_guide_when_the_viewer_asks_and_back_returns_to_video() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, overlay(Some(true)), true);
    let app = &mut f.app;
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    let id = player(app).id;
    assert_eq!(player(app).bounds.size.width.0, 1920.0);
    // The guide menu confirms it stayed in browsing.
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favorites…"));
    step(app.back());
    step(app.back());
    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
    step(app.back());
    step(app.back());
    // Re-entering the guide keeps its configured overlay layout and surface identity.
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favorites…"));
    assert_eq!(player(app).id, id);
    assert_eq!(player(app).bounds.size.width.0, 1920.0);
}

/// Back at the top level is the platform's (#277) in the overlay layout
/// too: with nothing playing, the rail registers no Back handler, so
/// Android leaves at once and shows its predictive back. A channel number
/// being typed there keeps the handler until Back has cancelled it.
#[test]
fn overlay_rail_with_nothing_playing_hands_back_to_the_platform() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, overlay(None), true);
    let app = &mut f.app;
    step(app.set_environment("TV remote", |env| {
        env.idiom = nsh_core::platform::Idiom::Tv;
    }));
    step(app.open_guide_groups());
    step(app.press_key("left"));
    step(app.press_key("left"));
    let handed = app
        .back()
        .expect_err("Back at the overlay layout's rail is the platform's");
    assert!(handed.contains("nothing to go back from"), "{handed}");
    // A digit starts a channel number: the first Back cancels it.
    step(app.press_key("2"));
    step(app.back());
    let handed = app
        .back()
        .expect_err("with the number cancelled, Back is the platform's again");
    assert!(handed.contains("nothing to go back from"), "{handed}");
}

/// Where the host asks before leaving (`iptv_app::confirm_leaving`, the
/// Tizen NSH host: Samsung's exit confirmation, #277), Back at the top
/// level stays the app's and opens No/Yes with No focused: OK there, or
/// Back, closes it and the app stays; Yes leaves.
#[test]
fn a_host_that_asks_before_leaving_confirms_back_at_the_top_level() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static LEFT: AtomicUsize = AtomicUsize::new(0);
    let Some(home) = isolated!() else { return };
    iptv_app::confirm_leaving(|| {
        LEFT.fetch_add(1, Ordering::SeqCst);
    });
    let mut f = start(home, overlay(None), true);
    let app = &mut f.app;
    step(app.set_environment("TV remote", |env| {
        env.idiom = nsh_core::platform::Idiom::Tv;
    }));
    // The overlay layout's rail with nothing playing: the top level.
    step(app.open_guide_groups());
    step(app.press_key("left"));
    step(app.press_key("left"));
    let ask = |app: &mut crate::headless::Headless| {
        step(app.back());
        step(app.wait_for("id:leave-panel >> text:Close Patina?"));
        assert!(app.shows("No") && app.shows("Yes"));
    };
    let gone = |app: &mut crate::headless::Headless| {
        step(app.wait_for_gone("text:Close Patina?", Duration::from_secs(2)));
    };
    ask(app);
    // No is focused: OK closes the prompt, and the app stays.
    step(app.press_key("enter"));
    gone(app);
    assert_eq!(LEFT.load(Ordering::SeqCst), 0);
    // Back closes it too.
    ask(app);
    step(app.back());
    gone(app);
    assert_eq!(LEFT.load(Ordering::SeqCst), 0);
    ask(app);
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    gone(app);
    assert_eq!(LEFT.load(Ordering::SeqCst), 0);
    // Down to Yes, then OK: the host leaves.
    ask(app);
    step(app.press_key("down"));
    step(app.press_key("enter"));
    gone(app);
    assert_eq!(LEFT.load(Ordering::SeqCst), 1);
    // Right selects Yes without wrapping on repeated presses.
    ask(app);
    step(app.press_key("right"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    gone(app);
    assert_eq!(LEFT.load(Ordering::SeqCst), 2);
    // Left selects No again, even after Right selected Yes.
    ask(app);
    step(app.press_key("right"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("enter"));
    gone(app);
    assert_eq!(LEFT.load(Ordering::SeqCst), 2);
}

/// The first run's welcome screen is the app's first screen too: where
/// the host asks before leaving, Back there asks over it (#277), and No
/// or Back returns to it.
#[test]
fn a_host_that_asks_before_leaving_confirms_back_on_the_welcome_screen() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static LEFT: AtomicUsize = AtomicUsize::new(0);
    let Some(_home) = isolated!() else { return };
    iptv_app::confirm_leaving(|| {
        LEFT.fetch_add(1, Ordering::SeqCst);
    });
    let mut config = nsh_engine::EngineConfig {
        viewport: nsh_core::Size::new(nsh_core::prelude::px(1920.0), nsh_core::prelude::px(1080.0)),
        ..nsh_engine::EngineConfig::new(nsh_core::element::Root::of::<iptv_app::App>())
    };
    config.environment.idiom = nsh_core::platform::Idiom::Tv;
    let mut app = crate::headless::Headless::start(config).unwrap();
    step(app.wait_for("id:wizard-welcome"));
    step(app.back());
    step(app.wait_for("id:leave-panel >> text:Close Patina?"));
    step(app.back());
    step(app.wait_for_gone("text:Close Patina?", Duration::from_secs(2)));
    step(app.wait_for("id:wizard-welcome"));
    assert_eq!(LEFT.load(Ordering::SeqCst), 0);
    step(app.back());
    step(app.wait_for("id:leave-panel >> text:Close Patina?"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Close Patina?", Duration::from_secs(2)));
    assert_eq!(LEFT.load(Ordering::SeqCst), 1);
}
