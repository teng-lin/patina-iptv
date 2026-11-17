//! Shared pointer/TV controls and guide preview retain one native player.
use crate::personalization::{self, step};
use iptv_settings::{Settings, Value};
use nsh_core::{MountItem, NodeId, ViewKind, platform::Idiom};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Lifetime {
    players: Vec<NodeId>,
    removed: Vec<NodeId>,
    inserts: HashMap<NodeId, usize>,
    sources: Vec<String>,
}
fn selected(app: &crate::Headless, id: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:{id}"))
        .is_ok_and(|s| s.selected)
}
fn persistent(app: &crate::Headless, lifetime: &Arc<Mutex<Lifetime>>, player: NodeId) {
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

fn preview_fixture(
    home: crate::isolated::Home,
) -> (personalization::Fixture, Arc<Mutex<Lifetime>>) {
    let lifetime: Arc<Mutex<Lifetime>> = Arc::default();
    let observed = lifetime.clone();
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
    let fixture = personalization::start_with_setup(home, settings, true, move |app| {
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
                    MountItem::Remove { id } | MountItem::Delete { id } => {
                        recorded.removed.push(*id)
                    }
                    _ => {}
                }
            }
        });
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
    // Live playback disables seek buttons, so Left skips Rewind to Mute.
    step(app.press_key("left"));
    assert!(selected(app, "play-bar-mute"));
    step(app.press_key("right"));
    assert!(selected(app, "play-bar-play-pause"));
    step(app.press_key("right"));
    assert!(selected(app, "play-bar-record"));
    // Down over live playback must keep focus in the bar, not zap channels.
    step(app.press_key("down"));
    assert!(selected(app, "play-bar-record"));
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
    step(app.press_key("g"));
    assert!(
        !selected(app, "guide-preview"),
        "Overlay returns focus to a visible guide row"
    );
    step(app.press_key("f"));
    step(app.press_key("stop"));
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
    step(app.set_environment("TV remote", |env| env.idiom = Idiom::Tv));
    step(app.set_viewport(nsh_core::Size::new(
        nsh_core::prelude::px(960.0),
        nsh_core::prelude::px(540.0),
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
        settings.bounds.origin.y.0 + settings.bounds.size.height.0 <= 540.0,
        "Settings is fully inside the TV viewport: {:?}",
        settings.bounds
    );
}

#[test]
fn sidebar_up_reaches_preview_on_desktop_and_tv_without_reopening_video() {
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
        step(app.press("id:guide-groups >> text:Favourites"));
        step(app.press("id:guide-groups >> text:All channels"));
        step(app.press_key("right"));
        step(app.press_key("down"));
        step(app.press_key("up"));
        // Guide A is focused while B keeps playing. Up must cancel A's pending autoplay.
        step(app.press_key("up"));
        assert!(selected(app, "guide-preview"), "first guide row, {idiom:?}");
        step(app.run_for(Duration::from_secs(3)));
        assert_eq!(iptv_app::player_opens(0), opens);
        step(app.press_key("down"));
        assert!(!selected(app, "guide-preview"));
        step(app.press("text:/^☰  /"));
        step(app.press("id:guide-groups >> text:Favourites"));
        let groups_width = width(app, "guide-groups");
        let rail_width = width(app, "navigation-rail");
        step(app.press_key("up"));
        assert!(selected(app, "guide-preview"), "Favourites, {idiom:?}");
        assert_eq!(width(app, "guide-groups"), groups_width);
        assert_eq!(width(app, "navigation-rail"), rail_width);
        step(app.press_key("down"));
        step(app.press_key("up"));
        step(app.back());
        assert!(!selected(app, "guide-preview"));
        // Left enters Search; its expanded sidebar remains in place during preview focus.
        step(app.press_key("left"));
        let rail_width = width(app, "navigation-rail");
        assert!(rail_width > 100.0, "expanded rail");
        step(app.press_key("up"));
        assert!(selected(app, "guide-preview"), "Search, {idiom:?}");
        assert_eq!(width(app, "navigation-rail"), rail_width);
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
