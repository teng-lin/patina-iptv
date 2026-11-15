//! The actual guide context menu opens independent players without tuning main.
#![cfg(any(target_os = "linux", target_os = "macos"))]
use crate::personalization::step;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::SourcePolicy;
use nsh_core::{
    EventKind, EventPayload, MountItem, NodeId, Point, Size, element::Root, prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Props = Arc<Mutex<HashMap<NodeId, HashMap<String, Value>>>>;

fn player_node(props: &Props, surface: usize) -> NodeId {
    let props = props.lock().unwrap();
    *props
        .iter()
        .find(|(_, values)| values.get("popupSurface") == Some(&json!(surface)))
        .expect("the persistent player for this surface")
        .0
}

fn prop(props: &Props, surface: usize, name: &str) -> Value {
    let id = player_node(props, surface);
    props.lock().unwrap()[&id][name].clone()
}

#[test]
fn guide_context_menu_opens_auxiliary_players_and_browsing_keeps_main_playback() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("guide.stay_on_guide", iptv_settings::Value::Bool(true))
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated guide".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("http://playlist.example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(crate::personalization::M3U);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    store.set_active_source(source).unwrap();
    drop(store);
    let mut config = EngineConfig::new(Root::of::<iptv_app::App>());
    config.viewport = Size::new(px(1920.0), px(1080.0));
    config.environment.os = if cfg!(target_os = "macos") {
        nsh_core::platform::Os::MacOs
    } else {
        nsh_core::platform::Os::Linux
    };
    let mut app = crate::Headless::start(config).unwrap();
    let props: Props = Arc::default();
    let observed = props.clone();
    app.observe(move |_, batch| {
        let mut observed = observed.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::ComponentProps { id, props } => {
                    let entry = observed.entry(*id).or_default();
                    for (name, value) in props {
                        entry.insert(name.to_string(), value.clone().unwrap_or(Value::Null));
                    }
                }
                MountItem::Delete { id } => {
                    observed.remove(id);
                }
                _ => {}
            }
        }
    });
    step(app.wait_for("text:Local A"));
    step(app.press("text:Local A"));
    step(
        app.wait_until("main preview plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    // A context-click must select B without watching it in the main player.
    step(app.right_click("id:guide-ui >> text:Local B"));
    step(app.wait_for("text:Watch in new window"));
    let main_opens = iptv_app::player_opens(0);
    step(app.press("text:Watch in new window"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-1").unwrap().is_some());
    assert!(iptv_app::player_has_source(1));
    step(app.wait_for("label:Play or pause"));
    assert!(app.screen().matches("label:Subtitles and tracks").unwrap());
    assert!(app.screen().matches("label:Full screen").unwrap());
    let main_source = prop(&props, 0, "source");
    let auxiliary_source = prop(&props, 1, "source");
    let auxiliary_node = player_node(&props, 1);
    let main_mode = app.window_state("main").unwrap().unwrap().mode;
    step(app.press("label:Full screen"));
    assert_eq!(
        app.window_state("player-window-1").unwrap().unwrap().mode,
        nsh_core::desktop::WindowMode::FullScreen
    );
    assert_eq!(app.window_state("main").unwrap().unwrap().mode, main_mode);
    step(app.press("label:Full screen"));

    // These are the original player's controls, rather than a separate toolbar.
    step(app.press("label:Play or pause"));
    app.settle();
    assert_eq!(prop(&props, 1, "paused"), json!(true));
    assert_eq!(prop(&props, 0, "paused"), json!(false));
    assert_eq!(iptv_app::player_opens(0), main_opens);
    step(app.press("label:Play or pause"));
    app.settle();
    assert_eq!(prop(&props, 1, "paused"), json!(false));

    // Native track events and the original chooser route back to B only.
    app.engine().control().dispatch_event(
        auxiliary_node,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "tracks".into(),
            data: json!({
                "video": [],
                "audio": [
                    {"id": "a1", "label": "First synthetic audio"},
                    {"id": "a2", "label": "Second synthetic audio"}
                ],
                "text": [],
                "selected": {"video": "auto", "audio": "a1", "text": "off"}
            }),
        },
    );
    app.settle();
    step(app.press("label:Subtitles and tracks"));
    step(app.wait_for("text:/Second synthetic audio/"));
    step(app.press("text:/Second synthetic audio/"));
    app.settle();
    assert_eq!(prop(&props, 1, "audioTrack"), json!("a2"));
    assert_eq!(prop(&props, 0, "audioTrack"), json!("auto"));

    // Drag the original volume control in B. Other players keep their level.
    let volume_bar = app
        .screen()
        .layout_nodes()
        .into_iter()
        .rev()
        .find(|node| {
            node.text.is_none()
                && !node.hidden
                && node.bounds.size.width.0 == 96.0
                && node.bounds.size.height.0 == 24.0
        })
        .expect("B's volume touch area")
        .id;
    step(app.drag(
        &format!("view:{volume_bar}"),
        Point::new(px(-20.0), px(0.0)),
    ));
    app.settle();
    assert!(prop(&props, 1, "volume").as_u64().unwrap() < 100);
    assert_eq!(prop(&props, 0, "volume"), json!(100));
    assert_eq!(prop(&props, 0, "muted"), json!(true));
    assert_eq!(prop(&props, 1, "muted"), json!(false));

    // Main Mute must reclaim audio even if its stored volume was already
    // unmuted when B became audible. The slider must also reclaim audio.
    step(app.press("id:guide-ui >> label:Local A"));
    step(app.hover("label:Video >> nth=0"));
    step(app.wait_for("id:play-bar-layer >> label:Mute >> nth=0"));
    step(app.press("id:play-bar-layer >> label:Mute >> nth=0"));
    app.settle();
    assert_eq!(prop(&props, 0, "muted"), json!(false));
    assert_eq!(prop(&props, 1, "muted"), json!(true));
    step(app.hover("id:auxiliary-player-1 >> label:Video"));
    step(app.press("id:auxiliary-player-1 >> label:Mute"));
    app.settle();
    assert_eq!(prop(&props, 0, "muted"), json!(true));
    let nodes = app.screen().layout_nodes();
    let main_volume = nodes
        .iter()
        .rev()
        .find(|node| {
            if node.text.is_some()
                || node.hidden
                || node.bounds.size.width.0 != 96.0
                || node.bounds.size.height.0 != 24.0
            {
                return false;
            }
            let mut ancestor = *node;
            while let Some(parent) = nodes
                .iter()
                .find(|p| p.id == ancestor.parent && p.id != ancestor.id)
            {
                if parent.test_id.as_deref() == Some("auxiliary-player-1") {
                    return false;
                }
                ancestor = parent;
            }
            true
        })
        .unwrap()
        .id;
    step(app.drag(
        &format!("view:{main_volume}"),
        Point::new(px(-20.0), px(0.0)),
    ));
    app.settle();
    assert_eq!(prop(&props, 0, "muted"), json!(false));
    assert_eq!(prop(&props, 1, "muted"), json!(true));
    step(app.press("id:play-bar-layer >> label:TV guide >> nth=0"));
    assert_eq!(prop(&props, 0, "source"), main_source);
    assert_eq!(prop(&props, 1, "source"), auxiliary_source);
    assert_eq!(player_node(&props, 1), auxiliary_node);

    // Channel changes use the auxiliary runtime slot, including connection
    // accounting; aliasing its stream into the original controls is not enough.
    let auxiliary_opens = iptv_app::player_opens(1);
    step(app.press("label:Channel list"));
    step(app.press("id:auxiliary-player-1 >> id:channel-list >> text:/^◂/"));
    step(app.press("id:auxiliary-player-1 >> id:channel-list-groups >> text:Sport"));
    assert!(
        app.screen().matches("id:guide-ui >> text:Local B").unwrap(),
        "browsing Sport in B must preserve the main guide's All channels list"
    );
    assert!(
        !app.screen()
            .matches("id:auxiliary-player-1 >> id:channel-list >> text:/Local B/")
            .unwrap()
    );
    step(app.press("id:auxiliary-player-1 >> id:channel-list-groups >> text:All channels"));
    step(app.wait_for("id:auxiliary-player-1 >> id:channel-list >> text:/Local A/"));
    step(app.press("id:auxiliary-player-1 >> id:channel-list >> text:/Local A/"));
    step(
        app.wait_until("auxiliary channel changed", Duration::from_secs(5), |_| {
            iptv_app::player_opens(1) > auxiliary_opens
        }),
    );
    assert_eq!(iptv_app::player_opens(0), main_opens);
    step(app.press("id:auxiliary-player-1 >> label:Video"));
    step(app.wait_for("label:Channel list"));
    step(app.press("label:Channel list"));
    step(app.wait_for("id:auxiliary-player-1 >> id:channel-list >> text:/Local B/"));
    step(app.press("id:auxiliary-player-1 >> id:channel-list >> text:/Local B/"));
    app.idle(Duration::from_millis(1200)).unwrap();

    assert_eq!(
        iptv_app::player_opens(0),
        main_opens,
        "opening B must not retune A"
    );
    assert!(!app.screen().matches("id:multiview-tile").unwrap());
    step(app.right_click("id:guide-ui >> text:Local C"));
    step(app.wait_for("text:Watch in new window"));
    step(app.press("text:Watch in new window"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-2").unwrap().is_some());
    assert!(iptv_app::player_has_source(1) && iptv_app::player_has_source(2));
    let third_source = prop(&props, 2, "source");
    let third_node = player_node(&props, 2);
    assert_eq!(iptv_app::player_opens(0), main_opens);
    // Opening an existing auxiliary channel must not start another player.
    let second_opens = iptv_app::player_opens(1);
    step(app.right_click("id:guide-ui >> text:Local B"));
    step(app.wait_for("text:Watch in new window"));
    step(app.press("text:Watch in new window"));
    assert_eq!(iptv_app::player_opens(1), second_opens);
    step(app.press_window_close("player-window-1"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-1").unwrap().is_none());
    assert!(app.window_state("player-window-2").unwrap().is_some());
    assert!(!iptv_app::player_has_source(1));
    assert!(iptv_app::player_has_source(2));
    assert_eq!(iptv_app::player_opens(0), main_opens);
    assert_eq!(prop(&props, 0, "source"), main_source);
    assert_eq!(prop(&props, 2, "source"), third_source);
    assert_eq!(player_node(&props, 2), third_node);

    // Reopen B: reuse its free surface with a fresh session/window identity.
    step(app.right_click("id:guide-ui >> text:Local B"));
    step(app.wait_for("text:Watch in new window"));
    step(app.press("text:Watch in new window"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-3").unwrap().is_some());
    assert!(app.window_state("player-window-1").unwrap().is_none());
    assert_ne!(player_node(&props, 1), auxiliary_node);
    assert_eq!(player_node(&props, 2), third_node);
    assert_eq!(iptv_app::player_opens(0), main_opens);
    // Guide disposes the window-local controls during their own press.
    step(app.hover("id:auxiliary-player-3 >> label:Video"));
    step(app.wait_for("id:auxiliary-player-3 >> label:TV guide"));
    step(app.press("id:auxiliary-player-3 >> label:TV guide"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(app.window_state("player-window-3").unwrap().is_none());
    step(app.press_window_close("player-window-2"));
    app.idle(Duration::from_millis(300)).unwrap();
    // Check actual native-player deletion. The debug source counter can
    // retain its last value if unmount cancels a pending null-prop update.
    assert!(!props.lock().unwrap().values().any(|values| {
        matches!(
            values.get("popupSurface").and_then(Value::as_u64),
            Some(1 | 2)
        )
    }));
    assert!(iptv_app::player_has_source(0));
}
