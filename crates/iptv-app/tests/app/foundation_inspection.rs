//! Read-only guide inspection uses off-screen sources and retained feed samples.
use crate::support::{self, Server, T};
use iptv_epg::FeedId;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::{GuideSource, SourcePolicy};
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

#[test]
fn inspect_offscreen_source_and_unmatched_samples_without_changing_saved_choices() {
    let Some(home) = isolated!() else {
        return;
    };
    let directory = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&directory).unwrap();
    let extra_names: Vec<_> = (0..60)
        .map(|index| {
            (
                format!("extra.{index:03}"),
                format!("Synthetic Extra {index:03}"),
            )
        })
        .collect();
    let mut extra: Vec<_> = extra_names
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str(), "Synthetic Extra Page Program"))
        .collect();
    extra.push(("extra.north", "Synthetic Extra", "Synthetic Extra Program"));
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
                ],
            ),
        ),
        ("/extra.xml", support::guide(support::now(), &extra)),
    ]);
    let mut store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
    let mut sources = Vec::new();
    for (name, channel) in [
        ("Synthetic viewer", "Synthetic Viewer"),
        ("Synthetic inspection", "Synthetic Amber"),
    ] {
        let playlist = format!(
            "#EXTM3U\n#EXTINF:-1 tvg-id=\"amber.main\" group-title=\"Generated\",{channel}\nhttp://stream.example.test/generated\n"
        );
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
        .set("epg.update", iptv_settings::Value::choice("on_request"))
        .unwrap();
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
    let mut t = T::new(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    });
    t.wait("text:/Synthetic Viewer/", 10);
    t.app.open_guide_groups().unwrap();
    t.key("left");
    t.key("left");
    t.press("text:Playlists >> nth=0");
    t.press("text:Synthetic viewer");
    t.press("text:Show this playlist");
    t.wait("text:Synthetic Viewer", 10);
    t.app.open_guide_groups().unwrap();
    t.key("left");
    t.key("left");
    t.press("text:Playlists >> nth=0");
    t.press("text:Synthetic inspection");
    t.press("text:Guide sources");
    t.wait("text:Guide sources · Synthetic inspection", 5);
    t.press("text:Update the guide now");
    t.wait("text:Guide updated", 15);
    t.press("text:Inspect guide coverage");
    t.wait("id:guide-inspection-screen", 5);
    t.wait("text:Synthetic Amber", 10);
    t.press("text:Synthetic Amber >> nth=0");
    t.wait("text:Synthetic Amber East", 10);
    t.press("text:Synthetic Amber East");
    t.wait("text:/Synthetic Unmatched Program/", 10);
    server.set(
        "/guide.xml",
        support::guide(
            support::now(),
            &[("amber.east", "Synthetic Amber East", "Changed After Probe")],
        ),
    );
    t.key("backspace");
    t.press("text:Synthetic Amber East");
    t.wait("text:/Synthetic Unmatched Program/", 10);
    assert!(!t.shows("text:/Changed After Probe/"));
    // Close returns to the original source screen; reopening starts a fresh request namespace.
    for _ in 0..3 {
        t.key("backspace");
    }
    t.gone("id:guide-inspection-screen", 5);
    t.press("text:Inspect guide coverage");
    t.wait("text:Inspect a guide link", 10);
    t.enter("Amber");
    t.wait("text:Synthetic Amber", 10);
    t.press("text:Inspect a guide link");
    t.enter(&server.url("/extra.xml"));
    t.wait("text:Synthetic Extra 000", 10);
    // The previous audit's filter must not poison the staged continuation cursor.
    for _ in 0..6 {
        t.key("pagedown");
    }
    for _ in 0..3 {
        t.key("down");
    }
    // Focused actions must actually be mounted inside the list viewport;
    // activating an offscreen row by its retained identity alone is not enough.
    let nodes = t.app.screen().layout_nodes();
    let next = nodes
        .iter()
        .find(|node| {
            !node.hidden
                && node
                    .text
                    .as_ref()
                    .is_some_and(|text| text.text.text == "Next")
        })
        .expect("keyboard focus reveals the Next row");
    let mut parent = next.parent;
    let viewport = loop {
        let node = nodes.iter().find(|node| node.id == parent).unwrap();
        if node.scroll.is_some() {
            break node.bounds;
        }
        assert_ne!(node.parent, parent, "Next belongs to a scrolling list");
        parent = node.parent;
    };
    assert!(
        next.bounds.origin.y.0 >= viewport.origin.y.0
            && next.bounds.origin.y.0 + next.bounds.size.height.0
                <= viewport.origin.y.0 + viewport.size.height.0,
        "the focused Next row is visible inside its viewport"
    );
    t.key("return");
    t.wait("text:Synthetic Extra 050", 10);
    assert!(!t.shows("text:Guide evidence changed; inspect again"));
    t.press("text:Synthetic Extra >> nth=0");
    t.wait("text:/Synthetic Extra Program/", 10);
    let store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
    assert_eq!(store.active_source().unwrap(), Some(sources[0]));
    assert!(store.epg_overrides(sources[1]).unwrap().is_empty());
    assert_eq!(
        store.guide_sources(sources[1]).unwrap().len(),
        1,
        "supplied feed remains a probe"
    );
}
