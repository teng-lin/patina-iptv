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

#[test]
fn tv_guide_preview_and_shared_bar_keep_the_existing_player_and_route_dpad() {
    let Some(home) = isolated!() else { return };
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
    let mut fixture = personalization::start_with_setup(home, settings, true, move |app| {
        app.observe(move |_, batch| {
            let mut recorded = observed.lock().unwrap();
            for item in &batch.items {
                match item {
                    MountItem::Component { id, name } if name.as_ref() == "tv.player" => {
                        recorded.players.push(*id)
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
