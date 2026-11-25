//! Existing UI preview/apply/verify/undo with actual durable results.
use crate::support::{self, Server, T};
use iptv_epg::FeedId;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::{GuideSource, SourcePolicy};
use nsh_core::{
    EventKind, EventPayload, MountItem, NodeId, Size, ViewKind, ViewProps, element::Root,
    prelude::*,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct PlaybackObserved {
    views: HashMap<NodeId, ViewProps>,
    commands: Vec<NodeId>,
    structural: Vec<NodeId>,
    hidden: Vec<NodeId>,
}

fn assert_playback_unchanged(
    t: &T,
    observed: &Arc<Mutex<PlaybackObserved>>,
    player: NodeId,
    ancestry: &[NodeId],
    command_count: usize,
    opens: u64,
) {
    let state = observed.lock().unwrap();
    assert_eq!(
        state.commands.iter().filter(|id| **id == player).count(),
        command_count,
        "repair sends no player commands"
    );
    assert!(
        !state.structural.iter().any(|id| ancestry.contains(id)),
        "repair never detaches, replaces or reparents the primary surface or its ancestors"
    );
    assert!(
        !state.hidden.iter().any(|id| ancestry.contains(id)),
        "repair never hides the primary surface or its ancestors"
    );
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "repair opens no additional stream"
    );
    assert!(iptv_app::player_has_source(0));
    let nodes = t.app.screen().layout_nodes();
    let players: Vec<_> = nodes
        .iter()
        .filter(|node| node.kind == ViewKind::Native)
        .collect();
    assert_eq!(players.len(), 1);
    assert_eq!(players[0].id, player);
    assert!(!players[0].hidden);
    assert!(
        nodes.iter().any(
            |node| state.views.get(&node.id).is_some_and(|props| matches!(
                props.accessibility.label.as_deref(),
                Some("Synthetic Viewer" | "Synthetic Viewer · Synthetic viewer")
            ) && props
                .accessibility
                .state
                .selected)
        ),
        "focus retains the same viewer channel across source-qualified name presentation"
    );
}

use nsh_engine::EngineConfig;

#[test]
fn inspected_candidate_applies_verifies_and_undoes_during_uninterrupted_viewer_playback() {
    let Some(home) = isolated!() else {
        return;
    };
    repair_journey(home, false);
}

#[test]
fn protected_repair_apply_and_history_undo_recheck_pin_after_preview() {
    let Some(home) = isolated!() else { return };
    repair_journey(home, true);
}

fn pin(t: &mut T, digits: &str) {
    for digit in digits.chars() {
        t.key(&digit.to_string());
    }
    t.press("id:parental-layer >> text:Continue");
}

fn lock(t: &mut T, player: NodeId) {
    t.app.engine().control().dispatch_event(
        player,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "test.parental.lock".into(),
            data: serde_json::Value::Null,
        },
    );
    t.app.settle();
}

fn repair_journey(home: crate::isolated::Home, protected: bool) {
    let directory = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&directory).unwrap();
    let server = Server::start(&[
        (
            "/guide.xml",
            support::guide(
                support::now(),
                &[
                    ("amber.main", "Synthetic Amber", "Synthetic Main Program"),
                    (
                        "amber.east",
                        "Synthetic Amber East",
                        "Synthetic Unmatched Program",
                    ),
                    (
                        "birch.east",
                        "Synthetic Birch East",
                        "Synthetic Birch Program",
                    ),
                ],
            ),
        ),
        (
            "/extra.xml",
            support::guide(
                support::now(),
                &[("extra.north", "Synthetic Extra", "Synthetic Extra Program")],
            ),
        ),
    ]);
    let mut store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
    let mut sources = Vec::new();
    for (name, channel) in [
        ("Synthetic viewer", "Synthetic Viewer"),
        ("Synthetic inspection", "Synthetic Amber"),
    ] {
        let mut playlist = format!(
            "#EXTM3U\n#EXTINF:-1 tvg-id=\"amber.main\" group-title=\"Generated\",{channel}\nhttp://stream.example.test/generated\n"
        );
        if name == "Synthetic inspection" {
            playlist.push_str("#EXTINF:-1 tvg-id=\"birch.main\" group-title=\"Generated\",Synthetic Birch\nhttp://stream.example.test/birch\n");
        }
        let path = home.path().join(format!("{}.m3u", sources.len()));
        std::fs::write(&path, &playlist).unwrap();
        let source = store
            .add_source(&Source {
                name: name.into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new(path.to_string_lossy()),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let parsed = iptv_providers::m3u::parse_m3u(&playlist);
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
        store
            .set_guide_sources(
                source,
                &[GuideSource {
                    feed: FeedId(1),
                    provider_index: None,
                    url: SecretUrl::new(server.url("/guide.xml")),
                    name: Some("Synthetic guide".into()),
                    enabled: true,
                    shift_minutes: 0,
                }],
            )
            .unwrap();
        sources.push(source);
    }
    store.set_active_source(sources[0]).unwrap();
    store.set_all_playlists(false).unwrap();
    drop(store);
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("guide.switch_delay", iptv_settings::Value::ms(0))
        .unwrap();
    settings
        .set("guide.stay_on_guide", iptv_settings::Value::Bool(true))
        .unwrap();
    settings
        .set("epg.update", iptv_settings::Value::choice("on_request"))
        .unwrap();
    if protected {
        settings
            .set(
                "parental.require_pin_for",
                iptv_settings::Value::choice("blocked_content_and_settings"),
            )
            .unwrap();
        settings
            .set(
                "parental.unlock_window",
                iptv_settings::Value::choice("every_time"),
            )
            .unwrap();
    }
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
    let mut t = T::new(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::now()),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    });
    let observed = Arc::new(Mutex::new(PlaybackObserved::default()));
    let changes = observed.clone();
    t.app.observe(move |_, batch| {
        let mut state = changes.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::ViewProps { id, props } => {
                    state.views.insert(*id, (**props).clone());
                }
                MountItem::AccessibilityLabel { id, label } => {
                    state.views.entry(*id).or_default().accessibility.label = label.clone();
                }
                MountItem::ComponentProps { id, .. } => state.commands.push(*id),
                MountItem::Insert { child, .. } => state.structural.push(*child),
                MountItem::Remove { id } | MountItem::Delete { id } => state.structural.push(*id),
                MountItem::ViewStyle { id, style } if style.hidden => state.hidden.push(*id),
                _ => {}
            }
        }
    });
    t.wait("text:/Synthetic Viewer/", 10);
    if protected {
        t.app.open_guide_groups().unwrap();
        t.key("left");
        t.key("left");
        t.press("text:Settings >> nth=0");
        t.wait("id:settings-screen", 5);
        t.press("text:Parental control >> nth=0");
        t.press("text:Set the PIN");
        t.wait("id:parental-layer >> text:Choose a new PIN", 20);
        pin(&mut t, "2468");
        t.wait("id:parental-layer >> text:Confirm new PIN", 20);
        pin(&mut t, "2468");
        t.wait("id:parental-layer >> text:Save your recovery code", 20);
        t.press("id:parental-layer >> text:I saved the code");
        t.wait("id:settings-screen", 5);
        t.press("id:settings-screen >> text:Close");
        t.gone("id:settings-screen", 5);
    }
    t.app.open_guide_groups().unwrap();
    t.key("left");
    t.key("left");
    t.press("text:Playlists >> nth=0");
    t.press("text:Synthetic viewer");
    t.press("text:Show this playlist");
    t.wait("id:guide-row >> text:Synthetic Viewer", 10);
    t.press("id:guide-row >> text:Synthetic Viewer");
    crate::cov_data::until(
        &mut t.app,
        "generated viewer stream active",
        std::time::Duration::from_secs(10),
        |_| iptv_app::player_has_source(0),
    );
    let nodes = t.app.screen().layout_nodes();
    let player = nodes
        .iter()
        .find(|node| node.kind == ViewKind::Native)
        .unwrap()
        .id;
    t.app.engine().control().dispatch_event(
        player,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "state".into(),
            data: serde_json::json!({"state":"ready"}),
        },
    );
    t.app.settle();
    // Keep the viewer's guide open while a different source is repaired.
    if t.shows("id:player-banner") {
        t.key("backspace");
    }
    let nodes = t.app.screen().layout_nodes();
    let mut ancestry = vec![player];
    let mut parent = nodes.iter().find(|node| node.id == player).unwrap().parent;
    while let Some(node) = nodes.iter().find(|node| node.id == parent) {
        ancestry.push(node.id);
        parent = node.parent;
    }
    let opens = iptv_app::player_opens(0);
    let command_count = {
        let mut state = observed.lock().unwrap();
        state.structural.clear();
        state.hidden.clear();
        state.commands.iter().filter(|id| **id == player).count()
    };
    assert_playback_unchanged(&t, &observed, player, &ancestry, command_count, opens);
    t.app.open_guide_groups().unwrap();
    t.key("left");
    t.key("left");
    t.press("text:Playlists >> nth=0");
    t.press("text:Synthetic inspection");
    t.press("text:Guide data");
    t.wait("text:Guide data · Synthetic inspection", 5);
    t.press("text:Update the guide now");
    if protected {
        t.wait("id:parental-layer >> text:Enter parental PIN", 20);
        pin(&mut t, "2468");
    }
    t.wait("text:Guide updated", 15);
    t.press("text:Inspect guide coverage");
    t.wait("id:guide-inspection-screen", 5);
    t.wait("text:Synthetic Amber", 10);
    t.press("text:Synthetic Amber >> nth=0");
    t.wait("text:Synthetic Amber East", 10);
    t.press("text:Synthetic Amber East");
    t.wait("text:/Synthetic Unmatched Program/", 10);
    t.press("text:Add this guide choice");
    t.wait("id:guide-repair-screen", 5);
    t.press("text:Keep selecting channels");
    t.gone("id:guide-repair-screen", 5);
    t.key("backspace");
    t.key("backspace");
    t.press("text:Synthetic Birch >> nth=0");
    t.gone("id:guide-inspection-screen >> text:Loading…", 10);
    t.press("text:Reuse inspected guide");
    t.gone("id:guide-inspection-screen >> text:Loading…", 10);
    t.wait("text:Synthetic Birch East", 10);
    t.press("text:Synthetic Birch East");
    t.wait("text:/Synthetic Birch Program/", 10);
    t.press("text:Add this guide choice");
    t.wait("id:guide-repair-screen", 5);
    t.press("text:Preview guide changes");
    t.wait("text:Apply guide changes", 10);
    assert_playback_unchanged(&t, &observed, player, &ancestry, command_count, opens);
    assert!(
        crate::isolated::open_database(directory.join("iptv.sqlite"))
            .unwrap()
            .epg_overrides(sources[1])
            .unwrap()
            .is_empty(),
        "preview does not mutate saved choices"
    );
    let durable = || {
        let store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
        (
            store.epg_overrides(sources[1]).unwrap(),
            store.recent_guide_repairs(sources[1], 20).unwrap(),
        )
    };
    if protected {
        let before = durable();
        assert!(before.0.is_empty() && before.1.is_empty());
        // Preview grants no authority: revoke the earlier source-update
        // grant after the immutable plan has already been produced.
        lock(&mut t, player);
        t.press("text:Apply guide changes");
        t.wait("id:parental-layer >> text:Enter parental PIN", 20);
        t.press("id:parental-cancel");
        assert_eq!(durable(), before);
        // Re-preview is still read-only and available while locked.
        t.press("text:Preview guide changes");
        t.wait("text:Apply guide changes", 10);
        assert!(!t.shows("id:parental-layer >> text:Enter parental PIN"));
        t.press("text:Apply guide changes");
        t.wait("id:parental-layer >> text:Enter parental PIN", 20);
        pin(&mut t, "2469");
        t.wait(
            "id:parental-layer >> text:Incorrect PIN or recovery code",
            20,
        );
        assert_eq!(durable(), before);
        pin(&mut t, "2468");
    } else {
        t.press("text:Apply guide changes");
    }
    t.wait("text:Guide choices saved; verification pending", 10);
    crate::cov_data::until(
        &mut t.app,
        "configured staged repair refresh",
        std::time::Duration::from_secs(15),
        |_| {
            let store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
            store
                .load_epg(
                    sources[1],
                    &iptv_store_sqlite::EpgWindow::from(support::now(), 1),
                )
                .unwrap()
                .bindings()
                .any(|(_, reference)| reference.epg_id == "amber.east")
        },
    );
    t.press("text:Verify guide changes");
    t.wait("text:Guide coverage verified", 10);
    assert_playback_unchanged(&t, &observed, player, &ancestry, command_count, opens);
    let store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
    assert_eq!(store.epg_overrides(sources[1]).unwrap().len(), 2);
    assert_eq!(store.active_source().unwrap(), Some(sources[0]));
    drop(store);
    if protected {
        // Re-open a durable history entry instead of relying on the
        // just-applied screen's retained outcome or authorization.
        t.key("backspace");
        t.press("text:Recent guide repairs");
        t.wait("text:/^History · /", 10);
        t.press("text:/^History · /");
        t.wait("text:Undo guide changes", 10);
        let before = durable();
        assert_eq!(before.0.len(), 2);
        assert_eq!(before.1.len(), 1);
        assert!(!before.1[0].undone);
        lock(&mut t, player);
        t.press("text:Undo guide changes");
        t.wait("id:parental-layer >> text:Enter parental PIN", 20);
        t.press("id:parental-cancel");
        assert_eq!(durable(), before);
        t.press("text:Undo guide changes");
        t.wait("id:parental-layer >> text:Enter parental PIN", 20);
        pin(&mut t, "2469");
        t.wait(
            "id:parental-layer >> text:Incorrect PIN or recovery code",
            20,
        );
        assert_eq!(durable(), before);
        pin(&mut t, "2468");
    } else {
        t.press("text:Undo guide changes");
    }
    t.wait("text:Guide changes undone", 10);
    assert_playback_unchanged(&t, &observed, player, &ancestry, command_count, opens);
    assert!(
        crate::isolated::open_database(directory.join("iptv.sqlite"))
            .unwrap()
            .epg_overrides(sources[1])
            .unwrap()
            .is_empty()
    );
    assert!(
        durable().1[0].undone,
        "Undo updates the durable history entry"
    );
}
