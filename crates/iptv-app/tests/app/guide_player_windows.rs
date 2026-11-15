//! The actual guide context menu opens independent players without tuning main.
#![cfg(target_os = "linux")]
use crate::personalization::step;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::SourcePolicy;
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::time::Duration;

#[test]
fn guide_context_menu_opens_auxiliary_players_and_browsing_keeps_main_playback() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("guide.stay_on_guide", iptv_settings::Value::Bool(true))
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated guide".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("http://playlist.example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(crate::personalization::M3U);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    store.set_active_source(source).unwrap();
    drop(store);
    let mut config = EngineConfig::new(Root::of::<iptv_app::App>());
    config.viewport = Size::new(px(1920.0), px(1080.0));
    config.environment.os = nsh_core::platform::Os::Linux;
    let mut app = crate::Headless::start(config).unwrap();
    step(app.wait_for("text:Local A"));
    step(app.press("text:Local A"));
    step(
        app.wait_until("main preview plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    // A context-click must select B without watching it in the main player.
    step(app.right_click("id:guide-ui >> text:Local B"));
    step(app.wait_for("text:Watch in new window"));
    let main_opens = iptv_app::player_opens(0);
    step(app.press("text:Watch in new window"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-1").unwrap().is_some());
    assert!(iptv_app::player_has_source(1));
    step(app.wait_for("label:Play or pause"));
    assert!(app.screen().matches("label:Subtitles and tracks").unwrap());
    assert!(app.screen().matches("label:Full screen").unwrap());
    let main_mode = app.window_state("main").unwrap().unwrap().mode;
    step(app.press("label:Full screen"));
    assert_eq!(
        app.window_state("player-window-1").unwrap().unwrap().mode,
        nsh_core::desktop::WindowMode::FullScreen
    );
    assert_eq!(app.window_state("main").unwrap().unwrap().mode, main_mode);
    step(app.press("label:Full screen"));

    // These are the original player's controls, rather than a separate toolbar.
    step(app.press("label:Play or pause"));
    assert_eq!(iptv_app::player_opens(0), main_opens);
    step(app.press("label:Play or pause"));

    // Channel changes use the auxiliary runtime slot, including connection
    // accounting; aliasing its stream into the original controls is not enough.
    let auxiliary_opens = iptv_app::player_opens(1);
    step(app.press("label:Channel list"));
    step(app.wait_for("id:auxiliary-player >> id:channel-list >> text:/Local A/"));
    step(app.press("id:auxiliary-player >> id:channel-list >> text:/Local A/"));
    step(
        app.wait_until("auxiliary channel changed", Duration::from_secs(5), |_| {
            iptv_app::player_opens(1) > auxiliary_opens
        }),
    );
    assert_eq!(iptv_app::player_opens(0), main_opens);
    step(app.hover("id:auxiliary-player >> label:Video"));
    step(app.wait_for("label:Channel list"));
    step(app.press("label:Channel list"));
    step(app.wait_for("id:auxiliary-player >> id:channel-list >> text:/Local B/"));
    step(app.press("id:auxiliary-player >> id:channel-list >> text:/Local B/"));
    app.idle(Duration::from_millis(1200)).unwrap();

    assert_eq!(
        iptv_app::player_opens(0),
        main_opens,
        "opening B must not retune A"
    );
    assert!(!app.screen().matches("id:multiview-tile").unwrap());
    step(app.right_click("id:guide-ui >> text:Local C"));
    step(app.wait_for("text:Watch in new window"));
    step(app.press("text:Watch in new window"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-2").unwrap().is_some());
    assert!(iptv_app::player_has_source(1) && iptv_app::player_has_source(2));
    assert_eq!(iptv_app::player_opens(0), main_opens);
    // Opening an existing auxiliary channel must not start another player.
    let second_opens = iptv_app::player_opens(1);
    step(app.right_click("id:guide-ui >> text:Local B"));
    step(app.wait_for("text:Watch in new window"));
    step(app.press("text:Watch in new window"));
    assert_eq!(iptv_app::player_opens(1), second_opens);
    step(app.press_window_close("player-window-1"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-1").unwrap().is_none());
    assert!(app.window_state("player-window-2").unwrap().is_some());
    assert!(!iptv_app::player_has_source(1));
    assert!(iptv_app::player_has_source(2));
    assert_eq!(iptv_app::player_opens(0), main_opens);
}
