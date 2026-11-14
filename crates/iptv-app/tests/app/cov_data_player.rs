//! The player's health and controls, run headless on a manual clock: the
//! test plays the `tv.player` host, sending its `state`, `error`,
//! `position`, `size`, `tracks`, `format`, `pip` and `pipAction` events
//! to surface 0 and reading the props the app sends it (`source`,
//! `videoTrack`, `volume`, ...).
//!
//! What it checks: the waiting pill (loading, buffering, a reconnect
//! quiet for a second, then "Reconnecting…"), a dead live stream's
//! retries and stall checks until the failure card after 150 s (retries
//! going on behind it), OK and "Try again" retrying at once, a fatal
//! error's card at once, a never-played stream's card after the fast
//! attempts, a refusal's wording, a closed live stream reconnecting at
//! once; the tracks the host reports making the strip's choosers act, a
//! chosen track, delay, picture size reaching the host, a new stream
//! forgetting them; the strip's and the card's buttons, the remote's
//! other keys (previous channel, info, play/pause, the guide over the
//! video, Stop); the window's buttons; catch-up: an older day paged in,
//! reopening where it played, giving up after the fast attempts, seeking
//! (keys, the play bar's buttons and its progress line), the end of a
//! replay and Next back to live; the play bar: what the host reported,
//! mute and volume, tracks, programme details, hiding by itself, picture
//! in picture (stopping playback closes it).
//!
//! Synthetic data only: a three-channel playlist "picked" from a file,
//! channel one with three days of catch-up, its guide a local XMLTV file.

use crate::browse_fixture;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::time::Clock;
use nsh_core::{
    EventKind, EventPayload, ImageData, MountItem, NodeId, Point, Size, ViewKind, element::Root,
    prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value, json};

const IMPORT: Duration = Duration::from_secs(20);
/// Programmes of 45 minutes, "Show <n>" counting from the one on now (0).
const LEN: i64 = 45 * 60;
const CARD: &str = "id:player-card";

/// The props the app last sent each native component.
type Props = Arc<Mutex<HashMap<NodeId, HashMap<String, Value>>>>;

fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv>\
         <channel id=\"one\"><display-name>One</display-name></channel>\
         <channel id=\"two\"><display-name>Two</display-name></channel>",
    );
    let first = now - 3 * 86_400 + 20 * 60;
    let live = (now - first) / LEN;
    for channel in ["one", "two"] {
        let mut k = 0;
        while first + k * LEN < now + 6 * 3_600 {
            let (start, stop) = (first + k * LEN, first + (k + 1) * LEN);
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>Show {}</title></programme>",
                format_xmltv_time(start),
                format_xmltv_time(stop),
                k - live
            ));
            k += 1;
        }
    }
    xml + "</tv>"
}

fn step(r: Result<(), String>) {
    r.unwrap_or_else(|e| panic!("{e}"));
}

/// Surface 0's `tv.player` (the only one outside multiview).
fn player(app: &Headless) -> NodeId {
    let natives: Vec<_> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.kind == ViewKind::Native)
        .collect();
    assert_eq!(natives.len(), 1, "one player surface");
    natives[0].id
}

/// The host sends `name` with `data` from surface 0; the app settles.
fn send(app: &mut Headless, name: &str, data: Value) {
    let id = player(app);
    app.engine().control().dispatch_event(
        id,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: name.into(),
            data,
        },
    );
    app.settle();
}

fn state(app: &mut Headless, state: &str) {
    send(app, "state", json!({ "state": state }));
}

fn network_error(app: &mut Headless) {
    send(
        app,
        "error",
        json!({ "code": "ERROR_CODE_IO_NETWORK_CONNECTION_FAILED" }),
    );
}

/// The last value of prop `name` the app sent surface 0.
fn prop(app: &Headless, props: &Props, name: &str) -> Value {
    let id = player(app);
    props
        .lock()
        .unwrap()
        .get(&id)
        .and_then(|p| p.get(name).cloned())
        .unwrap_or(Value::Null)
}

/// The view taking touches along a bar (the volume, the progress line):
/// laid over the bar it shares its bounds with (sized `size(w, h)`), the
/// last such one on screen.
fn touch_area(app: &Headless, size: impl Fn(f32, f32) -> bool) -> String {
    let nodes = app.screen().layout_nodes();
    let over = nodes
        .iter()
        .enumerate()
        .filter(|(i, n)| {
            n.text.is_none()
                && !n.hidden
                && size(n.bounds.size.width.0, n.bounds.size.height.0)
                && nodes[..*i].iter().any(|m| m.bounds == n.bounds)
        })
        .map(|(_, n)| n.id)
        .last()
        .expect("a touch area");
    format!("view:{over}")
}

/// Whether something on screen matches `selector`.
fn seen(app: &Headless, selector: &str) -> bool {
    app.screen().matches(selector).unwrap()
}

fn opens() -> u64 {
    iptv_app::player_opens(0)
}

/// Moves the clock by `time` (timers run as they come due).
fn run(app: &mut Headless, time: Duration) {
    step(app.run_for(time));
}

#[test]
fn the_player_follows_its_host_through_waits_failures_and_replays() {
    let Some(home) = isolated!() else { return };
    browse_fixture::two_step_preview(home.path());
    // Log lines run as in the app (into the test's captured output).
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    // Where the first playlist's last channel goes is taken (a directory):
    // saving it fails, which only logs.
    let data = home.data_dir();
    std::fs::create_dir_all(data.join("last_channel-1")).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"3\",Test Channel One\nhttp://stream.example.test/one.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/two.m3u8\n\
         #EXTINF:-1 tvg-id=\"three\" group-title=\"News\",Test Channel Three\nhttp://stream.example.test/three.m3u8\n",
        guide_path.display()
    );
    let size = Size::new(px(960.0), px(540.0));
    let mut config = EngineConfig {
        viewport: size,
        clock: Clock::manual(SystemTime::now()),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    // The window full screen video fills.
    config.environment.window = size;
    let mut app = Headless::start(config).unwrap();
    let props: Props = Arc::default();
    {
        let props = props.clone();
        app.observe(move |_, batch| {
            let mut props = props.lock().unwrap();
            for item in &batch.items {
                if let MountItem::ComponentProps { id, props: changed } = item {
                    let entry = props.entry(*id).or_default();
                    for (name, value) in changed {
                        entry.insert(name.to_string(), value.clone().unwrap_or(Value::Null));
                    }
                }
            }
        });
    }
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
    step(app.wait_for_within("text:Show 0", IMPORT));
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));

    // --- Waiting -----------------------------------------------------------
    // OK plays channel one in the preview: the host gets its stream, live,
    // and the preview's pill says it loads until the host is ready.
    let before = opens();
    step(app.press_key("enter"));
    step(app.wait_for("text:Loading…"));
    assert_eq!(opens(), before + 1, "one open");
    // No channel played before it: ⏮ does nothing.
    step(app.press_key("p"));
    assert_eq!(opens(), before + 1, "nothing else opened");
    let source = prop(&app, &props, "source");
    assert_eq!(source["url"], "http://stream.example.test/one.m3u8");
    assert_eq!(
        (&source["live"], &source["reconnect"]),
        (&json!(true), &json!(false))
    );
    state(&mut app, "ready");
    step(app.wait_for_gone("text:Loading…", Duration::from_secs(1)));
    // Rebuffering shows its own pill; what the app does not know changes
    // nothing.
    state(&mut app, "buffering");
    step(app.wait_for("text:Buffering…"));
    state(&mut app, "ready");
    step(app.wait_for_gone("text:Buffering…", Duration::from_secs(1)));
    state(&mut app, "warming-up");
    send(&mut app, "unknown", json!({}));
    assert!(!seen(&app, "text:/…$/"), "no pill");

    // A stream that played drops: it reconnects quietly for a second
    // (the last frame stays), then says so; the retry reopens it as a
    // reconnect after a second.
    network_error(&mut app);
    assert!(!seen(&app, "text:Reconnecting…"), "quiet at first");
    run(&mut app, Duration::from_millis(1100));
    step(app.wait_for("text:Reconnecting…"));
    assert_eq!(opens(), before + 2, "the retry reopened it");
    assert_eq!(prop(&app, &props, "source")["reconnect"], json!(true));
    state(&mut app, "ready");
    step(app.wait_for_gone("text:Reconnecting…", Duration::from_secs(1)));

    // --- Failures ----------------------------------------------------------
    // Full screen; then the stream dies for good: stall checks and retries
    // go on (each reopened stream stalls after 15 s) until it has been down
    // 150 s, then the card says so, retries going on behind it.
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner"));
    let before = opens();
    network_error(&mut app);
    let mut down = Duration::ZERO;
    while !seen(&app, "text:/^Can't play/") {
        assert!(down < Duration::from_secs(200), "no card after {down:?}");
        run(&mut app, Duration::from_secs(5));
        down += Duration::from_secs(5);
    }
    assert!(down >= Duration::from_secs(150), "the card after {down:?}");
    step(app.wait_for("text:Can't play Test Channel One"));
    // The last failure was a stall.
    step(app.wait_for("text:The stream stopped sending video."));
    step(app.wait_for("text:/· still trying$/"));
    step(app.wait_for("text:OK  Try again"));
    assert!(opens() >= before + 6, "{} retries", opens() - before);
    // Behind the card retries go on (every 30 s, each stalling in 15).
    let behind = opens();
    run(&mut app, Duration::from_secs(46));
    assert!(opens() > behind, "a retry behind the card");
    assert!(seen(&app, "text:/^Can't play/"), "the card stays");

    // OK tries again at once: a new session, the card gone.
    let before = opens();
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:/^Can't play/", Duration::from_secs(1)));
    assert_eq!(opens(), before + 1);
    state(&mut app, "ready");

    // A fatal error (the provider has no such stream): the card at once,
    // no retries, its code as the detail.
    send(
        &mut app,
        "error",
        json!({ "code": "ERROR_CODE_IO_BAD_HTTP_STATUS", "http": 404 }),
    );
    step(app.wait_for("text:The provider doesn't have this stream."));
    step(app.wait_for("text:ERROR_CODE_IO_BAD_HTTP_STATUS 404"));
    let before = opens();
    run(&mut app, Duration::from_secs(40));
    assert_eq!(opens(), before, "no retries");

    // Try again; a stream that never plays gives the fast attempts, then
    // the card (retrying behind it).
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:/^Can't play/", Duration::from_secs(1)));
    for _ in 0..6 {
        network_error(&mut app);
        run(&mut app, Duration::from_secs(5));
    }
    step(app.wait_for("text:Can't play Test Channel One"));
    step(app.wait_for("text:/· still trying$/"));

    // A refusal names the provider's limit when it is not known.
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:/^Can't play/", Duration::from_secs(1)));
    for _ in 0..6 {
        send(
            &mut app,
            "error",
            json!({ "code": "ERROR_CODE_IO_BAD_HTTP_STATUS", "http": 403 }),
        );
        run(&mut app, Duration::from_secs(5));
    }
    step(app.wait_for("text:The provider is refusing this stream for now."));

    // Zapping starts a new session: the card goes.
    step(app.press_key("down"));
    step(app.wait_for_gone("text:/^Can't play/", Duration::from_secs(1)));
    step(app.wait_for("id:player-banner >> text:/Test Channel Two$/"));
    state(&mut app, "ready");

    // --- Tracks and format -------------------------------------------------
    // What the host reports of the open stream: its tracks make the
    // strip's track buttons act; a chosen track goes back to the host.
    send(&mut app, "size", json!({ "width": 1280, "height": 720 }));
    send(&mut app, "format", json!({ "fps": 25.0, "channels": 6 }));
    send(&mut app, "format", json!({}));
    send(&mut app, "tracks", tracks());
    send(&mut app, "tracks", tracks());
    send(&mut app, "position", json!({ "ms": 4000 }));
    assert_eq!(prop(&app, &props, "videoTrack"), json!("auto"));
    step(app.press_key("m"));
    step(app.wait_for(STRIP));
    for _ in 0..5 {
        step(app.press_key("right"));
    }
    step(app.press_key("enter"));
    step(app.wait_for("text:/720/"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    app.settle();
    let video = prop(&app, &props, "videoTrack");
    assert!(video == json!("v1") || video == json!("v2"), "{video}");
    // Subtitles by a tap on the strip: on.
    if !seen(&app, STRIP) {
        step(app.press_key("m"));
        step(app.wait_for(STRIP));
    }
    step(app.press("text:Subtitles · Off"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    app.settle();
    assert_eq!(prop(&app, &props, "textTrack"), json!("t1"));
    // The strip's other buttons by taps, each opening its chooser or
    // screen: the audio track, the sound's delay, the picture size, the
    // sleep timer (set, then off again), the favourite, Search, the
    // channel list; Recordings does nothing yet.
    let pick_next = |app: &mut Headless, button: &str| {
        strip(app);
        step(app.press(&format!("text:{button}")));
        step(app.press_key("down"));
        step(app.press_key("enter"));
        app.settle();
    };
    pick_next(&mut app, "/^Audio · /");
    assert_eq!(prop(&app, &props, "audioTrack"), json!("a2"));
    pick_next(&mut app, "Audio delay · 0 ms");
    assert_ne!(prop(&app, &props, "audioDelay"), json!(0));
    let normal = prop(&app, &props, "pictureSize");
    pick_next(&mut app, "Picture size · Normal");
    assert_ne!(prop(&app, &props, "pictureSize"), normal);
    pick_next(&mut app, "Sleep timer · Off");
    strip(&mut app);
    step(app.press("text:/^Sleep timer · 15/"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    strip(&mut app);
    step(app.wait_for("text:Sleep timer · Off"));
    step(app.press("text:☆ Favourite"));
    step(app.wait_for("text:★ Favourite"));
    // Recordings is not available yet: nothing opens.
    step(app.press("text:Recordings"));
    assert!(!seen(&app, "text:No recent searches yet"));
    // Another stream forgets the tracks and the choices.
    // (`M` toggles the strip, which may be up without its hint.)
    step(app.press_key("m"));
    if seen(&app, STRIP) {
        step(app.press_key("m"));
    }
    step(app.wait_for_gone(STRIP, Duration::from_secs(1)));
    step(app.press_key("up"));
    step(app.wait_for("id:player-banner >> text:/Test Channel One$/"));
    app.settle();
    assert_eq!(prop(&app, &props, "videoTrack"), json!("auto"));
    assert_eq!(prop(&app, &props, "textTrack"), json!("off"));

    // --- The remote's other keys ------------------------------------------
    // Right: the most recent channel (Two); ⏮ (P) the one before (One).
    step(app.press_key("right"));
    step(app.wait_for("id:player-banner >> text:/Test Channel Two$/"));
    step(app.press_key("p"));
    step(app.wait_for("id:player-banner >> text:/Test Channel One$/"));
    // Info, play/pause, and the seek keys, which a live stream ignores.
    step(app.press_key("i"));
    step(app.wait_for(CARD));
    step(app.press_key("space"));
    step(app.wait_for("text:PAUSED"));
    step(app.press_key("space"));
    step(app.press_key("["));
    step(app.press_key("]"));
    assert!(!seen(&app, "text:/^⇢/"), "no seek in a live stream");
    // A live stream the server closed reconnects at once.
    state(&mut app, "ready");
    let before = opens();
    state(&mut app, "ended");
    run(&mut app, Duration::from_millis(100));
    assert_eq!(opens(), before + 1, "reconnected");
    state(&mut app, "ready");
    // The guide over the video (`G`) hides the status layer; again: the
    // guide alone, OK full screen again.
    network_error(&mut app);
    run(&mut app, Duration::from_millis(1500));
    step(app.wait_for("text:Reconnecting…"));
    step(app.press_key("g"));
    step(app.wait_for_gone("text:Reconnecting…", Duration::from_secs(1)));
    state(&mut app, "ready");
    step(app.press_key("g"));
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner >> text:/Test Channel One$/"));

    // The window's buttons (Android's picture in picture): pause, play,
    // next and previous channel.
    send(&mut app, "pipAction", json!({ "action": "pause" }));
    step(app.wait_for("text:PAUSED"));
    send(&mut app, "pipAction", json!({ "action": "play" }));
    step(app.wait_for_gone("text:PAUSED", Duration::from_secs(1)));
    send(&mut app, "pipAction", json!({ "action": "next" }));
    step(app.wait_for("text:/Test Channel Two$/"));
    send(&mut app, "pipAction", json!({ "action": "previous" }));
    step(app.wait_for("text:/Test Channel One$/"));
    send(&mut app, "pipAction", json!({ "action": "unknown" }));
    // The desktop's window state as the host reports it again: no change.
    send(
        &mut app,
        "pip",
        json!({ "supported": true, "tv": false, "active": false }),
    );

    // --- Catch-up ---------------------------------------------------------
    // Stop (long Back): nothing plays, the guide.
    step(app.press_key("stop"));
    step(app.wait_for("text:☰  All channels"));
    assert!(!iptv_app::player_has_source(0), "stopped");
    let before = opens();
    step(app.press_key("stop"));
    assert_eq!(opens(), before, "nothing to stop");
    // Two days back: older programmes than the guide holds (the last day)
    // are paged in from the store; one there replays; back at now they are
    // dropped again.
    step(app.press_key("mod+left"));
    step(app.press_key("mod+left"));
    step(app.wait_for("text:/^Show -[5-7][0-9]$/"));
    step(app.press_key("enter"));
    step(app.wait_for("text:CATCH-UP"));
    state(&mut app, "ready");
    step(app.press_key("stop"));
    step(app.wait_for("text:☰  All channels"));
    // Two programmes back on channel one, OK replays it.
    step(app.press_key("home"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Show -2"));
    step(app.press_key("enter"));
    step(app.wait_for("text:CATCH-UP"));
    let replay = prop(&app, &props, "source");
    assert_eq!(replay["live"], json!(false));
    state(&mut app, "ready");
    send(&mut app, "position", json!({ "ms": 60_000 }));
    // A replay that played reopens where it was after an error.
    let before = opens();
    network_error(&mut app);
    run(&mut app, Duration::from_millis(1100));
    assert_eq!(opens(), before + 1, "reopened");
    let again = prop(&app, &props, "source");
    assert_ne!(again["url"], replay["url"], "a minute on");
    // It gives up after the fast attempts: the card says how many.
    for _ in 0..6 {
        network_error(&mut app);
        run(&mut app, Duration::from_secs(5));
    }
    step(app.wait_for("text:Can't reach the provider. Check the connection."));
    step(app.wait_for("text:/· tried 5 times$/"));
    let before = opens();
    run(&mut app, Duration::from_secs(40));
    assert_eq!(opens(), before, "a replay does not retry behind its card");
    // Try again, then a seek: Right chooses five minutes on, which opens
    // once the keys pause.
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:/^Can't play/", Duration::from_secs(1)));
    state(&mut app, "ready");
    let before = opens();
    step(app.press_key("right"));
    step(app.wait_for("text:/^⇢/"));
    run(&mut app, Duration::from_secs(1));
    step(app.wait_for_gone("text:/^⇢/", Duration::from_secs(1)));
    assert_eq!(opens(), before + 1, "the archive reopened at the seek");
    // Left: five minutes back.
    step(app.press_key("left"));
    step(app.wait_for("text:/^⇢/"));
    run(&mut app, Duration::from_secs(1));
    assert_eq!(opens(), before + 2, "and back");
    // The replay ends: its card says so, and Next (⏭) returns to live.
    state(&mut app, "ready");
    state(&mut app, "ended");
    step(app.wait_for("text:The programme has ended"));
    step(app.wait_for("text:Next (⏭) returns to live."));
    step(app.press_key("n"));
    app.settle();
    assert_eq!(prop(&app, &props, "source")["live"], json!(true));
    assert_eq!(
        prop(&app, &props, "source")["url"],
        "http://stream.example.test/one.m3u8"
    );
    state(&mut app, "ready");

    // --- The card's action cards ------------------------------------------
    // OK, then Right to a card and OK: Channels (the list over the video),
    // the favourite, More controls (the strip).
    if seen(&app, CARD) {
        step(app.back());
        step(app.wait_for_gone(CARD, Duration::from_secs(1)));
    }
    let card_action = |app: &mut Headless, rights: usize| {
        step(app.press_key("enter"));
        step(app.wait_for("id:player-actions"));
        for _ in 0..rights {
            step(app.press_key("right"));
        }
        step(app.press_key("enter"));
    };
    card_action(&mut app, 3);
    step(app.back());
    card_action(&mut app, 4);
    step(app.wait_for("text:★ Favourite"));
    step(app.back());
    card_action(&mut app, 6);
    step(app.wait_for(STRIP));
    step(app.press_key("m"));
    step(app.wait_for_gone(STRIP, Duration::from_secs(1)));
    // Left: the channel list over the video; Shift + Left beside it.
    step(app.press_key("left"));
    step(app.back());
    step(app.press_key("shift+left"));
    step(app.back());

    // --- The play bar -----------------------------------------------------
    // A pointer: a tap on the video shows the bar, with what the host
    // reported of the stream (lines, class, frame rate).
    step(app.set_environment("a desktop", |e| e.idiom = Idiom::Desktop));
    send(&mut app, "size", json!({ "width": 1280, "height": 720 }));
    send(&mut app, "format", json!({ "fps": 50.0 }));
    send(&mut app, "tracks", tracks());
    step(app.press("label:Video"));
    step(app.wait_for("label:Mute"));
    step(app.wait_for("text:720p"));
    step(app.wait_for("text:/50/"));
    // Mute, then the desktop's volume along its bar.
    step(app.press("label:Mute"));
    assert_eq!(prop(&app, &props, "muted"), json!(true));
    step(app.press("label:Mute"));
    assert_eq!(prop(&app, &props, "muted"), json!(false));
    let bar = touch_area(&app, |w, h| w == 96.0 && h == 24.0);
    step(app.drag(&bar, Point::new(px(-20.0), px(0.0))));
    let volume = prop(&app, &props, "volume").as_u64().unwrap();
    assert!(volume < 100, "volume {volume}");
    // The keyboard's volume keys step it.
    step(app.press_key("ctrl+up"));
    assert!(prop(&app, &props, "volume").as_u64().unwrap() > volume);
    step(app.press_key("ctrl+down"));
    assert_eq!(prop(&app, &props, "volume").as_u64().unwrap(), volume);
    // Subtitles and tracks: its chooser, closed by a second press.
    step(app.press("label:Subtitles and tracks"));
    step(app.wait_for("text:Subtitles"));
    step(app.press("label:Subtitles and tracks"));
    // The window full screen, and back.
    step(app.press("label:Full screen"));
    step(app.press("label:Full screen"));
    // It hides by itself a few seconds after the last input.
    run(&mut app, Duration::from_secs(8));
    step(app.wait_for_gone("label:Mute", Duration::from_secs(1)));
    // Picture in picture from the bar, and back to the app.
    step(app.press("label:Video"));
    step(app.wait_for("label:Picture in picture"));
    step(app.press("label:Picture in picture"));
    step(app.wait_for("text:Playing in picture-in-picture"));
    step(app.press("id:pip-return"));
    step(app.wait_for_gone("text:Playing in picture-in-picture", Duration::from_secs(1)));
    // A failed stream's card has its Try again button for the pointer.
    send(
        &mut app,
        "error",
        json!({ "code": "ERROR_CODE_IO_BAD_HTTP_STATUS", "http": 410 }),
    );
    step(app.wait_for("text:Try again"));
    let before = opens();
    step(app.press("text:Try again"));
    step(app.wait_for_gone("text:Try again", Duration::from_secs(1)));
    assert_eq!(opens(), before + 1, "tried again");
    state(&mut app, "ready");

    // A replay under the pointer: the bar's TV guide, then two programmes
    // back; the bar's rewind and fast-forward, and its progress line under
    // a finger, seek in it; its programme details open and close.
    step(app.press("label:Video"));
    step(app.wait_for("label:TV guide"));
    step(app.press("label:TV guide"));
    step(app.wait_for_gone("label:Mute", Duration::from_secs(1)));
    step(app.press_key("home"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("enter"));
    step(app.wait_for("text:CATCH-UP"));
    state(&mut app, "ready");
    step(app.press("label:Video"));
    step(app.wait_for("label:Fast-forward"));
    let before = opens();
    step(app.press("label:Fast-forward"));
    run(&mut app, Duration::from_secs(1));
    assert_eq!(opens(), before + 1, "reopened 30 s on");
    step(app.press("label:Rewind"));
    run(&mut app, Duration::from_secs(1));
    assert_eq!(opens(), before + 2, "reopened 10 s back");
    let line = touch_area(&app, |w, h| h == 20.0 && w > 100.0);
    step(app.drag(&line, Point::new(px(60.0), px(0.0))));
    app.settle();
    assert_eq!(opens(), before + 3, "reopened where the finger lifted");
    let titles = app.screen().count("text:Show -2").unwrap();
    step(app.press("label:Programme details"));
    step(app.wait_for(&format!("text:Show -2 >> nth={titles}")));
    step(app.press("label:Programme details"));

    // The strip's channel list over the video, closed again.
    strip(&mut app);
    step(app.press("text:Channels"));
    step(app.back());

    // In picture in picture, stopping playback in the main window closes
    // the window: it has nothing to show.
    step(app.press("label:Video"));
    step(app.wait_for("label:Picture in picture"));
    step(app.press("label:Picture in picture"));
    step(app.wait_for("text:Playing in picture-in-picture"));
    step(app.press_key("stop"));
    step(app.wait_for_gone("text:Playing in picture-in-picture", Duration::from_secs(1)));
    assert!(!iptv_app::player_has_source(0));
}

const STRIP: &str = "text:/M or Esc hide/";

/// The control strip over full-screen video (Menu, `M`, when it is not up).
fn strip(app: &mut Headless) {
    if !seen(app, STRIP) {
        step(app.press_key("m"));
        // Under touch the strip may be up without its keys' hint: then
        // `M` just closed it.
        if !seen(app, STRIP) {
            step(app.press_key("m"));
        }
        step(app.wait_for(STRIP));
    }
}

/// A `tracks` event: two video tracks, two audio, one subtitle.
fn tracks() -> Value {
    json!({
        "video": [
            { "id": "v1", "label": "", "width": 1280, "height": 720, "fps": 25.0, "bitrate": 3_000_000 },
            { "id": "v2", "width": 640, "height": 360, "fps": 25.0, "bitrate": 800_000 }
        ],
        "audio": [
            { "id": "a1", "label": "Main", "language": "eng", "channels": 2, "codec": "aac" },
            { "id": "a2", "language": "fre", "channels": 6, "codec": "ac3" }
        ],
        "text": [{ "id": "t1", "language": "eng" }],
        "selected": { "video": "auto", "audio": "a1", "text": "off" }
    })
}
