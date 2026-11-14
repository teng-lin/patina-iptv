//! The native accessibility props must exclude opacity-only UI and modal
//! underlays. Headless pointer/text selectors deliberately still see those
//! mounted nodes, so inspect the actual native props and their ancestors.
use crate::personalization::{self, Fixture, step};
use iptv_settings::{Settings, Value};
use nsh_core::{MountBatch, MountItem, NodeId, ViewKind, ViewProps, platform::Idiom};
use nsh_testing::Headless;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct NativeProps {
    views: HashMap<NodeId, ViewProps>,
    players: Vec<NodeId>,
    removed: Vec<NodeId>,
    inserts: HashMap<NodeId, usize>,
}

#[derive(Clone, Default)]
struct Exposure(Arc<Mutex<NativeProps>>);

impl Exposure {
    fn record(&self, batch: &MountBatch) {
        let mut native = self.0.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::ViewProps { id, props } => {
                    native.views.insert(*id, (**props).clone());
                }
                MountItem::Component { id, name } if name.as_ref() == "tv.player" => {
                    native.players.push(*id)
                }
                MountItem::Insert { child, .. } => *native.inserts.entry(*child).or_default() += 1,
                MountItem::Remove { id } | MountItem::Delete { id } => native.removed.push(*id),
                _ => {}
            }
        }
    }

    #[track_caller]
    fn shown(&self, app: &Headless, test_id: &str, expected: bool) {
        let nodes = app.screen().layout_nodes();
        let mut node = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(test_id))
            .unwrap_or_else(|| panic!("{test_id} is not mounted"));
        let native = self.0.lock().unwrap();
        assert!(
            native.views.contains_key(&node.id),
            "observed the named container's native props"
        );
        let mut exposed = true;
        loop {
            // A plain ancestor with default props may have no ViewProps
            // item. Its accessibility-hidden value is false by default.
            exposed &= !native
                .views
                .get(&node.id)
                .is_some_and(|props| props.accessibility.hidden)
                && !node.hidden;
            let Some(parent) = nodes.iter().find(|parent| parent.id == node.parent) else {
                break;
            };
            node = parent;
        }
        assert_eq!(
            exposed, expected,
            "{test_id}: effective accessibility exposure"
        );
    }

    #[track_caller]
    fn selected_channel(&self, app: &Headless, name: &str) {
        self.shown(app, "guide-ui", true);
        let nodes = app.screen().layout_nodes();
        let guide = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("guide-ui"))
            .unwrap()
            .id;
        let native = self.0.lock().unwrap();
        assert!(
            nodes.iter().any(|node| {
                if !native.views.get(&node.id).is_some_and(|props| {
                    props.accessibility.label.as_deref() == Some(name)
                        && props.accessibility.state.selected
                }) {
                    return false;
                }
                let mut ancestor = node.parent;
                while let Some(parent) = nodes.iter().find(|parent| parent.id == ancestor) {
                    if parent.id == guide {
                        return true;
                    }
                    ancestor = parent.parent;
                }
                false
            }),
            "the mounted guide's content focus should remain on {name}"
        );
    }

    #[track_caller]
    fn persistent_player(&self, app: &Headless, id: NodeId) {
        let native = self.0.lock().unwrap();
        assert_eq!(native.players, [id], "one persistent player component");
        assert!(
            !native.removed.contains(&id),
            "the player was never detached"
        );
        assert_eq!(
            native.inserts.get(&id),
            Some(&1),
            "the player was never reparented"
        );
        assert!(
            app.screen()
                .layout_nodes()
                .iter()
                .any(|node| node.id == id && !node.hidden)
        );
    }
}

fn fixture(home: crate::isolated::Home) -> (Fixture, Exposure) {
    let exposure = Exposure::default();
    let observed = exposure.clone();
    let mut settings = Settings::new();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    let fixture = personalization::start_with_setup(home, settings, true, move |app| {
        app.observe(move |_, batch| observed.record(batch));
    });
    (fixture, exposure)
}

#[test]
fn inactive_player_ui_stays_out_of_accessibility_and_returns_without_recreating_video() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture(home);
    let app = &mut fixture.app;
    step(app.set_environment("a TV with TalkBack", |env| {
        env.idiom = Idiom::Tv;
        env.screen_reader = true;
    }));
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.kind == ViewKind::Native)
        .unwrap()
        .id;
    exposure.shown(app, "guide-ui", true);
    for inactive in [
        "video",
        "channel-list-layer",
        "chooser-layer",
        "favorites-editor",
        "menu-layer",
        "description-layer",
        "player-panels",
    ] {
        exposure.shown(app, inactive, false);
    }
    step(app.press_key("down"));
    exposure.selected_channel(app, "Local B");
    step(app.press_key("enter"));
    step(app.wait_for("text:▶"));
    let opens = iptv_app::player_opens(0);
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner"));
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "video", true);
    exposure.shown(app, "player-panels", true);
    exposure.shown(app, "player-top-bar", false);
    exposure.shown(app, "player-controls", false);
    step(app.run_for(Duration::from_secs(4)));
    exposure.shown(app, "player-panels", false);
    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
    exposure.shown(app, "player-card", true);
    exposure.shown(app, "player-controls", false);
    step(app.press_key("m"));
    exposure.shown(app, "player-controls", true);
    step(app.press("text:Picture size · Normal"));
    exposure.shown(app, "chooser-layer", true);
    exposure.shown(app, "player-panels", false);
    exposure.shown(app, "video", false);
    step(app.back());
    exposure.shown(app, "chooser-layer", false);
    exposure.shown(app, "player-controls", true);
    step(app.back());
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    exposure.shown(app, "video", false);
    exposure.selected_channel(app, "Local B");
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "opening UI does not reopen the stream"
    );
    exposure.persistent_player(app, player);
}

#[test]
fn modal_screens_and_editors_hide_only_their_underlay_and_restore_it_on_back() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture(home);
    let app = &mut fixture.app;
    step(app.press("text:/^☰  /"));
    step(app.press_key("left"));
    step(app.press("label:Settings"));
    step(app.wait_for("id:settings-screen"));
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "settings-screen", true);
    exposure.shown(app, "settings-content", true);
    step(app.press_key("right"));
    for _ in 0..15 {
        if app.screen().matches("text:Language").unwrap() && app.press("text:Language").is_ok() {
            break;
        }
        step(app.press_key("down"));
    }
    step(app.wait_for("id:settings-editor"));
    exposure.shown(app, "settings-editor", true);
    exposure.shown(app, "settings-content", false);
    step(app.back());
    exposure.shown(app, "settings-content", true);
    step(app.back());
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    step(app.press("label:Playlists"));
    step(app.wait_for("id:playlists-screen"));
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "playlists-list", true);
    step(app.press("text:Synthetic personalization"));
    exposure.shown(app, "playlists-list", false);
    step(app.back());
    exposure.shown(app, "playlists-list", true);
    step(app.press("id:playlists-screen >> text:+ Add playlist"));
    step(app.wait_for("text:Playlist link"));
    exposure.shown(app, "playlists-screen", false);
    // Settings alerts are siblings of its screen and can outlive it. A
    // higher opaque screen must hide the whole layer, including an alert.
    exposure.shown(app, "settings-layer", false);
    step(app.back());
    exposure.shown(app, "playlists-screen", true);
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    exposure.shown(app, "settings-layer", true);
}

#[test]
fn a_popup_hides_its_underlay_while_the_pointer_bar_keeps_its_visible_controls() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture(home);
    let app = &mut fixture.app;
    step(app.press_key("down"));
    step(app.press_key("m"));
    exposure.shown(app, "menu-panel", true);
    exposure.shown(app, "guide-ui", false);
    step(app.press("text:Manage favourites…"));
    exposure.shown(app, "favorites-editor", true);
    exposure.shown(app, "menu-layer", false);
    exposure.shown(app, "guide-ui", false);
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local B");
    step(app.press_key("enter"));
    step(app.press_key("enter"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Picture size"));
    exposure.shown(app, "play-bar-layer", true);
    step(app.press("label:Picture size"));
    exposure.shown(app, "chooser-layer", true);
    exposure.shown(app, "play-bar-layer", true);
    exposure.shown(app, "video", false);
    step(app.press("label:Picture size"));
    exposure.shown(app, "chooser-layer", false);
    step(app.press("label:Programme details"));
    exposure.shown(app, "description-layer", true);
    exposure.shown(app, "menu-layer", true);
    exposure.shown(app, "menu-panel", false);
    exposure.shown(app, "play-bar-layer", true);
    step(app.press("label:Programme details"));
    exposure.shown(app, "description-layer", false);
    exposure.shown(app, "menu-layer", false);
    step(app.press("label:Guide"));
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local B");
}
