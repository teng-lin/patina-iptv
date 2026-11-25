use crate::personalization;
use iptv_settings::{Settings, Value};
use nsh_core::{MountItem, ViewKind};
use personalization::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
fn preview(app: &crate::headless::Headless) -> bool {
    app.screen()
        .layout_nodes()
        .iter()
        // The player: not the device observer (`tv.device`), which has no size.
        .find(|n| n.kind == ViewKind::Native && n.test_id.as_deref() != Some("device-signals"))
        .unwrap()
        .bounds
        .size
        .width
        .0
        < 1900.0
}

#[test]
fn guide_activates_a_live_channel_twice_and_wraps_rows_and_groups() {
    use crate::tv_shared_controls::{persistent, record_players};

    let Some(home) = isolated!() else { return };
    let mut observed = None;
    let mut f = start_with_setup(home, Settings::new(), true, |app| {
        observed = Some(record_players(app));
    });
    let lifetime = observed.unwrap();
    let app = &mut f.app;
    step(app.set_environment("desktop keyboard and pointer", |env| {
        env.idiom = nsh_core::platform::Idiom::Desktop;
        env.os = nsh_core::platform::Os::Linux;
    }));
    let player = lifetime.lock().unwrap().players[0];
    let row = |channel: &str| format!("id:guide-ui >> label:{channel}");
    let selected = |app: &crate::Headless, selector: &str| {
        app.screen().accessibility_state(selector).unwrap().selected
    };

    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    step(app.wait_until(
        "Local A plays in the preview",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0),
    ));
    assert!(preview(app));
    assert!(selected(app, &row("Local A")));
    let opens = iptv_app::player_opens(0);

    step(app.press_key("up"));
    assert!(
        selected(app, &row("Local C")),
        "Up wraps with a playing preview"
    );
    step(app.press_key("down"));
    assert!(
        selected(app, &row("Local A")),
        "Down wraps to the first row"
    );
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner >> text:/Local A$/"));
    assert!(!preview(app));
    step(app.run_for(Duration::from_millis(500)));
    assert_eq!(iptv_app::player_opens(0), opens);
    persistent(app, &lifetime, player);

    step(app.back());
    step(app.back());
    step(app.press_key("tab"));
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.wait_for("role:grid[Favorites]"));
    step(app.press_key("up"));
    step(app.wait_for("role:grid[Sport]"));
    step(app.press_key("down"));
    step(app.wait_for("role:grid[Favorites]"));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("right"));
    assert!(selected(app, &row("Local A")));

    step(app.press(&row("Local B")));
    step(app.run_for(Duration::from_millis(500)));
    step(app.wait_until(
        "Local B plays in the preview",
        Duration::from_secs(5),
        |_| {
            lifetime
                .lock()
                .unwrap()
                .sources
                .last()
                .is_some_and(|url| url == "http://example.test/b")
        },
    ));
    assert!(preview(app));
    assert!(selected(app, &row("Local B")));
    step(app.press(&row("Local B")));
    step(app.wait_for("id:player-banner >> text:/Local B$/"));
    assert!(!preview(app));
    step(app.run_for(Duration::from_millis(500)));
    assert_eq!(iptv_app::player_opens(0), opens + 1);
    persistent(app, &lifetime, player);
}
#[test]
fn rapid_navigation_cancels_tuning_menus_cancel_preview_and_input_extends_fullscreen_timeout() {
    // On a TV: elsewhere a preview never goes full screen by itself (#281).
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[("IPTV_TEST_DEVICE_SIGNALS", "1")],
    ) else {
        return;
    };
    let mut s = Settings::new();
    for (k, v) in [
        ("guide.autoplay", Value::Bool(true)),
        ("guide.after_choosing", Value::choice("stay")),
        ("guide.fullscreen_timeout", Value::secs(2)),
    ] {
        s.set(k, v).unwrap();
    }
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let observed = opened.clone();
    let mut f = start_with_setup(home, s, true, |app| {
        app.observe(move |_, batch| {
            for item in &batch.items {
                if let MountItem::ComponentProps { props, .. } = item {
                    for (key, value) in props {
                        if key.as_ref() == "source"
                            && let Some(source) =
                                value.as_ref().map(crate::support::upstream_source)
                            && let Some(url) = source["url"].as_str()
                        {
                            observed.lock().unwrap().push(url.into());
                        }
                    }
                }
            }
        });
    });
    let app = &mut f.app;
    crate::device_settings::report(app, "tv", false, true);
    step(app.run_for(Duration::from_millis(600)));
    loaded_stream(app, &opened, "http://example.test/a");
    assert!(preview(app));
    opened.lock().unwrap().clear();
    step(app.press_key("down"));
    step(app.run_for(Duration::from_millis(150)));
    step(app.press_key("down"));
    step(app.run_for(Duration::from_millis(499)));
    assert!(opened.lock().unwrap().is_empty());
    step(app.run_for(Duration::from_millis(1)));
    loaded_stream(app, &opened, "http://example.test/c");
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/c"]);
    // Browsing a future cell cancels preview work. Returning to now
    // resumes it for the same playing channel without reopening its stream.
    opened.lock().unwrap().clear();
    step(app.press_key("right"));
    step(app.run_for(Duration::from_millis(600)));
    assert!(opened.lock().unwrap().is_empty());
    step(app.press_key("home"));
    // A non-navigation input extends the inactivity deadline.
    step(app.run_for(Duration::from_millis(1900)));
    assert!(preview(app));
    step(app.press_key("space"));
    step(app.run_for(Duration::from_millis(1900)));
    assert!(preview(app));
    step(app.run_for(Duration::from_millis(101)));
    assert!(!preview(app));
    // Hide the banner then return to the guide.
    step(app.back());
    step(app.back());
    assert!(preview(app));
    opened.lock().unwrap().clear();
    step(app.press_key("up"));
    step(app.press_key("m"));
    step(app.wait_for("id:menu-panel >> text:Channel options…"));
    step(app.run_for(Duration::from_secs(1)));
    assert!(
        opened.lock().unwrap().is_empty(),
        "menu cancels a pending tune"
    );
    step(app.back());
    step(app.run_for(Duration::from_millis(500)));
    assert!(
        opened.lock().unwrap().is_empty(),
        "closing context actions keeps autoplay held, like a touch hold"
    );
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    loaded_stream(app, &opened, "http://example.test/b");
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/b"]);
    // An explicit choice on the playing preview goes full screen immediately.
    step(app.press_key("enter"));
    assert!(!preview(app));
    // Preview list: choosing another channel obeys delay and stays open.
    step(app.press_key("shift+left"));
    step(app.wait_for("id:guide-timeline"));
    step(app.press_key("up"));
    opened.lock().unwrap().clear();
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    loaded_stream(app, &opened, "http://example.test/a");
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/a"]);
    assert!(preview(app));
    step(app.back());
    assert!(!preview(app));
}

#[test]
fn cold_start_resumes_the_stored_channel_with_an_auto_hiding_banner() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(true))
        .unwrap();
    let fixture = start(home, settings, true);
    drop(fixture.app);
    let key = iptv_providers::m3u::parse_m3u(M3U).channels.items[0]
        .id
        .as_str()
        .to_owned();
    std::fs::write(
        fixture
            .home
            .data_dir()
            .join(format!("last_channel-{}", fixture.source.0)),
        key,
    )
    .unwrap();

    // One cached playlist with refresh disabled takes the early-resume path.
    // Keep the isolated home alive while a new app reads its saved channel.
    let before = iptv_app::player_opens(0);
    let mut app = crate::cov_data::start();
    step(app.wait_for_within(
        "id:player-banner >> text:/Local A$/",
        Duration::from_secs(20),
    ));
    step(app.wait_until(
        "the resumed native source is published",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0) && iptv_app::player_opens(0) > before,
    ));
    assert!(iptv_app::player_has_source(0));
    assert_eq!(
        iptv_app::player_opens(0),
        before + 1,
        "one resumed native source"
    );
    step(app.run_for(Duration::from_secs(10)));
    assert!(!app.screen().matches("id:player-banner").unwrap());
    // OK can still open the full information card after the banner expires.
    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
}
