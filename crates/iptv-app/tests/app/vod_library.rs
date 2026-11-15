//! Generated on-demand catalogue drives browsing, episodes and persistent playback.
use crate::{headless::Headless, personalization::step, support::Server};
use iptv_providers::{
    HttpResponse, Secret,
    model::{ContentKind, Source, SourceConfig, XtreamCredentials},
};
use nsh_core::{
    EventKind, EventPayload, MountItem, NodeId, Size, ViewKind, element::Root, prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
fn player(app: &Headless) -> NodeId {
    let nodes: Vec<_> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.kind == ViewKind::Native)
        .collect();
    assert_eq!(nodes.len(), 1, "persistent player surface");
    nodes[0].id
}

#[test]
fn movies_and_series_reach_playback_and_resume_by_identity() {
    let Some(home) = isolated!() else { return };
    exercise(home, nsh_core::platform::Idiom::Pad);
}

#[test]
fn tv_remote_selects_movies_seasons_and_episodes() {
    let Some(home) = isolated!() else { return };
    exercise(home, nsh_core::platform::Idiom::Tv);
}

fn exercise(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom) {
    let server = Server::start(&[(
        "/player_api.php",
        r#"{
  "info": {
    "name": "Generated Voyage",
    "plot": "Generated series plot"
  },
  "episodes": {
    "1": [
      {
        "id": 101,
        "episode_num": 1,
        "title": "Generated First",
        "container_extension": "mp4"
      },
      {
        "id": 102,
        "episode_num": 2,
        "title": "Generated Second",
        "container_extension": "mp4"
      }
    ],
    "2": [
      {
        "id": 201,
        "episode_num": 1,
        "title": "Generated Third",
        "container_extension": "mp4"
      }
    ]
  }
}"#
        .into(),
    )]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    settings
        .set(
            "general.resume_last_channel",
            iptv_settings::Value::Bool(false),
        )
        .unwrap();
    settings
        .set("guide.autoplay", iptv_settings::Value::Bool(false))
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
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
        r#"[
  {
    "stream_id": 10,
    "name": "Generated Aurora",
    "container_extension": "mp4"
  },
  {
    "stream_id": 11,
    "name": "Generated Brook",
    "container_extension": "mp4"
  }
]"#,
    ))
    .unwrap();
    let series = iptv_providers::xtream::parse_series_list(&HttpResponse::ok(
        r#"[{"series_id":19,"name":"Generated Voyage","category_id":"8"}]"#,
    ))
    .unwrap();
    let categories = iptv_providers::xtream::parse_categories(
        &HttpResponse::ok(r#"[{"category_id":"8","category_name":"Generated serials"}]"#),
        ContentKind::Series,
    )
    .unwrap();
    let mut groups = parsed.groups;
    groups.extend(categories.items);
    store
        .replace_catalog(
            source,
            &groups,
            &parsed.channels.items,
            &movies.items,
            &series.items,
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
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = idiom;
    let mut app = Headless::start(config).unwrap();
    let props: Arc<Mutex<HashMap<String, Value>>> = Arc::default();
    let observed = props.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props: changed, .. } = item {
                for (key, value) in changed {
                    observed
                        .lock()
                        .unwrap()
                        .insert(key.to_string(), value.clone().unwrap_or(Value::Null));
                }
            }
        }
    });
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    let surface = player(&app);
    step(app.press("text:/^☰  /"));
    step(app.press("label:Films"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("text:/^Generated Aurora/"));
    step(app.wait_for("text:Play from beginning"));
    if idiom == nsh_core::platform::Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Play"));
    }
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    assert_eq!(player(&app), surface);
    let opened = props.lock().unwrap()["source"].clone();
    assert_eq!(opened["live"], false);
    assert!(
        opened["url"]
            .as_str()
            .unwrap()
            .ends_with("/movie/generated-user/generated-pass/10.mp4")
    );
    let token = opened["token"].clone();
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "timeline".into(),
            data: json!({
            "token":token,
            "seekId":0,
            "ms":37000,
            "durationMs":120000,
            "seekable":true}
            ),
        },
    );
    app.settle();
    step(app.back());
    step(app.press("text:/^☰  /"));
    step(app.press("label:Films"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("text:Continue watching"));
    step(app.wait_for("text:/^Generated Aurora/"));
    assert_eq!(app.screen().count("id:vod-title").unwrap(), 1);
    step(app.back());
    step(app.press("text:/^☰  /"));
    step(app.press("label:Series"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("text:/^Generated Voyage/"));
    step(app.wait_for("text:/Generated First/"));
    if idiom == nsh_core::platform::Idiom::Tv {
        for _ in 0..2 {
            step(app.press_key("right"));
        }
        step(app.press_key("down"));
        step(app.press_key("down"));
        step(app.press_key("enter"));
    } else {
        step(app.press("text:/Generated Second/"));
    }
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    assert_eq!(player(&app), surface);
    let opened = props.lock().unwrap()["source"].clone();
    assert!(
        opened["url"]
            .as_str()
            .unwrap()
            .ends_with("/series/generated-user/generated-pass/102.mp4")
    );
    let token = opened["token"].clone();
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "timeline".into(),
            data: json!({
            "token":token,
            "seekId":0,
            "ms":120000,
            "durationMs":120000,
            "seekable":true}
            ),
        },
    );
    app.settle();
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "state".into(),
            data: json!({
            "state":"ended"}
            ),
        },
    );
    step(
        app.wait_until("next episode", Duration::from_secs(10), |_| {
            props
                .lock()
                .unwrap()
                .get("source")
                .and_then(|s| s["url"].as_str())
                .is_some_and(|url| url.ends_with("/series/generated-user/generated-pass/201.mp4"))
        }),
    );
    assert_eq!(player(&app), surface);
    drop(app);
    let store = crate::isolated::open_database(&path).unwrap();
    assert_eq!(
        store
            .vod_progress(&iptv_core::vod::Key {
                source: source.0,
                kind: iptv_core::vod::Kind::Movie,
                id: movies.items[0].id.to_string()
            })
            .unwrap()
            .resume_ms(),
        37_000
    );
}
