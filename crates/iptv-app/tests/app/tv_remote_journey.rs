//! The TV remote's watching journey (#87), driven by a remote's raw key
//! phases as Android reports them (`iptv_app::use_remote_key_phases`): a
//! key down, its auto-repeats and its release, with the app telling a tap
//! from a hold on its own clock.
//!
//! What it checks: long OK opens the controls once and its repeats and
//! release do nothing more; rapid taps each act once; a press held while
//! the surface changes (a tap plays a channel) does nothing on release or
//! by timer; a held OK keeps the info card up and presses the card it was
//! held on. The journey: groups, choose a channel (full screen), the
//! controls, browse another channel without retuning, zap and the
//! previous channel, back to the guide and its mini-player, all on one
//! persistent `tv.player` with counted stream opens.
//!
//! Synthetic data only: the personalization fixture's three local channels.

use crate::headless::KeyPhase;
use crate::personalization::{self, Fixture, step};
use crate::tv_shared_controls::{Lifetime, persistent, record_players, selected};
use iptv_settings::{Settings, Value};
use nsh_core::platform::{Idiom, Os};
use nsh_core::{NodeId, Size, ViewKind, prelude::px};
use nsh_tv::RemoteKey;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The app's log lines, printed and kept: what a dropped press leaves.
struct Lines(Mutex<Vec<String>>);

static LOG: Lines = Lines(Mutex::new(Vec::new()));

impl log::Log for Lines {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let line = record.args().to_string();
            eprintln!("[{}] {line}", record.target());
            self.0.lock().unwrap().push(line);
        }
    }

    fn flush(&self) {}
}

/// How many log lines so far contain `text`.
fn logged(text: &str) -> usize {
    LOG.0
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.contains(text))
        .count()
}

/// A TV on Android at 960×540 with remote phases; choosing plays full
/// screen and browsing never tunes.
fn tv(home: crate::isolated::Home) -> (Fixture, Arc<Mutex<Lifetime>>, NodeId) {
    iptv_app::use_remote_key_phases();
    log::set_logger(&LOG).expect("this process's only logger");
    log::set_max_level(log::LevelFilter::Info);
    let mut settings = Settings::new();
    settings
        .set("guide.stay_on_guide", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings.set("guide.switch_delay", Value::ms(250)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings
        .set("player.controls_panel_timeout", Value::secs(5))
        .unwrap();
    let lifetime = Arc::new(Mutex::new(None));
    let recorder = lifetime.clone();
    let fixture = personalization::launch(
        home,
        settings,
        true,
        |config| {
            config.environment.idiom = Idiom::Tv;
            config.environment.os = Os::Android;
            config.viewport = Size::new(px(960.0), px(540.0));
        },
        move |app| *recorder.lock().unwrap() = Some(record_players(app)),
    );
    let lifetime = lifetime.lock().unwrap().take().unwrap();
    let mut fixture = fixture;
    // The list's name opens the groups; the remote's Right enters the
    // guide's channels, focus on Local A.
    step(fixture.app.press("text:/^☰  /"));
    step(fixture.app.remote_tap(RemoteKey::Right));
    let player = fixture
        .app
        .screen()
        .layout_nodes()
        .iter()
        .find(|n| n.kind == ViewKind::Native)
        .expect("one tv.player")
        .id;
    (fixture, lifetime, player)
}

fn shows(app: &crate::Headless, selector: &str) -> bool {
    app.screen().matches(selector).unwrap()
}

/// Whether the channel list over the video is open: one of its rows has
/// the focus pill (the layer stays mounted, unseen, while closed).
fn list_open(app: &crate::Headless) -> bool {
    let dark = nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff);
    let screen = app.screen();
    screen
        .texts()
        .iter()
        .filter(|t| t.contains("  Local "))
        .any(|t| {
            screen
                .span_style(&format!("id:channel-list >> text:{t}"))
                .is_ok_and(|s| s.color == dark)
        })
}

/// Whether the programme and channel menu is open (its items exist only
/// while it is).
fn menu_open(app: &crate::Headless) -> bool {
    shows(app, "id:menu-panel >> text:Manage favourites…")
}

/// The last source the player was given.
fn source(lifetime: &Arc<Mutex<Lifetime>>) -> Option<String> {
    lifetime.lock().unwrap().sources.last().cloned()
}

/// Waits until the player has been given `url`, then for its stream open.
#[track_caller]
fn plays(app: &mut crate::Headless, lifetime: &Arc<Mutex<Lifetime>>, url: &str, opens: u64) {
    step(
        app.wait_until(&format!("{url} opens"), Duration::from_secs(10), |_| {
            source(lifetime).as_deref() == Some(url) && iptv_app::player_opens(0) >= opens
        }),
    );
    assert_eq!(iptv_app::player_opens(0), opens, "{url}");
}

/// A hold of `key` for `ms`: its down, an auto-repeat every 50 ms when
/// `repeats` (an Android remote; an IR remote sends none), its release.
fn hold(app: &mut crate::Headless, key: RemoteKey, ms: u64, repeats: bool) {
    step(app.remote_phase(key, KeyPhase::Down));
    let mut held = 0;
    while held < ms {
        step(app.run_for(Duration::from_millis(50)));
        held += 50;
        if repeats {
            step(app.remote_phase(key, KeyPhase::Repeat));
        }
    }
    step(app.remote_phase(key, KeyPhase::Up));
}

/// Back from full screen to the guide: the banner times out first (Back
/// would only hide it), then Back leaves full screen.
fn back_to_guide(app: &mut crate::Headless) {
    step(app.run_for(Duration::from_millis(3_500)));
    assert!(!shows(app, "id:player-banner"));
    step(app.back());
}

#[test]
fn long_ok_rapid_taps_and_a_surface_change_during_a_hold() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home);
    let app = &mut fixture.app;

    // A quick OK on the guide's first channel plays it full screen.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner"));
    plays(app, &lifetime, "http://example.test/a", 1);

    // Long OK, with an Android remote's repeats and then an IR remote's
    // single down and up: the controls open once, Play/pause focused; no
    // info card, no press of Play/pause by the repeats or the release.
    for repeats in [true, false] {
        hold(app, RemoteKey::Ok, 700, repeats);
        step(app.wait_for("id:play-bar-layer"));
        assert!(selected(app, "play-bar-play-pause"), "repeats: {repeats}");
        assert!(
            !shows(app, "id:player-card"),
            "no short OK after a long one"
        );
        assert!(iptv_app::player_has_source(0));
        step(app.back());
        step(app.wait_for_gone("id:play-bar-layer", Duration::from_secs(2)));
    }

    // Rapid taps: each acts once, on its release, without waiting for
    // the clock. OK shows the card, OK again hides it, a third shows it.
    step(app.remote_tap(RemoteKey::Ok));
    assert!(shows(app, "id:player-card"), "first tap");
    step(app.remote_tap(RemoteKey::Ok));
    assert!(!shows(app, "id:player-card"), "second tap");
    step(app.remote_tap(RemoteKey::Ok));
    assert!(shows(app, "id:player-card"), "third tap");
    assert_eq!(iptv_app::player_opens(0), 1, "taps never retune");

    // OK held on the card's Guide action as the card's own timer runs
    // out: the press keeps the card up, and its release presses Guide.
    step(app.remote_tap(RemoteKey::Right));
    step(app.run_for(Duration::from_millis(4_900)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.run_for(Duration::from_millis(200)));
    assert!(shows(app, "id:player-card"), "a held key keeps the card");
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.wait_for_gone("id:player-card", Duration::from_secs(2)));
    assert!(!shows(app, "id:player-banner"), "the guide, not the video");
    assert_eq!(iptv_app::player_opens(0), 1, "the guide keeps the stream");

    // OK held in the guide while a tap plays Local B full screen: the
    // press began on the guide, so neither its short action on an early
    // release (the info card) nor its long one by timer (the guide's
    // menu, or the player's controls) acts on the video.
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local B"));
    step(app.wait_for("id:player-banner"));
    step(app.run_for(Duration::from_millis(100)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    plays(app, &lifetime, "http://example.test/b", 2);
    assert!(
        !shows(app, "id:player-card"),
        "the early release does nothing"
    );
    assert_eq!(
        logged("ok Short dropped: the surface changed from Guide to Player"),
        1
    );
    back_to_guide(app);

    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.wait_for("id:player-banner"));
    step(app.run_for(Duration::from_millis(450)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    plays(app, &lifetime, "http://example.test/c", 3);
    assert!(!menu_open(app), "no menu by the timer");
    assert!(!shows(app, "id:play-bar-layer"), "no controls by a repeat");
    assert!(!shows(app, "id:player-card"), "no card by the release");
    assert_eq!(
        logged("ok Long dropped: the surface changed from Guide to Player"),
        1
    );
    persistent(app, &lifetime, player);
}

#[test]
fn the_remote_journey_browses_without_retuning_and_returns_to_the_guide() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home);
    let app = &mut fixture.app;

    // Groups: hold Left in the guide; Right returns to the channels.
    hold(app, RemoteKey::Left, 600, true);
    step(app.wait_for("id:guide-groups >> text:Favourites"));
    step(app.remote_tap(RemoteKey::Right));
    assert_eq!(iptv_app::player_opens(0), 0, "browsing never plays");

    // Choose Local A: full screen. The controls, then Back.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner >> text:/Local A/"));
    plays(app, &lifetime, "http://example.test/a", 1);
    step(app.remote_tap(RemoteKey::Menu));
    step(app.wait_for("id:play-bar-layer"));
    assert!(selected(app, "play-bar-play-pause"));
    step(app.back());
    step(app.wait_for_gone("id:play-bar-layer", Duration::from_secs(2)));

    // Browse another channel in the list over the video: focus moves to
    // Local B, nothing retunes (autoplay is off); Back closes the list.
    step(app.remote_tap(RemoteKey::Left));
    assert!(list_open(app), "the list over the video");
    step(app.remote_tap(RemoteKey::Down));
    step(app.run_for(Duration::from_secs(3)));
    assert_eq!(
        iptv_app::player_opens(0),
        1,
        "browsing the list never tunes"
    );
    assert_eq!(source(&lifetime).as_deref(), Some("http://example.test/a"));
    step(app.back());
    assert!(!list_open(app), "Back closes the list");

    // Zap down to Local B, then the previous channel (Right) is Local A.
    step(app.remote_tap(RemoteKey::Down));
    plays(app, &lifetime, "http://example.test/b", 2);
    step(app.remote_tap(RemoteKey::Right));
    plays(app, &lifetime, "http://example.test/a", 3);
    step(app.remote_tap(RemoteKey::Previous));
    plays(app, &lifetime, "http://example.test/b", 4);

    // Back returns to the guide; browsing there does not tune either.
    back_to_guide(app);
    for key in [RemoteKey::Down, RemoteKey::Up, RemoteKey::Up] {
        step(app.remote_tap(key));
    }
    step(app.run_for(Duration::from_secs(2)));
    assert_eq!(
        iptv_app::player_opens(0),
        4,
        "browsing the guide never tunes"
    );
    // Up from the first row reaches the mini-player, which shows the
    // playing channel; OK returns that stream to full screen as it is.
    for _ in 0..3 {
        if selected(app, "guide-preview") {
            break;
        }
        step(app.remote_tap(RemoteKey::Up));
    }
    assert!(selected(app, "guide-preview"));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner >> text:/Local B/"));
    assert_eq!(
        iptv_app::player_opens(0),
        4,
        "the mini-player never reopens"
    );
    back_to_guide(app);
    assert_eq!(source(&lifetime).as_deref(), Some("http://example.test/b"));
    persistent(app, &lifetime, player);
}
