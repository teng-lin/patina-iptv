//! Shared pointer/TV controls and guide preview retain one native player.
use crate::browse_fixture::choose_fullscreen;
use crate::personalization::{self, step};
use iptv_settings::{Settings, Value};
use nsh_core::{MountItem, NodeId, ViewKind, platform::Idiom};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
pub(crate) struct Lifetime {
    pub(crate) players: Vec<NodeId>,
    pub(crate) removed: Vec<NodeId>,
    pub(crate) inserts: HashMap<NodeId, usize>,
    pub(crate) sources: Vec<String>,
}
pub(crate) fn selected(app: &crate::Headless, id: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:{id}"))
        .is_ok_and(|s| s.selected)
}
pub(crate) fn persistent(app: &crate::Headless, lifetime: &Arc<Mutex<Lifetime>>, player: NodeId) {
    let recorded = lifetime.lock().unwrap();
    assert_eq!(recorded.players, [player]);
    assert!(!recorded.removed.contains(&player), "player never detaches");
    assert_eq!(
        recorded.inserts.get(&player),
        Some(&1),
        "player never reparents"
    );
    assert!(
        app.screen()
            .layout_nodes()
            .iter()
            .any(|n| n.id == player && !n.hidden)
    );
}

/// Records the `tv.player` mounts, moves and sources from now on, for
/// [`persistent`] (call it before the first batch is applied).
pub(crate) fn record_players(app: &mut crate::Headless) -> Arc<Mutex<Lifetime>> {
    let lifetime: Arc<Mutex<Lifetime>> = Arc::default();
    record_players_into(app, lifetime.clone());
    lifetime
}

fn record_players_into(app: &mut crate::Headless, observed: Arc<Mutex<Lifetime>>) {
    app.observe(move |_, batch| {
        let mut recorded = observed.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::Component { id, name } if name.as_ref() == "tv.player" => {
                    recorded.players.push(*id)
                }
                MountItem::ComponentProps { props, .. } => {
                    for (key, value) in props {
                        if key.as_ref() == "source"
                            && let Some(source) =
                                value.as_ref().map(crate::support::upstream_source)
                            && let Some(url) = source["url"].as_str()
                        {
                            recorded.sources.push(url.into());
                        }
                    }
                }
                MountItem::Insert { child, .. } => {
                    *recorded.inserts.entry(*child).or_default() += 1
                }
                MountItem::Remove { id } | MountItem::Delete { id } => recorded.removed.push(*id),
                _ => {}
            }
        }
    });
}

fn preview_fixture(
    home: crate::isolated::Home,
) -> (personalization::Fixture, Arc<Mutex<Lifetime>>) {
    preview_fixture_with(home, |_| {})
}

/// [`preview_fixture`] with its settings adjusted by `adjust`.
pub(crate) fn preview_fixture_with(
    home: crate::isolated::Home,
    adjust: impl FnOnce(&mut Settings),
) -> (personalization::Fixture, Arc<Mutex<Lifetime>>) {
    let lifetime: Arc<Mutex<Lifetime>> = Arc::default();
    let recorder = lifetime.clone();
    let mut settings = Settings::new();
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(true)).unwrap();
    settings
        .set("guide.switch_delay", Value::ms(1_500))
        .unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    adjust(&mut settings);
    let fixture = personalization::start_with_setup(home, settings, true, move |app| {
        record_players_into(app, recorder);
    });
    (fixture, lifetime)
}

#[test]
fn tv_guide_preview_and_shared_bar_keep_the_existing_player_and_route_dpad() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime) = preview_fixture(home);
    let app = &mut fixture.app;
    step(app.set_environment("TV remote", |env| env.idiom = Idiom::Tv));
    step(app.set_viewport(nsh_core::Size::new(
        nsh_core::prelude::px(960.0),
        nsh_core::prelude::px(540.0),
    )));
    let player = app
        .screen()
        .layout_nodes()
        .iter()
        .find(|n| n.kind == ViewKind::Native)
        .unwrap()
        .id;
    // Play B, then focus A: opening the preview must return B, not tune A.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(
        app.wait_until("B plays in preview", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    let opens = iptv_app::player_opens(0);
    step(app.press_key("up"));
    assert!(!selected(app, "guide-preview"));
    step(app.press_key("tab"));
    step(app.press_key("left"));
    step(app.press_key("up"));
    assert!(selected(app, "guide-preview"));
    step(app.press_key("pagedown"));
    step(app.press_key("home"));
    assert!(
        selected(app, "guide-preview"),
        "guide commands do not change the preview's channel anchor"
    );
    // Leaving the guide for the preview cancels A's pending autoplay.
    step(app.run_for(Duration::from_secs(3)));
    assert_eq!(iptv_app::player_opens(0), opens);
    persistent(app, &lifetime, player);
    step(app.press_key("down"));
    assert!(!selected(app, "guide-preview"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner"));
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "preview OK does not tune A"
    );
    persistent(app, &lifetime, player);

    step(app.press_key("m"));
    step(app.wait_for("id:play-bar-layer"));
    assert!(selected(app, "play-bar-play-pause"));
    let bounds = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("play-bar-play-pause"))
        .expect("the play/pause button")
        .bounds;
    assert!((bounds.origin.x.0 + bounds.size.width.0 / 2.0 - 480.0).abs() <= 0.5);
    // Guide and Favorite are one step either side of live transport.
    step(app.press_key("left"));
    assert!(selected(app, "play-bar-guide"));
    step(app.press_key("right"));
    assert!(selected(app, "play-bar-play-pause"));
    step(app.press_key("right"));
    assert!(selected(app, "play-bar-favorite"));
    // Down over live playback must keep focus in the bar, not zap channels.
    step(app.press_key("down"));
    assert!(selected(app, "play-bar-favorite"));
    assert_eq!(iptv_app::player_opens(0), opens);
    for _ in 0..12 {
        if selected(app, "play-bar-picture") {
            break;
        }
        step(app.press_key("right"));
    }
    assert!(selected(app, "play-bar-picture"));
    step(app.press_key("enter"));
    step(app.wait_for("id:chooser-layer >> text:Stretch to fill"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Stretch to fill", Duration::from_secs(2)));
    assert!(
        selected(app, "play-bar-picture"),
        "popup restores control identity"
    );
    persistent(app, &lifetime, player);
    step(app.back());
    step(app.wait_for_gone("id:play-bar-layer", Duration::from_secs(2)));
    step(app.back());
    assert!(
        selected(app, "guide-preview"),
        "Back restores mini-player focus"
    );
    step(app.press_key("down"));
    assert!(!selected(app, "guide-preview"));
    assert_eq!(iptv_app::player_opens(0), opens);
    persistent(app, &lifetime, player);
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.press_key("m"));
    step(app.wait_for("id:play-bar-layer"));
    step(app.press_key("left"));
    assert!(selected(app, "play-bar-guide"));
    step(app.press_key("enter"));
    step(app.wait_for("id:guide-preview"));
    assert_eq!(iptv_app::player_opens(0), opens);
    persistent(app, &lifetime, player);
    step(app.press_key("f"));
    step(app.press_key("g"));
    assert!(
        !selected(app, "guide-preview"),
        "Overlay returns focus to a visible guide row"
    );
    step(app.press_key("f"));
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    step(app.wait_until(
        "stop clears preview playback",
        Duration::from_secs(3),
        |_| !iptv_app::player_has_source(0),
    ));
    assert!(!selected(app, "guide-preview"));
    step(app.press_key("m"));
    step(app.wait_for("id:menu-panel"));
    assert!(
        !app.screen().matches("id:play-bar-layer").unwrap(),
        "Menu after Stop stays in the guide"
    );
    persistent(app, &lifetime, player);
}

fn width(app: &crate::Headless, id: &str) -> f32 {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("missing {id}"))
        .bounds
        .size
        .width
        .0
}

#[test]
fn tv_rail_keeps_settings_inside_the_available_height() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, _) = preview_fixture(home);
    let app = &mut fixture.app;
    step(app.set_environment("TV remote with a bottom inset", |env| {
        env.idiom = Idiom::Tv;
        env.safe_area.bottom = nsh_core::prelude::px(24.0);
    }));
    step(app.set_viewport(nsh_core::Size::new(
        nsh_core::prelude::px(640.0),
        nsh_core::prelude::px(360.0),
    )));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("id:navigation-rail >> text:Settings"));

    let settings = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| {
            !node.hidden
                && node
                    .text
                    .as_ref()
                    .is_some_and(|text| text.text.text == "Settings")
        })
        .expect("Settings is laid out in the visible rail");
    assert!(
        settings.bounds.origin.y.0 + settings.bounds.size.height.0 <= 336.0,
        "Settings is fully inside the 24-point bottom-safe TV viewport: {:?}",
        settings.bounds
    );
}

#[test]
fn the_rail_reaches_preview_on_desktop_and_tv_without_reopening_video() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime) = preview_fixture(home);
    let app = &mut fixture.app;
    let player = app
        .screen()
        .layout_nodes()
        .iter()
        .find(|node| node.kind == ViewKind::Native)
        .unwrap()
        .id;
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(
        app.wait_until("B plays in preview", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    let opens = iptv_app::player_opens(0);
    for idiom in [Idiom::Desktop, Idiom::Tv] {
        step(app.set_environment("shared preview navigation", |env| {
            env.idiom = idiom;
            env.os = if idiom == Idiom::Desktop {
                nsh_core::platform::Os::Linux
            } else {
                nsh_core::platform::Os::Android
            };
        }));
        let (w, h) = if idiom == Idiom::Tv {
            (960.0, 540.0)
        } else {
            (1280.0, 720.0)
        };
        step(app.set_viewport(nsh_core::Size::new(
            nsh_core::prelude::px(w),
            nsh_core::prelude::px(h),
        )));
        step(app.press("text:/^☰  /"));
        step(app.press("id:guide-groups >> text:Favorites"));
        if idiom == Idiom::Desktop {
            step(app.press_key("tab"));
        }
        // Left enters Search; its expanded sidebar remains in place during preview focus.
        step(app.press_key("left"));
        let rail_width = width(app, "navigation-rail");
        assert!(rail_width > 100.0, "expanded rail");
        step(app.press_key("up"));
        assert!(selected(app, "guide-preview"), "Search, {idiom:?}");
        assert_eq!(width(app, "navigation-rail"), rail_width);
        step(app.run_for(Duration::from_secs(3)));
        assert_eq!(iptv_app::player_opens(0), opens);
        step(app.press_key("down"));
        step(app.press_key("enter"));
        step(app.wait_for("id:search-screen"));
        step(app.back());
        step(app.wait_for_gone("id:search-screen", Duration::from_secs(2)));
        // Back from the preview also returns to Search, whose OK opens search again.
        step(app.press_key("up"));
        assert!(selected(app, "guide-preview"));
        step(app.back());
        step(app.press_key("enter"));
        step(app.wait_for("id:search-screen"));
        step(app.back());
        step(app.wait_for_gone("id:search-screen", Duration::from_secs(2)));
        assert_eq!(iptv_app::player_opens(0), opens);
        persistent(app, &lifetime, player);
    }
}

#[test]
fn numeric_channel_entry_commits_corrected_identity_and_invalid_input_does_not_zap() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime) = preview_fixture(home);
    let app = &mut fixture.app;
    step(app.set_environment("TV digits", |env| env.idiom = Idiom::Tv));
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.kind == ViewKind::Native)
        .unwrap()
        .id;
    step(app.press_key("2"));
    step(app.press_key("9"));
    step(app.press_key("backspace"));
    step(app.press_key("enter"));
    step(app.wait_until(
        "corrected numeric choice publishes Local B's identified source",
        Duration::from_secs(10),
        |_| {
            lifetime
                .lock()
                .unwrap()
                .sources
                .last()
                .is_some_and(|url| url == "http://example.test/b")
        },
    ));
    assert_eq!(lifetime.lock().unwrap().sources, ["http://example.test/b"]);
    let opens = iptv_app::player_opens(0);
    step(app.press_key("9"));
    step(app.press_key("9"));
    step(app.press_key("enter"));
    step(app.wait(Duration::from_millis(100)));
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "unknown number never aliases channel zero"
    );
    persistent(app, &lifetime, player);
}

#[test]
fn live_player_panels_expire_and_back_dismisses_without_stopping() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("guide.stay_on_guide", Value::Bool(false))
        .unwrap();
    settings
        .set("player.controls_panel_timeout", Value::secs(5))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(500)).unwrap();
    let mut fixture = personalization::start(home, settings, true);
    let app = &mut fixture.app;
    step(app.set_environment("TV remote", |env| env.idiom = Idiom::Tv));
    choose_fullscreen(app);
    step(app.wait_for("id:player-banner"));
    step(app.run_for(Duration::from_millis(3_100)));
    assert!(!app.screen().matches("id:player-banner").unwrap());
    let playing = iptv_app::player_opens(0);
    assert!(iptv_app::player_has_source(0));

    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
    step(app.run_for(Duration::from_millis(4_500)));
    assert!(app.screen().matches("id:player-card").unwrap());
    step(app.run_for(Duration::from_millis(600)));
    assert!(!app.screen().matches("id:player-card").unwrap());
    assert_eq!(iptv_app::player_opens(0), playing);
    assert!(iptv_app::player_has_source(0));

    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
    step(app.back());
    assert!(!app.screen().matches("id:player-card").unwrap());
    assert!(iptv_app::player_has_source(0));

    step(app.press_key("down"));
    step(app.wait_for("id:player-banner"));
    step(
        app.wait_until("the next channel starts", Duration::from_secs(5), |_| {
            iptv_app::player_opens(0) == playing + 1
        }),
    );
    step(app.run_for(Duration::from_millis(3_100)));
    assert!(!app.screen().matches("id:player-banner").unwrap());
    assert!(iptv_app::player_has_source(0));
}

#[test]
fn tv_shows_a_visible_back_once_touch_is_used_and_hides_it_for_keys() {
    let Some(home) = isolated!() else { return };
    let mut fixture = personalization::start(home, Settings::new(), true);
    let app = &mut fixture.app;
    step(app.set_environment("TV remote", |env| env.idiom = Idiom::Tv));
    choose_fullscreen(app);
    step(app.wait_for("id:player-banner"));
    step(app.press_key("m"));
    step(app.wait_for("id:play-bar-layer"));
    assert!(
        !app.screen().matches("id:play-bar-back").unwrap(),
        "the remote has its own Back"
    );
    // A touch on a control: the input, not the device, decides.
    step(app.press("id:play-bar-mute"));
    step(app.wait_for("id:play-bar-back"));
    // A key returns to the remote's view.
    step(app.press_key("right"));
    step(app.wait_for_gone("id:play-bar-back", Duration::from_secs(2)));
    // So does the remote's Back, which the platform handles.
    step(app.press("id:play-bar-mute"));
    step(app.wait_for("id:play-bar-back"));
    step(app.press_remote(nsh_tv::RemoteKey::Back));
    step(app.wait_for_gone("id:play-bar-back", Duration::from_secs(2)));
}

/// The menu's heading names the channel; Back closes it.
#[track_caller]
fn menu_opens_for(app: &mut crate::Headless, channel: &str, program_menu: bool) {
    let favorite = "id:menu-panel >> text:/^(Add to favorites|Remove from favorites)$/";
    step(app.wait_for(favorite));
    assert!(
        app.screen()
            .matches(&format!("id:menu-panel >> text:/^{channel}$/"))
            .unwrap(),
        "the menu is for {channel}: {:?}",
        app.screen().texts()
    );
    if program_menu {
        step(app.wait_for("id:menu-panel >> text:Manage favorites…"));
    } else {
        assert!(!app.shows("id:menu-panel >> text:Search"));
        assert!(!app.shows("id:menu-panel >> text:Manage favorites…"));
    }
    step(app.back());
    step(app.wait_for_gone(favorite, Duration::from_secs(2)));
}

const GUIDE_MORE: &str = "id:guide-ui >> label:More options";

#[test]
fn touch_and_pointer_reach_the_menu_without_a_shortcut() {
    let Some(home) = isolated!() else { return };
    let mut fixture = personalization::start(home, Settings::new(), true);
    let app = &mut fixture.app;
    // A desktop: a mouse to right-click with, a touch screen to long-press.
    step(app.set_environment("desktop", |env| {
        env.os = nsh_core::platform::Os::Linux;
        env.idiom = Idiom::Desktop;
    }));
    let opens = iptv_app::player_opens(0);
    // Keys drive the app: they have Menu, so no ⋯; the title keeps its room.
    assert!(!app.screen().matches("id:guide-more").unwrap());
    let room = width(app, "guide-more-slot");
    assert!(room > 0.0);

    // A tap (here on the idle preview, which does nothing else) shows the
    // ⋯ by the focused program, on Local A, in that room; it opens the
    // Menu key's menu.
    step(app.press("id:guide-preview"));
    step(app.wait_for("id:guide-more"));
    assert_eq!(width(app, "guide-more-slot"), room, "no text moves");
    step(app.press(GUIDE_MORE));
    menu_opens_for(app, "Local A", true);
    assert_eq!(iptv_app::player_opens(0), opens, "a menu never plays");

    // Long press and right-click target the channel header or program.
    // Neither gesture starts playback.
    step(app.long_press("id:guide-ui >> label:Local B"));
    menu_opens_for(app, "Local B", false);
    step(app.right_click("id:guide-ui >> label:Local B"));
    menu_opens_for(app, "Local B", false);
    // The ⋯ follows what was tapped: Local B now.
    step(app.press(GUIDE_MORE));
    menu_opens_for(app, "Local B", true);
    // The first row's second slot: the hour airing now (the first is the
    // ended hour before it, which this row cannot replay or focus).
    let cell = "id:guide-ui >> role:cell[No information] >> nth=1";
    step(app.right_click(cell));
    menu_opens_for(app, "Local A", true);
    step(app.long_press(cell));
    menu_opens_for(app, "Local A", true);
    assert_eq!(iptv_app::player_opens(0), opens, "a menu never plays");

    // A key goes back to the remote's view.
    step(app.press_key("down"));
    step(app.wait_for_gone("id:guide-more", Duration::from_secs(2)));

    // The player's channel list: a ⋯ per row for channel actions.
    choose_fullscreen(app);
    step(app.wait_for("id:player-banner"));
    step(app.press_key("m"));
    step(app.wait_for("text:Channels"));
    step(app.press("text:Channels"));
    step(app.wait_for("id:channel-list-more"));
    // Down then Enter played the second row; the first row's ⋯ is pressed.
    step(app.wait_for("text:2  Local B  ▶"));
    // The row's ▶ can show before the player is told to open Local B (a
    // slow runner): count from that open, not before it.
    step(
        app.wait_until("Local B's stream opens", Duration::from_secs(5), |_| {
            iptv_app::player_opens(0) > opens
        }),
    );
    let playing = iptv_app::player_opens(0);
    step(app.press("label:More options for Local A"));
    menu_opens_for(app, "Local A", false);
    step(app.wait(Duration::from_secs(2)));
    assert_eq!(iptv_app::player_opens(0), playing, "a menu never zaps");
    assert!(app.screen().matches("text:2  Local B  ▶").unwrap());
}

#[test]
fn a_tablet_shows_the_more_control_before_any_tap() {
    let Some(home) = isolated!() else { return };
    let mut fixture = personalization::launch(
        home,
        Settings::new(),
        true,
        |config| {
            config.environment.idiom = Idiom::Pad;
            config.environment.os = nsh_core::platform::Os::Android;
        },
        |_| {},
    );
    let app = &mut fixture.app;
    // A tablet is touched first: the ⋯ shows at once, so the first tap
    // need not land on a program (which would play it).
    step(app.wait_for("id:guide-more"));
    step(app.press(GUIDE_MORE));
    menu_opens_for(app, "Local A", true);
    assert_eq!(iptv_app::player_opens(0), 0, "a menu never plays");
    // A key still hides it.
    step(app.press_key("down"));
    step(app.wait_for_gone("id:guide-more", Duration::from_secs(2)));
}

#[test]
fn a_tv_remote_sees_no_more_control() {
    let Some(home) = isolated!() else { return };
    let mut fixture = personalization::launch(
        home,
        Settings::new(),
        true,
        |config| {
            config.environment.idiom = Idiom::Tv;
            config.environment.os = nsh_core::platform::Os::Android;
        },
        |_| {},
    );
    let app = &mut fixture.app;
    // A TV starts with its remote: no ⋯ before or after a key.
    assert!(!app.screen().matches("id:guide-more").unwrap());
    for key in ["right", "down", "right", "left"] {
        step(app.press_key(key));
        assert!(
            !app.screen().matches("id:guide-more").unwrap(),
            "the remote has Menu"
        );
    }
    step(app.press_key("m"));
    step(app.wait_for("id:menu-panel >> text:Manage favorites…"));
    assert!(!app.screen().matches("id:guide-more").unwrap());
}

#[test]
fn the_header_more_control_waits_for_an_inline_channel_reorder() {
    let Some(home) = isolated!() else { return };
    let mut fixture = personalization::start(home, Settings::new(), true);
    let app = &mut fixture.app;
    // A TV reorders inline, in the guide's own columns; touch drives it.
    step(app.set_environment("TV with touch", |env| {
        env.os = nsh_core::platform::Os::Android;
        env.idiom = Idiom::Tv;
    }));
    step(app.press("id:guide-preview"));
    step(app.wait_for("id:guide-more"));
    // The slot is mounted with the ⋯ only: a TV's title keeps that room.
    step(app.press_key("tab"));
    assert!(!app.screen().matches("id:guide-more-slot").unwrap());
    step(app.long_press("id:guide-groups >> text:News"));
    step(app.press("id:menu-panel >> text:Reorder channels"));
    step(app.wait_for("id:organization-screen >> text:Reorder channels"));
    step(app.wait_for("id:guide-row >> text:/Local B/"));
    // Touch drives the app, but the guide takes no pointer while the order
    // is unfinished: no ⋯ that could not be pressed.
    assert!(!app.screen().matches("id:guide-more").unwrap());
}
