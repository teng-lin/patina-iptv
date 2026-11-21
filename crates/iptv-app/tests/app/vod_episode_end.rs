//! An episode ending never changes the screen on its own (#187): full
//! screen goes on to the next episode (or its series' details), the
//! series' details keep the next one in the preview (or stopped, chosen),
//! and anywhere else playback stops, the next episode up next. A PIN is
//! asked for only when the viewer plays the next episode.
use crate::{
    headless::Headless,
    personalization::step,
    support::Server,
    vod_library::{SERIES_INFO, engine, episode_key, is_selected, observe_props, player},
};
use iptv_providers::{
    HttpResponse, Secret,
    model::{ContentKind, Source, SourceConfig, XtreamCredentials},
};
use nsh_core::{EventKind, EventPayload, NodeId};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Where the viewer is when the first episode ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum At {
    /// That episode's player, full screen.
    FullScreen,
    /// The series' details, Back from full screen (#147).
    Details,
    /// The Shows list, Back from the details.
    Shows,
    /// The guide, Back from the Shows list.
    Guide,
}

/// What a PIN protects (generated PIN 2468).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lock {
    None,
    /// The series: its PIN is asked for when it is opened.
    Series,
    /// Only the next episode.
    NextEpisode,
}

const PIN: &str = "id:parental-layer >> text:Enter parental PIN";
const READY: &str = "text:Ready for the next episode";
const FIRST: &str = "label:/^1 · Generated First/";
const SECOND: &str = "label:/^2 · Generated Second/";
const SERIES: &str = "label:/^Generated Voyage ·/";

type Props = Arc<Mutex<HashMap<String, Value>>>;

struct Run {
    app: Headless,
    props: Props,
    surface: NodeId,
    path: PathBuf,
    series: iptv_core::vod::Key,
}

fn source_url(props: &Props) -> Option<String> {
    props
        .lock()
        .unwrap()
        .get("source")
        .and_then(|s| s["url"].as_str().map(str::to_owned))
}

fn plays(app: &mut Headless, props: &Props, episode: &str) {
    let suffix = format!("/series/generated-user/generated-pass/{episode}.mp4");
    step(app.wait_until(
        &format!("episode {episode} plays"),
        Duration::from_secs(10),
        |_| source_url(props).is_some_and(|url| url.ends_with(&suffix)),
    ));
}

/// A generated series of three episodes, the viewer at `at` with its first
/// episode playing.
fn watch(home: crate::isolated::Home, at: At, autoplay: bool, lock: Lock) -> Run {
    let server = Server::start(&[("/player_api.php", SERIES_INFO.into())]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", false),
        ("guide.autoplay", false),
        ("vod.autoplay_next_episode", autoplay),
        ("parental.vod_enabled", lock != Lock::None),
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
        .replace_catalog(source, &groups, &parsed.channels.items, &[], &series.items)
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
    if lock != Lock::None {
        // Generated PIN 2468, salt 0x63, PBKDF2-SHA256/210000.
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
        let locked = match lock {
            Lock::Series => series_key.clone(),
            _ => episode_key(&series_key, "102"),
        };
        store
            .set_parental_blocked(&iptv_runtime::vod::target(&locked), true)
            .unwrap();
    }
    drop(store);
    let mut app = Headless::start(engine(nsh_core::platform::Idiom::Tv)).unwrap();
    let props = observe_props(&mut app);
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    let surface = player(&app);
    step(app.press("text:/^☰  /"));
    step(app.press("id:open-shows"));
    step(app.wait_for("id:vod-screen"));
    step(app.press(SERIES));
    if lock == Lock::Series {
        step(app.wait_for_within(PIN, Duration::from_secs(20)));
        for digit in "2468".chars() {
            step(app.press_key(&digit.to_string()));
        }
        step(app.press("id:parental-layer >> text:Continue"));
        step(app.wait_for_gone(PIN, Duration::from_secs(20)));
    }
    step(app.wait_for(FIRST));
    step(app.press(FIRST));
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    plays(&mut app, &props, "101");
    assert!(!app.shows("Enter parental PIN"), "the series was unlocked");
    if at != At::FullScreen {
        // Back from the episode: the series' details (#147).
        step(app.back());
        step(app.wait_for("id:vod-screen"));
        step(app.wait_for(FIRST));
    }
    if matches!(at, At::Shows | At::Guide) {
        step(app.back());
        step(app.wait_for_gone(FIRST, Duration::from_secs(5)));
        step(app.wait_for(SERIES));
    }
    if at == At::Guide {
        // Back leaves the list for its categories, then the guide.
        for _ in 0..4 {
            if !app.screen().matches("id:vod-screen").unwrap() {
                break;
            }
            step(app.back());
        }
        step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    }
    assert!(iptv_app::player_has_source(0), "the episode plays on");
    Run {
        app,
        props,
        surface,
        path,
        series: series_key,
    }
}

/// The player reports the playing episode's last frame, then its end.
fn end(run: &mut Run) {
    let token = run.props.lock().unwrap()["source"]["token"].clone();
    for (name, data) in [
        (
            "timeline",
            json!({"token": token, "seekId": 0, "ms": 120_000, "durationMs": 120_000, "seekable": true}),
        ),
        ("state", json!({"state": "ended"})),
    ] {
        run.app.engine().control().dispatch_event(
            run.surface,
            EventKind::NativeEvent,
            EventPayload::Native {
                name: name.into(),
                data,
            },
        );
        run.app.settle();
    }
}

/// Nothing plays and no further episode opened since `opens`.
fn stopped(run: &mut Run, opens: u64) {
    step(
        run.app
            .wait_until("playback stopped", Duration::from_secs(10), |_| {
                !iptv_app::player_has_source(0)
            }),
    );
    assert_eq!(player(&run.app), run.surface, "the same player surface");
    assert_eq!(iptv_app::player_opens(0), opens, "no next episode opened");
}

/// The finished episode is saved watched, and its series waits in
/// Continue watching for the next one.
fn up_next(run: &mut Run) {
    let first = episode_key(&run.series, "101");
    let path = run.path.clone();
    step(run.app.wait_until(
        "the finished episode saved watched",
        Duration::from_secs(10),
        |_| {
            crate::isolated::open_database(&path)
                .and_then(|store| store.vod_progress(&first))
                .is_ok_and(|point| point.completed)
        },
    ));
    let store = crate::isolated::open_database(&run.path).unwrap();
    let series = store.vod_progress(&run.series).unwrap();
    assert!(!series.completed, "the series is not finished");
    let shelf = store
        .vod_page(
            &iptv_core::vod::Query {
                kind: iptv_core::vod::Kind::Series,
                continue_watching: true,
                ..Default::default()
            },
            0,
            20,
        )
        .unwrap()
        .0;
    assert_eq!(
        shelf.iter().map(|c| &c.key).collect::<Vec<_>>(),
        [&run.series],
        "Continue watching holds the series"
    );
}

/// Opening the series again chooses the next episode.
fn opens_on_next(run: &mut Run) {
    step(run.app.press(SERIES));
    step(run.app.wait_for(SECOND));
    assert!(
        is_selected(&run.app, SECOND),
        "the series' details choose the next episode"
    );
}

#[test]
fn full_screen_with_autoplay_plays_the_next_episode_full_screen() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::FullScreen, true, Lock::None);
    end(&mut run);
    plays(&mut run.app, &run.props, "102");
    assert!(!run.app.shows("Ready for the next episode"));
    assert!(
        !run.app.screen().matches("id:vod-screen").unwrap(),
        "still full screen"
    );
    assert_eq!(player(&run.app), run.surface);
}

#[test]
fn full_screen_without_autoplay_opens_the_series_details() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::FullScreen, false, Lock::None);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    step(run.app.wait_for(READY));
    step(run.app.wait_for("id:vod-screen"));
    assert!(is_selected(&run.app, SECOND), "the next episode chosen");
    assert_eq!(iptv_app::player_opens(0), opens, "nothing more plays");
}

#[test]
fn series_details_with_autoplay_play_the_next_episode_in_the_preview() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Details, true, Lock::None);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    plays(&mut run.app, &run.props, "102");
    assert_eq!(iptv_app::player_opens(0), opens + 1, "one stream opens");
    // The details stay, the next episode (season 1's second) chosen.
    assert!(run.app.screen().matches("id:vod-screen").unwrap());
    step(
        run.app
            .wait_until("the next episode chosen", Duration::from_secs(5), |s| {
                s.accessibility_state(SECOND)
                    .is_ok_and(|state| state.selected)
            }),
    );
    assert!(run.app.shows("Season 1"));
    assert!(!run.app.shows("Ready for the next episode"));
    assert!(!run.app.shows("Enter parental PIN"));
    assert_eq!(player(&run.app), run.surface);
    // That one ending in the preview goes on to season 2's first.
    end(&mut run);
    plays(&mut run.app, &run.props, "201");
    assert_eq!(iptv_app::player_opens(0), opens + 2);
    assert!(run.app.screen().matches("id:vod-screen").unwrap());
    step(
        run.app
            .wait_until("season 2's first chosen", Duration::from_secs(5), |s| {
                s.accessibility_state("label:/^1 · Generated Third/")
                    .is_ok_and(|state| state.selected)
            }),
    );
    assert!(run.app.shows("Season 2"));
    // OK on Play takes it full screen the usual way.
    step(run.app.press_key("enter"));
    step(
        run.app
            .wait_for_gone("id:vod-screen", Duration::from_secs(5)),
    );
    step(run.app.wait_until(
        "the chosen episode opens again full screen",
        Duration::from_secs(10),
        |_| iptv_app::player_opens(0) == opens + 3,
    ));
    plays(&mut run.app, &run.props, "201");
}

#[test]
fn series_details_without_autoplay_stop_and_choose_the_next_episode() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Details, false, Lock::None);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    stopped(&mut run, opens);
    step(run.app.wait_for(READY));
    assert!(run.app.screen().matches("id:vod-screen").unwrap());
    assert!(is_selected(&run.app, SECOND), "the next episode chosen");
    up_next(&mut run);
}

#[test]
fn a_list_with_autoplay_stops_with_the_next_episode_up_next() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Shows, true, Lock::None);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    stopped(&mut run, opens);
    // Still the Shows list, on the series.
    assert!(run.app.screen().matches("id:vod-screen").unwrap());
    assert!(is_selected(&run.app, SERIES), "the series' card chosen");
    assert!(!run.app.shows("Ready for the next episode"));
    up_next(&mut run);
    opens_on_next(&mut run);
}

#[test]
fn the_guide_without_autoplay_stops_with_the_next_episode_up_next() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Guide, false, Lock::None);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    stopped(&mut run, opens);
    step(run.app.wait_for("text:Nothing playing"));
    assert!(
        !run.app.screen().matches("id:vod-screen").unwrap(),
        "still the guide"
    );
    up_next(&mut run);
}

#[test]
fn the_guide_with_autoplay_stops_too() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Guide, true, Lock::None);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    stopped(&mut run, opens);
    step(run.app.wait_for("text:Nothing playing"));
    assert!(!run.app.screen().matches("id:vod-screen").unwrap());
    up_next(&mut run);
}

#[test]
fn a_list_without_autoplay_stops_too() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Shows, false, Lock::None);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    stopped(&mut run, opens);
    assert!(run.app.screen().matches("id:vod-screen").unwrap());
    assert!(!run.app.shows("Ready for the next episode"));
    up_next(&mut run);
    opens_on_next(&mut run);
}

/// The series' PIN was entered to open it: the next episode plays in the
/// preview with no prompt.
#[test]
fn an_unlocked_series_plays_the_next_episode_in_the_preview_without_a_prompt() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Details, true, Lock::Series);
    end(&mut run);
    plays(&mut run.app, &run.props, "102");
    assert!(!run.app.shows("Enter parental PIN"), "no PIN prompt");
    assert!(run.app.screen().matches("id:vod-screen").unwrap());
    assert!(is_selected(&run.app, SECOND));
}

/// The next episode needs a PIN: no prompt pops up; it waits, chosen, and
/// its PIN is asked for once the viewer plays it.
#[test]
fn a_next_episode_needing_a_pin_waits_without_a_prompt() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::Details, true, Lock::NextEpisode);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    stopped(&mut run, opens);
    step(run.app.wait_for(READY));
    assert!(!run.app.shows("Enter parental PIN"), "no PIN prompt");
    assert!(run.app.screen().matches("id:vod-screen").unwrap());
    assert!(is_selected(&run.app, SECOND), "the next episode chosen");
    // OK on Play asks for its PIN.
    step(run.app.press_key("enter"));
    step(run.app.wait_for_within(PIN, Duration::from_secs(20)));
}

/// Full screen too, a next episode needing a PIN pops no prompt: its
/// series' details open on it, as with autoplay off.
#[test]
fn full_screen_with_a_next_episode_needing_a_pin_opens_the_details_without_a_prompt() {
    let Some(home) = isolated!() else { return };
    let mut run = watch(home, At::FullScreen, true, Lock::NextEpisode);
    let opens = iptv_app::player_opens(0);
    end(&mut run);
    step(run.app.wait_for(READY));
    step(run.app.wait_for("id:vod-screen"));
    assert!(!run.app.shows("Enter parental PIN"), "no PIN prompt");
    assert!(is_selected(&run.app, SECOND), "the next episode chosen");
    assert_eq!(iptv_app::player_opens(0), opens, "nothing more plays");
}
