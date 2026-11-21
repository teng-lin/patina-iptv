//! Gesture intent on a tablet's guide (#88), run headless on an Android
//! tablet: vertical drags scroll the channels, horizontal drags pan the
//! timeline, a diagonal drag goes the way of its larger component, a drag
//! released moving flings on, and a gesture the system cancels comes to
//! rest where it is. None of them plays, opens details or a menu, or
//! schedules a recording, whether it starts on a channel, inside a
//! programme, or on the boundary between two short programmes. Crossing
//! the drag threshold also cancels a preview `guide.autoplay` was about
//! to start, while a tap's own choice still plays.
//!
//! Synthetic data only: a generated 30-channel playlist and an XMLTV
//! guide around now with long and short programmes, picked from files.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use iptv_settings::{Settings, Value};
use nsh_core::picker::PickedFile;
use nsh_core::platform::{Idiom, Os};
use nsh_core::{
    EventKind, EventPayload, ImageData, MountItem, Point, ROOT_ID, Size, TouchEvent, TouchPhase,
    TouchPoint, ViewKind, element::Root, prelude::*,
};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(30);
const CHANNELS: usize = 30;
/// Programme lengths in minutes, by channel: long cells and short ones
/// whose boundaries sit a few dp apart.
const LENGTHS: [i64; 4] = [180, 45, 15, 30];
/// Longer than the switch delay: a preview that was going to start has.
const SETTLE: Duration = Duration::from_millis(3_000);

fn name(n: usize) -> String {
    format!("Tab Channel {n:02}")
}

/// Channel `n`'s programmes are "C<n> P<k>", `k` counting from the one on
/// air at `now` (0); channel 1 replays a day back.
fn guide(now: i64) -> String {
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for n in 1..=CHANNELS {
        xml.push_str(&format!(
            "<channel id=\"c{n}\"><display-name>{}</display-name></channel>",
            name(n)
        ));
    }
    for n in 1..=CHANNELS {
        let len = LENGTHS[(n - 1) % LENGTHS.len()] * 60;
        // Programme 0 began a third of its length ago.
        let first = now - len / 3;
        let count = (14 * 3600 / len).max(4);
        for k in -count..count {
            let start = first + k * len;
            xml.push_str(&format!(
                "<programme channel=\"c{n}\" start=\"{}\" stop=\"{}\"><title>C{n} P{k}</title></programme>",
                format_xmltv_time(start),
                format_xmltv_time(start + len),
            ));
        }
    }
    xml + "</tv>"
}

fn playlist(guide: &str) -> String {
    let mut m3u = format!("#EXTM3U url-tvg=\"{guide}\"\n");
    for n in 1..=CHANNELS {
        let catchup = if n == 1 {
            " catchup=\"shift\" catchup-days=\"1\""
        } else {
            ""
        };
        m3u.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"c{n}\" group-title=\"Generated\"{catchup},{}\nhttp://stream.example.test/{n}.m3u8\n",
            name(n)
        ));
    }
    m3u
}

#[track_caller]
fn step(result: Result<(), String>) {
    if let Err(error) = result {
        panic!("{error}");
    }
}

/// The channel names on screen, top to bottom.
fn on_screen(app: &Headless) -> Vec<usize> {
    let mut rows: Vec<(f32, usize)> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter_map(|n| {
            let t = n.text.as_ref()?;
            let k: usize = t.text.text.strip_prefix("Tab Channel ")?.parse().ok()?;
            let y = n.bounds.origin.y.0;
            (0.0..800.0).contains(&y).then_some((y, k))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, k)| k).collect()
}

/// Where the channel list is scrolled to: the top of channel `row`'s
/// name (the list mounts a few rows beyond the screen too).
fn row_y(app: &Headless, row: usize) -> f32 {
    let name = name(row);
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.text.as_ref().is_some_and(|t| t.text.text == name))
        .map(|n| n.bounds.origin.y.0)
        .unwrap_or_else(|| panic!("{name} is mounted"))
}

/// Channel one's programme numbers on screen: the timeline's place.
fn shown(app: &Headless) -> Vec<i64> {
    let mut found: Vec<i64> = app
        .screen()
        .texts()
        .into_iter()
        .filter_map(|t| t.strip_prefix("C1 P")?.parse().ok())
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// The channels the player was given, in order (a retry of the same
/// stream is not a new one).
type Tuned = Arc<Mutex<Vec<String>>>;

fn watch_tunes(app: &mut Headless) -> Tuned {
    let tuned: Tuned = Arc::default();
    let seen = tuned.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(source) = value.as_ref().map(crate::support::upstream_source)
                        && let Some(url) = source["url"].as_str()
                    {
                        let mut seen = seen.lock().unwrap();
                        if seen.last().map(String::as_str) != Some(url) {
                            seen.push(url.to_owned());
                        }
                    }
                }
            }
        }
    });
    tuned
}

/// The persistent player's width: the window's when full screen.
fn video_width_of(screen: &nsh_testing::Screen) -> f32 {
    screen
        .layout_nodes()
        .iter()
        .find(|n| n.kind == ViewKind::Native)
        .expect("the player")
        .bounds
        .size
        .width
        .0
}

/// Nothing a scroll may cause: no channel tuned but the first one's
/// autoplay preview, the guide still on screen (not full screen), no
/// details, menu or recording.
#[track_caller]
fn nothing_activated(app: &Headless, tuned: &Tuned, db: &std::path::Path, what: &str) {
    assert_eq!(
        *tuned.lock().unwrap(),
        ["http://stream.example.test/1.m3u8"],
        "{what}: nothing tuned"
    );
    assert!(
        video_width_of(app.screen()) < 1280.0,
        "{what}: the video stays in the preview, not full screen"
    );
    assert!(
        !app.screen()
            .matches("id:menu-panel >> text:Details")
            .unwrap(),
        "{what}: no menu"
    );
    assert!(
        !app.screen()
            .matches("id:description-layer >> text:/^C[0-9]+ P/")
            .unwrap(),
        "{what}: no details"
    );
    let jobs = crate::isolated::open_database(db)
        .unwrap()
        .recording_jobs()
        .unwrap();
    assert!(jobs.is_empty(), "{what}: no recording");
}

/// The system takes the finger's touches (a notification shade pulled,
/// a call): NSH's responder hears the cancel.
fn cancel_touches(app: &mut Headless) {
    let event = TouchEvent {
        phase: TouchPhase::Cancel,
        timestamp: 0.0,
        points: vec![TouchPoint {
            id: 1,
            target: ROOT_ID,
            path: Vec::new(),
            page: Point::default(),
            location: Point::default(),
            changed: true,
            pointer_type: nsh_core::PointerType::Touch,
            pressure: TouchPoint::default_pressure(TouchPhase::Cancel, true),
            tilt_x: 0.0,
            tilt_y: 0.0,
        }],
        modifiers: Default::default(),
        activated: None,
    };
    app.engine().control().dispatch_event(
        ROOT_ID,
        EventKind::Touch,
        EventPayload::Touch(Box::new(event)),
    );
    app.settle();
}

/// The tablet's guide with the generated playlist and guide imported,
/// `guide.autoplay` on: the app, the channels it tuned, and its database.
fn start(home: &crate::isolated::Home) -> (Headless, Tuned, std::path::PathBuf) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = playlist(&guide_path.display().to_string());
    let data = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&data).unwrap();
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(true)).unwrap();
    // Longer than a drag: a tap's choice still waits when a pan settles.
    settings
        .set("guide.switch_delay", Value::ms(2_000))
        .unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let db = data.join("iptv.sqlite");

    let mut config = EngineConfig {
        viewport: Size::new(px(1280.0), px(800.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = Idiom::Pad;
    config.environment.os = Os::Android;
    let mut app = Headless::start(config).unwrap();
    let tuned = watch_tunes(&mut app);
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
    step(app.wait_for_within("text:C1 P0", IMPORT));
    (app, tuned, db)
}

#[test]
fn tablet_drags_scroll_and_pan_without_playing_or_recording() {
    let Some(home) = isolated!() else { return };
    let (mut app, tuned, db) = start(&home);
    // Autoplay previews the first channel: the baseline. (The generated
    // stream never plays headless, so the player retries it meanwhile:
    // what counts is which channels were tuned.)
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "start");
    // Row 10's place measures the channels' scroll.
    const ANCHOR: usize = 10;
    let top = row_y(&app, ANCHOR);
    let at_now = shown(&app);
    assert_eq!(on_screen(&app).first(), Some(&1));
    assert!(!app.screen().matches("text:Now").unwrap(), "at now");

    // A preview waiting to start (a key moved the focus) is cancelled by
    // a horizontal drag: it pans to later programmes and nothing plays.
    step(app.press_key("down"));
    step(app.drag(
        &format!("text:{}", name(2)),
        Point::new(px(-420.0), px(8.0)),
    ));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "horizontal drag");
    step(app.wait_for("text:Now"));
    let later = shown(&app);
    assert!(
        later.iter().max() > at_now.iter().max(),
        "panned later: {at_now:?} -> {later:?}"
    );
    assert_eq!(
        row_y(&app, ANCHOR),
        top,
        "a horizontal drag scrolls no channels"
    );

    // Back at now on the same channel: the cancelled preview stays
    // cancelled (the pan never left the channel).
    step(app.press("text:Now"));
    step(app.wait_for_gone("text:Now", Duration::from_secs(3)));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "panned back to now");

    // At the list's top a drag down scrolls nothing (a platform's scroll
    // view may report no drag then), yet it crosses the threshold: the
    // preview a key started waiting for is cancelled.
    step(app.press_key("down"));
    step(app.drag(&format!("text:{}", name(2)), Point::new(px(4.0), px(300.0))));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "a drag down at the top");
    assert_eq!(row_y(&app, ANCHOR), top, "nothing to scroll above the top");
    step(app.press_key("up"));

    // Vertically: the channels scroll, the timeline stays, and the preview
    // the key started waiting for is cancelled too.
    step(app.press_key("down"));
    step(app.drag(
        &format!("text:{}", name(3)),
        Point::new(px(6.0), px(-300.0)),
    ));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "vertical drag");
    let scrolled = row_y(&app, ANCHOR);
    assert!(
        scrolled < top - 100.0,
        "the channels scrolled: {top} -> {scrolled}"
    );
    assert!(!app.screen().matches("text:Now").unwrap(), "no pan");

    // Diagonals go the way of their larger component. The first pans into
    // listings not read yet: the rows the finger scrolled stay where they
    // are when that day arrives (it once scrolled them back to the
    // focused channel).
    let rows = on_screen(&app);
    let before = row_y(&app, ANCHOR);
    step(app.drag(
        &format!("text:{}", name(rows[2])),
        Point::new(px(-260.0), px(120.0)),
    ));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "mostly horizontal diagonal");
    step(app.wait_for("text:Now"));
    assert_eq!(
        row_y(&app, ANCHOR),
        before,
        "a mostly horizontal drag scrolls nothing"
    );
    step(app.press("text:Now"));
    step(app.wait_for_gone("text:Now", Duration::from_secs(3)));
    // A steeper one, 1.5:1, still pans. (The harness models no native
    // scroll view's own slop, so this passes whatever the pan's activation
    // offset: the iPad simulator's list taking such diagonals is not
    // reproduced here.)
    step(app.drag(
        &format!("text:{}", name(rows[3])),
        Point::new(px(-300.0), px(200.0)),
    ));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "1.5:1 diagonal");
    step(app.wait_for("text:Now"));
    assert_eq!(row_y(&app, ANCHOR), before, "a 1.5:1 drag scrolls nothing");
    step(app.press("text:Now"));
    step(app.wait_for_gone("text:Now", Duration::from_secs(3)));
    step(app.drag(
        &format!("text:{}", name(rows[4])),
        Point::new(px(90.0), px(-200.0)),
    ));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "mostly vertical diagonal");
    assert!(
        row_y(&app, ANCHOR) < before - 100.0,
        "a mostly vertical drag scrolls"
    );
    assert!(
        !app.screen().matches("text:Now").unwrap(),
        "and pans nothing"
    );

    // Back to the top, by drags only.
    for _ in 0..4 {
        let row = on_screen(&app)[1];
        step(app.drag(
            &format!("text:{}", name(row)),
            Point::new(px(4.0), px(400.0)),
        ));
    }
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "scrolling back");
    assert_eq!(on_screen(&app).first(), Some(&1), "back at the top");

    // Drags that start on programmes: a long one, the boundary between
    // two adjacent fifteen-minute ones, and a past one.
    for cell in ["text:C1 P0", "text:C3 P1", "text:C3 P2", "text:C2 P-1"] {
        step(app.drag(cell, Point::new(px(-150.0), px(4.0))));
        step(app.wait(SETTLE));
        nothing_activated(&app, &tuned, &db, cell);
        step(app.press("text:Now"));
        step(app.wait_for_gone("text:Now", Duration::from_secs(3)));
    }
    step(app.drag("text:C1 P0", Point::new(px(600.0), px(0.0))));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "drag onto replayable listings");
    assert!(
        shown(&app).iter().any(|&k| k < 0),
        "dragged back to the past: {:?}",
        shown(&app)
    );
    step(app.press("text:Now"));
    step(app.wait_for_gone("text:Now", Duration::from_secs(3)));

    // A fling (released at full speed) carries on past the finger.
    step(app.drag("text:C1 P0", Point::new(px(-200.0), px(0.0))));
    let lifted = shown(&app);
    step(app.wait(SETTLE));
    let rested = shown(&app);
    nothing_activated(&app, &tuned, &db, "fling");
    assert!(
        rested.iter().max() >= lifted.iter().max(),
        "the fling carried on: {lifted:?} -> {rested:?}"
    );
    step(app.press("text:Now"));
    step(app.wait_for_gone("text:Now", Duration::from_secs(3)));

    // A gesture the system cancels mid-drag rests where it is: no fling,
    // no tap, no tune.
    step(app.drag_and_hold("text:C1 P0", Point::new(px(-400.0), px(0.0))));
    cancel_touches(&mut app);
    let held = shown(&app);
    step(app.drop_held());
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "cancelled drag");
    assert_eq!(shown(&app), held, "a cancelled drag does not fling");
    step(app.press("text:Now"));
    step(app.wait_for_gone("text:Now", Duration::from_secs(3)));

    // Keys move the focus off where a pan left it and back: the pan's
    // hold on the channel ended, so coming back to that later programme
    // starts nothing (only a focus on a programme on air would).
    step(app.press_key("down"));
    step(app.drag(
        &format!("text:{}", name(2)),
        Point::new(px(-420.0), px(8.0)),
    ));
    step(app.wait(SETTLE));
    step(app.press_key("right"));
    step(app.press_key("left"));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "keys after a pan");
    step(app.press("text:Now"));
    step(app.wait_for_gone("text:Now", Duration::from_secs(3)));

    // Under the threshold a drag is still a tap, an 8 dp sideways wobble
    // included: it chooses the live programme. A pan right after it,
    // before the switch delay, keeps that choice: it plays full screen.
    step(app.drag(&format!("text:{}", name(5)), Point::new(px(8.0), px(1.0))));
    step(app.drag(
        "id:guide-timeline >> text:C5 P0",
        Point::new(px(-420.0), px(4.0)),
    ));
    let chosen = tuned.clone();
    step(
        app.wait_until("the tap tuned", Duration::from_secs(5), move |_| {
            chosen.lock().unwrap().last().map(String::as_str)
                == Some("http://stream.example.test/5.m3u8")
        }),
    );
    step(app.wait_until("full screen", Duration::from_secs(3), |s| {
        video_width_of(s) >= 1280.0
    }));
    let tunes = tuned.lock().unwrap().len();

    // The player's channel list: a key starts a preview of the next
    // channel, and a drag on the list cancels it.
    if bounds_of(&app, "play-bar-channels").is_none() {
        step(app.press("label:Video"));
    }
    step(app.press("id:play-bar-channels"));
    step(app.wait_for("id:channel-list >> text:/Tab Channel 07$/"));
    step(app.press_key("down"));
    step(app.drag(
        "id:channel-list >> text:/Tab Channel 07$/",
        Point::new(px(4.0), px(-300.0)),
    ));
    step(app.wait(SETTLE));
    // One lock for the value and its message (two would deadlock on a
    // failure).
    let after = tuned.lock().unwrap().clone();
    assert_eq!(
        after.len(),
        tunes,
        "a drag on the channel list tunes nothing: {after:?}"
    );
    let jobs = crate::isolated::open_database(&db)
        .unwrap()
        .recording_jobs()
        .unwrap();
    assert!(jobs.is_empty());
}

fn bounds_of(app: &Headless, test_id: &str) -> Option<f32> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| !n.hidden && n.test_id.as_deref() == Some(test_id))
        .map(|n| n.bounds.size.width.0)
}

/// #156: a long press on another channel, or on one of its programmes,
/// focuses it and opens its menu. Dismissed (Back), or left for the
/// programme's details, it tunes nothing until a key moves the focus,
/// which autoplays as before. A menu action that plays still plays.
#[test]
fn tablet_long_press_menus_tune_nothing_until_the_focus_moves() {
    let Some(home) = isolated!() else { return };
    let (mut app, tuned, db) = start(&home);
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "start");

    let menu = "id:menu-panel >> text:Details";
    step(app.long_press(&format!("id:guide-ui >> label:{}", name(4))));
    step(app.wait_for(menu));
    step(app.back());
    step(app.wait_for_gone(menu, Duration::from_secs(3)));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "a channel's menu dismissed");

    step(app.long_press("text:C5 P0"));
    step(app.wait_for(menu));
    step(app.back());
    step(app.wait_for_gone(menu, Duration::from_secs(3)));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "a programme's menu dismissed");

    // Its details, from the menu, then Back: the same.
    step(app.long_press("text:C7 P0"));
    step(app.wait_for(menu));
    step(app.press(menu));
    let details = "id:description-layer >> text:C7 P0";
    step(app.wait_for(details));
    step(app.back());
    step(app.wait_for_gone(details, Duration::from_secs(3)));
    step(app.wait(SETTLE));
    nothing_activated(&app, &tuned, &db, "a programme's details closed");

    // A key moves the focus on: autoplay previews that channel.
    step(app.press_key("down"));
    let seen = tuned.clone();
    step(
        app.wait_until("autoplay resumes", Duration::from_secs(5), move |_| {
            seen.lock().unwrap().last().map(String::as_str)
                == Some("http://stream.example.test/8.m3u8")
        }),
    );

    // Channel 1 replays: its menu's Play from the beginning plays.
    let before = tuned.lock().unwrap().len();
    step(app.long_press("text:C1 P0"));
    step(app.wait_for("id:menu-panel >> text:Play from the beginning"));
    step(app.press("id:menu-panel >> text:Play from the beginning"));
    let seen = tuned.clone();
    step(
        app.wait_until("the replay plays", Duration::from_secs(5), move |_| {
            let seen = seen.lock().unwrap();
            seen.len() > before
                && seen
                    .last()
                    .is_some_and(|url| url.starts_with("http://stream.example.test/1.m3u8"))
        }),
    );
}
