//! A tablet viewer's journey by touch alone (#88), run headless on an
//! Android tablet: visible controls reach and play a channel, a past
//! programme, a movie and an episode, and their visible Back and Close
//! return. Rotating, a narrow split-screen window and the largest system
//! text keep what plays (the same `tv.player`, never detached or
//! re-parented, no stream opened again, the movie's progress), the open
//! details and the selected episode, with the primary controls inside the
//! window. A software keyboard over search, and switching between touch
//! and a keyboard, keep a usable layout, and typing in the field never
//! reaches the player's shortcuts. Android's audio interruptions (a call,
//! another app's media, unplugged headphones) pause and resume by the
//! core's policy and never restart a stream the viewer stopped.
//!
//! Synthetic data only: a generated playlist and XMLTV guide picked from
//! files, and a generated on-demand catalogue on a local mock server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use crate::personalization::step;
use iptv_epg::format_xmltv_time;
use iptv_settings::{Settings, Value};
use nsh_core::picker::PickedFile;
use nsh_core::platform::{Idiom, Os};
use nsh_core::{
    EventKind, EventPayload, ImageData, MountItem, NodeId, Size, ViewKind, element::Root,
    prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value as Json, json};

const IMPORT: Duration = Duration::from_secs(30);
const WAIT: Duration = Duration::from_secs(5);
/// The tablet's landscape window (a 2560 × 1600 panel at 2×).
const LANDSCAPE: (f32, f32) = (1280.0, 800.0);
/// Rotated, half a split screen either way, a third of one, and back.
const PRESENTATIONS: [(f32, f32); 6] = [
    (800.0, 1280.0),
    (640.0, 800.0),
    (800.0, 640.0),
    (400.0, 800.0),
    (1280.0, 400.0),
    LANDSCAPE,
];
/// Android's largest system text sizes: 1.3 (before 14) and 2.0.
const TEXT: [f32; 2] = [1.3, 2.0];

/// What the app gave its player, and how the player view lived.
#[derive(Default)]
struct Seen {
    players: Vec<NodeId>,
    removed: Vec<NodeId>,
    inserts: HashMap<NodeId, usize>,
    /// Each stream asked for, in order (a retry of the same is not new).
    sources: Vec<String>,
    props: HashMap<String, Json>,
}

type Shared = Arc<Mutex<Seen>>;

fn observe(app: &mut Headless) -> Shared {
    let seen: Shared = Arc::default();
    let shared = seen.clone();
    app.observe(move |_, batch| {
        let mut seen = shared.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::Component { id, name } if name.as_ref() == "tv.player" => {
                    seen.players.push(*id)
                }
                MountItem::ComponentProps { props, .. } => {
                    for (key, value) in props {
                        let value = value.clone().unwrap_or(Json::Null);
                        if key.as_ref() == "source"
                            && let Some(url) =
                                crate::support::upstream_source(&value)["url"].as_str()
                            && seen.sources.last().map(String::as_str) != Some(url)
                        {
                            seen.sources.push(url.to_owned());
                        }
                        seen.props.insert(key.to_string(), value);
                    }
                }
                MountItem::Insert { child, .. } => *seen.inserts.entry(*child).or_default() += 1,
                MountItem::Remove { id } | MountItem::Delete { id } => seen.removed.push(*id),
                _ => {}
            }
        }
    });
    seen
}

fn tablet(size: (f32, f32)) -> EngineConfig {
    let mut config = EngineConfig {
        viewport: Size::new(px(size.0), px(size.1)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = Idiom::Pad;
    config.environment.os = Os::Android;
    config
}

fn player(app: &Headless) -> NodeId {
    let players: Vec<_> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.kind == ViewKind::Native)
        .collect();
    assert_eq!(players.len(), 1, "one persistent player surface");
    players[0].id
}

/// The player view was mounted once and never removed or moved.
#[track_caller]
fn persistent(app: &Headless, seen: &Shared, surface: NodeId) {
    let seen = seen.lock().unwrap();
    assert_eq!(seen.players, [surface], "one tv.player for the run");
    assert!(!seen.removed.contains(&surface), "never detached");
    assert_eq!(seen.inserts.get(&surface), Some(&1), "never re-parented");
    assert!(
        app.screen()
            .layout_nodes()
            .iter()
            .any(|n| n.id == surface && !n.hidden),
        "never hidden"
    );
}

fn sources(seen: &Shared) -> Vec<String> {
    seen.lock().unwrap().sources.clone()
}

fn video_width(app: &Headless) -> f32 {
    let surface = player(app);
    app.screen()
        .layout_nodes()
        .iter()
        .find(|n| n.id == surface)
        .unwrap()
        .bounds
        .size
        .width
        .0
}

/// The bounds of the first shown view `test_id` names.
fn bounds(app: &Headless, test_id: &str) -> Option<[f32; 4]> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| !n.hidden && n.test_id.as_deref() == Some(test_id))
        .map(|n| {
            let b = n.bounds;
            [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
        })
}

/// A primary control lies wholly inside the window (`below`: above a
/// keyboard of that height).
#[track_caller]
fn reachable(app: &Headless, test_id: &str, (w, h): (f32, f32), below: f32, what: &str) {
    let b = bounds(app, test_id).unwrap_or_else(|| panic!("{test_id} shown: {what}"));
    assert!(
        b[0] >= -0.5 && b[1] >= -0.5 && b[0] + b[2] <= w + 0.5 && b[1] + b[3] <= h - below + 0.5,
        "{test_id} {b:?} inside {w}x{h} (keyboard {below}): {what}"
    );
    assert!(b[2] > 0.0 && b[3] > 0.0, "{test_id} has a size: {what}");
}

fn present(app: &mut Headless, size: (f32, f32), text: f32) {
    step(app.set_viewport(Size::new(px(size.0), px(size.1))));
    step(app.set_environment("system text size", |e| e.font_scale = text));
    app.settle();
}

/// The bar over full-screen video, shown by a tap on the video when an
/// earlier step left it hidden.
fn show_bar(app: &mut Headless) {
    if bounds(app, "play-bar-back").is_none() {
        step(app.press("id:video"));
    }
    step(app.wait_for("id:play-bar-back"));
}

/// The video fills the window, with the bar and its visible ‹.
fn full_screen(app: &mut Headless, width: f32) {
    step(app.wait_until("full screen", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|n| n.kind == ViewKind::Native && n.bounds.size.width.0 >= width)
    }));
    show_bar(app);
}

/// Back in the guide: the video in its preview.
fn in_preview(app: &mut Headless) {
    step(app.wait_until("the guide's preview", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|n| n.kind == ViewKind::Native && n.bounds.size.width.0 < LANDSCAPE.0)
    }));
}

// --- Live: a channel and a past programme -------------------------------

const SLOT: i64 = 3600;

/// "Morning Talk" on now on channel one, "Recital Replay" two hours back
/// (in its archive), "Gala Evening" two hours on; channel two's "Other".
fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv><channel id=\"one\"><display-name>One</display-name></channel>\
         <channel id=\"two\"><display-name>Two</display-name></channel>",
    );
    for k in -20i64..20 {
        let start = now - 25 * 60 + k * SLOT;
        let one = match k {
            0 => "Morning Talk".to_owned(),
            -2 => "Recital Replay".to_owned(),
            2 => "Gala Evening".to_owned(),
            _ => format!("Show {k}"),
        };
        for (channel, title) in [("one", one), ("two", format!("Other {k}"))] {
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
                format_xmltv_time(start),
                format_xmltv_time(start + SLOT),
            ));
        }
    }
    xml + "</tv>"
}

fn playlist(guide: &str) -> String {
    format!(
        "#EXTM3U url-tvg=\"{guide}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"1\",Test Channel One\nhttp://stream.example.test/one.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/two.m3u8\n"
    )
}

#[test]
fn touch_alone_plays_a_channel_and_a_past_programme_and_returns() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = playlist(&guide_path.display().to_string());
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    let data = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();

    let mut app = Headless::start(tablet(LANDSCAPE)).unwrap();
    let seen = observe(&mut app);
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    step(app.wait_for("text:No playlists yet"));
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Morning Talk", IMPORT));
    let surface = player(&app);

    // A channel: a tap on its name plays it full screen; the visible ‹
    // returns to the guide on it, still playing in the preview.
    step(app.press("text:Test Channel Two"));
    full_screen(&mut app, LANDSCAPE.0);
    assert_eq!(sources(&seen), ["http://stream.example.test/two.m3u8"]);
    assert_eq!(video_width(&app), LANDSCAPE.0, "full screen");
    step(app.press("id:play-bar-back"));
    step(app.wait_for("text:Other 0"));
    assert!(
        video_width(&app) < LANDSCAPE.0,
        "back in the guide's preview"
    );
    assert_eq!(sources(&seen).len(), 1, "Back opens nothing");

    // A past programme in the archive: the visible ‹ steps back to it,
    // and a tap replays it full screen.
    for _ in 0..4 {
        step(app.press("label:Go back 30 minutes"));
    }
    step(app.wait_for("text:Recital Replay"));
    step(app.press("text:Recital Replay"));
    full_screen(&mut app, LANDSCAPE.0);
    let replay = sources(&seen);
    assert_eq!(replay.len(), 2, "{replay:?}");
    assert!(
        replay[1].contains("one"),
        "the replay is channel one's: {replay:?}"
    );
    assert_eq!(video_width(&app), LANDSCAPE.0);
    step(app.press("id:play-bar-back"));
    step(app.wait_until("back in the guide", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|n| n.kind == ViewKind::Native && n.bounds.size.width.0 < LANDSCAPE.0)
    }));

    // An upcoming programme's details open by a tap and offer, by
    // visible controls, what its menu holds.
    step(app.press("text:Now"));
    for _ in 0..4 {
        step(app.press("label:Go forward 30 minutes"));
    }
    step(app.wait_for("text:Gala Evening"));
    step(app.press("text:Gala Evening"));
    step(app.wait_for("id:description-layer >> text:Gala Evening"));
    assert_eq!(sources(&seen).len(), 2, "details never play");
    persistent(&app, &seen, surface);
}

// --- On demand: a movie and an episode -----------------------------------

#[test]
fn touch_alone_plays_a_movie_and_an_episode_through_rotation_split_screen_and_large_text() {
    let Some(home) = isolated!() else { return };
    let _server = crate::pane_collapse::library(&home);
    let mut app = Headless::start(tablet(LANDSCAPE)).unwrap();
    let seen = observe(&mut app);
    step(app.wait_for_within("text:Generated group", IMPORT));
    let surface = player(&app);

    // A movie: the shortcut, its poster, Play.
    step(app.press("id:open-movies"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("label:/^Generated Aurora ·/"));
    step(app.wait_for("text:Play"));
    step(app.press("text:Play"));
    step(app.wait_for_gone("id:vod-screen", WAIT));
    let movie = sources(&seen);
    assert_eq!(movie.len(), 1);
    assert!(movie[0].ends_with("/movie/generated-user/generated-pass/10.mp4"));
    let token = seen.lock().unwrap().props["source"]["token"].clone();
    // The player reports 37 s of 2 min played.
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "timeline".into(),
            data: json!({"token": token, "seekId": 0, "ms": 37_000, "durationMs": 120_000, "seekable": true}),
        },
    );
    app.settle();
    show_bar(&mut app);
    step(app.wait_for("text:0:37"));

    // Rotation, split screen and the largest text keep the movie, its
    // progress and the bar's controls on screen, on the same player.
    for text in TEXT {
        for size in PRESENTATIONS {
            present(&mut app, size, text);
            show_bar(&mut app);
            let what = format!("movie at {size:?}, text {text}");
            assert_eq!(sources(&seen), movie, "no stream opened: {what}");
            assert_eq!(video_width(&app), size.0, "still full screen: {what}");
            assert!(app.shows("0:37"), "progress kept: {what}");
            for control in ["play-bar-back", "play-bar-play-pause"] {
                reachable(&app, control, size, 0.0, &what);
            }
            persistent(&app, &seen, surface);
        }
    }
    present(&mut app, LANDSCAPE, 1.0);

    // Pause and play by the visible button: the same stream.
    show_bar(&mut app);
    step(app.press("id:play-bar-play-pause"));
    assert_eq!(seen.lock().unwrap().props["paused"], true);
    step(app.press("id:play-bar-play-pause"));
    assert_eq!(seen.lock().unwrap().props["paused"], false);
    assert_eq!(sources(&seen), movie);

    // The visible ‹ returns to the guide, the movie playing on in its
    // preview. (Back to the title's details instead: not yet, see the
    // validation record.)
    step(app.press("id:play-bar-back"));
    in_preview(&mut app);
    assert_eq!(sources(&seen), movie, "Back opens nothing");
    persistent(&app, &seen, surface);

    // An episode: the shortcut, the show, the episode.
    step(app.press("id:open-shows"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("label:/^Generated Voyage ·/"));
    step(app.wait_for("text:/Generated First/"));
    step(app.press("text:/Generated Second/"));
    step(app.wait_for_gone("id:vod-screen", WAIT));
    full_screen(&mut app, LANDSCAPE.0);
    let episode = sources(&seen);
    assert_eq!(episode.len(), 2, "{episode:?}");
    assert!(episode[1].ends_with("/series/generated-user/generated-pass/102.mp4"));
    // The visible ‹ returns to the guide; the show's details open again
    // by touch, the played episode selected.
    step(app.press("id:play-bar-back"));
    in_preview(&mut app);
    assert_eq!(sources(&seen), episode, "Back opens nothing");
    step(app.press("id:open-shows"));
    step(app.press("label:/^Generated Voyage ·/"));
    step(app.wait_for("text:/Generated First/"));

    // The show's details, rotated and split at the largest text: the
    // episode stays selected by identity and Close stays reachable.
    let episodes = |app: &Headless| {
        [
            "label:/^1 · Generated First/",
            "label:/^2 · Generated Second/",
        ]
        .map(|e| {
            app.screen()
                .accessibility_state(e)
                .is_ok_and(|s| s.selected)
        })
    };
    let selected = episodes(&app);
    assert_eq!(
        selected.iter().filter(|s| **s).count(),
        1,
        "one episode selected: {selected:?}"
    );
    for text in TEXT {
        for size in PRESENTATIONS {
            present(&mut app, size, text);
            let what = format!("details at {size:?}, text {text}");
            assert_eq!(episodes(&app), selected, "episode kept: {what}");
            assert!(app.shows("Generated Voyage"), "details open: {what}");
            assert_eq!(sources(&seen), episode, "no stream opened: {what}");
            let close = app
                .screen()
                .id("id:vod-screen >> label:Close")
                .ok()
                .and_then(|id| {
                    app.screen()
                        .layout_nodes()
                        .into_iter()
                        .find(|n| n.id == id && !n.hidden)
                })
                .map(|n| n.bounds);
            let close = close.unwrap_or_else(|| panic!("Close shown: {what}"));
            assert!(
                close.origin.x.0 + close.size.width.0 <= size.0 + 0.5
                    && close.origin.y.0 + close.size.height.0 <= size.1 + 0.5,
                "Close {close:?} inside the window: {what}"
            );
            persistent(&app, &seen, surface);
        }
    }
    present(&mut app, LANDSCAPE, 1.0);
    // Close leaves the show for the shelf, then the shelf for the guide.
    for _ in 0..3 {
        if !app.screen().matches("id:vod-screen").unwrap() {
            break;
        }
        step(app.press("id:vod-screen >> label:Close"));
    }
    step(app.wait_for_gone("id:vod-screen", WAIT));
    assert_eq!(sources(&seen), episode);
    persistent(&app, &seen, surface);
}

// --- The keyboard: software, hardware, and the player's shortcuts -------

#[test]
fn a_keyboard_over_search_edits_text_without_reaching_the_player() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    let mut fixture = crate::personalization::start_with_setup(home, settings, true, |_| {});
    let app = &mut fixture.app;
    step(app.set_environment("Android tablet", |e| {
        e.idiom = Idiom::Pad;
        e.os = Os::Android;
    }));
    step(app.set_viewport(Size::new(px(LANDSCAPE.0), px(LANDSCAPE.1))));
    let seen = observe(app);
    let surface = player(app);

    // Play a channel full screen by touch.
    step(app.press("id:guide-ui >> text:/Local A$/"));
    full_screen(app, LANDSCAPE.0);
    // The video can fill the screen a frame before its source is set.
    let opened = seen.clone();
    step(app.wait_until("the channel plays", WAIT, move |_| {
        !opened.lock().unwrap().sources.is_empty()
    }));
    let playing = sources(&seen);
    assert_eq!(playing.len(), 1, "{playing:?}");
    let paused = seen.lock().unwrap().props.get("paused").cloned();
    let muted = seen.lock().unwrap().props.get("muted").cloned();

    // Search from the guide's rail, by touch, over the playing preview.
    step(app.press("id:play-bar-back"));
    step(app.wait_until("in the guide", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|n| n.kind == ViewKind::Native && n.bounds.size.width.0 < LANDSCAPE.0)
    }));
    step(app.press("text:/^☰  /"));
    step(app.press("text:Search"));
    step(app.wait_for("id:search-field"));
    // The software keyboard comes up over the lower part of the window.
    for (size, keyboard) in [
        (LANDSCAPE, 340.0),
        ((800.0, 1280.0), 420.0),
        ((640.0, 800.0), 340.0),
    ] {
        step(app.set_viewport(Size::new(px(size.0), px(size.1))));
        step(app.set_environment("software keyboard", |e| e.keyboard_height = px(keyboard)));
        app.settle();
        reachable(
            app,
            "search-field",
            size,
            keyboard,
            &format!("{size:?} with the keyboard up"),
        );
    }
    step(app.set_viewport(Size::new(px(LANDSCAPE.0), px(LANDSCAPE.1))));
    // Letters, space and digits that are player shortcuts elsewhere only
    // type: nothing pauses, mutes, zaps or leaves the screen.
    step(app.type_text("id:search-field", "Local k m f 2 "));
    assert_eq!(
        app.screen().input_text("id:search-field").unwrap(),
        Some("Local k m f 2 ")
    );
    step(app.press_key("space"));
    step(app.press_key("k"));
    step(app.press_key("m"));
    assert!(app.screen().matches("id:search-field").unwrap());
    let props = seen.lock().unwrap().props.clone();
    assert_eq!(props.get("paused").cloned(), paused, "typing never pauses");
    assert_eq!(props.get("muted").cloned(), muted, "typing never mutes");
    assert_eq!(sources(&seen), playing, "typing never zaps");

    // The keyboard goes down: the field and the results stay usable, by
    // touch and by a hardware keyboard alike.
    step(app.set_environment("keyboard down", |e| e.keyboard_height = px(0.0)));
    app.settle();
    step(app.clear("id:search-field"));
    step(app.type_text("id:search-field", "Local"));
    step(app.wait_for("text:/Channels · 3/"));
    step(app.press_key("down"));
    step(app.press("text:/Channels · 3/"));
    reachable(app, "search-field", LANDSCAPE, 0.0, "keyboard down");
    assert_eq!(sources(&seen), playing);
    assert_eq!(player(app), surface);
}

// --- Interruptions: the audio focus and route -----------------------------

fn audio(app: &mut Headless, surface: NodeId, change: &str) {
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "audio".into(),
            data: json!({ "change": change }),
        },
    );
    app.settle();
}

fn paused(seen: &Shared) -> bool {
    seen.lock().unwrap().props.get("paused") == Some(&Json::Bool(true))
}

#[test]
fn calls_and_unplugged_headphones_pause_by_policy_and_never_restart_a_stop() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    let mut fixture = crate::personalization::start_with_setup(home, settings, true, |_| {});
    let app = &mut fixture.app;
    step(app.set_environment("Android tablet", |e| {
        e.idiom = Idiom::Pad;
        e.os = Os::Android;
    }));
    step(app.set_viewport(Size::new(px(LANDSCAPE.0), px(LANDSCAPE.1))));
    let seen = observe(app);
    let surface = player(app);
    step(app.press("id:guide-ui >> text:/Local A$/"));
    full_screen(app, LANDSCAPE.0);
    // The video can fill the screen a frame before its source is set.
    let opened = seen.clone();
    step(app.wait_until("the channel plays", WAIT, move |_| {
        !opened.lock().unwrap().sources.is_empty()
    }));
    let playing = sources(&seen);
    assert_eq!(playing.len(), 1);
    assert!(!paused(&seen));

    // A call: paused, and resumed when it ends. Keys that are no playback
    // action meanwhile (Info, moving a highlight) leave it the call's.
    audio(app, surface, "focusLossTransient");
    assert!(paused(&seen), "a call pauses");
    step(app.press_remote(nsh_tv::RemoteKey::Info));
    audio(app, surface, "focusGain");
    assert!(!paused(&seen), "its end resumes");

    // Headphones unplugged: paused for good.
    audio(app, surface, "noisy");
    assert!(paused(&seen), "the route going away pauses");
    audio(app, surface, "focusGain");
    assert!(paused(&seen), "and nothing resumes it");
    // The viewer resumes by the visible button.
    show_bar(app);
    step(app.press("id:play-bar-play-pause"));
    assert!(!paused(&seen));

    // Another app takes the audio for good: paused, not resumed.
    audio(app, surface, "focusLoss");
    assert!(paused(&seen));
    audio(app, surface, "focusGain");
    assert!(paused(&seen));
    step(app.press("id:play-bar-play-pause"));
    assert!(!paused(&seen));

    // During a call the viewer plays, then pauses again: their pause.
    audio(app, surface, "focusLossTransient");
    assert!(paused(&seen));
    step(app.press("id:play-bar-play-pause"));
    step(app.press("id:play-bar-play-pause"));
    assert!(paused(&seen));
    audio(app, surface, "focusGain");
    assert!(paused(&seen), "the viewer's pause is kept");
    step(app.press("id:play-bar-play-pause"));
    assert!(!paused(&seen));
    assert_eq!(sources(&seen), playing, "no stream opened by any of it");

    // A call, then the viewer stops: the call's end restarts nothing.
    audio(app, surface, "focusLossTransient");
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    step(app.wait_until("stopped", WAIT, |_| !iptv_app::player_has_source(0)));
    audio(app, surface, "focusGain");
    step(app.wait(Duration::from_secs(1)));
    assert!(!iptv_app::player_has_source(0), "a stop stays stopped");
    assert_eq!(sources(&seen), playing, "no stream opened again");
    // Observed from after the player mounted: never removed or moved since.
    let seen = seen.lock().unwrap();
    assert!(!seen.removed.contains(&surface) && !seen.inserts.contains_key(&surface));
    assert_eq!(player(app), surface);
}

const TILE: &str = "id:multiview-tile";
const TILE_MENU: &str = "id:multiview-menu";
const TILE_PAUSED: &str = "id:multiview-paused";

fn paused_tiles(app: &Headless) -> usize {
    app.screen()
        .count(&format!("{TILE} >> {TILE_PAUSED}"))
        .unwrap()
}

/// Multiview: the heard tile (the active one; the others are muted) is
/// what a call or unplugged headphones pause, and what a call's end
/// resumes.
#[test]
fn interruptions_pause_and_resume_the_heard_multiview_tile() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    let mut fixture = crate::personalization::start_with_setup(home, settings, true, |_| {});
    let app = &mut fixture.app;
    step(app.set_environment("Android tablet", |e| {
        e.idiom = Idiom::Pad;
        e.os = Os::Android;
    }));
    step(app.set_viewport(Size::new(px(LANDSCAPE.0), px(LANDSCAPE.1))));
    let surface = player(app);
    step(app.press("id:guide-ui >> text:/Local A$/"));
    full_screen(app, LANDSCAPE.0);
    let seen = observe(app);
    step(app.press("label:Multiview"));
    step(app.wait_for(TILE));
    step(app.press("id:multiview-picker >> text:/Local B$/"));
    step(app.wait_until("two tiles", WAIT, |s| s.count(TILE) == Ok(2)));
    assert_eq!(paused_tiles(app), 0);
    // The first tile (its stream open; the fixture's account allows one)
    // becomes the heard one.
    step(app.press(&format!("{TILE} >> text:/Local A$/")));
    if app.screen().matches(TILE_MENU).unwrap() {
        step(app.press(&format!("{TILE} >> text:/Local A$/")));
    }

    // Headphones unplugged: the heard tile pauses, and stays paused.
    audio(app, surface, "noisy");
    assert_eq!(paused_tiles(app), 1, "the heard tile pauses");
    audio(app, surface, "focusGain");
    assert_eq!(paused_tiles(app), 1, "nothing resumes it");

    // The viewer plays it again from its menu.
    assert!(
        app.screen()
            .matches(&format!("{TILE} >> text:/Local A$/"))
            .unwrap()
    );
    step(app.press(&format!("{TILE} >> text:/Local A$/")));
    step(app.wait_for(TILE_MENU));
    step(app.press(&format!("{TILE_MENU} >> text:Play")));
    step(app.wait_for_gone(TILE_MENU, WAIT));
    assert_eq!(paused_tiles(app), 0);

    // A call pauses the heard tile; its end resumes it, also after the
    // highlight moved in the tile's menu meanwhile.
    audio(app, surface, "focusLossTransient");
    assert_eq!(paused_tiles(app), 1, "a call pauses the heard tile");
    step(app.press(&format!("{TILE} >> text:/Local A$/")));
    step(app.wait_for(TILE_MENU));
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.back());
    step(app.wait_for_gone(TILE_MENU, WAIT));
    audio(app, surface, "focusGain");
    assert_eq!(paused_tiles(app), 0, "its end resumes it");

    // A call pauses it again, then multiview ends: the stream surface 0
    // keeps stays paused full screen, and the pause is still the call's
    // (its end resumes it there).
    audio(app, surface, "focusLossTransient");
    assert_eq!(paused_tiles(app), 1);
    // Back with no dialog open leaves to the active tile, full screen.
    step(app.back());
    step(app.wait_for_gone(TILE, WAIT));
    assert!(paused(&seen), "the kept stream stays paused");
    audio(app, surface, "focusGain");
    assert!(!paused(&seen), "the call's end resumes it full screen");
}
