//! A movie that ends while the play bar is up: the bar's ‹ still takes a
//! press on its center and goes back (#186). On a tablet on its side at
//! the system text size the issue was seen with.
use crate::{
    headless::Headless,
    personalization::step,
    support::Server,
    vod_library::{SERIES_INFO, observe_props, player},
};
use iptv_providers::{
    HttpResponse, Secret,
    model::{Source, SourceConfig, XtreamCredentials},
};
use nsh_core::{EventKind, EventPayload, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use serde_json::json;
use std::time::Duration;

#[test]
fn the_back_button_answers_on_its_center_after_a_movie_ends_under_the_bar() {
    let Some(home) = isolated!() else { return };
    finished_movie(home, false);
}

#[test]
fn dedicated_pause_keeps_a_finished_movie_closed() {
    let Some(home) = isolated!() else { return };
    finished_movie(home, true);
}

fn finished_movie(home: crate::isolated::Home, pause_after_end: bool) {
    let server = Server::start(&[("/player_api.php", SERIES_INFO.into())]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    for key in ["general.resume_last_channel", "guide.autoplay"] {
        settings
            .set(key, iptv_settings::Value::Bool(false))
            .unwrap();
    }
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated on-demand playlist".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: server.base.clone(),
                username: Secret::new("generated-user"),
                password: Secret::new("generated-pass"),
                user_agent: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated live\nhttps://example.test/live.ts\n",
    );
    let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
        r#"[{"stream_id": 10, "name": "Generated Aurora", "container_extension": "mp4"}]"#,
    ))
    .unwrap();
    store
        .replace_catalog(
            source,
            &parsed.groups,
            &parsed.channels.items,
            &movies.items,
            &[],
        )
        .unwrap();
    store
        .set_source_policy(
            source,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    // A tablet on its side (1920 × 1200 px at 1.5).
    let mut config = EngineConfig {
        viewport: Size::new(px(1280.0), px(800.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = nsh_core::platform::Idiom::Pad;
    config.environment.font_scale = 1.15;
    let mut app = Headless::start(config).unwrap();
    let props = observe_props(&mut app);
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    let surface = player(&app);
    crate::browse_fixture::open_rail_item(&mut app, "Movies");
    step(app.wait_for("id:vod-screen"));
    step(app.press("label:/^Generated Aurora ·/"));
    step(app.wait_for("text:Play from beginning"));
    step(app.press("text:Play"));
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    let token = props.lock().unwrap()["source"]["token"].clone();
    let send = |app: &mut Headless, name: &str, data: serde_json::Value| {
        app.engine().control().dispatch_event(
            surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: name.into(),
                data,
            },
        );
        app.settle();
    };
    send(&mut app, "state", json!({"state": "ready"}));
    send(
        &mut app,
        "timeline",
        json!({"token": token, "seekId": 0, "ms": 25_000, "durationMs": 30_000, "seekable": true}),
    );
    // A tap on the video shortly before the end shows the bar and its ‹.
    if !app.screen().matches("id:play-bar-back").unwrap() {
        step(app.press("id:video"));
    }
    step(app.wait_for("id:play-bar-back"));
    let shown = app.screen().id("id:play-bar-back").unwrap();
    send(
        &mut app,
        "timeline",
        json!({"token": token, "seekId": 0, "ms": 30_000, "durationMs": 30_000, "seekable": true}),
    );
    send(&mut app, "state", json!({"state": "ended"}));
    step(app.wait_for("text:Playback finished"));
    // The bar stayed up through the end: the same ‹, where it was.
    assert_eq!(
        app.screen().id("id:play-bar-back"),
        Ok(shown),
        "the bar shown before the end is still up"
    );
    let opens = iptv_app::player_opens(0);
    if pause_after_end {
        step(app.press_remote(nsh_tv::RemoteKey::Pause));
        assert_eq!(
            iptv_app::player_opens(0),
            opens,
            "Pause cannot restart a finished movie"
        );
        assert_eq!(props.lock().unwrap()["paused"], true);
        return;
    }
    // A press on the ‹'s center (its chevron), as a finger lands there:
    // nothing drawn over it takes the press, and Back leaves for the
    // movie's details.
    step(app.press("id:play-bar-back"));
    step(app.wait_for("id:vod-screen"));
    step(app.wait_for("text:Play from beginning"));
    assert_eq!(player(&app), surface, "the same player surface");
    assert_eq!(iptv_app::player_opens(0), opens, "Back opens no stream");
}
