//! Guide, player and display settings the device decides (#281), run
//! headless with Android's observer simulated (`device_wiring.rs`,
//! `DeviceComponent.kt`):
//!
//! - the rows offered on a phone, a tablet, a TV and a desktop, with and
//!   without a keyboard or a D-pad, in compact and expanded windows;
//! - the one "After choosing a channel" choice, which a phone or a tablet
//!   does not follow, and a compact window, where the guide shows its
//!   preview whatever its layout setting;
//! - panels that stay while a screen reader or Switch Access runs, and
//!   wait as long as Android's accessibility timeout asks.
//!
//! Synthetic data only: the personalization playlist (M3U).

use std::time::Duration;

use crate::{Headless, personalization};
use iptv_settings::{Settings, Value};
use nsh_core::prelude::px;
use nsh_core::{EventKind, EventPayload, NodeId, Size};
use personalization::step;

const WAIT: Duration = Duration::from_secs(5);
/// The strip's hint line: the strip is open.
const STRIP: &str = "text:/M or Esc hide/";

fn node(app: &Headless, test_id: &str) -> NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(test_id))
        .unwrap_or_else(|| panic!("missing {test_id}"))
        .id
}

/// The host reports `name` with `data`, as Android's activity does.
fn native(app: &mut Headless, name: &str, data: serde_json::Value) {
    app.engine().control().dispatch_event(
        node(app, "device-signals"),
        EventKind::NativeEvent,
        EventPayload::Native {
            name: name.into(),
            data,
        },
    );
    app.settle();
    step(app.wait(Duration::ZERO));
}

/// The host reports `form` with this input, as Android's activity does.
pub(crate) fn report(app: &mut Headless, form: &str, keyboard: bool, dpad: bool) {
    native(
        app,
        "device",
        serde_json::json!({ "form": form, "keyboard": keyboard, "dpad": dpad }),
    );
}

fn resize(app: &mut Headless, (width, height): (f32, f32)) {
    step(app.set_viewport(Size::new(px(width), px(height))));
    app.settle();
}

/// The rows offered now (`device::offered_rows`): the screen's own rows
/// are a virtual list, and most of them lie below its fold.
fn rows(app: &mut Headless) -> Vec<String> {
    app.settle();
    let texts = app.screen().texts();
    let line = texts
        .iter()
        .find(|t| t.starts_with("rows:"))
        .unwrap_or_else(|| panic!("no row probe in {:?}", &texts[..texts.len().min(3)]));
    line.split_whitespace().skip(1).map(str::to_owned).collect()
}

fn fixture(here: &str, settings: Settings) -> Option<personalization::Fixture> {
    let home = crate::isolated::run_with_env(here, &[("IPTV_TEST_DEVICE_SIGNALS", "1")])?;
    let mut fixture = personalization::start(home, settings, true);
    resize(&mut fixture.app, (1280.0, 720.0));
    Some(fixture)
}

/// Rows every device keeps: the panel timeouts under Accessibility, the
/// jumps on the Advanced page, the guide's minute span (every layout
/// of the guide has a time grid).
const EVERYWHERE: [&str; 3] = [
    "guide.visible_minutes",
    "catchup.forward_step",
    "catchup.rewind_step",
];

#[track_caller]
fn expect_rows(app: &mut Headless, what: &str, shown: &[&str], hidden: &[&str]) {
    let rows = rows(app);
    for key in EVERYWHERE.iter().chain(shown) {
        assert!(
            rows.iter().any(|r| r == key),
            "{what}: {key} shown in {rows:?}"
        );
    }
    for key in hidden {
        assert!(
            !rows.iter().any(|r| r == key),
            "{what}: {key} hidden in {rows:?}"
        );
    }
}

#[test]
fn settings_rows_follow_the_form_factor_the_input_and_the_window() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), Settings::new()) else {
        return;
    };
    let app = &mut fixture.app;
    const LAYOUT: [&str; 3] = [
        "appearance.ui_transparency",
        "guide.browse_mode",
        "guide.list_browse_mode",
    ];

    // A desktop: everything but the TV's full-screen timeout.
    expect_rows(
        app,
        "desktop",
        &[
            "general.clock",
            "guide.after_choosing",
            "player.info_on_switch",
        ],
        &["guide.fullscreen_timeout"],
    );
    expect_rows(app, "desktop", &LAYOUT, &[]);

    // A TV with its remote: the full-screen timeout too.
    report(app, "tv", false, true);
    expect_rows(
        app,
        "tv",
        &[
            "general.clock",
            "guide.after_choosing",
            "guide.fullscreen_timeout",
            "player.info_on_switch",
        ],
        &[],
    );

    // A touch-only phone: the system's clock, always full screen after
    // choosing, no zapping keys.
    report(app, "phone", false, false);
    expect_rows(
        app,
        "touch phone",
        &LAYOUT,
        &[
            "general.clock",
            "guide.after_choosing",
            "guide.fullscreen_timeout",
            "player.info_on_switch",
        ],
    );
    // A keyboard, or a D-pad, brings channel-change info back.
    report(app, "phone", true, false);
    expect_rows(
        app,
        "phone with a keyboard",
        &["player.info_on_switch"],
        &[],
    );
    report(app, "tablet", false, true);
    expect_rows(
        app,
        "tablet with a D-pad",
        &["player.info_on_switch"],
        &["general.clock", "guide.after_choosing"],
    );

    // A compact window: the layout decides, on a tablet and a desktop.
    report(app, "tablet", false, false);
    resize(app, (400.0, 700.0));
    expect_rows(
        app,
        "compact tablet",
        &[],
        &[LAYOUT.as_slice(), &["player.info_on_switch"]].concat(),
    );
    report(app, "desktop", true, false);
    expect_rows(app, "compact desktop", &["general.clock"], &LAYOUT);
    resize(app, (1280.0, 720.0));
    expect_rows(app, "expanded desktop", &LAYOUT, &[]);
}

fn overlay(after_choosing: &str) -> Settings {
    let mut s = Settings::new();
    s.set("guide.browse_mode", Value::choice("overlay"))
        .unwrap();
    s.set("guide.after_choosing", Value::choice(after_choosing))
        .unwrap();
    s.set("guide.switch_delay", Value::ms(0)).unwrap();
    s
}

/// OK on the focused channel of the guide over the video: whether it
/// went full screen (its banner) or the guide stayed (its menu opens).
fn choose(app: &mut Headless) -> bool {
    step(app.press_key("enter"));
    step(app.wait_until("the channel tunes", WAIT, |_| {
        iptv_app::player_has_source(0)
    }));
    step(app.run_for(Duration::from_millis(500)));
    let full = app.screen().matches("id:player-banner").unwrap();
    if !full {
        step(app.press_key("m"));
        step(app.wait_for("id:menu-panel >> text:Channel options…"));
        step(app.back());
    }
    full
}

#[test]
fn the_one_stay_choice_keeps_the_guide_over_the_video_except_on_handhelds() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), overlay("stay")) else {
        return;
    };
    let app = &mut fixture.app;
    // A desktop (and a TV) keep the guide over the video.
    assert!(!choose(app), "a desktop stays in the guide");
    // A phone or a tablet goes full screen, whatever was chosen.
    report(app, "tablet", false, false);
    step(app.press_key("down"));
    assert!(choose(app), "a tablet goes full screen");
}

#[test]
fn choosing_goes_full_screen_by_default_on_a_tv() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), overlay("full_screen"))
    else {
        return;
    };
    let app = &mut fixture.app;
    report(app, "tv", false, true);
    assert!(choose(app), "full screen");
}

#[test]
fn a_compact_window_shows_the_guide_beside_its_preview() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), overlay("stay")) else {
        return;
    };
    let app = &mut fixture.app;
    report(app, "desktop", true, false);
    // A channel plays under the guide, which stays over it.
    assert!(!choose(app), "the guide stays over the video");
    // The overlay layout (the video fills the window) until the window
    // turns compact, then the preview layout; back again when it widens.
    let video = video_width;
    assert_eq!(video(app.screen()), 1280.0, "the guide over the video");
    resize(app, (400.0, 700.0));
    step(app.wait_until("the preview layout", WAIT, |s| {
        let w = video(s);
        w > 0.0 && w < 400.0
    }));
    resize(app, (1280.0, 720.0));
    step(app.wait_until("the overlay layout again", WAIT, |s| video(s) == 1280.0));
}

/// The width of the player's video (0 when none shows).
fn video_width(s: &nsh_testing::Screen) -> f32 {
    s.layout_nodes()
        .into_iter()
        .find(|n| {
            n.kind == nsh_core::ViewKind::Native && n.test_id.as_deref() != Some("device-signals")
        })
        .map_or(0.0, |n| n.bounds.size.width.0)
}

#[test]
fn the_guide_key_over_full_screen_video_shows_the_preview_in_a_compact_window() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), Settings::new()) else {
        return;
    };
    let app = &mut fixture.app;
    report(app, "phone", false, false);
    crate::browse_fixture::choose_fullscreen(app);
    step(app.wait_for("id:player-banner"));
    resize(app, (400.0, 700.0));
    step(app.wait_until("full screen", WAIT, |s| video_width(s) == 400.0));
    // The Guide key opens the guide beside its preview, not over the video.
    step(app.press_key("g"));
    step(app.wait_until("the preview layout", WAIT, |s| {
        let w = video_width(s);
        w > 0.0 && w < 400.0
    }));
}

/// Full screen with the control strip open, which hides after 1 s.
fn strip(here: &str) -> Option<personalization::Fixture> {
    let mut settings = Settings::new();
    settings
        .set("player.controls_panel_timeout", Value::secs(1))
        .unwrap();
    let mut fixture = fixture(here, settings)?;
    let app = &mut fixture.app;
    crate::browse_fixture::choose_fullscreen(app);
    step(app.wait_for("id:player-banner"));
    report(app, "tv", false, true);
    open_strip(app);
    Some(fixture)
}

fn open_strip(app: &mut Headless) {
    if !app.screen().matches(STRIP).unwrap() {
        step(app.press_key("m"));
    }
    step(app.wait_for_within(STRIP, WAIT));
}

#[track_caller]
fn stays(app: &mut Headless, what: &str) {
    step(app.run_for(Duration::from_secs(6)));
    assert!(
        app.screen().matches(STRIP).unwrap(),
        "{what}: the strip stays"
    );
}

#[track_caller]
fn hides(app: &mut Headless, within: Duration) {
    step(app.run_for(within));
    step(app.wait_for_gone(STRIP, Duration::from_secs(3)));
}

#[test]
fn the_control_strip_stays_while_a_screen_reader_runs() {
    fn here() {}
    let Some(mut fixture) = strip(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    step(app.set_environment("TalkBack on", |env| env.screen_reader = true));
    stays(app, "TalkBack");
    // The viewer hides it; shown again, it still stays.
    step(app.press_key("m"));
    step(app.wait_for_gone(STRIP, Duration::from_secs(3)));
    open_strip(app);
    stays(app, "TalkBack, shown again");
    // Without the reader its own delay counts again.
    step(app.set_environment("TalkBack off", |env| env.screen_reader = false));
    hides(app, Duration::from_millis(1_500));
}

#[test]
fn the_control_strip_follows_switch_access_and_the_recommended_timeout() {
    fn here() {}
    let Some(mut fixture) = strip(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    report(app, "tablet", false, false);
    // Switch Access runs: the strip stays until hidden.
    native(
        app,
        "accessibility",
        serde_json::json!({ "controls_ms": 0, "text_ms": 0, "switch_access": true }),
    );
    stays(app, "Switch Access");
    // Switch Access off, a "time to take action" of 10 s: the strip waits
    // that long rather than the tablet’s fixed 7 s.
    native(
        app,
        "accessibility",
        serde_json::json!({ "controls_ms": 10_000, "text_ms": 0, "switch_access": false }),
    );
    open_strip(app);
    step(app.press_key("right"));
    step(app.run_for(Duration::from_millis(8_000)));
    assert!(
        app.screen().matches(STRIP).unwrap(),
        "the strip waits the recommended 10 s"
    );
    hides(app, Duration::from_millis(2_500));
}

#[test]
fn ipad_switch_control_holds_panels_and_releases_the_timer_when_disabled() {
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[
            ("IPTV_TEST_DEVICE_SIGNALS", "1"),
            ("IPTV_TEST_PLATFORM", "ipad"),
        ],
    ) else {
        return;
    };
    let mut fixture = personalization::start(home, Settings::new(), true);
    let app = &mut fixture.app;
    native(
        app,
        "accessibility",
        serde_json::json!({"switch_access": true}),
    );
    crate::browse_fixture::choose_fullscreen(app);
    step(app.wait_for("id:player-banner"));
    step(app.run_for(Duration::from_secs(10)));
    assert!(app.screen().matches("id:player-banner").unwrap());
    open_strip(app);
    stays(app, "iPad Switch Control");
    // A keyboard attaching changes the gates while Switch Control stays active.
    native(
        app,
        "device",
        serde_json::json!({"keyboard": true, "dpad": false}),
    );
    stays(app, "iPad Switch Control with keyboard");
    native(
        app,
        "accessibility",
        serde_json::json!({"switch_access": false}),
    );
    hides(app, Duration::from_secs(10));
}

#[test]
fn ipad_switch_control_holds_panels_and_the_play_bar_until_it_stops() {
    fn here() {}
    let home = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[
            ("IPTV_TEST_DEVICE_SIGNALS", "1"),
            ("IPTV_TEST_PLATFORM", "ipad"),
        ],
    );
    let Some(home) = home else {
        return;
    };
    let mut fixture = personalization::start(home, Settings::new(), true);
    let app = &mut fixture.app;
    crate::browse_fixture::choose_fullscreen(app);
    native(
        app,
        "accessibility",
        serde_json::json!({"switch_access": true}),
    );
    open_strip(app);
    step(app.run_for(Duration::from_secs(20)));
    assert!(
        app.screen().matches(STRIP).unwrap(),
        "Switch Control holds the strip"
    );
    step(app.press_key("m"));
    step(app.press("id:video"));
    step(app.wait_for("id:play-bar-layer"));
    step(app.run_for(Duration::from_secs(20)));
    assert!(
        app.screen().matches("id:play-bar-layer").unwrap(),
        "Switch Control holds the play bar"
    );
    native(
        app,
        "accessibility",
        serde_json::json!({"switch_access": false}),
    );
    step(app.run_for(Duration::from_secs(4)));
    step(app.wait_for_gone("id:play-bar-layer", WAIT));
}
