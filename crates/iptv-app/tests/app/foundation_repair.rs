//! Existing UI preview/apply/verify/undo with actual durable results.
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
fn inspected_candidate_applies_verifies_and_undoes_without_switching_active_source() {
    let Some(home) = isolated!() else {
        return;
    };
    let directory = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&directory).unwrap();
    let server = Server::start(&[
        (
            "/guide.xml",
            support::guide(
                support::now(),
                &[
                    ("amber.main", "Synthetic Amber", "Synthetic Main Programme"),
                    (
                        "amber.east",
                        "Synthetic Amber East",
                        "Synthetic Unmatched Programme",
                    ),
                    (
                        "birch.east",
                        "Synthetic Birch East",
                        "Synthetic Birch Programme",
                    ),
                ],
            ),
        ),
        (
            "/extra.xml",
            support::guide(
                support::now(),
                &[(
                    "extra.north",
                    "Synthetic Extra",
                    "Synthetic Extra Programme",
                )],
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
        .set("epg.update_on_start", iptv_settings::Value::Bool(false))
        .unwrap();
    settings
        .set("epg.update_interval", iptv_settings::Value::Duration(0))
        .unwrap();
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
    let mut t = T::new(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::now()),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    });
    t.wait("text:/Synthetic Viewer/", 10);
    t.press("text:/^☰  /");
    t.key("left");
    t.key("left");
    t.press("text:Playlists >> nth=0");
    t.press("text:Synthetic viewer");
    t.press("text:Show this playlist");
    t.wait("text:Synthetic Viewer", 10);
    t.press("text:/^☰  /");
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
    t.wait("text:/Synthetic Unmatched Programme/", 10);
    t.press("text:Add this guide choice");
    t.wait("id:guide-repair-screen", 5);
    t.press("text:Keep selecting channels");
    t.gone("id:guide-repair-screen", 5);
    t.key("backspace");
    t.key("backspace");
    t.press("text:Synthetic Birch >> nth=0");
    t.gone("text:Loading…", 10);
    t.press("text:Reuse inspected guide");
    t.gone("text:Loading…", 10);
    t.wait("text:Synthetic Birch East", 10);
    t.press("text:Synthetic Birch East");
    t.wait("text:/Synthetic Birch Programme/", 10);
    t.press("text:Add this guide choice");
    t.wait("id:guide-repair-screen", 5);
    t.press("text:Preview guide changes");
    t.wait("text:Apply guide changes", 10);
    assert!(
        crate::isolated::open_database(directory.join("iptv.sqlite"))
            .unwrap()
            .epg_overrides(sources[1])
            .unwrap()
            .is_empty(),
        "preview does not mutate saved choices"
    );
    t.press("text:Apply guide changes");
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
    let store = crate::isolated::open_database(directory.join("iptv.sqlite")).unwrap();
    assert_eq!(store.epg_overrides(sources[1]).unwrap().len(), 2);
    assert_eq!(store.active_source().unwrap(), Some(sources[0]));
    drop(store);
    t.press("text:Undo guide changes");
    t.wait("text:Guide changes undone", 10);
    assert!(
        crate::isolated::open_database(directory.join("iptv.sqlite"))
            .unwrap()
            .epg_overrides(sources[1])
            .unwrap()
            .is_empty()
    );
}
