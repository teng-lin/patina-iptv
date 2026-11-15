//! Retained playback through the rendered shell and its native player contract.
use crate::{headless::Headless, personalization::step};
use iptv_core::{
    on_demand::Progress,
    recording::{JobId, Request, Status},
};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value as Setting};
use nsh_core::{
    EventKind, EventPayload, MountItem, Size, ViewKind, element::Root, platform::Idiom, prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[test]
fn saved_recording_resumes_seeks_paused_and_starts_again_without_changing_surfaces() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Setting::Bool(false))
        .unwrap();
    settings
        .set("guide.autoplay", Setting::Bool(false))
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let database = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&database).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated playback source".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated channel\nhttps://example.test/generated.ts\n",
    );
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
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
    let id = store
        .schedule_recording(
            JobId(97),
            &Request {
                source: source.0,
                channel: parsed.channels.items[0].id.as_str().into(),
                channel_name: "Generated channel".into(),
                title: "Generated saved programme".into(),
                start: 1800000000,
                end: 1800000120,
            },
            1790000000,
        )
        .unwrap();
    store
        .transition_recording(
            id,
            Status::Scheduled,
            Status::Completed,
            18800,
            "Recording completed",
        )
        .unwrap();
    store
        .save_recording_progress(
            id,
            Progress::observed(30_000, Some(120_000), false),
            1790000000,
        )
        .unwrap();
    drop(store);
    let media = iptv_runtime::recordings::media_directory(&root);
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join("97.ts"), vec![0x47; 18800]).unwrap();
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.), px(1080.)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = Idiom::Tv;
    let mut app = Headless::start(config).unwrap();
    let props = Arc::new(Mutex::new(HashMap::<String, Value>::new()));
    let observed = props.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (name, value) in props {
                    observed
                        .lock()
                        .unwrap()
                        .insert(name.to_string(), value.clone().unwrap_or(Value::Null));
                }
            }
        }
    });
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    step(app.press("text:/^☰  /"));
    step(app.press("label:Recordings"));
    step(app.wait_for("id:recording-job"));
    step(app.press("id:recording-job"));
    step(app.wait_for("text:Resume recording"));
    step(app.wait_for("text:Play from beginning"));
    step(app.wait_for("text:Delete recording"));
    step(app.press("text:Resume recording"));
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(10)));
    let source = props.lock().unwrap()["source"].clone();
    assert_eq!(source["live"], false);
    let token = source["token"].as_u64().unwrap();
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.kind == ViewKind::Native)
        .unwrap()
        .id;
    let send = |app: &mut Headless, name: &str, data: Value| {
        app.engine().control().dispatch_event(
            player,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: name.into(),
                data,
            },
        );
        app.settle();
    };
    send(&mut app, "state", json!({"state":"ready"}));
    send(
        &mut app,
        "timeline",
        json!({"token":token,"seekId":0,"ms":0,"durationMs":120000,"seekable":true}),
    );
    assert_eq!(props.lock().unwrap()["seek"]["ms"], 30000);
    send(
        &mut app,
        "timeline",
        json!({"token":token,"seekId":1,"ms":30000,"durationMs":120000,"seekable":true}),
    );
    let before = iptv_app::player_opens(0);
    step(app.press_key("space"));
    assert_eq!(props.lock().unwrap()["paused"], true);
    step(app.press_key("left"));
    let seek = props.lock().unwrap()["seek"].clone();
    assert_eq!(seek["ms"], 0);
    assert_eq!(
        iptv_app::player_opens(0),
        before,
        "native seek keeps the open stream"
    );
    assert_eq!(props.lock().unwrap()["paused"], true);
    send(
        &mut app,
        "timeline",
        json!({"token":token,"seekId":seek["id"],"ms":0,"durationMs":120000,"seekable":true}),
    );
    send(
        &mut app,
        "timeline",
        json!({"token":token-1,"seekId":99,"ms":99000,"durationMs":120000,"seekable":true}),
    );
    send(&mut app, "state", json!({"state":"ended"}));
    step(app.wait_for("text:Playback finished"));
    step(app.press("text:Play again"));
    step(
        app.wait_until("recording reopened", Duration::from_secs(10), |_| {
            iptv_app::player_opens(0) == before + 1
        }),
    );
    let next_token = props.lock().unwrap()["source"]["token"].as_u64().unwrap();
    assert_ne!(next_token, token);
    send(&mut app, "state", json!({"state":"ready"}));
    send(
        &mut app,
        "timeline",
        json!({"token":next_token,"seekId":0,"ms":0,"durationMs":120000,"seekable":true}),
    );
    step(app.press_key("stop"));
    step(app.wait_for_within("text:Generated group", Duration::from_secs(10)));
    let saved = crate::isolated::open_database(&database)
        .unwrap()
        .recording_progress(id)
        .unwrap();
    assert_eq!(
        saved.position_ms, 0,
        "stale source events cannot replace the final position"
    );
    assert!(!saved.completed);
}
