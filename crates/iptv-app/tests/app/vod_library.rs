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
    exercise(home, nsh_core::platform::Idiom::Pad, ResumeCase::Seekable);
}

#[test]
fn tv_remote_selects_movies_seasons_and_episodes() {
    let Some(home) = isolated!() else { return };
    exercise(home, nsh_core::platform::Idiom::Tv, ResumeCase::Seekable);
}

#[test]
fn an_unseekable_resumed_episode_still_completes_and_advances() {
    let Some(home) = isolated!() else { return };
    exercise(home, nsh_core::platform::Idiom::Pad, ResumeCase::Unseekable);
}

#[test]
fn a_resumed_episode_ending_before_its_first_timeline_still_advances() {
    let Some(home) = isolated!() else { return };
    exercise(
        home,
        nsh_core::platform::Idiom::Pad,
        ResumeCase::EndedBeforeTimeline,
    );
}

#[test]
fn tablet_reaches_unfinished_titles_of_two_playlists_from_the_rail() {
    let Some(home) = isolated!() else { return };
    discover(home, nsh_core::platform::Idiom::Pad);
}

#[test]
fn tv_remote_resumes_from_continue_watching_on_the_same_surface() {
    let Some(home) = isolated!() else { return };
    discover(home, nsh_core::platform::Idiom::Tv);
}

#[test]
fn desktop_clicks_through_continue_watching_and_recently_added() {
    let Some(home) = isolated!() else { return };
    discover(home, nsh_core::platform::Idiom::Desktop);
}

#[test]
fn tv_remote_retries_failed_details_and_playback_exactly() {
    let Some(home) = isolated!() else { return };
    recover(home, nsh_core::platform::Idiom::Tv);
}

#[test]
fn tablet_retries_failed_details_and_playback_exactly() {
    let Some(home) = isolated!() else { return };
    recover(home, nsh_core::platform::Idiom::Pad);
}

#[test]
fn desktop_retries_failed_details_and_playback_exactly() {
    let Some(home) = isolated!() else { return };
    recover(home, nsh_core::platform::Idiom::Desktop);
}

#[test]
fn tv_remote_reopens_recently_added_on_a_title_pushed_past_two_thousand_others() {
    let Some(home) = isolated!() else { return };
    deep_focus(home, nsh_core::platform::Idiom::Tv);
}

#[test]
fn tablet_reopens_recently_added_on_a_deep_title_and_loads_the_titles_above() {
    let Some(home) = isolated!() else { return };
    deep_focus(home, nsh_core::platform::Idiom::Pad);
}

/// Recently added focuses its newest title; the shelf closes, and 2,100
/// newer titles arrive on another playlist meanwhile. Reopening it shows
/// the one bounded page around that title, now 2,101st, with the focus
/// still on it (OK opens it); the titles above load on demand.
fn deep_focus(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom) {
    let server = Server::start(&[("/player_api.php", "{}".into())]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", false),
        ("guide.autoplay", false),
    ] {
        settings
            .set(key, iptv_settings::Value::Bool(value))
            .unwrap();
    }
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated live\nhttps://example.test/live.ts\n",
    );
    let xtream = |name: &str| Source {
        name: format!("Generated {name}"),
        config: SourceConfig::Xtream(XtreamCredentials {
            server_url: server.base.clone(),
            username: Secret::new(format!("generated-{name}")),
            password: Secret::new("generated-pass"),
            user_agent: None,
        }),
    };
    let source = store.add_source(&xtream("alpha")).unwrap();
    let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
        r#"[
  {"stream_id": 10, "name": "Generated Aurora", "added": "1700000100", "container_extension": "mp4"},
  {"stream_id": 11, "name": "Generated Brook", "container_extension": "mp4"}
]"#,
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
    let quiet = iptv_store_sqlite::SourcePolicy {
        refresh_on_start: false,
        ..Default::default()
    };
    store.set_source_policy(source, quiet).unwrap();
    drop(store);
    let mut app = Headless::start(engine(idiom)).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    let surface = player(&app);
    step(app.press("text:/^☰  /"));
    step(app.press("label:Recently added"));
    step(app.wait_for("text:/^Generated Aurora · Film · Generated alpha/"));
    assert_eq!(app.screen().count("id:vod-previous").unwrap(), 0);
    // Aurora, the newest, has the focus; Back closes the shelf.
    step(app.back());
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    // Another playlist brings 2,100 newer titles meanwhile.
    let newer: Vec<_> = (0..2_100)
        .map(|i| {
            json!({
                "stream_id": 1_000 + i,
                "name": format!("Generated newer {i:04}"),
                "added": (1_800_000_000 + i).to_string(),
                "container_extension": "mp4"
            })
        })
        .collect();
    let newer = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
        Value::Array(newer).to_string(),
    ))
    .unwrap();
    let mut store = crate::isolated::open_database(&path).unwrap();
    let beta = store.add_source(&xtream("beta")).unwrap();
    store
        .replace_catalog(beta, &[], &[], &newer.items, &[])
        .unwrap();
    store.set_source_policy(beta, quiet).unwrap();
    drop(store);
    // Reopened, the shelf shows Aurora's page (from the 2,081st title)
    // instead of the newest 40, and does not read the 2,080 above it.
    step(app.press("text:/^☰  /"));
    step(app.press("label:Recently added"));
    step(app.wait_for("text:/^Generated Aurora · Film · Generated alpha/"));
    step(app.wait_for("id:vod-previous"));
    // Its neighbours in the new order are there: the oldest newer title
    // just above and Brook (undated) just below.
    step(app.wait_for("text:/^Generated newer 0000 · Film · Generated beta/"));
    step(app.wait_for("text:/^Generated Brook · Film · Generated alpha/"));
    if idiom == nsh_core::platform::Idiom::Tv {
        // OK opens the focused title: still Aurora, found by identity.
        step(app.press_key("enter"));
        step(app.wait_for("text:Play from beginning"));
        step(app.wait_for("text:/^Generated Aurora · Film/"));
        step(app.back());
        step(app.wait_for("id:vod-previous"));
        // Up walks to the window's first title (the newer 19, 2,081st);
        // one more Up reads the page above and steps onto the newer 20.
        for _ in 0..21 {
            step(app.press_key("up"));
        }
        step(app.wait_for("text:/^Generated newer 0020 · Film/"));
        step(app.press_key("enter"));
        step(app.wait_for("text:Play from beginning"));
        step(app.wait_for("text:/^Generated newer 0020 · Film · Generated beta/"));
        step(app.back());
    } else {
        // The titles above load on demand, a bounded page at a time; the
        // focus stays on Aurora, and more titles remain above.
        step(app.press("id:vod-previous"));
        step(app.wait_for("text:/^Generated Aurora · Film · Generated alpha/"));
        step(app.wait_for("id:vod-previous"));
        step(app.press("text:/^Generated Aurora · Film · Generated alpha/"));
        step(app.wait_for("text:Play from beginning"));
        step(app.back());
    }
    step(app.back());
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    assert_eq!(player(&app), surface, "the same player surface stays");
}

#[test]
fn tv_remote_retry_never_plays_from_a_playlist_edited_after_the_failure() {
    let Some(home) = isolated!() else { return };
    rebind(home, nsh_core::platform::Idiom::Tv);
}

#[test]
fn desktop_retry_never_plays_from_a_playlist_edited_after_the_failure() {
    let Some(home) = isolated!() else { return };
    rebind(home, nsh_core::platform::Idiom::Desktop);
}

/// Playback fails while the playlist is switched off; before Retry, the
/// playlist is switched on again but its credentials were edited, so it is
/// another incarnation under the same id. Retry refuses it: nothing plays,
/// the stale details close and no Retry is offered. Choosing the title
/// again reads it from the playlist as it is now and plays with its new
/// credentials.
fn rebind(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom) {
    let server = Server::start(&[("/player_api.php", "{}".into())]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", false),
        ("guide.autoplay", false),
    ] {
        settings
            .set(key, iptv_settings::Value::Bool(value))
            .unwrap();
    }
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated edited playlist".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: server.base.clone(),
                username: Secret::new("generated-user"),
                password: Secret::new("generated-old-pass"),
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
    let enabled = iptv_store_sqlite::SourcePolicy {
        refresh_on_start: false,
        ..Default::default()
    };
    store.set_source_policy(source, enabled).unwrap();
    store
        .save_vod_progress(
            &iptv_core::vod::Key {
                source: source.0,
                kind: iptv_core::vod::Kind::Movie,
                id: movies.items[0].id.to_string(),
            },
            None,
            iptv_core::vod::Progress::observed(37_000, Some(120_000), false),
        )
        .unwrap();
    drop(store);
    let mut app = Headless::start(engine(idiom)).unwrap();
    let props = observe_props(&mut app);
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    let surface = player(&app);
    step(app.press("text:/^☰  /"));
    step(app.press("label:Continue watching"));
    step(app.wait_for("text:/^Generated Aurora · Film · Generated edited playlist/"));
    let tv = idiom == nsh_core::platform::Idiom::Tv;
    if tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:/^Generated Aurora · Film/"));
    }
    step(app.wait_for("text:Play from beginning"));
    // Switched off: playback fails, with Retry.
    let mut store = crate::isolated::open_database(&path).unwrap();
    store
        .set_source_policy(
            source,
            iptv_store_sqlite::SourcePolicy {
                enabled: false,
                ..enabled
            },
        )
        .unwrap();
    drop(store);
    if tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Resume"));
    }
    step(app.wait_for("text:Playback unavailable; refresh the playlist and try again"));
    assert_eq!(app.screen().count("id:vod-retry").unwrap(), 1);
    // Switched on again, but with edited credentials: another incarnation
    // under the same id, with the same title identities.
    let mut store = crate::isolated::open_database(&path).unwrap();
    let mut edited = store.source(source).unwrap().unwrap().source;
    let SourceConfig::Xtream(credentials) = &mut edited.config else {
        unreachable!("the generated playlist is Xtream")
    };
    credentials.password = Secret::new("generated-new-pass");
    store.update_source(source, &edited).unwrap();
    store.set_source_policy(source, enabled).unwrap();
    drop(store);
    if tv {
        // Left from Play reaches Retry.
        step(app.press_key("left"));
        step(app.press_key("enter"));
    } else {
        step(app.press("id:vod-retry"));
    }
    step(app.wait_for_gone("text:Play from beginning", Duration::from_secs(10)));
    step(app.wait_for("text:Playback unavailable; refresh the playlist and try again"));
    assert_eq!(
        app.screen().count("id:vod-retry").unwrap(),
        0,
        "no Retry that rebinds"
    );
    assert_eq!(app.screen().count("id:vod-screen").unwrap(), 1);
    assert!(
        props.lock().unwrap().get("source").is_none_or(|source| {
            !source["url"]
                .as_str()
                .is_some_and(|url| url.contains("/movie/"))
        }),
        "nothing played from the edited playlist"
    );
    // Chosen again, the title plays from the playlist as it is now.
    if tv {
        step(app.press_key("enter"));
        step(app.wait_for("text:Play from beginning"));
        step(app.press_key("enter"));
    } else {
        step(app.press("text:/^Generated Aurora · Film/"));
        step(app.wait_for("text:Play from beginning"));
        step(app.press("text:Resume"));
    }
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(10)));
    assert_eq!(player(&app), surface, "the same player surface plays it");
    let opened = props.lock().unwrap()["source"].clone();
    assert!(
        opened["url"]
            .as_str()
            .unwrap()
            .ends_with("/movie/generated-user/generated-new-pass/10.mp4"),
        "the playlist's current credentials: {opened}"
    );
}

fn engine(idiom: nsh_core::platform::Idiom) -> EngineConfig {
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = idiom;
    if idiom == nsh_core::platform::Idiom::Desktop {
        config.environment.os = nsh_core::platform::Os::Linux;
    }
    config
}

fn observe_props(app: &mut Headless) -> Arc<Mutex<HashMap<String, Value>>> {
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
    props
}

const SERIES_INFO: &str = r#"{
  "info": {"name": "Generated Voyage", "plot": "Generated series plot"},
  "episodes": {
    "1": [
      {"id": 101, "episode_num": 1, "title": "Generated First", "container_extension": "mp4"},
      {"id": 102, "episode_num": 2, "title": "Generated Second", "container_extension": "mp4"}
    ],
    "2": [
      {"id": 201, "episode_num": 1, "title": "Generated Third", "container_extension": "mp4"}
    ]
  }
}"#;

fn episode_key(series: &iptv_core::vod::Key, provider_id: &str) -> iptv_core::vod::Key {
    iptv_core::vod::Key {
        source: series.source,
        kind: iptv_core::vod::Kind::Episode,
        id: format!(
            "{}:{}",
            series.id,
            iptv_providers::ContentId::from_provider(
                iptv_providers::model::SourceKind::Xtream,
                ContentKind::Episode,
                provider_id
            )
        ),
    }
}

/// Failures on a generated local provider, each followed by Retry. Retry
/// repeats exactly what failed: a series whose details could not be read
/// opens (not the shelf's first page again), and a playback that failed
/// because its playlist was switched off meanwhile plays the same, non-
/// preferred episode in the same way once the playlist is back, after
/// fresh details and a fresh link. The player surface stays the same.
fn recover(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom) {
    // No provider answers yet: series details cannot be read.
    let server = Server::start(&[]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", false),
        ("guide.autoplay", false),
    ] {
        settings
            .set(key, iptv_settings::Value::Bool(value))
            .unwrap();
    }
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated retry playlist".into(),
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
    let series = iptv_providers::xtream::parse_series_list(&HttpResponse::ok(
        r#"[{"series_id":19,"name":"Generated Voyage","last_modified":"1700000200"}]"#,
    ))
    .unwrap();
    store
        .replace_catalog(
            source,
            &parsed.groups,
            &parsed.channels.items,
            &[],
            &series.items,
        )
        .unwrap();
    let enabled = iptv_store_sqlite::SourcePolicy {
        refresh_on_start: false,
        ..Default::default()
    };
    store.set_source_policy(source, enabled).unwrap();
    let series_key = iptv_core::vod::Key {
        source: source.0,
        kind: iptv_core::vod::Kind::Series,
        id: series.items[0].id.to_string(),
    };
    // The first episode was left at 50 s; the second, watched last, at
    // 37 s: details prefer the second.
    for (episode, ms) in [("101", 50_000), ("102", 37_000)] {
        store
            .save_vod_progress(
                &episode_key(&series_key, episode),
                Some(&series_key.id),
                iptv_core::vod::Progress::observed(ms, Some(120_000), false),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(1100));
    }
    drop(store);
    let mut app = Headless::start(engine(idiom)).unwrap();
    let props = observe_props(&mut app);
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    let surface = player(&app);
    step(app.press("text:/^☰  /"));
    step(app.press("label:Continue watching"));
    step(app.wait_for("text:/^Generated Voyage · TV series · Generated retry playlist/"));
    let tv = idiom == nsh_core::platform::Idiom::Tv;
    if tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:/^Generated Voyage · TV series/"));
    }
    step(app.wait_for("text:Details unavailable; refresh the playlist and try again"));
    assert_eq!(app.screen().count("id:vod-retry").unwrap(), 1);
    assert_eq!(app.screen().count("text:Play from beginning").unwrap(), 0);
    assert_eq!(
        app.screen().count("id:vod-title").unwrap(),
        1,
        "the shelf stays as it was"
    );
    // The provider answers now: Retry opens the same series.
    server.set("/player_api.php", SERIES_INFO.into());
    if tv {
        // Up from the first title reaches Retry; OK runs it.
        step(app.press_key("up"));
        step(app.press_key("enter"));
    } else {
        step(app.press("id:vod-retry"));
    }
    step(app.wait_for("text:Play from beginning"));
    step(app.wait_for("text:/Generated Second/"));
    assert_eq!(app.screen().count("id:vod-retry").unwrap(), 0);
    // The playlist is switched off before playback: it fails.
    let mut store = crate::isolated::open_database(&path).unwrap();
    store
        .set_source_policy(
            source,
            iptv_store_sqlite::SourcePolicy {
                enabled: false,
                ..enabled
            },
        )
        .unwrap();
    drop(store);
    if tv {
        // The first episode, not the preferred second, from its start.
        // Right on Seasons changes the season; Down enters Episodes.
        for key in ["right", "right", "down", "up", "left", "up"] {
            step(app.press_key(key));
        }
        step(app.press_key("enter"));
    } else {
        // A tap on the first episode plays (resumes) it.
        step(app.press("text:/Generated First/"));
    }
    step(app.wait_for("text:Playback unavailable; refresh the playlist and try again"));
    assert_eq!(app.screen().count("id:vod-retry").unwrap(), 1);
    assert_eq!(
        app.screen().count("text:Play from beginning").unwrap(),
        1,
        "the details stay open"
    );
    // Back on: Retry plays the same episode the same way.
    let mut store = crate::isolated::open_database(&path).unwrap();
    store.set_source_policy(source, enabled).unwrap();
    drop(store);
    if tv {
        // From Restart, Left reaches Play, then Retry.
        for key in ["left", "left", "enter"] {
            step(app.press_key(key));
        }
    } else {
        step(app.press("id:vod-retry"));
    }
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(10)));
    assert_eq!(player(&app), surface, "the same player surface plays it");
    let opened = props.lock().unwrap()["source"].clone();
    assert!(
        opened["url"]
            .as_str()
            .unwrap()
            .ends_with("/series/generated-user/generated-pass/101.mp4"),
        "the episode chosen before the failure: {opened}"
    );
    let token = opened["token"].clone();
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "timeline".into(),
            data: json!({"token":token,"seekId":0,"ms":0,"durationMs":120000,"seekable":true}),
        },
    );
    if tv {
        app.settle();
        assert!(
            props
                .lock()
                .unwrap()
                .get("seek")
                .is_none_or(|seek| !seek.to_string().contains("50000")),
            "Play from beginning stays a restart after Retry"
        );
    } else {
        step(app.wait_until("resume seek", Duration::from_secs(5), |_| {
            props
                .lock()
                .unwrap()
                .get("seek")
                .is_some_and(|seek| seek.to_string().contains("50000"))
        }));
    }
}

/// Two generated Xtream playlists on one local mock server carry the same
/// movie and series ids and titles. Progress saved on both appears together
/// in Continue watching, each with its playlist's name; choosing the newer
/// one resumes it through details on the persistent player surface.
fn discover(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom) {
    let server = Server::start(&[("/player_api.php", "{}".into())]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", false),
        ("guide.autoplay", false),
    ] {
        settings
            .set(key, iptv_settings::Value::Bool(value))
            .unwrap();
    }
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated live\nhttps://example.test/live.ts\n",
    );
    let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
        r#"[
  {"stream_id": 10, "name": "Generated Aurora", "added": "1700000100", "container_extension": "mp4"},
  {"stream_id": 11, "name": "Generated Brook", "container_extension": "mp4"}
]"#,
    ))
    .unwrap();
    let series = iptv_providers::xtream::parse_series_list(&HttpResponse::ok(
        r#"[{"series_id":19,"name":"Generated Voyage","last_modified":"1700000200"}]"#,
    ))
    .unwrap();
    let mut keys = Vec::new();
    for name in ["alpha", "beta"] {
        let source = store
            .add_source(&Source {
                name: format!("Generated {name}"),
                config: SourceConfig::Xtream(XtreamCredentials {
                    server_url: server.base.clone(),
                    username: Secret::new(format!("generated-{name}")),
                    password: Secret::new("generated-pass"),
                    user_agent: None,
                }),
            })
            .unwrap();
        store
            .replace_catalog(
                source,
                &parsed.groups,
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
        keys.push(iptv_core::vod::Key {
            source: source.0,
            kind: iptv_core::vod::Kind::Movie,
            id: movies.items[0].id.to_string(),
        });
    }
    // Progress times are whole seconds: the second playlist's is newer.
    store
        .save_vod_progress(
            &keys[0],
            None,
            iptv_core::vod::Progress::observed(37_000, Some(120_000), false),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    store
        .save_vod_progress(
            &keys[1],
            None,
            iptv_core::vod::Progress::observed(61_000, Some(120_000), false),
        )
        .unwrap();
    drop(store);
    let mut app = Headless::start(engine(idiom)).unwrap();
    let props = observe_props(&mut app);
    // The guide stays the start screen.
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    assert_eq!(app.screen().count("id:vod-screen").unwrap(), 0);
    let surface = player(&app);
    step(app.press("text:/^☰  /"));
    step(app.press("label:Continue watching"));
    step(app.wait_for("id:vod-screen"));
    step(app.wait_for("text:/^Generated Aurora · Film · Generated beta/"));
    assert_eq!(
        app.screen().count("id:vod-title").unwrap(),
        2,
        "one row per playlist, nothing else"
    );
    step(app.wait_for("text:/^Generated Aurora · Film · Generated alpha/"));
    assert_eq!(
        app.screen().count("id:vod-search").unwrap(),
        0,
        "a shelf has no search or filters to step through"
    );
    if idiom == nsh_core::platform::Idiom::Tv {
        // The newest progress is first and focused: OK opens it, OK resumes.
        step(app.press_key("enter"));
        step(app.wait_for("text:Play from beginning"));
        step(app.press_key("enter"));
    } else {
        step(app.press("text:/^Generated Aurora · Film · Generated beta/"));
        step(app.wait_for("text:Play from beginning"));
        step(app.press("text:Resume"));
    }
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    assert_eq!(player(&app), surface, "the same player surface plays it");
    let opened = props.lock().unwrap()["source"].clone();
    assert!(
        opened["url"]
            .as_str()
            .unwrap()
            .ends_with("/movie/generated-beta/generated-pass/10.mp4"),
        "the chosen playlist's own copy: {opened}"
    );
    let token = opened["token"].clone();
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "timeline".into(),
            data: json!({"token":token,"seekId":0,"ms":0,"durationMs":120000,"seekable":true}),
        },
    );
    step(app.wait_until("resume seek", Duration::from_secs(5), |_| {
        props
            .lock()
            .unwrap()
            .get("seek")
            .is_some_and(|seek| seek.to_string().contains("61000"))
    }));
    // Recently added mixes both playlists' movies and series.
    step(app.back());
    step(app.press("text:/^☰  /"));
    step(app.press("label:Recently added"));
    step(app.wait_for("text:/^Generated Voyage · TV series · Generated alpha/"));
    assert_eq!(app.screen().count("id:vod-title").unwrap(), 6);
    step(app.back());
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    assert_eq!(player(&app), surface);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResumeCase {
    Seekable,
    Unseekable,
    EndedBeforeTimeline,
}

fn exercise(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom, resume: ResumeCase) {
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
    let series_key = iptv_core::vod::Key {
        source: source.0,
        kind: iptv_core::vod::Kind::Series,
        id: series.items[0].id.to_string(),
    };
    let second_key = iptv_core::vod::Key {
        source: source.0,
        kind: iptv_core::vod::Kind::Episode,
        id: format!(
            "{}:{}",
            series_key.id,
            iptv_providers::ContentId::from_provider(
                iptv_providers::model::SourceKind::Xtream,
                ContentKind::Episode,
                "102"
            )
        ),
    };
    if resume != ResumeCase::Seekable {
        store
            .save_vod_progress(
                &second_key,
                Some(&series_key.id),
                iptv_core::vod::Progress::observed(37_000, Some(120_000), false),
            )
            .unwrap();
    }
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
    assert!(
        app.screen()
            .accessibility_state("id:vod-title >> nth=0")
            .unwrap()
            .selected
    );
    assert!(
        !app.screen()
            .accessibility_state("id:vod-title >> nth=1")
            .unwrap()
            .selected
    );
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
    step(app.press("id:vod-continue"));
    step(app.wait_for("text:/^Generated Aurora/"));
    assert_eq!(app.screen().count("id:vod-title").unwrap(), 1);
    step(app.back());
    step(app.press("text:/^☰  /"));
    step(app.press("label:Series"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("text:/^Generated Voyage/"));
    step(app.wait_for("text:/Generated First/"));
    let selected = (0..app.screen().count("id:vod-episode").unwrap())
        .filter(|at| {
            app.screen()
                .accessibility_state(&format!("id:vod-episode >> nth={at}"))
                .unwrap()
                .selected
        })
        .count();
    assert_eq!(
        selected, 1,
        "the episode selection has one content identity"
    );
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
    if resume != ResumeCase::Seekable {
        assert_eq!(
            crate::isolated::open_database(&path)
                .unwrap()
                .vod_progress(&second_key)
                .unwrap()
                .position_ms,
            37_000,
            "retain saved resume until an authoritative native timeline or end"
        );
    }
    if resume != ResumeCase::EndedBeforeTimeline {
        app.engine().control().dispatch_event(
            surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: "timeline".into(),
                data: json!({
                    "token":token,"seekId":0,
                    "ms":if resume==ResumeCase::Seekable {120000} else {7000},
                    "durationMs":120000,"seekable":resume==ResumeCase::Seekable
                }),
            },
        );
        app.settle();
    }
    if resume != ResumeCase::Seekable {
        assert!(props.lock().unwrap().get("seek").is_none_or(Value::is_null));
    }
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
    // The final timeline can mark the tail completed before Ended arrives.
    // The forced final save must not put that series back in Continue watching.
    let tail_token = props.lock().unwrap()["source"]["token"].clone();
    for ms in [114_000, 119_000] {
        app.engine().control().dispatch_event(
            surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: "timeline".into(),
                data: json!({"token":tail_token,"seekId":0,"ms":ms,"durationMs":120000,"seekable":true}),
            },
        );
        app.settle();
    }
    step(app.wait_until(
        "tail progress saved before EOF",
        Duration::from_secs(5),
        |_| {
            crate::isolated::open_database(&path)
                .and_then(|store| store.vod_progress(&series_key))
                .is_ok_and(|point| point.position_ms == 119_000)
        },
    ));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .vod_progress(&series_key)
            .unwrap()
            .completed,
        "a later five-second tick must preserve completion before native EOF"
    );
    for (name, data) in [
        ("state", json!({"state":"ready"})),
        (
            "timeline",
            json!({"token":tail_token,"seekId":0,"ms":120000,"durationMs":120000,"seekable":true}),
        ),
        ("state", json!({"state":"ended"})),
    ] {
        app.engine().control().dispatch_event(
            surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: name.into(),
                data,
            },
        );
        app.settle();
    }
    drop(app);
    let store = crate::isolated::open_database(&path).unwrap();
    assert!(store.vod_progress(&second_key).unwrap().completed);
    assert!(store.vod_progress(&series_key).unwrap().completed);
    assert!(
        store
            .vod_page(
                &iptv_core::vod::Query {
                    kind: iptv_core::vod::Kind::Series,
                    continue_watching: true,
                    ..Default::default()
                },
                0,
                20
            )
            .unwrap()
            .0
            .is_empty()
    );
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
