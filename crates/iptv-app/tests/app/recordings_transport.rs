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
    EventKind, EventPayload, MountItem, Point, Size, ViewKind, element::Root, platform::Idiom,
    prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

type Observed = Arc<Mutex<HashMap<String, Value>>>;
fn saved_fixture(home: &crate::isolated::Home) -> (Headless, std::path::PathBuf, JobId, Observed) {
    saved_fixture_with_protection(home, false)
}
fn saved_fixture_with_protection(
    home: &crate::isolated::Home,
    protected: bool,
) -> (Headless, std::path::PathBuf, JobId, Observed) {
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Setting::Bool(false))
        .unwrap();
    settings
        .set("guide.autoplay", Setting::Bool(false))
        .unwrap();
    settings
        .set("parental.live_enabled", Setting::Bool(protected))
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
    if protected {
        // Generated PIN2468, salt0x63, PBKDF2-SHA256/210000. No user credential.
        store
            .set_parental_credential(&iptv_store_sqlite::parental::ParentalCredential {
                salt: [0x63; 16],
                verifier: [
                    108, 253, 222, 238, 40, 38, 77, 8, 100, 135, 138, 42, 133, 48, 171, 236, 123,
                    169, 134, 85, 199, 28, 212, 46, 152, 192, 13, 230, 58, 195, 156, 119,
                ],
                recovery_verifier: [0; 32],
                failures: 0,
                retry_after: 0,
            })
            .unwrap();
        store
            .set_parental_blocked(
                &iptv_core::parental::Target::live(source.0, parsed.channels.items[0].id.as_str()),
                true,
            )
            .unwrap();
    }
    let id = store
        .schedule_recording(
            JobId(97),
            &Request {
                source: source.0,
                channel: parsed.channels.items[0].id.as_str().into(),
                channel_name: "Generated channel".into(),
                title: "Generated saved program".into(),
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
    step(app.open_guide_groups());
    step(app.press("label:Recordings"));
    step(app.wait_for("id:recording-job"));
    (app, database, id, props)
}

#[test]
fn saved_recording_resumes_seeks_paused_and_starts_again_without_changing_surfaces() {
    let Some(home) = isolated!() else { return };
    let (mut app, database, id, props) = saved_fixture(&home);
    step(app.long_press("id:recording-job"));
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
    step(app.wait_for("id:player-time-line"));
    step(app.wait_for("text:0:00"));
    step(app.wait_for("text:2:00"));
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
    assert_eq!(
        props.lock().unwrap()["paused"],
        false,
        "Play again resumes playback"
    );
    send(&mut app, "state", json!({"state":"ready"}));
    send(
        &mut app,
        "timeline",
        json!({"token":next_token,"seekId":0,"ms":0,"durationMs":120000,"seekable":true}),
    );
    // Touch users get the same recording timeline and can seek without reopening.
    step(app.press_key("space"));
    step(app.set_environment("a tablet", |e| {
        e.idiom = Idiom::Pad;
        e.window = Size::new(px(1920.), px(1080.));
    }));
    step(app.press("label:Video"));
    step(app.wait_for("id:play-bar-time-line >> text:2:00"));
    let touch_opens = iptv_app::player_opens(0);
    step(app.drag("id:play-bar-seek", Point::new(px(0.0), px(0.0))));
    let touch_seek = props.lock().unwrap()["seek"].clone();
    assert_eq!(touch_seek["ms"], 60_000);
    assert_eq!(props.lock().unwrap()["paused"], true);
    assert_eq!(iptv_app::player_opens(0), touch_opens);
    send(
        &mut app,
        "timeline",
        json!({"token":next_token,"seekId":touch_seek["id"],"ms":60000,"durationMs":120000,"seekable":true}),
    );
    send(
        &mut app,
        "timeline",
        json!({"token":token,"seekId":99,"ms":99000,"durationMs":120000,"seekable":true}),
    );
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    step(
        app.wait_until("recording source stopped", Duration::from_secs(10), |_| {
            !iptv_app::player_has_source(0)
        }),
    );
    step(app.wait_for_within("text:Generated group", Duration::from_secs(10)));
    assert_eq!(
        app.screen()
            .layout_nodes()
            .into_iter()
            .filter(|node| node.kind == ViewKind::Native)
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        [player],
        "recording Stop preserves the native surface"
    );
    let saved = crate::isolated::open_database(&database)
        .unwrap()
        .recording_progress(id)
        .unwrap();
    assert_eq!(
        saved.position_ms, 60_000,
        "stale source events cannot replace the final position"
    );
    assert!(!saved.completed);
}

#[test]
fn tv_saved_playback_returns_through_guide_preview_without_reopening_the_player() {
    let Some(home) = isolated!() else { return };
    let (mut app, _, _, props) = saved_fixture(&home);
    step(app.press("id:recording-job"));
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(10)));
    let source = props.lock().unwrap()["source"].clone();
    let opens = iptv_app::player_opens(0);
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.kind == ViewKind::Native)
        .unwrap()
        .id;
    step(app.press_key("m"));
    step(app.wait_for("id:play-bar-layer"));
    for _ in 0..16 {
        if app
            .screen()
            .accessibility_state("id:play-bar-guide")
            .is_ok_and(|s| s.selected)
        {
            break;
        }
        step(app.press_key("left"));
    }
    assert!(
        app.screen()
            .accessibility_state("id:play-bar-guide")
            .unwrap()
            .selected
    );
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:play-bar-layer", Duration::from_secs(2)));
    step(app.wait_for("id:guide-preview"));
    // A recording also owns the preview when no live channel is playing.
    // Search remains a route to preview focus on TV and desktop.
    for idiom in [Idiom::Desktop, Idiom::Tv] {
        step(app.set_environment("saved preview navigation", |env| {
            env.idiom = idiom;
            env.os = if idiom == Idiom::Desktop {
                nsh_core::platform::Os::Linux
            } else {
                nsh_core::platform::Os::Android
            };
        }));
        let (w, h) = if idiom == Idiom::Tv {
            (960.0, 540.0)
        } else {
            (1280.0, 720.0)
        };
        step(app.set_viewport(Size::new(px(w), px(h))));
        step(app.open_guide_groups());
        step(app.press_key("left"));
        step(app.press_key("up"));
        assert!(
            app.screen()
                .accessibility_state("id:guide-preview")
                .unwrap()
                .selected
        );
        step(app.press_key("down"));
        step(app.press_key("enter"));
        step(app.wait_for("id:search-screen"));
        step(app.back());
        step(app.wait_for_gone("id:search-screen", Duration::from_secs(2)));
        step(app.press_key("right"));
        step(app.press("id:guide-groups >> text:Favorites"));
        step(app.press_key("up"));
        assert!(
            !app.screen()
                .accessibility_state("id:guide-preview")
                .unwrap()
                .selected,
            "Up wraps even with a recording playing in the preview"
        );
        step(app.press("id:guide-groups >> text:All channels"));
        step(app.press_key("right"));
        assert_eq!(iptv_app::player_opens(0), opens);
        assert_eq!(props.lock().unwrap()["source"], source);
    }
    step(app.press_key("tab"));
    step(app.press_key("left"));
    step(app.press_key("up"));
    assert!(
        app.screen()
            .accessibility_state("id:guide-preview")
            .is_ok_and(|s| s.selected && !s.disabled)
    );
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:player-banner", Duration::from_secs(2)));
    step(app.wait_for("id:player-panels >> text:Generated saved program"));
    step(app.wait_for("id:player-time-line"));
    assert_eq!(iptv_app::player_opens(0), opens);
    assert_eq!(props.lock().unwrap()["source"], source);
    assert!(
        app.screen()
            .layout_nodes()
            .into_iter()
            .any(|n| n.id == player && !n.hidden)
    );
}

#[test]
fn back_returns_to_the_library_and_delete_handles_the_playing_file_after_consent() {
    let Some(home) = isolated!() else { return };
    let (mut app, database, id, props) = saved_fixture(&home);
    step(app.set_environment("a tablet", |e| {
        e.idiom = Idiom::Pad;
        e.window = Size::new(px(1920.), px(1080.));
    }));
    step(app.press("id:recording-job"));
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(10)));
    let source = props.lock().unwrap()["source"].clone();
    let opens = iptv_app::player_opens(0);
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.kind == ViewKind::Native)
        .unwrap()
        .id;
    step(app.press("id:video"));
    step(app.press("id:play-bar-back"));
    step(app.wait_for("id:recordings-screen"));
    step(app.wait_for("id:recording-return-to-player"));
    assert_eq!(props.lock().unwrap()["source"], source);
    step(app.press("id:recording-return-to-player"));
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(2)));
    assert_eq!(iptv_app::player_opens(0), opens);
    step(app.press("label:TV guide"));
    step(app.wait_for("id:guide-preview"));
    assert!(
        !app.screen()
            .matches("id:guide-preview >> text:Nothing playing")
            .unwrap()
    );
    step(app.press("id:guide-preview"));
    step(app.press("id:video"));
    assert_eq!(iptv_app::player_opens(0), opens);
    step(app.press("id:play-bar-back"));
    step(app.wait_for("id:recordings-screen"));
    // Deleting a different retained file never stops the current recording's playback.
    let mut store = crate::isolated::open_database(&database).unwrap();
    let mut other = store.recording_job(id).unwrap().unwrap().request;
    other.title = "Generated other program".into();
    other.start += 200;
    other.end += 200;
    store
        .schedule_recording(JobId(98), &other, 1790000000)
        .unwrap();
    store
        .transition_recording(
            JobId(98),
            Status::Scheduled,
            Status::Completed,
            18800,
            "Recording completed",
        )
        .unwrap();
    std::fs::write(
        iptv_runtime::recordings::media_directory(&home.data_dir()).join("98.ts"),
        vec![0x47; 18800],
    )
    .unwrap();
    drop(store);
    step(app.wait_for_within(
        "label:/^Delete recording · Generated other program/",
        Duration::from_secs(10),
    ));
    step(app.press("label:/^Delete recording · Generated other program/"));
    step(app.press("id:recording-delete-confirmation >> text:Delete recording"));
    step(app.wait_for_gone(
        "label:/^Delete recording · Generated other program/",
        Duration::from_secs(10),
    ));
    assert_eq!(props.lock().unwrap()["source"], source);
    assert_eq!(iptv_app::player_opens(0), opens);
    step(app.press("label:/^Delete recording · Generated saved program/"));
    step(app.wait_for("id:recording-delete-confirmation"));
    step(app.press("id:recording-delete-confirmation >> text:Cancel"));
    assert_eq!(props.lock().unwrap()["source"], source);
    assert!(
        crate::isolated::open_database(&database)
            .unwrap()
            .recording_job(id)
            .unwrap()
            .is_some()
    );
    step(app.press("label:/^Delete recording · Generated saved program/"));
    step(app.press("id:recording-delete-confirmation >> text:Delete recording"));
    step(app.wait_for_within("text:/No recordings here/", Duration::from_secs(10)));
    assert!(props.lock().unwrap()["source"].is_null());
    assert!(
        !iptv_runtime::recordings::media_directory(&home.data_dir())
            .join("97.ts")
            .exists()
    );
    assert!(
        crate::isolated::open_database(&database)
            .unwrap()
            .recording_job(id)
            .unwrap()
            .is_none()
    );
    assert!(
        app.screen()
            .layout_nodes()
            .iter()
            .any(|n| n.kind == ViewKind::Native && n.id == player)
    );
}

#[test]
fn a_saved_recording_grant_revoked_during_preparation_never_reaches_the_native_player() {
    let Some(home) = isolated!() else { return };
    let (mut app, _database, _id, props) = saved_fixture_with_protection(&home, true);
    let before = iptv_app::player_opens(0);
    let preparation = iptv_app::hold_source_preparation();
    step(app.press("id:recording-job"));
    step(app.wait_for("id:parental-layer"));
    for digit in ["2", "4", "6", "8"] {
        step(app.press_key(digit));
    }
    step(app.press("id:parental-layer >> text:Continue"));
    // Gate entry is an external worker acknowledgement and need not produce a
    // UI batch after the PIN/library transition has finished rendering.
    step(crate::isolated::wait_outside(
        &mut app,
        "authorized saved source waiting for retirement",
        Duration::from_secs(10),
        || preparation.wait_entered(Duration::ZERO),
    ));
    // The playback task reaches preparation before the recording action's
    // follow-up jobs read has closed its busy library overlay. Back belongs
    // to that overlay until it acknowledges the move to full-screen playback.
    step(app.wait_for_gone("id:parental-continue", Duration::from_secs(10)));
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(10)));
    assert_eq!(iptv_app::player_opens(0), before, "source is still held");
    step(app.back());
    step(app.wait_for("id:recording-return-to-player"));
    let surface = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.kind == ViewKind::Native)
        .unwrap()
        .id;
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "test.parental.lock".into(),
            data: Value::Null,
        },
    );
    app.settle();
    preparation.release();
    // The selected playback is cleared by final admission. This follows the
    // actual unlock dialog, so the test revokes a real verified content grant.
    step(app.wait_for_gone("id:recording-return-to-player", Duration::from_secs(5)));
    assert_eq!(
        iptv_app::player_opens(0),
        before,
        "no native source may be submitted after revocation"
    );
    assert!(
        props
            .lock()
            .unwrap()
            .get("source")
            .is_none_or(Value::is_null)
    );
}

#[test]
fn foreground_before_ready_reauthorizes_retained_recording_without_transport_error() {
    let Some(home) = isolated!() else { return };
    let (mut app, _, _, props) = saved_fixture(&home);
    step(app.long_press("id:recording-job"));
    step(app.wait_for("text:Resume recording"));
    step(app.press("text:Resume recording"));
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(10)));
    let source = props.lock().unwrap()["source"].clone();
    let token = source["token"].as_u64().unwrap();
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.kind == ViewKind::Native)
        .unwrap()
        .id;
    let before = iptv_app::player_opens(0);
    app.engine().control().dispatch_event(
        player,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "reauthorize".into(),
            data: json!({"token":token,"allowed":true}),
        },
    );
    step(app.wait_until(
        "foreground publishes a newly authorized token",
        Duration::from_secs(10),
        |_| iptv_app::player_opens(0) > before,
    ));
    let next = props.lock().unwrap()["source"]["token"].clone();
    assert_ne!(next, source["token"]);
    assert!(
        !app.screen()
            .matches("text:On-demand playback unavailable")
            .unwrap()
    );
    app.engine().control().dispatch_event(
        player,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "reauthorize".into(),
            data: json!({"token":token,"allowed":true}),
        },
    );
    step(app.wait(Duration::from_millis(100)));
    assert_eq!(
        props.lock().unwrap()["source"]["token"],
        next,
        "old foreground event cannot reopen a newer source"
    );
}
