use crate::headless::Headless;
use crate::personalization;
use iptv_settings::Settings;
use personalization::*;
use std::time::Duration;
fn settings(app: &mut Headless) {
    crate::personalization::open_program_menu(app);
    step(app.wait_for("text:Manage favorites…"));
    step(app.press("text:Settings >> nth=0"));
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
            .filter(|n| n.test_id.as_deref() == Some("guide-program"))
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
        .filter(|n| n.test_id.as_deref() == Some("guide-program"))
        .map(|n| n.bounds.size.width.0)
        .fold(0.0_f32, f32::max);
    assert!(
        ((largest + 3.0) / (before_cell + 3.0) - 150.0 / 240.0).abs() < 0.02,
        "visible time span changes the actual program widths: {before_cell} -> {largest}"
    );
    settings(app);
    setting(app, "Channel numbers");
    close(app);
    step(app.wait(Duration::from_millis(50)));
    assert!(width(app, "guide-channel-number") > 0.0);
}

#[test]
fn the_about_page_shows_the_privacy_address_before_opening_and_after_browser_failure() {
    const URL: &str = "https://teng-lin.github.io/patina-iptv/privacy-policy/";
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, Settings::new(), false);
    let app = &mut fixture.app;
    settings(app);
    step(app.press("text:About"));
    step(app.press_key("right"));
    // Reach the privacy row by its help text, independently of the number of
    // update preferences that precede it. Focusing must not open the browser.
    for _ in 0..8 {
        if app
            .screen()
            .matches(&format!("id:settings-screen >> text:{URL}"))
            .unwrap()
        {
            break;
        }
        step(app.press_key("down"));
        app.idle(Duration::from_millis(20)).unwrap();
    }
    step(app.wait_for(&format!("id:settings-screen >> text:{URL}")));
    assert!(app.screen().opened_urls().is_empty());

    step(app.press("text:Privacy policy"));
    assert_eq!(app.screen().opened_urls(), [URL]);
    assert!(
        app.screen()
            .matches(&format!("id:settings-screen >> text:{URL}"))
            .unwrap()
    );

    step(app.fail_open_urls(URL));
    step(app.press("text:Privacy policy"));
    step(app.wait_for(&format!("id:app-notice >> text:{URL}")));
    assert_eq!(app.screen().opened_urls(), [URL, URL]);
}

#[test]
fn the_about_update_action_keeps_manual_downloads_viewer_initiated() {
    use crate::app_updates::{Api, release};
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    // Keep the default weekly discovery in flight while navigating the actual
    // About page; the manual check then joins or refreshes that owned request.
    let mut fixture = start(home, Settings::new(), false);
    let app = &mut fixture.app;
    settings(app);
    step(app.press("text:About"));
    step(app.wait_for("id:settings-content >> text:Weekly"));
    assert!(app.screen().opened_urls().is_empty());
    api.serve(200, release("9.8.7", true));
    step(app.press("id:settings-content >> text:Check for updates"));
    step(app.wait_for("id:app-update-panel"));
    step(app.wait_for("text:/Latest version: 9.8.7/"));
    step(app.wait_for("id:app-update-notes >> text:/The complete ending of the notes/"));
    assert!(api.requests() > 0, "the actual HTTP discovery ran");
    assert!(
        app.screen().opened_urls().is_empty(),
        "checking and reading notes do not start a browser download"
    );
    assert!(
        app.screen().dialogs().is_empty(),
        "checking does not prompt an installer"
    );

    let published: serde_json::Value = serde_json::from_str(&release("9.8.7", true)).unwrap();
    let (button, expected) = if app
        .screen()
        .matches("id:app-update-panel >> text:Download package")
        .unwrap()
    {
        (
            "Download package",
            published[0]["assets"][0]["browser_download_url"]
                .as_str()
                .unwrap(),
        )
    } else {
        // A development Linux executable has no installed package marker. Its
        // explicit manual route is the release page, rather than an APK or a
        // package from another installation channel.
        (
            "Release notes and downloads",
            published[0]["html_url"].as_str().unwrap(),
        )
    };
    step(app.press(&format!("id:app-update-panel >> text:{button}")));
    step(app.wait_until(
        "viewer-requested manual download opens",
        Duration::from_secs(5),
        |screen| !screen.opened_urls().is_empty(),
    ));
    assert_eq!(app.screen().opened_urls(), [expected]);
}

#[test]
fn changing_the_guide_layout_preference_retires_the_keyboard_override() {
    let Some(home) = isolated!() else { return };
    let mut preference = Settings::new();
    preference
        .set("guide.browse_mode", iptv_settings::Value::choice("overlay"))
        .unwrap();
    let mut f = start(home, preference, true);
    let app = &mut f.app;
    let player = |app: &Headless| {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("video-player"))
            .expect("persistent player")
    };
    let id = player(app).id;
    assert_eq!(player(app).bounds.size.width.0, 1920.0);
    step(app.press_key("o"));
    assert!(player(app).bounds.size.width.0 < 1920.0);

    for choice in ["Video beside the list", "List over the video"] {
        settings(app);
        // The rows also have a heading called Guide layout. Locate the
        // editable row by the accessible label that includes its value.
        for _ in 0..100 {
            if app.press("label:/^Guide layout · /").is_ok() {
                break;
            }
            step(app.press_key("down"));
        }
        step(app.wait_for("id:settings-editor"));
        step(app.press(&format!("id:settings-editor >> text:/{choice}/")));
        close(app);
    }
    assert_eq!(player(app).bounds.size.width.0, 1920.0);
    assert_eq!(player(app).id, id);
}

#[test]
fn category_switches_remap_focus_to_the_same_channel_and_preserve_favorites() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, Settings::new(), true);
    let app = &mut fixture.app;
    step(app.press_key("down"));
    step(app.press_key("down"));

    settings(app);
    setting(app, "All channels category");
    setting(app, "Favorites category");
    setting(app, "Recently watched category");
    close(app);
    step(app.wait_for("role:grid[Sport]"));
    step(app.wait_for("text:★ Local C"));

    step(app.open_guide_groups());
    for name in ["Favorites", "Recently watched", "All channels"] {
        assert!(
            !app.screen()
                .matches(&format!("id:guide-groups >> text:{name}"))
                .unwrap()
        );
    }
    for name in ["News", "Sport"] {
        step(app.wait_for(&format!("id:guide-groups >> text:{name}")));
    }
    let dir = fixture.path.parent().unwrap();
    personalization::saved_settings(app, &dir.join("settings.json"), |settings| {
        [
            iptv_settings::keys::SHOW_ALL_CHANNELS,
            iptv_settings::keys::SHOW_FAVORITES,
            iptv_settings::keys::SHOW_RECENT,
        ]
        .into_iter()
        .all(|key| !settings.bool(key, &iptv_settings::Target::Global))
    });
}
