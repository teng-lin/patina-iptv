use crate::personalization;
use iptv_settings::{Settings, Value};
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
        .filter(|n| n.test_id.as_deref() == Some("video-player"))
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
    step(app.wait_for("id:menu-panel >> text:Channel options…"));
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
    step(app.wait_for("id:menu-panel >> text:Channel options…"));
    step(app.back());
    step(app.back());
    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
    step(app.back());
    step(app.back());
    // Re-entering the guide keeps its configured overlay layout and surface identity.
    step(app.press_key("m"));
    step(app.wait_for("id:menu-panel >> text:Channel options…"));
    assert_eq!(player(app).id, id);
    assert_eq!(player(app).bounds.size.width.0, 1920.0);
}

#[test]
fn fullscreen_and_overlay_shortcuts_reuse_the_player_with_overlay_preferred() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, overlay(None), true);
    let app = &mut f.app;
    let id = player(app).id;
    let wait_for_width = |app: &mut crate::headless::Headless, full_width| {
        step(app.wait_until(
            "the requested video layout",
            Duration::from_secs(2),
            |screen| {
                screen
                    .layout_nodes()
                    .iter()
                    .any(|node| node.id == id && (node.bounds.size.width.0 == 1920.0) == full_width)
            },
        ));
    };
    step(app.press_key("f"));
    step(app.press_key("f"));
    wait_for_width(app, false);
    step(app.press_key("o"));
    wait_for_width(app, true);
    step(app.press_key("o"));
    wait_for_width(app, false);
    assert_eq!(player(app).id, id);
}

#[test]
fn returning_from_a_compact_window_does_not_restore_a_forced_overlay() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), true);
    let app = &mut f.app;
    let id = player(app).id;
    let assert_width = |app: &mut crate::headless::Headless, width, full_width| {
        step(app.wait_until(
            "the resized video layout",
            Duration::from_secs(2),
            |screen| {
                screen
                    .layout_nodes()
                    .iter()
                    .any(|node| node.id == id && (node.bounds.size.width.0 == width) == full_width)
            },
        ));
    };
    step(app.press_key("o"));
    assert_width(app, 1920.0, true);
    step(app.set_viewport(nsh_core::Size::new(
        nsh_core::prelude::px(400.0),
        nsh_core::prelude::px(700.0),
    )));
    app.settle();
    assert_width(app, 400.0, false);
    step(app.set_viewport(nsh_core::Size::new(
        nsh_core::prelude::px(1920.0),
        nsh_core::prelude::px(1080.0),
    )));
    app.settle();
    assert_width(app, 1920.0, false);
    step(app.press_key("o"));
    assert_width(app, 1920.0, true);
    assert_eq!(player(app).id, id);
}

#[test]
fn disabling_the_playing_playlist_keeps_the_overlay_and_player_surface() {
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, Source, SourceConfig},
    };

    let Some(home) = isolated!() else { return };
    let mut f = personalization::launch_prepared(
        home,
        overlay(Some(true)),
        true,
        |path, first| {
            let mut store = crate::isolated::open_database(path).unwrap();
            let text = personalization::M3U.replace("Local ", "Second ");
            let file = path.with_file_name("second.m3u");
            std::fs::write(&file, &text).unwrap();
            let source = store
                .add_source(&Source {
                    name: "Synthetic second playlist".into(),
                    config: SourceConfig::M3u(M3uSourceConfig {
                        location: SecretUrl::new(file.to_string_lossy()),
                        user_agent: None,
                        referrer: None,
                    }),
                })
                .unwrap();
            let catalog = iptv_providers::m3u::parse_m3u(&text);
            store
                .replace_catalog(source, &catalog.groups, &catalog.channels.items, &[], &[])
                .unwrap();
            store.set_active_source(first).unwrap();
            store.set_all_playlists(false).unwrap();
            store
                .set_source_policy(
                    source,
                    iptv_store_sqlite::SourcePolicy {
                        enabled: false,
                        refresh_on_start: false,
                        ..Default::default()
                    },
                )
                .unwrap();
        },
        |_| {},
        |_| {},
    );
    let app = &mut f.app;
    step(app.open_guide_groups());
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    step(
        app.wait_until("the first playlist plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    let id = player(app).id;

    // Settings → Playlists: the second playlist's check box shows it, the
    // playing one's hides it.
    step(app.open_settings_playlists());
    let second = "role:checkbox[/^Synthetic second playlist/]";
    step(app.wait_for(second));
    step(app.press(second));
    step(
        app.wait_until("the second playlist shown", Duration::from_secs(5), |s| {
            s.accessibility_state(second)
                .is_ok_and(|a| a.checked == Some(nsh_core::Checked::True))
        }),
    );
    step(app.press("role:checkbox[/^Synthetic personalization/]"));
    step(app.wait_for("id:guide-ui >> label:Second A"));
    assert!(!iptv_app::player_has_source(0));
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
