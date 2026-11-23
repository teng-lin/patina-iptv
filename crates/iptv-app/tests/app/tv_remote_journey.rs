//! The TV remote's watching journey (#87), driven by a remote's raw key
//! phases as Android reports them (`iptv_app::use_remote_key_phases`): a
//! key down, its auto-repeats and its release, with the app telling a tap
//! from a hold on its own clock.
//!
//! What it checks: either OK press opens the play bar once and the hold's
//! repeats and release do nothing more; rapid taps each act once; a press held while
//! the surface changes (a tap plays a channel) does nothing on release or
//! by timer; a held OK keeps the info card up and presses the card it was
//! held on. The journey: groups, choose a channel (full screen), the
//! controls, browse another channel without retuning, zap and the
//! previous channel, back to the guide and its mini-player, all on one
//! persistent `tv.player` with counted stream opens. At extra large text,
//! every play bar control the remote focuses stays whole on screen.
//!
//! Synthetic data only: the personalization fixture's three local channels.

use crate::browse_fixture::bind_ok_to_info_card;
use crate::headless::KeyPhase;
use crate::personalization::{self, Fixture, step};
use crate::tv_shared_controls::{Lifetime, persistent, record_players, selected};
use iptv_settings::{KeyBinding, PressKind, Settings, Value, keys};
use nsh_core::platform::{Idiom, Os};
use nsh_core::{NodeId, Size, ViewKind, prelude::px};
use nsh_tv::RemoteKey;
use std::path::Path;
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

/// A TV on Android at 960×540 with remote phases and autoplay disabled.
fn tv(
    home: crate::isolated::Home,
    adjust: impl FnOnce(&mut Settings),
) -> (Fixture, Arc<Mutex<Lifetime>>, NodeId) {
    tv_with(home, adjust, |_, _| {})
}

/// [`tv`], with `prepare` changing the stored library before start.
fn tv_with(
    home: crate::isolated::Home,
    adjust: impl FnOnce(&mut Settings),
    prepare: impl FnOnce(&Path, iptv_store_sqlite::SourceId),
) -> (Fixture, Arc<Mutex<Lifetime>>, NodeId) {
    iptv_app::use_remote_key_phases();
    log::set_logger(&LOG).expect("this process's only logger");
    log::set_max_level(log::LevelFilter::Info);
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings.set("guide.switch_delay", Value::ms(250)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings
        .set("player.controls_panel_timeout", Value::secs(5))
        .unwrap();
    adjust(&mut settings);
    let lifetime = Arc::new(Mutex::new(None));
    let recorder = lifetime.clone();
    let fixture = personalization::launch_prepared(
        home,
        settings,
        true,
        prepare,
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

#[test]
fn long_ok_on_a_group_reorders_it_in_the_guide() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, _, _) = tv(home, |_| {});
    let app = &mut fixture.app;
    step(app.press("text:/^☰  /"));
    step(app.press("id:guide-groups >> text:Sport"));
    hold(app, RemoteKey::Ok, 700, true);
    step(app.wait_for("id:menu-panel >> role:header[Sport]"));
    step(app.press("id:menu-panel >> text:Reorder groups"));
    step(app.wait_for("id:organization-row-1 >> text:Sport"));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.remote_tap(RemoteKey::Up));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:organization-row-0 >> text:Sport"));
    step(app.remote_tap(RemoteKey::Back));
    step(app.back());
    step(app.wait_for("text:Save order"));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    let store = crate::isolated::open_database(&fixture.path).unwrap();
    assert_eq!(store.guide_groups(fixture.source).unwrap()[0].name, "Sport");
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

/// Whether the program and channel menu is open (its items exist only
/// while it is).
fn menu_open(app: &crate::Headless) -> bool {
    shows(app, "id:menu-panel >> text:Manage favorites…")
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

fn first_channel_fullscreen(app: &mut crate::Headless, lifetime: &Arc<Mutex<Lifetime>>) {
    step(app.remote_tap(RemoteKey::Ok));
    plays(app, lifetime, "http://example.test/a", 1);
    assert!(shows(app, "id:guide-preview"));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner >> text:/Local A/"));
}

/// A hold of `key` for `ms`: its down, an auto-repeat every 50 ms when
/// `repeats` (an Android remote; an IR remote sends none), its release.
fn hold(app: &mut crate::Headless, key: RemoteKey, ms: u64, repeats: bool) {
    hold_open(app, key, ms, repeats);
    step(app.remote_phase(key, KeyPhase::Up));
}

/// [`hold`] without its release: the key is still down.
fn hold_open(app: &mut crate::Headless, key: RemoteKey, ms: u64, repeats: bool) {
    step(app.remote_phase(key, KeyPhase::Down));
    let mut held = 0;
    while held < ms {
        step(app.run_for(Duration::from_millis(50)));
        held += 50;
        if repeats {
            step(app.remote_phase(key, KeyPhase::Repeat));
        }
    }
}

/// A hold decided by an auto-repeat: the first event is a repeat (focus
/// arrived mid-press, so no down armed a timer), the next comes past the
/// long-press threshold. The key stays down.
fn repeat_hold(app: &mut crate::Headless, key: RemoteKey) {
    step(app.remote_phase(key, KeyPhase::Repeat));
    step(app.run_for(Duration::from_millis(450)));
    step(app.remote_phase(key, KeyPhase::Repeat));
}

/// PIN 2468 (generated, as `recordings_transport`) protecting Local C.
fn protect_local_c(path: &Path, source: iptv_store_sqlite::SourceId) {
    let mut store = crate::isolated::open_database(path).unwrap();
    store
        .set_parental_credential(&iptv_store_sqlite::parental::ParentalCredential {
            salt: [0x63; 16],
            verifier: [
                108, 253, 222, 238, 40, 38, 77, 8, 100, 135, 138, 42, 133, 48, 171, 236, 123, 169,
                134, 85, 199, 28, 212, 46, 152, 192, 13, 230, 58, 195, 156, 119,
            ],
            recovery_verifier: [0; 32],
            failures: 0,
            retry_after: 0,
        })
        .unwrap();
    let m = iptv_providers::m3u::parse_m3u(personalization::M3U);
    store
        .set_parental_blocked(
            &iptv_core::parental::Target::live(source.0, m.channels.items[2].id.as_str()),
            true,
        )
        .unwrap();
}

const PIN: &str = "id:parental-layer >> text:Enter parental PIN";

/// Back from full screen to the guide: the banner times out first (Back
/// would only hide it), then Back leaves full screen.
fn back_to_guide(app: &mut crate::Headless) {
    step(app.run_for(Duration::from_millis(3_500)));
    assert!(!shows(app, "id:player-banner"));
    step(app.back());
}

fn remap_guide_down_to_select(settings: &mut Settings) {
    settings
        .set(
            keys::KEY_OVERRIDES.as_str(),
            Value::KeyBindings(vec![KeyBinding::new(
                Some("guide"),
                "down",
                PressKind::Short,
                "select",
            )]),
        )
        .unwrap();
}

#[test]
fn a_remapped_select_only_activates_once_per_remote_press() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home, remap_guide_down_to_select);
    let app = &mut fixture.app;

    step(app.remote_phase(RemoteKey::Down, KeyPhase::Down));
    plays(app, &lifetime, "http://example.test/a", 1);
    step(app.remote_phase(RemoteKey::Down, KeyPhase::Repeat));
    step(app.remote_phase(RemoteKey::Down, KeyPhase::Up));
    assert!(shows(app, "id:guide-preview"));
    assert!(!shows(app, "id:player-banner"));

    step(app.remote_tap(RemoteKey::Down));
    step(app.wait_for("id:player-banner >> text:/Local A/"));
    assert_eq!(iptv_app::player_opens(0), 1);
    persistent(app, &lifetime, player);
}

#[test]
fn a_desktop_key_remapped_to_select_repeats_only_outside_the_guide_grid() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    remap_guide_down_to_select(&mut settings);
    let mut fixture = personalization::start(home, settings, true);
    let app = &mut fixture.app;

    assert_eq!(iptv_app::keyboard_repeats("down"), Some(false));
    assert_eq!(iptv_app::keyboard_repeats("up"), Some(true));
    step(app.press_key("tab"));
    assert_eq!(iptv_app::keyboard_repeats("down"), Some(true));
    step(app.press_key("tab"));
    assert_eq!(iptv_app::keyboard_repeats("down"), Some(false));

    step(app.press_key("down"));
    step(app.wait_until(
        "the remapped key starts preview playback",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0),
    ));
    assert!(shows(app, "id:guide-preview"));
    step(app.press_key("down"));
    step(app.wait_for("id:player-banner >> text:/Local A/"));
    assert_eq!(iptv_app::keyboard_repeats("down"), Some(true));
    assert_eq!(iptv_app::player_opens(0), 1);
}

#[test]
fn long_ok_rapid_taps_and_a_surface_change_during_a_hold() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home, |_| {});
    let app = &mut fixture.app;

    first_channel_fullscreen(app, &lifetime);

    // Long OK, with an Android remote's repeats and then an IR remote's
    // single down and up: the controls open once, Play/pause focused; no
    // info card, no press of Play/pause by the repeats or the release.
    for repeats in [true, false] {
        hold_open(app, RemoteKey::Ok, 700, repeats);
        // The hold opened the bar while the key was still down.
        assert!(shows(app, "id:play-bar-layer"), "repeats: {repeats}");
        assert!(selected(app, "play-bar-play-pause"), "repeats: {repeats}");
        step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
        assert!(selected(app, "play-bar-play-pause"), "repeats: {repeats}");
        assert!(
            !shows(app, "id:player-card"),
            "no short OK after a long one"
        );
        assert!(iptv_app::player_has_source(0));
        step(app.back());
        step(app.wait_for_gone("id:play-bar-layer", Duration::from_secs(2)));
    }
    // A repeat decides the hold before any timer: a press that focus
    // reached mid-hold (no down, so no timer) goes long on its repeat.
    repeat_hold(app, RemoteKey::Ok);
    assert!(shows(app, "id:play-bar-layer"), "decided by the repeat");
    assert!(selected(app, "play-bar-play-pause"));
    assert_eq!(
        logged("key=ok phase=Repeat ctx=Player action=Some(ShowControls)"),
        1
    );
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.back());
    step(app.wait_for_gone("id:play-bar-layer", Duration::from_secs(2)));
    assert_eq!(
        logged("play bar: Play or pause"),
        0,
        "no hold pressed Play/pause"
    );

    // The first tap opens the same bar. Each subsequent tap activates
    // its focused control once, without reopening or retuning the stream.
    step(app.remote_tap(RemoteKey::Ok));
    assert!(shows(app, "id:play-bar-layer"));
    assert!(selected(app, "play-bar-play-pause"));
    assert!(!shows(app, "id:player-card"));
    assert_eq!(logged("play bar: Play or pause"), 0);
    step(app.remote_tap(RemoteKey::Ok));
    assert_eq!(logged("play bar: Play or pause"), 1);
    step(app.remote_tap(RemoteKey::Ok));
    assert_eq!(logged("play bar: Play or pause"), 2);
    assert_eq!(iptv_app::player_opens(0), 1, "taps never retune");
    step(app.back());
    back_to_guide(app);

    // A press that started on the guide must not open the play bar after
    // a touch takes the video full screen.
    step(app.press("id:guide-ui >> label:Local B"));
    plays(app, &lifetime, "http://example.test/b", 2);
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local B"));
    step(app.wait_for("id:player-banner"));
    step(app.run_for(Duration::from_millis(100)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    plays(app, &lifetime, "http://example.test/b", 2);
    assert!(
        !shows(app, "id:play-bar-layer"),
        "the early release does nothing"
    );
    assert_eq!(
        logged("ok Short dropped: the surface changed from Guide (Pane(Guide)) to Player (Video)"),
        1
    );
    back_to_guide(app);

    step(app.press("id:guide-ui >> label:Local C"));
    plays(app, &lifetime, "http://example.test/c", 3);
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
        logged("ok Long dropped: the surface changed from Guide (Pane(Guide)) to Player (Video)"),
        1
    );
    persistent(app, &lifetime, player);
}

#[test]
fn the_remote_journey_browses_without_retuning_and_returns_to_the_guide() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home, |_| {});
    let app = &mut fixture.app;

    // Groups: hold Left in the guide; Right returns to the channels.
    hold(app, RemoteKey::Left, 600, true);
    step(app.wait_for("id:guide-groups >> text:Favorites"));
    hold(app, RemoteKey::Ok, 600, false);
    step(app.wait_for("id:menu-panel >> text:Channel order"));
    assert!(!shows(app, "id:menu-panel >> text:Rename group"));
    step(app.back());
    step(app.wait_for_gone(
        "id:menu-panel >> text:Channel order",
        Duration::from_secs(2),
    ));
    assert!(shows(app, "id:guide-groups >> text:Favorites"));
    step(app.remote_tap(RemoteKey::Down));
    hold(app, RemoteKey::Ok, 600, true);
    step(app.wait_for("id:menu-panel >> role:header[News]"));
    assert!(shows(app, "id:menu-panel >> text:Rename group"));
    step(app.back());
    step(app.wait_for_gone("id:menu-panel >> text:Rename group", Duration::from_secs(2)));
    step(app.remote_tap(RemoteKey::Up));
    step(app.remote_tap(RemoteKey::Right));
    assert_eq!(iptv_app::player_opens(0), 0, "browsing never plays");

    // Choose Local A: full screen. The controls, then Back.
    first_channel_fullscreen(app, &lifetime);
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
    hold(app, RemoteKey::Ok, 600, true);
    step(app.wait_for("id:menu-panel >> role:header[Local B]"));
    step(app.wait_for("id:menu-panel >> text:Channel options…"));
    assert!(!menu_open(app), "the list opens channel actions");
    step(app.back());
    assert!(list_open(app), "Back closes only the channel menu");
    let row = "id:channel-list >> role:row >> text:2  Local B";
    let selected_text = app.screen().span_style(row).unwrap().color;
    assert_eq!(
        selected_text,
        nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff),
        "Back leaves the same channel focused",
    );
    hold(app, RemoteKey::Ok, 600, true);
    step(app.wait_for("id:menu-panel >> text:Remove from favorites"));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for_gone(
        "id:menu-panel >> text:Remove from favorites",
        Duration::from_secs(2),
    ));
    hold(app, RemoteKey::Ok, 600, false);
    step(app.wait_for("id:menu-panel >> text:Add to favorites"));
    step(app.back());
    // A touch that changes columns while OK is down cannot retarget its
    // pending channel gesture to the group underneath.
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:channel-list >> text:/^◂/"));
    step(app.wait_for("id:channel-list-groups >> text:News"));
    step(app.run_for(Duration::from_millis(600)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    assert!(!shows(app, "id:menu-panel >> text:Channel order"));
    assert!(!shows(app, "id:menu-panel >> text:Channel options…"));
    step(app.back());
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
    // Up from the rail's Search item reaches the mini-player; OK returns
    // the playing channel to full screen as it is.
    hold(app, RemoteKey::Left, 600, true);
    step(app.remote_tap(RemoteKey::Left));
    step(app.remote_tap(RemoteKey::Up));
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

/// The play bar's control ids, in their order on the bar.
const BAR: [&str; 14] = [
    "play-bar-mute",
    "play-bar-channels",
    "play-bar-guide",
    "play-bar-rewind",
    "play-bar-play-pause",
    "play-bar-forward",
    "play-bar-favorite",
    "play-bar-pip",
    "play-bar-multiview",
    "play-bar-record",
    "play-bar-picture",
    "play-bar-tracks",
    "play-bar-details",
    "play-bar-more",
];

#[test]
fn extra_large_text_keeps_every_focused_bar_control_on_screen() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home, |settings| {
        settings
            .set("appearance.font_size", Value::Enum("extra_large".into()))
            .unwrap();
    });
    let app = &mut fixture.app;
    first_channel_fullscreen(app, &lifetime);
    step(app.remote_tap(RemoteKey::Menu));
    step(app.wait_for("id:play-bar-layer"));
    // Left to the first enabled control, then Right through all of them:
    // exactly one shows focus at a time, whole and inside the screen.
    for _ in 0..BAR.len() {
        step(app.remote_tap(RemoteKey::Left));
    }
    let mut visited = Vec::new();
    for _ in 0..BAR.len() {
        let focused: Vec<_> = BAR.iter().filter(|id| selected(app, id)).collect();
        assert_eq!(focused.len(), 1, "one focused control: {focused:?}");
        let id = *focused[0];
        let nodes = app.screen().layout_nodes();
        let bounds = nodes
            .iter()
            .find(|n| n.test_id.as_deref() == Some(id))
            .unwrap()
            .bounds;
        assert!(
            bounds.size.width.0 > 0.0
                && bounds.size.height.0 > 0.0
                && bounds.origin.x.0 >= -1.0
                && bounds.origin.y.0 >= -1.0
                && bounds.origin.x.0 + bounds.size.width.0 <= 961.0
                && bounds.origin.y.0 + bounds.size.height.0 <= 541.0,
            "{id} is whole on screen at extra large text: {bounds:?}"
        );
        if !visited.contains(&id) {
            visited.push(id);
        }
        step(app.remote_tap(RemoteKey::Right));
    }
    assert!(
        visited.len() >= 6,
        "the remote reaches the bar's live controls: {visited:?}"
    );
    assert!(!visited.contains(&"play-bar-rewind"), "live skips rewind");
    assert_eq!(iptv_app::player_opens(0), 1);
    persistent(app, &lifetime, player);
}

#[test]
fn a_press_belongs_to_its_owner_and_a_prompt_takes_its_keys() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv_with(
        home,
        |settings| {
            bind_ok_to_info_card(settings);
            settings
                .set("parental.pin_input", Value::Enum("number_keys".into()))
                .unwrap();
            settings
                .set("parental.live_enabled", Value::Bool(true))
                .unwrap();
            settings
                .set("guide.stay_on_guide", Value::Bool(true))
                .unwrap();
        },
        protect_local_c,
    );
    let app = &mut fixture.app;
    step(app.remote_tap(RemoteKey::Ok));
    plays(app, &lifetime, "http://example.test/a", 1);

    // OK held in the guide while a tap on Local C opens its PIN prompt; OK
    // goes up while the prompt shows; the prompt is canceled before the
    // hold would have been long: no menu by a late timer.
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.back());
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));
    step(app.run_for(Duration::from_millis(600)));
    assert!(!menu_open(app), "no menu after the key went up");
    assert_eq!(logged("long press by timer"), 0);

    // On a PIN prompt the platform's Back closes it once: the raw Back is
    // not a second step back (to the groups). OK on the focused Local C.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_tap(RemoteKey::Back));
    step(app.back());
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));
    step(app.remote_tap(RemoteKey::Right));
    assert_eq!(
        logged("key=right phase=Down ctx=Guide"),
        1,
        "still on the channels after one step back"
    );
    step(app.remote_tap(RemoteKey::Left));

    // OK that submits the PIN, held: its repeats neither start a hold
    // (the play bar) nor act anywhere.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    for digit in [2, 4, 6, 8] {
        step(app.remote_tap(RemoteKey::Digit(digit)));
    }
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.wait_for_gone(PIN, Duration::from_secs(20)));
    for _ in 0..18 {
        step(app.run_for(Duration::from_millis(50)));
        step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    }
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    // (Opens are not counted here: the headless player reports no state,
    // so a long wait lets the stall recovery retry the previous stream.)
    step(
        app.wait_until("Local C plays", Duration::from_secs(10), |_| {
            source(&lifetime).as_deref() == Some("http://example.test/c")
        }),
    );
    assert!(
        !shows(app, "id:play-bar-layer"),
        "no hold from the PIN's OK"
    );
    assert!(!shows(app, "id:player-card"), "no tap from the PIN's OK");
    assert!(!menu_open(app), "no menu from the PIN's OK");
    // OK on the playing Local C: full screen.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner"));
    step(app.run_for(Duration::from_millis(3_500)));

    // The card and the strip are both Controls: OK held on the card while
    // a tap opens More controls does not press the strip's first control
    // (Search) on release.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-actions"));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:player-actions >> text:More controls"));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    assert!(
        !shows(app, "id:search-screen"),
        "nothing pressed on the strip"
    );
    assert_eq!(
        logged("ok Short dropped: the surface changed from Controls (Card) to Controls (Strip)"),
        1
    );
    step(app.back());

    // Back held on the card while a tap takes its TV guide action: its
    // release does not also step back in the guide (Android's own Back
    // follows the raw release; here `back`).
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-actions"));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Down));
    step(app.press("id:player-actions >> text:TV guide"));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Up));
    step(app.back());
    assert_eq!(logged("back Short dropped"), 1);
    assert_eq!(logged("released after a long back; no step back"), 1);
    step(app.remote_tap(RemoteKey::Right));
    assert_eq!(
        logged("key=right phase=Down ctx=Guide"),
        2,
        "still on the channels"
    );
    persistent(app, &lifetime, player);
}

#[test]
fn holds_keep_their_card_decide_once_and_menu_does_not_toggle() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home, |settings| {
        bind_ok_to_info_card(settings);
        // The card's own timeout is shorter than the long press.
        settings
            .set("player.controls_panel_timeout", Value::secs(1))
            .unwrap();
        settings.set("remote.long_press", Value::ms(2_000)).unwrap();
    });
    let app = &mut fixture.app;

    // A held Menu opens the guide's menu once; its repeats leave it open.
    hold(app, RemoteKey::Menu, 450, true);
    assert!(menu_open(app), "the menu stays open through the repeats");
    step(app.back());
    assert!(!menu_open(app));

    first_channel_fullscreen(app, &lifetime);
    // A tap of OK held 1.5 s on the card's TV guide action (a remote with
    // no repeats): the card outlives its 1 s timeout and the release
    // presses the action.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-actions"));
    step(app.remote_tap(RemoteKey::Right));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    // One auto-repeat early in the hold, then none (it must not pull the
    // card's deadline back to its own 1 s timeout).
    step(app.run_for(Duration::from_millis(400)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    step(app.run_for(Duration::from_millis(1_100)));
    assert!(shows(app, "id:player-card"), "the held key keeps the card");
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.wait_for_gone("id:player-card", Duration::from_secs(2)));
    assert!(!shows(app, "id:player-banner"), "the guide, not the video");

    // On the play bar, a hold decided by its repeat returns focus to
    // Play/pause, as one decided by the timer does.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner"));
    step(app.remote_tap(RemoteKey::Menu));
    step(app.wait_for("id:play-bar-layer"));
    step(app.remote_tap(RemoteKey::Right));
    assert!(!selected(app, "play-bar-play-pause"));
    // Earlier presses' timers have run, so only a repeat can decide.
    step(app.run_for(Duration::from_millis(2_100)));
    assert!(shows(app, "id:play-bar-layer"));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    step(app.run_for(Duration::from_millis(2_050)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    assert!(
        selected(app, "play-bar-play-pause"),
        "the hold refocuses Play/pause"
    );
    assert_eq!(
        logged("key=ok phase=Repeat ctx=Controls action=Some(ShowControls)"),
        1
    );
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    assert_eq!(logged("play bar: Play or pause"), 0);
    // Menu held with the bar open hides it; its repeats do not reopen it.
    hold(app, RemoteKey::Menu, 300, true);
    assert!(
        !shows(app, "id:play-bar-layer"),
        "Menu's repeats leave it hidden"
    );
    assert_eq!(iptv_app::player_opens(0), 1);
    persistent(app, &lifetime, player);
}

#[test]
fn a_back_or_ok_held_into_a_pin_prompt_stays_cut_short() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv_with(
        home,
        |settings| {
            bind_ok_to_info_card(settings);
            settings
                .set("parental.live_enabled", Value::Bool(true))
                .unwrap();
            settings
                .set("guide.stay_on_guide", Value::Bool(true))
                .unwrap();
        },
        protect_local_c,
    );
    let app = &mut fixture.app;
    step(app.remote_tap(RemoteKey::Ok));
    plays(app, &lifetime, "http://example.test/a", 1);

    // Back down in the guide, a tap opens Local C's PIN prompt, Back goes
    // up under it: Android's Back on that release does not dismiss the
    // prompt the press never saw.
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Up));
    step(app.back());
    assert!(shows(app, PIN), "the earlier Back leaves the prompt open");
    // A fresh Back closes it (and finds no guard left armed).
    step(app.remote_tap(RemoteKey::Back));
    step(app.back());
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));

    // A Back guard armed by a dropped Back whose callback never came: a
    // fresh Back on a PIN prompt still closes it.
    step(app.remote_tap(RemoteKey::Up));
    step(app.remote_tap(RemoteKey::Up));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner"));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-actions"));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Down));
    step(app.press("id:player-actions >> text:TV guide"));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Up));
    // (No platform Back follows: Android canceled it.) OK on Local C.
    step(app.remote_tap(RemoteKey::Down));
    step(app.remote_tap(RemoteKey::Down));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_tap(RemoteKey::Back));
    step(app.back());
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));

    // OK held in the guide while a tap opens the prompt; a repeat reaches
    // the prompt; a touch cancels it while OK is still down: the next
    // repeats and the release choose nothing.
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    step(app.press("id:parental-cancel"));
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));
    let selects = logged("key=ok phase=Up ctx=Guide action=Some(Select)");
    for _ in 0..3 {
        step(app.run_for(Duration::from_millis(50)));
        step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    }
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.run_for(Duration::from_millis(600)));
    assert_eq!(
        logged("key=ok phase=Up ctx=Guide action=Some(Select)"),
        selects,
        "the cut-short OK selects nothing"
    );
    assert!(!shows(app, PIN), "no prompt reopened");
    assert!(!menu_open(app), "no menu by a new hold");
    persistent(app, &lifetime, player);
}

#[test]
fn touch_around_a_held_key_leaves_it_cut_short_and_back_with_its_prompt() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv_with(
        home,
        |settings| {
            settings
                .set("parental.live_enabled", Value::Bool(true))
                .unwrap();
            settings
                .set("guide.stay_on_guide", Value::Bool(true))
                .unwrap();
            // Holds outlast a prompt opened and closed by touch.
            settings.set("remote.long_press", Value::ms(2_000)).unwrap();
        },
        protect_local_c,
    );
    let app = &mut fixture.app;
    step(app.remote_tap(RemoteKey::Ok));
    plays(app, &lifetime, "http://example.test/a", 1);
    let swallowed = "released after a long back; no step back";
    let on_channels = "key=right phase=Down ctx=Guide";

    // OK held in the guide while touch opens Local C's PIN prompt and
    // cancels it, no key reaching the prompt; OK goes up before its hold
    // would be long: the prompt took the press, so the release chooses
    // nothing (it would open the prompt again).
    let select = "key=ok phase=Up ctx=Guide action=Some(Select)";
    let selects = logged(select);
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.press("id:parental-cancel"));
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.run_for(Duration::from_millis(300)));
    assert_eq!(logged(select), selects, "the cut-short OK selects nothing");
    assert!(!shows(app, PIN), "no prompt reopened");

    // Back held likewise, its hold timer running out under the prompt:
    // after touch cancels the prompt, Android's Back on the release does
    // not step back from the guide.
    let before = logged(swallowed);
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.run_for(Duration::from_millis(2_000)));
    step(app.press("id:parental-cancel"));
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Up));
    step(app.back());
    assert_eq!(logged(swallowed), before + 1, "the release takes no step");
    let rights = logged(on_channels);
    step(app.remote_tap(RemoteKey::Right));
    assert_eq!(logged(on_channels), rights + 1, "still on the channels");
    step(app.remote_tap(RemoteKey::Left));

    // Back pressed in the prompt, which touch then cancels while Back is
    // down: its repeats past the hold do nothing (no Long Back to the
    // video), and Android's Back on its release closes nothing beneath.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Down));
    step(app.press("id:parental-cancel"));
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));
    for _ in 0..44 {
        step(app.run_for(Duration::from_millis(50)));
        step(app.remote_phase(RemoteKey::Back, KeyPhase::Repeat));
    }
    let before = logged(swallowed);
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Up));
    step(app.back());
    assert_eq!(
        logged("key=back phase=Repeat"),
        0,
        "its repeats reach nothing"
    );
    assert!(!shows(app, "id:player-banner"), "no Long Back to the video");
    assert_eq!(logged(swallowed), before + 1, "the release takes no step");
    let rights = logged(on_channels);
    step(app.remote_tap(RemoteKey::Right));
    assert_eq!(logged(on_channels), rights + 1, "still on the channels");
    step(app.remote_tap(RemoteKey::Left));

    // Back held in the guide, touch opens the prompt, Right is tapped in
    // it, then Back goes up: Right's press does not lose Back's release,
    // so Android's Back on it leaves the prompt open.
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_tap(RemoteKey::Right));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Up));
    step(app.back());
    assert!(shows(app, PIN), "the earlier Back leaves the prompt open");
    step(app.remote_tap(RemoteKey::Back));
    step(app.back());
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));

    // Back held, OK pressed over it (Back's press is replaced), then
    // touch opens the prompt: Back, still down, is cut short too.
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Down));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.press("id:guide-ui >> label:Local C"));
    step(app.run_for(Duration::from_millis(300)));
    step(app.wait_for(PIN));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.remote_phase(RemoteKey::Back, KeyPhase::Up));
    step(app.back());
    assert!(shows(app, PIN), "the replaced Back leaves the prompt open");
    step(app.remote_tap(RemoteKey::Back));
    step(app.back());
    step(app.wait_for_gone(PIN, Duration::from_secs(2)));
    persistent(app, &lifetime, player);
}

#[test]
fn a_zap_or_menu_during_a_hold_does_not_cut_it_short_or_restart_it() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home, |settings| {
        bind_ok_to_info_card(settings);
        // The card's own timeout is shorter than the long press.
        settings
            .set("player.controls_panel_timeout", Value::secs(1))
            .unwrap();
        settings.set("remote.long_press", Value::ms(2_000)).unwrap();
    });
    let app = &mut fixture.app;
    first_channel_fullscreen(app, &lifetime);

    // A zap queued from the card, then OK held on its TV guide action
    // before the zap commits: the zap's banner restarts the card's own
    // timer, not the held key's, so the card outlives both and the
    // release presses the action.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-actions"));
    step(app.remote_tap(RemoteKey::Right));
    step(app.remote_tap(RemoteKey::Down));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.run_for(Duration::from_millis(1_500)));
    plays(app, &lifetime, "http://example.test/b", 2);
    assert!(shows(app, "id:player-card"), "the held key keeps the card");
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.wait_for_gone("id:player-card", Duration::from_secs(2)));
    assert!(!shows(app, "id:player-banner"), "the guide, not the video");

    // Right held on the card, OK goes down, then Right goes up: Right's
    // release leaves OK's hold of the card as it is.
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner"));
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-actions"));
    step(app.remote_phase(RemoteKey::Right, KeyPhase::Down));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.remote_phase(RemoteKey::Right, KeyPhase::Up));
    step(app.run_for(Duration::from_millis(1_500)));
    assert!(shows(app, "id:player-card"), "the held OK keeps the card");
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    step(app.wait_for_gone("id:player-card", Duration::from_secs(2)));
    assert!(!shows(app, "id:player-banner"), "the guide, not the video");

    // OK held on the play bar, Menu pressed over it (the bar hides): OK's
    // later repeats and release, now on the video, start no press of
    // their own (no info card).
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:player-banner"));
    step(app.remote_tap(RemoteKey::Menu));
    step(app.wait_for("id:play-bar-layer"));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Down));
    step(app.run_for(Duration::from_millis(50)));
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    step(app.remote_tap(RemoteKey::Menu));
    assert!(!shows(app, "id:play-bar-layer"), "Menu hid the bar");
    for _ in 0..3 {
        step(app.run_for(Duration::from_millis(50)));
        step(app.remote_phase(RemoteKey::Ok, KeyPhase::Repeat));
    }
    step(app.remote_phase(RemoteKey::Ok, KeyPhase::Up));
    assert!(
        !shows(app, "id:player-actions"),
        "the displaced OK opens no card"
    );
    persistent(app, &lifetime, player);
}

/// A desktop keyboard's Enter held on a PIN prompt: the keystroke that
/// closed the prompt does not fire again on its auto-repeats, and a fresh
/// press does. (The headless keyboard sends no repeats: what is checked is
/// the binding's repeat rule, which the platform's keymap follows.)
#[test]
fn a_keyboard_key_a_prompt_took_holds_its_repeats_until_pressed_again() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings
        .set("parental.pin_input", Value::Enum("number_keys".into()))
        .unwrap();
    settings
        .set("parental.live_enabled", Value::Bool(true))
        .unwrap();
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    let mut fixture =
        personalization::launch_prepared(home, settings, false, protect_local_c, |_| {}, |_| {});
    let app = &mut fixture.app;
    step(app.press("text:/^☰  /"));
    step(app.press_key("right"));
    assert_eq!(iptv_app::keyboard_repeats("enter"), Some(false));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for(PIN));
    for digit in ["2", "4", "6", "8"] {
        step(app.press_key(digit));
    }
    step(app.press_key("enter"));
    step(app.wait_for_gone(PIN, Duration::from_secs(20)));
    assert_eq!(
        iptv_app::keyboard_repeats("enter"),
        Some(false),
        "the Enter that submitted the PIN repeats nowhere"
    );
    assert_eq!(iptv_app::keyboard_repeats("8"), Some(false));
    assert_eq!(iptv_app::keyboard_repeats("down"), Some(true));
    // A fresh press acts, while held Enter remains a single activation.
    step(app.press_key("enter"));
    assert_eq!(iptv_app::keyboard_repeats("enter"), Some(false));
}

/// "Sound follows the selected tile" (#179) by a remote alone: the rail's
/// Settings, the Playback page, Down to its row, and OK turns it on, and
/// it is saved; OK again turns it off.
#[test]
fn a_remote_changes_the_sound_follows_the_selected_tile_row() {
    use crate::multiview_heard::{follows_on, saved_follows, to_follows_row};
    let Some(home) = isolated!() else { return };
    let path = home.data_dir().join("settings.json");
    let (mut fixture, _, _) = tv(home, |_| {});
    let app = &mut fixture.app;
    // Left from the channels to the groups, then the rail; Settings is
    // its last item.
    for _ in 0..2 {
        step(app.remote_tap(RemoteKey::Left));
    }
    step(app.wait_for("id:navigation-rail >> text:Settings"));
    for _ in 0..12 {
        step(app.remote_tap(RemoteKey::Down));
    }
    step(app.remote_tap(RemoteKey::Ok));
    step(app.wait_for("id:settings-screen"));
    // The pages open on General; Playback is the fifth.
    step(app.wait_for("text:Play the last channel on start"));
    for _ in 0..4 {
        step(app.remote_tap(RemoteKey::Down));
    }
    to_follows_row(app, |app, key| {
        let key = match key {
            "right" => RemoteKey::Right,
            _ => RemoteKey::Down,
        };
        step(app.remote_tap(key));
    });
    assert!(!follows_on(app), "off by default");
    step(app.remote_tap(RemoteKey::Ok));
    saved_follows(app, &path, true);
    assert!(follows_on(app));
    step(app.remote_tap(RemoteKey::Ok));
    saved_follows(app, &path, false);
    assert!(!follows_on(app));
}

#[path = "../../../iptv-nav/tests/support/remote_trace.rs"]
mod remote_trace;

#[test]
fn a_late_release_cannot_apply_the_displaced_guide_press() {
    use iptv_nav::{Phase, RemoteKey as NavKey};

    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime, player) = tv(home, |_| {});
    let app = &mut fixture.app;
    let mut last = 0;
    let right_before = logged("action=Some(MoveRight)");
    for (at, key, phase) in remote_trace::LATE_RELEASE {
        step(app.run_for(Duration::from_millis(at - last)));
        last = at;
        let key = match key {
            NavKey::Left => RemoteKey::Left,
            NavKey::Right => RemoteKey::Right,
            _ => unreachable!("the trace uses guide arrows"),
        };
        let phase = match phase {
            Phase::Down => KeyPhase::Down,
            Phase::Up => KeyPhase::Up,
            Phase::Repeat => KeyPhase::Repeat,
        };
        step(app.remote_phase(key, phase));
    }
    assert_eq!(logged("action=Some(MoveRight)") - right_before, 1);
    assert_eq!(logged("action=Some(MoveLeft)"), 0);
    assert!(!iptv_app::player_has_source(0));
    persistent(app, &lifetime, player);
}
