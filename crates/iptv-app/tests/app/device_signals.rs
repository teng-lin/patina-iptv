//! The device signals (#272), run headless with Android's observer
//! simulated (`device.rs`, `DeviceComponent.kt`): the host's reports of a
//! phone, a tablet, a TV and a desktop, a window resized across the size
//! classes and a hardware keyboard attached and detached all reach what
//! the gates read, while the app runs. The settings rows and the control
//! strip follow them without a rebuild; the input changes nothing they
//! offer, so the strip keeps its focus and the same controls (what the
//! form factor and the window change for multiview is
//! `multiview_by_device.rs`).
//!
//! Synthetic data only: the personalization playlist (M3U).

use std::time::Duration;

use crate::{Headless, personalization};
use nsh_core::prelude::px;
use nsh_core::{EventKind, EventPayload, NodeId, Size};
use personalization::step;

const WAIT: Duration = Duration::from_secs(5);
/// The strip's hint line: the strip is open.
const STRIP: &str = "text:/M or Esc hide/";

pub(crate) fn node(app: &Headless, test_id: &str) -> NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(test_id))
        .unwrap_or_else(|| panic!("missing {test_id}"))
        .id
}

/// The host reports `form` with this input, as Android's activity does
/// when the observer attaches and on each configuration change.
pub(crate) fn report(app: &mut Headless, form: &str, keyboard: bool, dpad: bool) {
    app.engine().control().dispatch_event(
        node(app, "device-signals"),
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "device".into(),
            data: serde_json::json!({ "form": form, "keyboard": keyboard, "dpad": dpad }),
        },
    );
    app.settle();
    step(app.wait(Duration::ZERO));
}

/// What the gates see now (`device::describe`; the probe has no size, so
/// it is read rather than waited for as shown).
#[track_caller]
fn expect(app: &mut Headless, signals: &str) {
    app.settle();
    assert_eq!(app.screen().count("id:device-signals-now").unwrap(), 1);
    let texts = app.screen().texts();
    assert!(
        texts.contains(&signals),
        "the gates see {signals:?}; on screen: {:?}",
        &texts[..texts.len().min(3)]
    );
}

fn fixture(here: &str) -> Option<personalization::Fixture> {
    let home = crate::isolated::run_with_env(here, &[("IPTV_TEST_DEVICE_SIGNALS", "1")])?;
    let mut fixture = personalization::start(home, iptv_settings::Settings::new(), false);
    step(fixture.app.set_viewport(Size::new(px(1280.0), px(720.0))));
    fixture.app.settle();
    Some(fixture)
}

#[test]
fn each_form_factor_reaches_the_gates() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    // A desktop build is a desktop with a keyboard until a report says
    // otherwise.
    expect(app, "desktop expanded keyboard");
    report(app, "phone", false, false);
    expect(app, "phone expanded");
    report(app, "tablet", false, false);
    expect(app, "tablet expanded");
    report(app, "tv", false, true);
    expect(app, "tv expanded dpad");
    report(app, "desktop", true, false);
    expect(app, "desktop expanded keyboard");

    // A report the app does not understand changes nothing.
    report(app, "watch", true, false);
    expect(app, "desktop expanded keyboard");
}

#[test]
fn a_window_class_change_reaches_the_gates() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    // A tablet whose window goes to split screen and back: the form
    // factor stays a tablet's, only the window's class changes.
    report(app, "tablet", false, false);
    expect(app, "tablet expanded");
    // The guide is drawn at 0.8 in windows this small, so 600 points hold
    // 750 at its scale (medium) and 400 hold 500 (compact).
    for (width, height, signals) in [
        (600.0, 400.0, "tablet medium"),
        (400.0, 700.0, "tablet compact"),
        (1280.0, 720.0, "tablet expanded"),
    ] {
        step(app.set_viewport(Size::new(px(width), px(height))));
        app.settle();
        expect(app, signals);
    }
}

#[test]
fn a_narrow_android_tablet_keeps_its_scale_and_a_tv_uses_its_reported_form() {
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[
            ("IPTV_TEST_DEVICE_SIGNALS", "1"),
            ("IPTV_TEST_PLATFORM", "android"),
        ],
    ) else {
        return;
    };
    let mut fixture = personalization::start(home, iptv_settings::Settings::new(), false);
    let app = &mut fixture.app;
    step(app.set_viewport(Size::new(px(490.0), px(660.0))));
    step(app.set_environment("narrow Android window", |env| {
        env.idiom = nsh_core::platform::Idiom::Phone;
        env.os = nsh_core::platform::Os::Android;
    }));
    report(app, "phone", false, false);
    expect(app, "phone medium");
    report(app, "tablet", false, false);
    expect(app, "tablet compact");
    step(app.set_viewport(Size::new(px(800.0), px(600.0))));
    report(app, "tv", false, false);
    expect(app, "tv expanded");
}

#[test]
fn a_keyboard_attach_and_detach_keeps_the_strip_focus() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    report(app, "tablet", false, false);
    expect(app, "tablet expanded");

    // Full screen, the control strip open on Channels (Search, then it).
    crate::browse_fixture::choose_fullscreen(app);
    step(app.wait_for("id:player-banner"));
    step(app.press_key("m"));
    step(app.wait_for_within(STRIP, WAIT));
    step(app.press_key("right"));

    // A keyboard is attached, then detached, while the strip is open.
    report(app, "tablet", true, false);
    expect(app, "tablet expanded keyboard");
    report(app, "tablet", false, false);
    expect(app, "tablet expanded");
    // A foldable's change of form factor, too.
    report(app, "phone", false, false);
    expect(app, "phone expanded");

    // The strip is still open on the control the viewer chose: OK there
    // opens the channel list.
    assert!(app.screen().matches(STRIP).unwrap(), "the strip stays open");
    step(app.press_key("enter"));
    step(app.wait_for_within("id:channel-list", WAIT));
}

#[test]
fn ipad_keyboard_reports_attach_and_detach_without_changing_form_factor() {
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
    let mut fixture = personalization::start(home, iptv_settings::Settings::new(), false);
    let app = &mut fixture.app;
    step(app.set_viewport(Size::new(px(1280.0), px(720.0))));
    for keyboard in [false, true, false] {
        app.engine().control().dispatch_event(
            node(app, "device-signals"),
            EventKind::NativeEvent,
            EventPayload::Native {
                name: "device".into(),
                data: serde_json::json!({"keyboard": keyboard, "dpad": false}),
            },
        );
        step(app.wait(Duration::ZERO));
        expect(
            app,
            if keyboard {
                "tablet expanded keyboard"
            } else {
                "tablet expanded"
            },
        );
        let text = app.screen().texts().join(" ");
        let rows = text.split("rows:").nth(1).expect("settings row probe");
        assert_eq!(rows.contains("player.info_on_switch"), keyboard);
    }
}
