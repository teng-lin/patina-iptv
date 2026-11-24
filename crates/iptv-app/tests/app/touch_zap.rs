//! Changing channel by touch over full-screen live video (#285), run
//! headless on an Android tablet: a swipe up tunes the channel Ch+ tunes
//! and a swipe down the one Ch− tunes, the banner of the channel it goes
//! to following the finger, then naming it at once on release while the
//! stream waits for `guide.switch_delay`. A burst of swipes opens only
//! the last channel's stream. A swipe brought back, one that starts at a
//! screen edge, one over a replay and one over multiview change nothing.
//! With Swipe to change channel off, a swipe does nothing, yet the play
//! bar's channel buttons and the video's accessibility actions still
//! zap, and the banner shows with `player.info_on_switch` off.
//!
//! Synthetic data only: a generated five-channel playlist and XMLTV guide
//! picked from files.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use crate::personalization::step;
use iptv_epg::format_xmltv_time;
use iptv_settings::{Settings, Value};
use nsh_core::picker::PickedFile;
use nsh_core::platform::{Idiom, Os};
use nsh_core::{
    EventKind, EventPayload, ImageData, MountItem, NodeId, Point, ROOT_ID, Size, TouchEvent,
    TouchPhase, TouchPoint, ViewKind, element::Root, prelude::*,
};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(30);
const WAIT: Duration = Duration::from_secs(5);
const SCREEN: (f32, f32) = (1280.0, 800.0);
/// `guide.switch_delay`: long enough to swipe several times meanwhile.
const DELAY: Duration = Duration::from_millis(1_500);
const CHANNELS: usize = 5;
const SLOT: i64 = 3600;

fn name(n: usize) -> String {
    format!("Zap Channel {n}")
}

fn url(n: usize) -> String {
    format!("http://stream.example.test/zap{n}.m3u8")
}

/// Channel `n` airs "Z<n> Now" now; channel one also "Z1 Past" two hours
/// back, in its archive.
fn guide(now: i64) -> String {
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for n in 1..=CHANNELS {
        xml.push_str(&format!(
            "<channel id=\"z{n}\"><display-name>{}</display-name></channel>",
            name(n)
        ));
    }
    for n in 1..=CHANNELS {
        for k in -20i64..20 {
            let start = now - 25 * 60 + k * SLOT;
            let title = match k {
                0 => format!("Z{n} Now"),
                -2 => format!("Z{n} Past"),
                _ => format!("Z{n} Show {k}"),
            };
            xml.push_str(&format!(
                "<programme channel=\"z{n}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
                format_xmltv_time(start),
                format_xmltv_time(start + SLOT),
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
            "#EXTINF:-1 tvg-id=\"z{n}\" group-title=\"Generated\"{catchup},{}\n{}\n",
            name(n),
            url(n)
        ));
    }
    m3u
}

/// The streams the player was given, in order (a retry of the same
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

fn streams(t: &Tuned) -> Vec<String> {
    t.lock().unwrap().clone()
}

/// The tablet with the generated playlist imported, the switch delay at
/// [`DELAY`] and `extra` settings.
fn start(home: &crate::isolated::Home, extra: &[(&str, Value)]) -> (Headless, Tuned) {
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
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings
        .set("guide.switch_delay", Value::ms(DELAY.as_millis() as i64))
        .unwrap();
    for (key, value) in extra {
        settings.set(key, value.clone()).unwrap();
    }
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();

    let mut config = EngineConfig {
        viewport: Size::new(px(SCREEN.0), px(SCREEN.1)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = Idiom::Pad;
    config.environment.os = Os::Android;
    // The switch delay runs on the app's clock: the test moves it.
    config.clock = nsh_core::time::Clock::manual(SystemTime::now());
    let mut app = Headless::start(config).unwrap();
    let tuned = watch_tunes(&mut app);
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    step(app.wait_for("id:wizard-welcome"));
    step(app.press("id:wizard-welcome-add"));
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Z1 Now", IMPORT));
    (app, tuned)
}

/// Channel `n` plays full screen, tuned by two taps on its row.
fn full_screen(app: &mut Headless, tuned: &Tuned, n: usize) {
    let row = format!("text:{}", name(n));
    step(app.press(&row));
    tunes(app, tuned, n, "the channel plays in the preview");
    step(app.press(&row));
    wide(app);
}

/// The video fills the screen.
fn wide(app: &mut Headless) {
    step(app.run_for(Duration::from_millis(200)));
    app.wait_until_fixed_clock("full screen", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|node| node.kind == ViewKind::Native && node.bounds.size.width.0 >= SCREEN.0)
    });
}

/// The switch delay passes: channel `n`'s stream opens (authorized on a
/// worker, while the clock stands still).
#[track_caller]
fn tunes(app: &mut Headless, tuned: &Tuned, n: usize, what: &str) {
    step(app.run_for(DELAY + Duration::from_millis(100)));
    let want = url(n);
    app.wait_until_fixed_clock(what, WAIT, |_| tuned.lock().unwrap().last() == Some(&want));
}

/// The switch delay passes, and the workers have had time to open a
/// stream: whatever was going to tune has.
fn quiet(app: &mut Headless) {
    step(app.run_for(DELAY + Duration::from_millis(100)));
    std::thread::sleep(Duration::from_millis(300));
    app.catch_up();
}

/// The full-screen tap catcher and the views around it, innermost first:
/// the path a platform reports a finger with.
fn video_path(app: &Headless) -> (NodeId, Vec<NodeId>) {
    let screen = app.screen();
    let video = screen.id("id:video").expect("the video's tap catcher");
    let nodes = screen.layout_nodes();
    let parent: std::collections::HashMap<_, _> = nodes.iter().map(|n| (n.id, n.parent)).collect();
    let mut path = vec![video];
    let mut at = video;
    while let Some(&up) = parent.get(&at) {
        if up == at || path.contains(&up) {
            break;
        }
        path.push(up);
        at = up;
    }
    if !path.contains(&ROOT_ID) {
        path.push(ROOT_ID);
    }
    (video, path)
}

fn touch(
    app: &mut Headless,
    phase: TouchPhase,
    target: NodeId,
    path: &[NodeId],
    at: (f32, f32),
    t: f64,
) {
    let event = TouchEvent {
        phase,
        timestamp: t,
        points: vec![TouchPoint {
            id: 1,
            target,
            path: if phase == TouchPhase::Start {
                path.to_vec()
            } else {
                Vec::new()
            },
            page: Point::new(px(at.0), px(at.1)),
            location: Point::new(px(at.0), px(at.1)),
            changed: true,
            pointer_type: nsh_core::PointerType::Touch,
            pressure: TouchPoint::default_pressure(phase, true),
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
}

/// The finger lifts inside the full-screen video, which Android's press
/// then reports as a tap whatever the finger did (its press ends early
/// only when the finger leaves the view): a swipe must not be one.
fn lift_press(app: &mut Headless) {
    // Android's host reports a finger's press with at least one click.
    press_video(app, 1);
}

/// The video's press: `clicks` 0 is a keyboard's or a screen reader's.
fn press_video(app: &mut Headless, clicks: u32) {
    let video = app.screen().id("id:video").unwrap();
    app.engine().control().dispatch_event(
        video,
        EventKind::Press,
        EventPayload::Press(nsh_core::PressEvent {
            clicks,
            ..Default::default()
        }),
    );
}

/// A finger on the video goes down at `from`, moves through `through`, 10
/// dp each 16 ms, and lifts at the last point, its press reported as
/// Android does ([`lift_press`]); the app's clock does not move
/// meanwhile. `hold`: it stays down at the last point.
fn finger(app: &mut Headless, from: (f32, f32), through: &[(f32, f32)], hold: bool) {
    let (target, path) = video_path(app);
    let mut t = 1_000.0;
    touch(app, TouchPhase::Start, target, &path, from, t);
    let mut at = from;
    for &to in through {
        let steps = ((to.0 - at.0).abs().max((to.1 - at.1).abs()) / 10.0)
            .ceil()
            .max(1.0) as u32;
        for i in 1..=steps {
            let f = i as f32 / steps as f32;
            t += 16.0;
            let p = (at.0 + (to.0 - at.0) * f, at.1 + (to.1 - at.1) * f);
            touch(app, TouchPhase::Move, target, &path, p, t);
        }
        at = to;
    }
    if !hold {
        touch(app, TouchPhase::End, target, &path, at, t);
        lift_press(app);
    }
    app.settle();
}

/// A swipe from the middle of the screen, `dy` dp (negative: up).
fn swipe(app: &mut Headless, dy: f32) {
    let from = (SCREEN.0 / 2.0, SCREEN.1 / 2.0);
    finger(app, from, &[(from.0, from.1 + dy)], false);
}

fn banner_names(app: &Headless, n: usize) -> bool {
    app.screen()
        .matches(&format!("id:player-banner >> text:/{}$/", name(n)))
        .unwrap()
}

fn act(app: &mut Headless, action: &str) {
    let video = app.screen().id("id:video").unwrap();
    app.engine().control().dispatch_event(
        video,
        EventKind::AccessibilityAction,
        EventPayload::Map(vec![(
            "actionName",
            nsh_core::PayloadValue::Str(action.into()),
        )]),
    );
    app.settle();
}

/// The top of the swipe's banner.
fn preview_top(app: &Headless) -> f32 {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|n| n.test_id.as_deref() == Some("swipe-preview"))
        .expect("the swipe's banner")
        .bounds
        .origin
        .y
        .0
}

#[test]
fn swipes_change_channel_as_ch_plus_and_minus_and_only_over_live_video() {
    let Some(home) = isolated!() else { return };
    let (mut app, tuned) = start(&home, &[]);
    full_screen(&mut app, &tuned, 3);
    let opened = streams(&tuned).len();

    // Up past the threshold, held: the banner of the channel Ch+ tunes
    // (the row above) follows the finger.
    let from = (SCREEN.0 / 2.0, 600.0);
    finger(&mut app, from, &[(from.0, 560.0)], true);
    assert!(
        app.screen()
            .matches(&format!("id:swipe-preview >> text:/{}$/", name(2)))
            .unwrap(),
        "the banner of channel two"
    );
    assert!(
        app.screen()
            .matches("id:swipe-preview >> text:Z2 Now")
            .unwrap()
    );
    let low = preview_top(&app);
    // On up, and released past a quarter of the height: the banner names
    // channel two at once, and its stream waits for the switch delay.
    let (target, path) = video_path(&app);
    let mut t = 2_000.0;
    for y in (300..560).rev().step_by(10) {
        t += 16.0;
        touch(
            &mut app,
            TouchPhase::Move,
            target,
            &path,
            (from.0, y as f32),
            t,
        );
    }
    app.settle();
    let high = preview_top(&app);
    assert!(
        high < low - 150.0,
        "the banner follows the finger: {low} -> {high}"
    );
    touch(&mut app, TouchPhase::End, target, &path, (from.0, 300.0), t);
    lift_press(&mut app);
    app.settle();
    assert!(
        !app.screen().matches("id:play-bar-layer").unwrap(),
        "a swipe is no tap: the play bar stays hidden"
    );
    assert!(!app.screen().matches("id:swipe-preview").unwrap());
    assert!(
        banner_names(&app, 2),
        "the banner names channel two at once"
    );
    assert_eq!(
        streams(&tuned).len(),
        opened,
        "nothing tunes before the delay"
    );
    tunes(&mut app, &tuned, 2, "swipe up: Ch+");

    // Down: Ch−, back to three.
    swipe(&mut app, 300.0);
    assert!(banner_names(&app, 3));
    tunes(&mut app, &tuned, 3, "swipe down: Ch−");

    // A lift that is the swipe's first report past the threshold, no move
    // between, swiped too: its press is no tap, which would drop the zap.
    let (target, path) = video_path(&app);
    touch(
        &mut app,
        TouchPhase::Start,
        target,
        &path,
        (640.0, 600.0),
        5_000.0,
    );
    touch(
        &mut app,
        TouchPhase::End,
        target,
        &path,
        (640.0, 300.0),
        5_050.0,
    );
    lift_press(&mut app);
    app.settle();
    assert!(!app.screen().matches("id:play-bar-layer").unwrap());
    assert!(banner_names(&app, 2), "the lift alone swiped up");
    tunes(&mut app, &tuned, 2, "a swipe reported only at its lift");
    swipe(&mut app, 300.0);
    tunes(&mut app, &tuned, 3, "swipe down again");

    // A platform that reports no press after a swipe (iOS's ends with the
    // finger's movement): a keyboard's or a screen reader's press after it
    // is still a tap.
    let from = (SCREEN.0 / 2.0, 600.0);
    finger(&mut app, from, &[(from.0, 560.0)], true);
    let (target, path) = video_path(&app);
    touch(
        &mut app,
        TouchPhase::End,
        target,
        &path,
        (from.0, 560.0),
        2_000.0,
    );
    app.settle();
    press_video(&mut app, 0);
    app.settle();
    assert!(
        app.screen().matches("id:play-bar-layer").unwrap(),
        "a screen reader's activation after a swipe shows the play bar"
    );
    press_video(&mut app, 0);
    app.settle();
    assert!(!app.screen().matches("id:play-bar-layer").unwrap());

    // A swipe brought back, one from the bottom edge, one from the top
    // edge and a horizontal drag change nothing.
    let opened = streams(&tuned).len();
    let mid = (SCREEN.0 / 2.0, 400.0);
    finger(&mut app, mid, &[(mid.0, 100.0), (mid.0, 395.0)], false);
    // (Each of these three is a tap for Android's press: two of them hide
    // the play bar again.)
    finger(&mut app, (mid.0, 790.0), &[(mid.0, 300.0)], false);
    finger(&mut app, (mid.0, 12.0), &[(mid.0, 500.0)], false);
    finger(&mut app, mid, &[(mid.0 + 400.0, 450.0)], false);
    finger(&mut app, mid, &[(mid.0 + 2.0, 400.0)], false);
    assert!(!app.screen().matches("id:swipe-preview").unwrap());
    assert!(!banner_names(&app, 2) && !banner_names(&app, 4));
    quiet(&mut app);
    assert_eq!(streams(&tuned).len(), opened, "none of them tuned");

    // A burst of swipes up: only the last channel's stream opens, after
    // the delay. Three up from three wraps to five.
    for _ in 0..3 {
        swipe(&mut app, -300.0);
    }
    assert!(
        banner_names(&app, 5),
        "the banner follows the burst: {:?}",
        app.screen().texts()
    );
    assert_eq!(streams(&tuned).len(), opened, "the burst opens nothing yet");
    tunes(&mut app, &tuned, 5, "the burst's last channel");
    quiet(&mut app);
    let after: Vec<_> = streams(&tuned).into_iter().skip(opened).collect();
    assert_eq!(after, [url(5)], "only the burst's last channel opens");

    // Multiview: a swipe on the video under it changes nothing.
    step(app.press("id:video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    step(app.wait_for("id:multiview-picker"));
    let opened = streams(&tuned).len();
    swipe(&mut app, -300.0);
    swipe(&mut app, 300.0);
    assert!(!app.screen().matches("id:swipe-preview").unwrap());
    quiet(&mut app);
    assert_eq!(
        streams(&tuned).len(),
        opened,
        "no swipe zapping in multiview"
    );
}

#[test]
fn a_swipe_over_a_replay_keeps_its_place() {
    let Some(home) = isolated!() else { return };
    let (mut app, tuned) = start(&home, &[]);
    // Channel one's past program, browsed to by a timeline drag and
    // replayed full screen by a tap.
    step(app.drag_and_hold("id:guide-row >> nth=0", Point::new(px(600.0), px(0.0))));
    step(app.wait_for("text:Z1 Past"));
    step(app.drop_held());
    step(app.press("text:Z1 Past"));
    wide(&mut app);
    quiet(&mut app);
    let replay = streams(&tuned);
    assert!(
        replay.last().is_some_and(|u| u.contains("zap1")),
        "channel one's replay: {replay:?}"
    );
    swipe(&mut app, -300.0);
    swipe(&mut app, 300.0);
    assert!(!app.screen().matches("id:swipe-preview").unwrap());
    assert!(!app.screen().matches("id:player-banner").unwrap());
    quiet(&mut app);
    assert_eq!(streams(&tuned), replay, "a replay takes no swipe");
}

#[test]
fn with_swiping_off_the_buttons_and_accessibility_actions_still_zap() {
    let Some(home) = isolated!() else { return };
    let (mut app, tuned) = start(
        &home,
        &[
            ("playback.swipe_to_zap", Value::Bool(false)),
            ("player.info_on_switch", Value::Bool(false)),
        ],
    );
    full_screen(&mut app, &tuned, 3);
    let opened = streams(&tuned).len();
    swipe(&mut app, -300.0);
    assert!(!app.screen().matches("id:swipe-preview").unwrap());
    quiet(&mut app);
    assert_eq!(streams(&tuned).len(), opened, "the swipe is off");

    // The play bar's next button, beside the channel's name: the bar's
    // line names channel two at once.
    if !app.screen().matches("id:play-bar-next-channel").unwrap() {
        step(app.press("id:video"));
    }
    step(app.wait_for("id:play-bar-next-channel"));
    step(app.press("id:play-bar-next-channel"));
    assert!(
        app.screen()
            .matches(&format!("id:play-bar-layer >> text:/{}$/", name(2)))
            .unwrap(),
        "the bar names channel two at once"
    );
    assert_eq!(streams(&tuned).len(), opened, "it waits for the delay");
    tunes(&mut app, &tuned, 2, "next: Ch+");
    step(app.press("id:play-bar-previous-channel"));
    tunes(&mut app, &tuned, 3, "previous: Ch−");

    // Over a chooser the bar stays usable: its next button closes the
    // chooser (about the channel it leaves), and the zap tunes.
    step(app.play_bar_option("Display"));
    step(app.wait_for("id:chooser-panel >> text:Picture size"));
    step(app.press("id:play-bar-next-channel"));
    step(app.wait_for_gone("id:chooser-panel >> text:Picture size", WAIT));
    tunes(&mut app, &tuned, 2, "next over a chooser");
    step(app.press("id:play-bar-previous-channel"));
    tunes(&mut app, &tuned, 3, "previous again");

    // A screen reader's actions on the video, with the bar hidden: the
    // banner shows although `player.info_on_switch` is off.
    step(app.run_for(Duration::from_secs(15)));
    assert!(!app.screen().matches("id:play-bar-next-channel").unwrap());
    act(&mut app, "next_channel");
    assert!(banner_names(&app, 2), "a touch's zap shows its banner");
    tunes(&mut app, &tuned, 2, "the next channel action");
    assert!(banner_names(&app, 2), "and again as it tunes");
    act(&mut app, "previous_channel");
    tunes(&mut app, &tuned, 3, "the previous channel action");
}
