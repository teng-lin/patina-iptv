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
        .find(|n| n.kind == ViewKind::Native)
        .unwrap()
        .bounds
        .size
        .width
        .0
        < 1900.0
}
#[test]
fn rapid_navigation_cancels_tuning_menus_cancel_preview_and_input_extends_fullscreen_timeout() {
    let Some(home) = isolated!() else { return };
    let mut s = Settings::new();
    for (k, v) in [
        ("guide.autoplay", Value::Bool(true)),
        ("guide.stay_on_guide", Value::Bool(true)),
        ("guide.stay_on_list", Value::Bool(true)),
        ("guide.fullscreen_timeout", Value::secs(2)),
    ] {
        s.set(k, v).unwrap();
    }
    let mut f = start(home, s, true);
    let app = &mut f.app;
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let observed = opened.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(url) = value.as_ref().and_then(|v| v["url"].as_str())
                    {
                        observed.lock().unwrap().push(url.into());
                    }
                }
            }
        }
    });
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
    step(app.wait_for("text:Manage favourites…"));
    step(app.run_for(Duration::from_secs(1)));
    assert!(
        opened.lock().unwrap().is_empty(),
        "menu cancels a pending tune"
    );
    step(app.back());
    step(app.run_for(Duration::from_millis(500)));
    loaded_stream(app, &opened, "http://example.test/b");
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/b"]);
    // An explicit choice on the playing preview goes full screen immediately.
    step(app.press_key("enter"));
    assert!(!preview(app));
    // Preview list: choosing another channel obeys delay and stays open.
    step(app.press_key("shift+left"));
    step(app.wait_for("text:/^☰  /"));
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
    let mut app = crate::cov_data::start();
    step(app.wait_for_within(
        "id:player-banner >> text:/Local A$/",
        Duration::from_secs(20),
    ));
    assert!(iptv_app::player_has_source(0));
    step(app.run_for(Duration::from_secs(10)));
    assert!(!app.screen().matches("id:player-banner").unwrap());
    // OK can still open the full information card after the banner expires.
    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
}
