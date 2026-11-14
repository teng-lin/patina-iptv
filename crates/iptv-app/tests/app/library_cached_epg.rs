//! Playlist refresh policy does not suppress independently configured guide updates.
use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::{GuideSource, SourcePolicy};
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[test]
fn cached_playlist_still_downloads_its_guide_when_epg_startup_refresh_is_enabled() {
    let Some(home) = isolated!() else { return };
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let text = "#EXTM3U\n#EXTINF:-1 tvg-id=\"a.test\" group-title=\"News\",Cached A\nhttp://stream.example.test/a\n";
    let file = home.path().join("playlist.m3u");
    std::fs::write(&file, text).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide = home.path().join("guide.xml");
    std::fs::write(
        &guide,
        format!(
            "<tv><channel id=\"a.test\"><display-name>Cached A</display-name></channel>\
             <programme channel=\"a.test\" start=\"{}\" stop=\"{}\">\
             <title>Independent guide refresh</title>\
             <desc>Synthetic test programme.</desc></programme></tv>",
            format_xmltv_time(now - 3600),
            format_xmltv_time(now + 3600)
        ),
    )
    .unwrap();
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Synthetic cache".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(file.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(text);
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
                feed: iptv_epg::FeedId(1001),
                provider_index: None,
                url: SecretUrl::new(guide.to_string_lossy()),
                name: Some("Local XMLTV".into()),
                enabled: true,
                shift_minutes: 0,
            }],
        )
        .unwrap();
    drop(store);
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("epg.update_on_start", iptv_settings::Value::Bool(true))
        .unwrap();
    settings
        .set("epg.update_interval", iptv_settings::Value::Duration(0))
        .unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    // An invalid current playlist proves the channel catalogue is read from cache.
    std::fs::write(&file, "#EXTM3U\n").unwrap();
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    app.wait_for_within("text:Cached A", Duration::from_secs(20))
        .unwrap();
    app.wait_for_within("text:Independent guide refresh", Duration::from_secs(20))
        .unwrap();
    let store = crate::isolated::open_database(&path).unwrap();
    assert!(store.import_status(source).unwrap().last_error.is_none());
    assert_eq!(store.import_status(source).unwrap().counts.channels, 1);
    assert!(store.epg_guide(source).unwrap().unwrap().bound_channels > 0);
    drop(store);
    drop(app);
    // Changing the EPG source forces a check even when its periodic interval is off.
    // An unavailable replacement feed must retain both successful caches.
    crate::isolated::open_database(&path)
        .unwrap()
        .set_guide_sources(
            source,
            &[GuideSource {
                feed: iptv_epg::FeedId(1001),
                provider_index: None,
                url: SecretUrl::new(home.path().join("unavailable.xml").to_string_lossy()),
                name: Some("Local XMLTV".into()),
                enabled: true,
                shift_minutes: 0,
            }],
        )
        .unwrap();
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    app.wait_for_within("text:Independent guide refresh", Duration::from_secs(20))
        .unwrap();
    let end = std::time::Instant::now() + crate::isolated::scaled_duration(Duration::from_secs(10));
    loop {
        let store = crate::isolated::open_database(&path).unwrap();
        if store
            .epg_guide(source)
            .unwrap()
            .unwrap()
            .last_error
            .is_some()
        {
            assert!(store.import_status(source).unwrap().last_error.is_none());
            assert!(store.epg_guide(source).unwrap().unwrap().bound_channels > 0);
            break;
        }
        assert!(
            std::time::Instant::now() < end,
            "guide failure was not recorded"
        );
        app.wait(Duration::from_millis(50)).unwrap();
    }
}
