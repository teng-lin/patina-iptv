use crate::personalization;
use iptv_settings::Settings;
use nsh_testing::Headless;
use personalization::*;
use std::time::Duration;
fn settings(app: &mut Headless) {
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favourites…"));
    step(app.press("text:Settings >> nth=1"));
    step(app.wait_for("text:Appearance >> nth=0"));
    step(app.press("text:Appearance >> nth=0"));
    step(app.press_key("right"));
}
fn setting(app: &mut Headless, label: &str) {
    let selector = format!("text:{label}");
    // Page/focus effects can still recycle and scroll rows after Right. Wait
    // for that layout before targeting a row by its visible text.
    assert!(app.idle(Duration::from_millis(50)).unwrap());
    for _ in 0..100 {
        if app.screen().matches(&selector).unwrap() && app.press(&selector).is_ok() {
            return;
        }
        step(app.press_key("down"));
        assert!(app.idle(Duration::from_millis(50)).unwrap());
    }
    panic!("missing {label}");
}
fn close(app: &mut Headless) {
    // Each Back leaves one level; a loaded machine may draw the level below
    // only a moment later, so let it settle before looking again.
    for _ in 0..8 {
        if !app.screen().matches("id:settings-screen").unwrap() {
            return;
        }
        step(app.back());
        app.idle(Duration::from_millis(250)).unwrap();
    }
    panic!("settings stayed open");
}
fn width(app: &Headless, id: &str) -> f32 {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some(id))
        .unwrap()
        .bounds
        .size
        .width
        .0
}
#[test]
fn appearance_controls_apply_live_persist_and_change_actual_guide_geometry() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    let before = app
        .screen()
        .span_style("text:★ Local A >> nth=0")
        .unwrap()
        .font_size
        .0;
    let cell_width = |app: &Headless| {
        app.screen()
            .layout_nodes()
            .into_iter()
            .filter(|n| n.test_id.as_deref() == Some("guide-programme"))
            .map(|n| n.bounds.size.width.0)
            .fold(0.0_f32, f32::max)
    };
    let before_cell = cell_width(app);
    let column = width(app, "guide-channel-number");
    let logo = width(app, "guide-channel-logo");
    assert!(column > 0.0 && logo > 0.0);
    settings(app);
    setting(app, "Text size");
    step(app.press("text:Large"));
    close(app);
    let after = app
        .screen()
        .span_style("text:★ Local A >> nth=0")
        .unwrap()
        .font_size
        .0;
    assert!((after / before - 1.15).abs() < 0.01);
    settings(app);
    setting(app, "Channel numbers");
    close(app);
    assert_eq!(width(app, "guide-channel-number"), 0.0);
    settings(app);
    setting(app, "Channel logos");
    close(app);
    assert_eq!(width(app, "guide-channel-logo"), 0.0);
    settings(app);
    setting(app, "Visible guide minutes");
    step(app.press_key("right"));
    step(app.press_key("right"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    close(app);
    let dir = f.path.parent().unwrap();
    let s = personalization::saved_settings(app, &dir.join("settings.json"), |settings| {
        settings.get("guide.visible_minutes").unwrap() == iptv_settings::Value::Int(240)
            && settings.get("guide.show_logos").unwrap() == iptv_settings::Value::Bool(false)
            && settings.get("guide.show_channel_numbers").unwrap()
                == iptv_settings::Value::Bool(false)
    });
    assert_eq!(
        s.get("guide.visible_minutes").unwrap(),
        iptv_settings::Value::Int(240)
    );
    assert_eq!(
        s.get("guide.show_logos").unwrap(),
        iptv_settings::Value::Bool(false)
    );
    assert_eq!(
        s.get("guide.show_channel_numbers").unwrap(),
        iptv_settings::Value::Bool(false)
    );
    // Changing the span affects cell geometry; ruler half-hours get closer.
    let largest = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.test_id.as_deref() == Some("guide-programme"))
        .map(|n| n.bounds.size.width.0)
        .fold(0.0_f32, f32::max);
    assert!(
        ((largest + 3.0) / (before_cell + 3.0) - 150.0 / 240.0).abs() < 0.02,
        "visible time span changes the actual programme widths: {before_cell} -> {largest}"
    );
    settings(app);
    setting(app, "Channel numbers");
    close(app);
    step(app.wait(Duration::from_millis(50)));
    assert!(width(app, "guide-channel-number") > 0.0);
}
