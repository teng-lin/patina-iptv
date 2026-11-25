//! Generated on-demand catalog drives browsing, episodes and persistent playback.
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
pub(crate) fn player(app: &Headless) -> NodeId {
    let nodes: Vec<_> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.kind == ViewKind::Native)
        .collect();
    assert_eq!(nodes.len(), 1, "persistent player surface");
    nodes[0].id
}

/// Back from a title playing full screen, by the idiom's own input: a
/// remote's Back (the platform's, as `app.back()` sends it); Backspace on
/// a desktop keyboard (Escape is the platform's Back, `app.back()`); on a
/// tablet the play bar's visible ‹ (labeled Movies or Shows), else the
/// system Back.
fn back_from_player(app: &mut Headless, idiom: nsh_core::platform::Idiom) {
    let bar = app.screen().matches("id:play-bar-back").unwrap();
    match idiom {
        nsh_core::platform::Idiom::Tv => step(app.back()),
        nsh_core::platform::Idiom::Desktop => step(app.press_key("backspace")),
        _ if bar => {
            let back = app.screen().id("id:play-bar-back").unwrap();
            let labeled = |label: &str| {
                let screen = app.screen();
                (0..screen.count(&format!("label:{label}")).unwrap())
                    .any(|at| screen.id(&format!("label:{label} >> nth={at}")) == Ok(back))
            };
            assert!(
                labeled("Movies") || labeled("Shows"),
                "the ‹ reads Movies or Shows"
            );
            step(app.press("id:play-bar-back"));
        }
        _ => step(app.back()),
    }
}

#[track_caller]
pub(crate) fn is_selected(app: &Headless, selector: &str) -> bool {
    app.screen()
        .accessibility_state(selector)
        .is_ok_and(|state| state.selected)
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
fn old_native_events_cannot_consume_resume_while_the_next_source_is_preparing() {
    let Some(home) = isolated!() else { return };
    exercise_preparing(
        home,
        nsh_core::platform::Idiom::Pad,
        ResumeCase::Unseekable,
        true,
    );
}

#[test]
fn prior_eof_play_pause_during_preparation_preserves_the_next_titles_resume() {
    let Some(home) = isolated!() else { return };
    exercise_preparing(
        home,
        nsh_core::platform::Idiom::Pad,
        ResumeCase::PriorEndedDuringPreparation,
        true,
    );
}

#[test]
fn disabling_a_vod_source_during_preparation_rejects_its_resolved_url() {
    let Some(home) = isolated!() else { return };
    exercise_admission(
        home,
        nsh_core::platform::Idiom::Pad,
        ResumeCase::Unseekable,
        true,
        Some(SourceMutation::Disable),
    );
}
#[test]
fn editing_vod_credentials_during_preparation_does_not_rebind_the_old_url() {
    let Some(home) = isolated!() else { return };
    exercise_admission(
        home,
        nsh_core::platform::Idiom::Pad,
        ResumeCase::Unseekable,
        true,
        Some(SourceMutation::Edit),
    );
}
#[test]
fn deleting_a_vod_source_during_preparation_rejects_its_resolved_url() {
    let Some(home) = isolated!() else { return };
    exercise_admission(
        home,
        nsh_core::platform::Idiom::Pad,
        ResumeCase::Unseekable,
        true,
        Some(SourceMutation::Delete),
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
    step(app.open_guide_groups());
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
    step(app.open_guide_groups());
    step(app.press("label:Recently added"));
    step(app.wait_for("text:/^Generated Aurora · Film · Generated alpha/"));
    step(app.wait_for("id:vod-previous"));
    // Its neighbors in the new order are there: the oldest newer title
    // just above and Brook (undated) just below.
    step(app.wait_for("text:/^Generated newer 0000 · Film · Generated beta/"));
    step(app.wait_for("text:/^Generated Brook · Film · Generated alpha/"));
    if idiom == nsh_core::platform::Idiom::Tv {
        // OK opens the focused title: still Aurora, found by identity.
        step(app.press_key("enter"));
        step(app.wait_for("text:Play from beginning"));
        step(app.wait_for("id:vod-focused-metadata >> text:Generated Aurora"));
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
        step(app.wait_for("id:vod-focused-metadata >> text:Generated newer 0020"));
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
    step(app.open_guide_groups());
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

pub(crate) fn engine(idiom: nsh_core::platform::Idiom) -> EngineConfig {
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

pub(crate) fn observe_props(app: &mut Headless) -> Arc<Mutex<HashMap<String, Value>>> {
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

pub(crate) const SERIES_INFO: &str = r#"{
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

pub(crate) fn episode_key(series: &iptv_core::vod::Key, provider_id: &str) -> iptv_core::vod::Key {
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
    step(app.open_guide_groups());
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
        for key in ["down", "down", "left", "up", "up", "right"] {
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
    let mut config = engine(idiom);
    if idiom == nsh_core::platform::Idiom::Pad {
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
    }
    let mut app = Headless::start(config).unwrap();
    let props = observe_props(&mut app);
    // The guide stays the start screen.
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    assert_eq!(app.screen().count("id:vod-screen").unwrap(), 0);
    let surface = player(&app);
    step(app.open_guide_groups());
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
    step(app.wait_until(
        "selected VOD source published",
        Duration::from_secs(5),
        |_| {
            props
                .lock()
                .unwrap()
                .get("source")
                .is_some_and(|source| source["url"].as_str().is_some())
        },
    ));
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
    // Back returns to the title's details on Resume, over the same
    // player, opening nothing; Back again to the shelf on that title, and
    // once more to the guide, the title playing on (#147).
    let opens = iptv_app::player_opens(0);
    back_from_player(&mut app, idiom);
    step(app.wait_for("id:vod-screen"));
    step(app.wait_for("text:Play from beginning"));
    assert!(
        is_selected(&app, "label:Resume"),
        "the movie's details, on Resume"
    );
    assert!(
        app.shows("Generated beta"),
        "the playlist it was chosen from"
    );
    assert_eq!(player(&app), surface, "the same player surface");
    step(app.back());
    step(app.wait_for_gone("text:Play from beginning", Duration::from_secs(5)));
    step(app.wait_for("text:/^Generated Aurora · Film · Generated beta/"));
    assert!(
        is_selected(&app, "label:/^Generated Aurora · Film · Generated beta/"),
        "the shelf, on the title it was opened from"
    );
    step(app.back());
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    assert_eq!(iptv_app::player_opens(0), opens, "Back opens no stream");
    assert_eq!(player(&app), surface);
    // Recently added mixes both playlists' movies and series.
    step(app.open_guide_groups());
    step(app.press("label:Recently added"));
    step(app.wait_for("text:/^Generated Voyage · TV series · Generated alpha/"));
    assert_eq!(app.screen().count("id:vod-title").unwrap(), 6);
    step(app.back());
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    assert_eq!(player(&app), surface);
    assert!(
        iptv_app::player_has_source(0),
        "browsing preserves active VOD playback"
    );
    let seek = props.lock().unwrap()["seek"].clone();
    app.engine().control().dispatch_event(surface, EventKind::NativeEvent, EventPayload::Native {
        name: "timeline".into(),
        data: json!({"token":token,"seekId":seek["id"],"ms":67000,"durationMs":120000,"seekable":true}),
    });
    app.settle();
    if idiom == nsh_core::platform::Idiom::Pad {
        step(app.press("id:guide-preview"));
        crate::cov_data_player::strip(&mut app);
        step(app.press("text:Sleep timer · Off"));
        step(app.press_key("down"));
        step(app.press_key("enter"));
        step(app.fast_forward(Duration::from_secs(15 * 60)));
    } else {
        step(app.press_remote(nsh_tv::RemoteKey::Stop));
    }
    step(app.wait_until("VOD stopped", Duration::from_secs(10), |_| {
        !iptv_app::player_has_source(0)
    }));
    assert_eq!(
        player(&app),
        surface,
        "Stop/sleep never detaches the persistent player"
    );
    step(
        app.wait_until("final VOD progress saved", Duration::from_secs(10), |_| {
            crate::isolated::open_database(&path)
                .and_then(|store| store.vod_progress(&keys[1]))
                .is_ok_and(|point| point.position_ms == 67_000)
        }),
    );
}

#[derive(Clone, Copy)]
enum SourceMutation {
    Disable,
    Edit,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResumeCase {
    Seekable,
    Unseekable,
    EndedBeforeTimeline,
    PriorEndedDuringPreparation,
}

fn exercise(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom, resume: ResumeCase) {
    exercise_preparing(home, idiom, resume, false);
}
fn exercise_preparing(
    home: crate::isolated::Home,
    idiom: nsh_core::platform::Idiom,
    resume: ResumeCase,
    hold_preparation: bool,
) {
    exercise_admission(home, idiom, resume, hold_preparation, None)
}
fn exercise_admission(
    home: crate::isolated::Home,
    idiom: nsh_core::platform::Idiom,
    resume: ResumeCase,
    hold_preparation: bool,
    mutation: Option<SourceMutation>,
) {
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
    let check_movie_swipes =
        idiom == nsh_core::platform::Idiom::Pad && resume == ResumeCase::Seekable;
    if check_movie_swipes {
        config.environment.os = nsh_core::platform::Os::Android;
    }
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
    crate::browse_fixture::open_rail_item(&mut app, "Movies");
    step(app.wait_for("id:vod-screen"));
    step(app.wait_for("id:vod-title >> nth=1"));
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
    if idiom == nsh_core::platform::Idiom::Tv {
        // Right enters the grid; the next Right changes the title, not a filter.
        step(app.press_key("right"));
        step(app.press_key("right"));
        assert!(
            app.screen()
                .accessibility_state("id:vod-title >> nth=1")
                .unwrap()
                .selected
        );
        step(app.wait_for("id:vod-focused-metadata >> text:Generated Brook"));
        step(app.press_key("left"));
        step(app.press_key("left"));
        step(app.wait_for("id:vod-continue"));
    }
    step(app.press("label:/^Generated Aurora ·/"));
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
    if check_movie_swipes {
        // Exercise the full-screen video's touch path with a real VOD
        // source selected and a live channel available in the same catalog.
        assert!(app.screen().layout_nodes().iter().any(|node| {
            node.id == surface && node.bounds.size == Size::new(px(1920.0), px(1080.0))
        }));
        let opens = iptv_app::player_opens(0);
        for dy in [-300.0, 300.0] {
            crate::touch_zap::swipe_without_preview(&mut app, dy);
            // Keep the system clock used by the rest of the movie journey.
            // Allow the default 500 ms live switch delay to elapse each time.
            step(app.wait(Duration::from_secs(1)));
            assert_eq!(player(&app), surface);
            assert_eq!(iptv_app::player_opens(0), opens);
            assert_eq!(props.lock().unwrap()["source"], opened);
        }
    }
    // Back returns to the movie's details on Resume, over the same
    // player, opening nothing; Back again to the movies on that title
    // (#147).
    let opens = iptv_app::player_opens(0);
    back_from_player(&mut app, idiom);
    step(app.wait_for("id:vod-screen"));
    step(app.wait_for("text:Play from beginning"));
    assert!(
        is_selected(&app, "label:Resume"),
        "the movie's details, on Resume"
    );
    assert_eq!(player(&app), surface, "the same player surface");
    step(app.back());
    step(app.wait_for_gone("text:Play from beginning", Duration::from_secs(5)));
    step(app.wait_for("label:/^Generated Aurora ·/"));
    assert!(
        is_selected(&app, "label:/^Generated Aurora ·/"),
        "the movies, on the movie played"
    );
    assert_eq!(iptv_app::player_opens(0), opens, "Back opens no stream");
    step(app.press("id:vod-continue"));
    step(app.wait_for("text:/^Generated Aurora/"));
    assert_eq!(app.screen().count("id:vod-title").unwrap(), 1);
    step(app.press("id:vod-screen >> label:Close"));
    crate::browse_fixture::open_rail_item(&mut app, "Shows");
    step(app.wait_for("id:vod-screen"));
    step(app.press("label:/^Generated Voyage ·/"));
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
    if resume == ResumeCase::PriorEndedDuringPreparation {
        app.engine().control().dispatch_event(
            surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: "state".into(),
                data: json!({"state":"ended"}),
            },
        );
        app.settle();
    }
    let held = hold_preparation.then(iptv_app::hold_source_preparation);
    let previous_source = props.lock().unwrap()["source"].clone();
    let previous_opens = iptv_app::player_opens(0);
    let previous_seek = props.lock().unwrap().get("seek").cloned();
    if idiom == nsh_core::platform::Idiom::Tv {
        for label in ["Play from beginning", "Next episode", "Block content"] {
            step(app.press_key("right"));
            assert!(
                app.screen()
                    .accessibility_state(&format!("label:{label}"))
                    .unwrap()
                    .selected
            );
        }
        step(app.press_key("left"));
        step(app.press_key("left"));
        step(app.press_key("left"));
        step(app.press_key("down"));
        step(app.press_key("down"));
        step(app.press_key("right"));
        // Up escapes the horizontal episodes to Seasons, then Down returns
        // to the same selected episode by identity.
        step(app.press_key("up"));
        step(app.press_key("down"));
        step(app.press_key("enter"));
    } else {
        step(app.press("text:/Generated Second/"));
    }
    if let Some(held) = &held {
        step(crate::isolated::wait_outside(
            &mut app,
            "source preparation acknowledged",
            Duration::from_secs(5),
            || held.wait_entered(Duration::ZERO),
        ));
        assert_eq!(props.lock().unwrap()["source"], previous_source);
        if !app.screen().matches("id:play-bar-layer").unwrap() {
            step(app.press("label:Video"));
        }
        step(app.wait_for("id:play-bar-play-pause"));
        let screen = app.screen();
        for id in ["id:play-bar-rewind", "id:play-bar-forward"] {
            assert!(
                !screen.matches(id).unwrap(),
                "{id} is unavailable until the source is published"
            );
        }
        assert!(
            screen
                .accessibility_state("id:play-bar-seek")
                .unwrap()
                .disabled,
            "the timeline cannot seek in the unpublished source"
        );
        for (name, data) in [
            (
                "timeline",
                json!({"token":previous_source["token"],"seekId":0,"ms":117000,"durationMs":120000,"seekable":true}),
            ),
            ("firstRender", json!({"token":previous_source["token"]})),
            ("position", json!({"ms":119000})),
            ("state", json!({"state":"ended"})),
            (
                "error",
                json!({"code":"ERROR_CODE_IO_NETWORK_CONNECTION_FAILED","http":503}),
            ),
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
        if resume == ResumeCase::PriorEndedDuringPreparation {
            step(app.press("id:play-bar-play-pause"));
            assert_eq!(
                iptv_app::player_opens(0),
                previous_opens,
                "PlayPause during preparation cannot replay prior EOF or replace this preparation"
            );
        }
        assert_eq!(
            props.lock().unwrap()["source"],
            previous_source,
            "old media events cannot publish or retry the new selection"
        );
        assert_eq!(
            props.lock().unwrap().get("seek").cloned(),
            previous_seek,
            "old timeline cannot consume the new resume request"
        );
        let point = crate::isolated::open_database(&path)
            .unwrap()
            .vod_progress(&second_key)
            .unwrap();
        assert_eq!(point.position_ms, 37_000);
        assert!(!point.completed, "old EOF cannot complete the new episode");
        if let Some(mutation) = mutation {
            let mut store = crate::isolated::open_database(&path).unwrap();
            match mutation {
                SourceMutation::Disable => {
                    let mut policy = store.source_policy(source).unwrap();
                    policy.enabled = false;
                    store.set_source_policy(source, policy).unwrap();
                }
                SourceMutation::Edit => {
                    let mut edited = store.source(source).unwrap().unwrap().source;
                    if let SourceConfig::Xtream(credentials) = &mut edited.config {
                        credentials.password = Secret::new("generated-replacement-password");
                    }
                    store.update_source(source, &edited).unwrap();
                }
                SourceMutation::Delete => {
                    assert!(store.delete_source(source).unwrap());
                }
            }
            drop(store);
            held.release();
            step(app.wait_until(
                "stale VOD authority rejected",
                Duration::from_secs(5),
                |_| {
                    props
                        .lock()
                        .unwrap()
                        .get("source")
                        .is_none_or(Value::is_null)
                },
            ));
            assert_eq!(
                iptv_app::player_opens(0),
                previous_opens,
                "a stale resolved source must never reach the decoder"
            );
            assert_eq!(
                player(&app),
                surface,
                "rejection retains the persistent native surface"
            );
            return;
        }
        held.release();
    }
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    step(app.wait_until(
        "new episode source published",
        Duration::from_secs(5),
        |_| {
            props
                .lock()
                .unwrap()
                .get("source")
                .and_then(|source| source["url"].as_str())
                .is_some_and(|url| url.ends_with("/series/generated-user/generated-pass/102.mp4"))
        },
    ));
    assert_eq!(player(&app), surface);
    let opened = props.lock().unwrap()["source"].clone();
    assert!(
        opened["url"]
            .as_str()
            .unwrap()
            .ends_with("/series/generated-user/generated-pass/102.mp4")
    );
    if held.is_some() {
        assert_eq!(
            iptv_app::player_opens(0),
            previous_opens + 1,
            "the prepared title publishes exactly one new native source"
        );
    }
    let token = opened["token"].clone();
    if held.is_some() {
        app.engine().control().dispatch_event(
            surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: "timeline".into(),
                data: json!({"token":token,"seekId":0,"ms":0,"durationMs":120000,"seekable":true}),
            },
        );
        app.settle();
        let seek = props.lock().unwrap()["seek"].clone();
        assert_eq!(seek["token"], token);
        assert_eq!(
            seek["ms"], 37_000,
            "the authoritative new timeline receives the preserved resume request"
        );
        app.engine().control().dispatch_event(
            surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: "timeline".into(),
                data: json!({
                    "token": token,
                    "seekId": seek["id"],
                    "ms": 37000,
                    "durationMs": 120000,
                    "seekable": true
                }),
            },
        );
        app.settle();
        step(app.wait_until(
            "seek controls re-enable after publication",
            Duration::from_secs(5),
            |screen| {
                screen
                    .accessibility_state("label:Rewind")
                    .is_ok_and(|state| !state.disabled)
            },
        ));
    }
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
    if resume != ResumeCase::Seekable && held.is_none() {
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
    // Back from the episode: the show's details, the played episode
    // (season 2's first) selected, over the same player, opening nothing
    // (#147).
    let opens = iptv_app::player_opens(0);
    back_from_player(&mut app, idiom);
    step(app.wait_for("id:vod-screen"));
    step(app.wait_for("text:/Generated Third/"));
    assert!(
        is_selected(&app, "label:/^1 · Generated Third/"),
        "the played episode selected"
    );
    assert!(app.shows("Season 2"));
    assert_eq!(player(&app), surface, "the same player surface");
    assert_eq!(iptv_app::player_opens(0), opens, "Back opens no stream");
    // Back again leaves the details for the shows.
    step(app.back());
    step(app.wait_for_gone("text:/Generated Third/", Duration::from_secs(5)));
    step(app.wait_for("label:/^Generated Voyage ·/"));
    assert_eq!(iptv_app::player_opens(0), opens);
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
        if resume == ResumeCase::PriorEndedDuringPreparation {
            0
        } else {
            37_000
        }
    );
}

#[test]
/// M3U imports never carry movies or shows: with only an M3U playlist the
/// rail offers neither, on desktop and on TV, rather than a catalog that
/// can only be empty. (An Xtream Codes playlist beside it, also with its
/// imports off, keeps them: `the_empty_state_covers_every_playlist_the_catalog_reads`.)
fn an_m3u_only_library_offers_no_movies_or_shows_on_desktop_and_tv() {
    let Some(home) = isolated!() else { return };
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("guide.autoplay", iptv_settings::Value::Bool(false))
        .unwrap();
    settings
        .set(
            "general.resume_last_channel",
            iptv_settings::Value::Bool(false),
        )
        .unwrap();
    let mut fixture = crate::personalization::start(home, settings, true);
    let app = &mut fixture.app;
    let surface = player(app);
    for idiom in [
        nsh_core::platform::Idiom::Desktop,
        nsh_core::platform::Idiom::Tv,
    ] {
        step(app.set_environment("library discovery", |env| {
            env.idiom = idiom;
            env.os = if idiom == nsh_core::platform::Idiom::Desktop {
                nsh_core::platform::Os::Linux
            } else {
                nsh_core::platform::Os::Android
            };
        }));
        let (w, h) = if idiom == nsh_core::platform::Idiom::Tv {
            (960.0, 540.0)
        } else {
            (1280.0, 720.0)
        };
        step(app.set_viewport(nsh_core::Size::new(
            nsh_core::prelude::px(w),
            nsh_core::prelude::px(h),
        )));
        if !app.shows("id:navigation-rail >> label:Search") {
            step(app.open_guide_groups());
        }
        step(app.wait_for("id:navigation-rail >> label:Search"));
        for label in ["Movies", "Shows"] {
            assert!(
                !app.shows(&format!("id:navigation-rail >> label:{label}")),
                "{label} is not offered on {idiom:?}"
            );
        }
        assert_eq!(player(app), surface);
        assert_eq!(
            iptv_app::player_opens(0),
            0,
            "discovery never tunes live video"
        );
    }
}

const M3U_ONLY: &str = "text:Movies and shows come from Xtream Codes and Stalker playlists. M3U playlists have live channels only.";

/// What the generated Xtream Codes server answers: every request fails,
/// or an account with one live channel and (`movies`) one movie. With
/// `failing_movies` only its movie list fails; while `holding`, the movie
/// list waits. `requests` counts what it was asked.
#[derive(Clone, Copy, Default)]
struct Provider {
    failing: bool,
    failing_movies: bool,
    movies: bool,
    holding: bool,
    requests: usize,
}

/// A local Xtream Codes server answering as `provider` says at each
/// request. Its base URL.
fn provider_server(provider: Arc<Mutex<Provider>>) -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let target = first.split(' ').nth(1).unwrap_or("/");
            let action = target
                .split(['?', '&'])
                .find_map(|p| p.strip_prefix("action="))
                .unwrap_or("");
            let now = crate::support::now();
            provider.lock().unwrap().requests += 1;
            while action == "get_vod_streams" && provider.lock().unwrap().holding {
                std::thread::sleep(Duration::from_millis(20));
            }
            let answer = *provider.lock().unwrap();
            let (status, body) = if answer.failing
                || (answer.failing_movies && action == "get_vod_streams")
            {
                ("503 Service Unavailable", String::new())
            } else if target.starts_with("/xmltv.php") {
                ("200 OK", "<?xml version=\"1.0\"?><tv></tv>".to_owned())
            } else {
                let body = match action {
                    "" | "get_account_info" => format!(
                        concat!(
                            r#"{{"user_info":{{"username":"generated-user","password":"generated-pass","auth":1,"#,
                            r#""status":"Active","exp_date":"{}","is_trial":"0","active_cons":"0","#,
                            r#""max_connections":"1","allowed_output_formats":["ts"]}},"#,
                            r#""server_info":{{"url":"127.0.0.1","port":"{port}","server_protocol":"http","#,
                            r#""timezone":"UTC","timestamp_now":{now}}}}}"#,
                        ),
                        now + 30 * 86_400,
                        port = port,
                        now = now,
                    ),
                    "get_live_categories" => {
                        r#"[{"category_id":"1","category_name":"Generated group","parent_id":0}]"#
                            .to_owned()
                    }
                    "get_live_streams" => concat!(
                        r#"[{"num":1,"name":"Generated live","stream_type":"live","#,
                        r#""stream_id":1,"category_id":"1","tv_archive":0}]"#
                    )
                    .to_owned(),
                    "get_vod_streams" if answer.movies => concat!(
                        r#"[{"stream_id":10,"name":"Generated Aurora","added":"1700000100","#,
                        r#""container_extension":"mp4"}]"#
                    )
                    .to_owned(),
                    _ => "[]".to_owned(),
                };
                ("200 OK", body)
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// A stored Xtream Codes playlist of the generated provider at `base`, with
/// a live channel and no movies or series, not refreshed at startup.
/// `include_vod` is the global movie and series import. Its store path and
/// source.
fn provider_playlist(
    home: &crate::isolated::Home,
    base: &str,
    include_vod: bool,
) -> (std::path::PathBuf, iptv_library::SourceId) {
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", false),
        ("guide.autoplay", false),
        ("playlists.include_vod", include_vod),
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
            name: "Generated provider".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: base.to_owned(),
                username: Secret::new("generated-user"),
                password: Secret::new("generated-pass"),
                user_agent: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated live\nhttps://example.test/live.ts\n",
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
    (path, source)
}

fn open_movies(app: &mut Headless) {
    crate::browse_fixture::open_rail_item(app, "Movies");
    step(app.wait_for("id:vod-screen"));
}

/// The empty screen's one action: OK where the titles would be (Right from
/// the categories), or a click.
fn empty_action(app: &mut Headless, tv: bool, label: &str) {
    step(app.wait_for(&format!("id:vod-empty >> label:{label}")));
    assert_eq!(app.screen().count("id:vod-empty-action").unwrap(), 1);
    if tv {
        step(app.press_key("right"));
        step(app.press_key("enter"));
    } else {
        step(app.press("id:vod-empty-action"));
    }
}

fn provider(failing: bool, movies: bool) -> Arc<Mutex<Provider>> {
    Arc::new(Mutex::new(Provider {
        failing,
        movies,
        ..Default::default()
    }))
}

const EMPTY_MOVIES: &str = "text:No movies found. Refresh the playlist to look for new ones.";

/// A failed import says so and Retry runs it again; an import that found no
/// movies offers Refresh, which says it is importing (nothing to press)
/// and brings the provider's new movie in place. While an import offers
/// nothing, the remote waits on the categories (#146): Right reaches
/// Search, not the empty pane, and Right reaches the action once it shows.
/// A search that finds nothing then offers no action.
fn failed_then_empty(home: crate::isolated::Home, idiom: nsh_core::platform::Idiom) {
    let provider = provider(true, false);
    let base = provider_server(provider.clone());
    let (path, source) = provider_playlist(&home, &base, true);
    let mut store = crate::isolated::open_database(&path).unwrap();
    store
        .record_import_failure(source, iptv_library::ImportErrorKind::Network)
        .unwrap();
    drop(store);
    let tv = idiom == nsh_core::platform::Idiom::Tv;
    let mut app = Headless::start(engine(idiom)).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    open_movies(&mut app);
    step(app.wait_for("text:Last refresh failed: no answer from the server"));
    // The provider answers now, still without movies: Retry imports again.
    provider.lock().unwrap().failing = false;
    empty_action(&mut app, tv, "Retry");
    step(app.wait_for_within(EMPTY_MOVIES, Duration::from_secs(20)));
    assert_eq!(
        app.screen()
            .count("text:Last refresh failed: no answer from the server")
            .unwrap(),
        0
    );
    // The provider adds a movie and is slow to list it: Refresh says it
    // is importing, with nothing to press, then shows the movie.
    {
        let mut answer = provider.lock().unwrap();
        answer.movies = true;
        answer.holding = true;
    }
    // Retry's import hid the action: the remote went to the categories,
    // and Right reaches the action now shown.
    empty_action(&mut app, tv, "Refresh now");
    step(app.wait_for("text:Importing movies and shows…"));
    assert_eq!(app.screen().count("id:vod-empty-action").unwrap(), 0);
    if tv {
        // Nothing to focus where the titles would be: Right from the
        // categories reaches Search, whose OK opens its field; closing it
        // keeps the remote there, and Left returns to the categories.
        step(app.press_key("right"));
        step(app.press_key("enter"));
        step(app.wait_for("id:vod-search"));
        step(app.back());
        step(app.wait_for_gone("id:vod-search", Duration::from_secs(2)));
        step(app.press_key("left"));
    }
    provider.lock().unwrap().holding = false;
    step(app.wait_for_within("text:Generated Aurora", Duration::from_secs(20)));
    assert_eq!(app.screen().count("id:vod-empty").unwrap(), 0);
    if !tv {
        // Right from the categories enters the titles; Up from the first
        // title reaches Search; OK opens its field.
        step(app.press_key("right"));
        step(app.press_key("up"));
        step(app.press_key("enter"));
        step(app.wait_for("id:vod-search"));
        step(app.type_text("id:vod-search", "generated nothing"));
        step(app.wait_for("text:No titles found."));
        assert_eq!(app.screen().count("id:vod-empty-action").unwrap(), 0);
    }
}

#[test]
fn tv_remote_retries_a_failed_import_and_refreshes_an_empty_one() {
    let Some(home) = isolated!() else { return };
    failed_then_empty(home, nsh_core::platform::Idiom::Tv);
}

#[test]
fn desktop_retries_a_failed_import_and_refreshes_an_empty_one() {
    let Some(home) = isolated!() else { return };
    failed_then_empty(home, nsh_core::platform::Idiom::Desktop);
}

/// A provider whose movie list fails while its live channels import: the
/// refresh keeps the channels, and Movies offers Retry, not an empty list.
#[test]
fn a_movie_list_that_failed_offers_retry_not_an_empty_catalog() {
    let Some(home) = isolated!() else { return };
    let provider = provider(false, true);
    provider.lock().unwrap().failing_movies = true;
    let base = provider_server(provider.clone());
    provider_playlist(&home, &base, true);
    let mut app = Headless::start(engine(nsh_core::platform::Idiom::Desktop)).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    open_movies(&mut app);
    empty_action(&mut app, false, "Refresh now");
    step(app.wait_for_within(
        "text:Last refresh failed: the server turned the request down",
        Duration::from_secs(20),
    ));
    assert_eq!(app.screen().count(EMPTY_MOVIES).unwrap(), 0);
    assert!(app.screen().count("text:Generated live").unwrap() > 0);
    provider.lock().unwrap().failing_movies = false;
    empty_action(&mut app, false, "Retry");
    step(app.wait_for_within("text:Generated Aurora", Duration::from_secs(20)));
}

/// The catalog reads every enabled playlist, so its empty state does too:
/// an M3U playlist on screen does not hide another playlist's import switch.
#[test]
fn the_empty_state_covers_every_playlist_the_catalog_reads() {
    let Some(home) = isolated!() else { return };
    let base = provider_server(provider(false, true));
    let (path, _) = provider_playlist(&home, &base, false);
    let mut store = crate::isolated::open_database(&path).unwrap();
    let m3u = home.path().join("generated.m3u");
    let text = "#EXTM3U\n#EXTINF:-1 group-title=\"Generated local\",Generated local live\nhttps://example.test/local.ts\n";
    std::fs::write(&m3u, text).unwrap();
    let local = store
        .add_source(&Source {
            name: "Generated file playlist".into(),
            config: SourceConfig::M3u(iptv_providers::model::M3uSourceConfig {
                location: iptv_providers::SecretUrl::new(m3u.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(text);
    store
        .replace_catalog(local, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            local,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    store.set_active_source(local).unwrap();
    store.set_all_playlists(false).unwrap();
    drop(store);
    let mut app = Headless::start(engine(nsh_core::platform::Idiom::Desktop)).unwrap();
    step(app.wait_for_within("text:/^Generated local live/", Duration::from_secs(20)));
    // The M3U playlist alone on screen, from the playlists screen.
    step(app.open_guide_groups());
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Playlists >> nth=0"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:Generated file playlist"));
    step(app.press("text:Generated file playlist"));
    step(app.wait_for("text:Show this playlist"));
    step(app.press("text:Show this playlist"));
    step(app.wait_for_within("text:Generated local live", Duration::from_secs(20)));
    assert_eq!(
        app.screen().count("text:/^Generated live/").unwrap(),
        0,
        "the M3U playlist alone is on screen"
    );
    open_movies(&mut app);
    step(app.wait_for("text:Movie and series imports are off for this playlist."));
    step(app.wait_for("id:vod-empty >> label:Playlist settings"));
    assert_eq!(app.screen().count(M3U_ONLY).unwrap(), 0);
}

/// Combined playlists: Retry of the first failed playlist imports it empty,
/// and the second playlist's failure takes the screen. The remote leaves
/// the action rather than retrying a playlist it was not shown.
#[test]
fn tv_remote_never_moves_onto_another_playlists_action() {
    let Some(home) = isolated!() else { return };
    let first = provider(false, false);
    let second = provider(true, false);
    let (path, a) = provider_playlist(&home, &provider_server(first), true);
    let mut store = crate::isolated::open_database(&path).unwrap();
    let b = store
        .add_source(&Source {
            name: "Generated second".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: provider_server(second.clone()),
                username: Secret::new("generated-user"),
                password: Secret::new("generated-pass"),
                user_agent: None,
            }),
        })
        .unwrap();
    store
        .set_source_policy(
            b,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    use iptv_library::ImportErrorKind;
    store
        .record_import_failure(a, ImportErrorKind::Network)
        .unwrap();
    store
        .record_import_failure(b, ImportErrorKind::HttpStatus)
        .unwrap();
    store.set_all_playlists(true).unwrap();
    drop(store);
    let mut app = Headless::start(engine(nsh_core::platform::Idiom::Tv)).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    open_movies(&mut app);
    step(app.wait_for("text:Last refresh failed: no answer from the server"));
    empty_action(&mut app, true, "Retry");
    step(app.wait_for_within(
        "text:Last refresh failed: the server turned the request down",
        Duration::from_secs(20),
    ));
    let asked = second.lock().unwrap().requests;
    // OK where the remote now is (the categories) retries nothing.
    step(app.press_key("enter"));
    step(app.wait_for("id:vod-empty >> label:Retry"));
    std::thread::sleep(Duration::from_millis(300));
    app.settle();
    assert_eq!(
        second.lock().unwrap().requests,
        asked,
        "the second playlist"
    );
    // Reached again, the second playlist's Retry runs.
    step(app.press_key("enter"));
    step(
        app.wait_until("second playlist retried", Duration::from_secs(10), |_| {
            second.lock().unwrap().requests > asked
        }),
    );
}

/// With the movie and series import off, the empty screen opens that
/// playlist's actions at the import switch. Nothing switches it on by
/// itself; once the viewer does and refreshes, the movie arrives.
#[test]
fn an_import_switched_off_opens_that_playlists_import_switch() {
    let Some(home) = isolated!() else { return };
    let base = provider_server(provider(false, true));
    provider_playlist(&home, &base, false);
    let mut app = Headless::start(engine(nsh_core::platform::Idiom::Desktop)).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    open_movies(&mut app);
    step(app.wait_for("text:Movie and series imports are off for this playlist."));
    empty_action(&mut app, false, "Playlist settings");
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(2)));
    step(app.wait_for("text:Movies and series: No"));
    step(app.press("text:Movies and series: No"));
    step(app.wait_for("text:Movies and series: Yes"));
    step(app.press("text:Refresh now"));
    step(app.wait_for_within("text:Generated provider", Duration::from_secs(20)));
    step(app.back());
    step(app.wait_for_gone("text:Generated provider", Duration::from_secs(5)));
    open_movies(&mut app);
    step(app.wait_for_within("text:Generated Aurora", Duration::from_secs(20)));
}
