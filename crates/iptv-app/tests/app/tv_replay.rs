//! A replay's keys on a TV, run headless with the TV idiom: the card
//! shows the replay's seek bar as its one time line (no second bar, no
//! action cards), Left/Right (`catchup.seek_keys`' default) choose a seek
//! shown on it, OK opens the archive there at once, and the next OK opens
//! the shared play bar with Play/pause focused.
//!
//! Synthetic data only: a playlist picked from a file, channel one with a
//! day of catch-up, its guide a local XMLTV file.

use nsh_core::MountItem;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);
/// Programs of 45 minutes, "Show <n>" counting from the one on now (0).
const LEN: i64 = 45 * 60;
const CARD: &str = "id:player-card";
const LINE: &str = "id:player-time-line";

fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv><channel id=\"one\"><display-name>One</display-name></channel>",
    );
    let first = now - 86_400 + 20 * 60;
    let live = (now - first) / LEN;
    let mut k = 0;
    while first + k * LEN < now + 6 * 3_600 {
        let (start, stop) = (first + k * LEN, first + (k + 1) * LEN);
        xml.push_str(&format!(
            "<programme channel=\"one\" start=\"{}\" stop=\"{}\"><title>Show {}</title></programme>",
            format_xmltv_time(start),
            format_xmltv_time(stop),
            k - live
        ));
        k += 1;
    }
    xml + "</tv>"
}

#[test]
fn a_replays_card_has_its_seek_bar_and_ok_seeks_then_opens_the_play_bar() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"1\",Test Channel One\nhttp://stream.example.test/one.m3u8\n",
        guide_path.display()
    );
    let config = EngineConfig {
        clock: nsh_core::time::Clock::manual(SystemTime::now()),
        viewport: Size::new(px(960.0), px(540.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    let lifetime: Arc<Mutex<Vec<MountItem>>> = Arc::default();
    let observed = lifetime.clone();
    app.observe(move |_, batch| observed.lock().unwrap().extend(batch.items.iter().cloned()));
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));

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
    step(app.wait_for_within("text:Show 0", IMPORT));
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));

    // The compact context keeps live replay reachable using only D-pad
    // keys. Inspecting the current program first opens no stream.
    let pick_menu = |app: &mut Headless, label: &str| {
        let selector = format!("id:menu-panel >> role:menuitem[{label}]");
        for _ in 0..40 {
            if app
                .screen()
                .accessibility_state(&selector)
                .is_ok_and(|state| state.selected)
            {
                step(app.press_key("enter"));
                return;
            }
            step(app.press_key("down"));
        }
        panic!("the D-pad did not reach {label}");
    };
    step(app.press_key("m"));
    pick_menu(&mut app, "More options");
    pick_menu(&mut app, "Details");
    step(app.wait_for("id:description-layer >> text:/Show 0/"));
    assert_eq!(iptv_app::player_opens(0), 0);
    step(app.back());
    step(app.press_key("m"));
    pick_menu(&mut app, "More options");
    pick_menu(&mut app, "Play from the beginning");
    step(app.wait_for("text:CATCH-UP"));
    let expected_start = ((now - 25 * 60).div_euclid(60) * 60).to_string();
    step(app.wait_until(
        "the native player opens the live program's archive",
        Duration::from_secs(5),
        |_| {
            let archive = lifetime.lock().unwrap().iter().rev().find_map(|item| {
                let MountItem::ComponentProps { props, .. } = item else {
                    return None;
                };
                props.iter().find_map(|(name, value)| {
                    let source = value.as_ref()?;
                    (name.as_ref() == "source" && source["live"] == false)
                        .then(|| crate::support::upstream_source(source))
                })
            });
            iptv_app::player_opens(0) == 1
                && archive.is_some_and(|source| {
                    source["url"].as_str().is_some_and(|url| {
                        url.split_once('?').is_some_and(|(_, query)| {
                            query.split('&').any(|pair| {
                                pair.strip_prefix("utc=") == Some(expected_start.as_str())
                            })
                        })
                    })
                })
        },
    ));
    step(app.press_key("g"));
    step(app.wait_for("id:guide-ui >> role:cell[Show 0]"));

    // Two programs back, OK replays it full screen: the card alone, its
    // time line the seek bar (position and length), no action cards.
    step(app.press_key("home"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Show -2"));
    step(app.press_key("enter"));
    step(app.wait_for(CARD));
    step(app.wait_for("text:CATCH-UP"));
    assert_eq!(app.screen().count(LINE).unwrap(), 1, "one time line");
    assert!(!app.screen().matches("id:player-actions").unwrap());
    step(app.wait_for(&format!("{LINE} >> text:45:00")));
    step(app.wait_for("text:/◀ ▶ seek/"));

    // Right chooses a seek a time-bar step on (5 min), shown on the bar;
    // OK opens the archive there at once.
    step(app.press_key("right"));
    step(app.wait_for(&format!("{LINE} >> text:⇢ 5:00")));
    // Native named-key repeats still seek: the one-shot action policy
    // must not turn a held seek arrow into a single step.
    let keys = lifetime
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|item| match item {
            MountItem::Keymap(keys) => Some(keys.clone()),
            _ => None,
        })
        .expect("mounted keymap");
    assert!(
        app.repeat_key_binding("right", &keys)
            .unwrap()
            .fired()
            .is_some()
    );
    step(app.wait_for(&format!("{LINE} >> text:⇢ 10:00")));
    step(app.press_key("left"));
    step(app.wait_for(&format!("{LINE} >> text:⇢ 5:00")));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:/^⇢/", Duration::from_secs(2)));
    step(app.wait_for(&format!("{LINE} >> text:5:00")));

    // The next OK opens the bar. Further presses activate Play/pause.
    step(app.press_key("enter"));
    step(app.wait_for("id:play-bar-layer"));
    step(app.wait_for("text:CATCH-UP"));
    step(app.press_key("enter"));
    step(app.wait_for("text:PAUSED"));
    step(app.press_key("enter"));
    step(app.wait_for("text:CATCH-UP"));
    assert!(!app.screen().matches(CARD).unwrap());

    // The same bar serves seekable TV playback. OK commits before the
    // debounce clock advances and never also pauses the player.
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some("video-player"))
        .unwrap()
        .id;
    step(app.press_key("down"));
    assert!(
        app.screen()
            .accessibility_state("id:play-bar-seek")
            .unwrap()
            .selected
    );
    let opens = iptv_app::player_opens(0);
    step(app.press_key("right"));
    step(app.wait_for("id:play-bar-time-line >> text:10:00"));
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "selection has not committed before OK"
    );
    assert!(
        !app.screen().matches(CARD).unwrap(),
        "the old card stays hidden behind the shared bar"
    );
    let clock_before_ok = app.clock().now();
    step(app.press_key("enter"));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while iptv_app::player_opens(0) == opens && std::time::Instant::now() < deadline {
        app.settle(); // flush native workers without advancing the manual clock
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        app.clock().now(),
        clock_before_ok,
        "OK committed with the debounce clock frozen"
    );
    assert_eq!(
        iptv_app::player_opens(0),
        opens + 1,
        "one explicit archive seek"
    );
    assert!(
        app.screen()
            .accessibility_state("id:play-bar-play-pause")
            .unwrap()
            .selected
    );
    assert!(!app.screen().matches(CARD).unwrap());
    step(app.run_for(Duration::from_secs(2)));
    assert_eq!(
        iptv_app::player_opens(0),
        opens + 1,
        "the canceled seek timer does not reopen again"
    );

    // Track selection by the remote on the same bar: the host reports a
    // subtitle track; Left reaches Tracks, OK opens its chooser, Down and
    // OK pick the track. The choice goes to the same player as a property:
    // no stream opens, and focus returns to Tracks.
    app.engine().control().dispatch_event(
        player,
        nsh_core::EventKind::NativeEvent,
        nsh_core::EventPayload::Native {
            name: "tracks".into(),
            data: serde_json::json!({
                "video": [],
                "audio": [{ "id": "a1", "language": "eng", "channels": 2 }],
                "text": [{ "id": "t1", "language": "eng" }],
                "selected": { "audio": "a1", "text": "off" }
            }),
        },
    );
    app.settle();
    for _ in 0..12 {
        if app
            .screen()
            .accessibility_state("id:play-bar-tracks")
            .is_ok_and(|s| s.selected)
        {
            break;
        }
        step(app.press_key("left"));
    }
    assert!(
        app.screen()
            .accessibility_state("id:play-bar-tracks")
            .unwrap()
            .selected,
        "Left reaches Tracks"
    );
    step(app.press_key("enter"));
    step(app.wait_for("id:chooser-layer >> text:/English$/"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for_gone(
        "id:chooser-layer >> text:/English$/",
        Duration::from_secs(2),
    ));
    let text_track = lifetime
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|item| match item {
            MountItem::ComponentProps { id, props } if *id == player => props
                .iter()
                .find(|(name, _)| name.as_ref() == "textTrack")
                .and_then(|(_, value)| value.clone()),
            _ => None,
        });
    assert_eq!(
        text_track,
        Some(serde_json::json!("t1")),
        "the remote picked t1"
    );
    assert!(
        app.screen()
            .accessibility_state("id:play-bar-tracks")
            .unwrap()
            .selected,
        "the chooser returns focus to Tracks"
    );
    assert_eq!(
        iptv_app::player_opens(0),
        opens + 1,
        "a track choice opens no stream"
    );

    let history = lifetime.lock().unwrap();
    let components: Vec<_> = history
        .iter()
        .filter_map(|item| match item {
            MountItem::Component { id, name } if name.as_ref() == "tv.player" => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(components, [player]);
    assert!(!history.iter().any(
        |item| matches!(item, MountItem::Remove { id } | MountItem::Delete { id } if *id == player)
    ));
    assert_eq!(
        history
            .iter()
            .filter(|item| matches!(item, MountItem::Insert { child, .. } if *child == player))
            .count(),
        1
    );
    let paused = history.iter().rev().find_map(|item| match item {
        MountItem::ComponentProps { id, props } if *id == player => {
            props.iter().find_map(|(name, value)| {
                if name.as_ref() == "paused" {
                    value.as_ref().and_then(serde_json::Value::as_bool)
                } else {
                    None
                }
            })
        }
        _ => None,
    });
    assert_eq!(paused, Some(false), "timeline OK never also pauses");
}
