//! Whole-store snapshots use generated sources and catalogues only.

use iptv_providers::SecretUrl;
use iptv_providers::model::{ContentKind, SourceConfig};
use iptv_store_sqlite::{ChannelQuery, OrderScope, SCHEMA_VERSION, Store};
use rusqlite::Connection;

use crate::common::*;

#[test]
fn wal_snapshot_preserves_credentials_organization_history_and_cached_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.sqlite");
    let archive = dir.path().join("archive.sqlite");
    let mut store = Store::open(&live).unwrap();
    let source = m3u_source("Generated playlist");
    let id = store.add_source(&source).unwrap();
    let catalog = catalog(6, 2, XT_PASSWORD);
    replace(&mut store, id, &catalog);
    let last = catalog.channels.last().unwrap().id.clone();
    store.set_favorite(id, &last, true).unwrap();
    store
        .rename_channel(id, &last, Some("Generated custom name"))
        .unwrap();
    store.move_channel(id, &OrderScope::All, &last, 0).unwrap();
    let group = store.create_custom_group(id, "Generated group").unwrap();
    store.add_to_custom_group(group, &last).unwrap();
    store
        .record_watch(id, &last, ContentKind::Live, 12345, Some(17))
        .unwrap();
    store.set_active_source(id).unwrap();
    let combined_key = format!("source:{}:{}", id.0, last.as_str());
    store.set_all_playlists(true).unwrap();
    store.set_combined_last_channel(&combined_key).unwrap();
    assert_eq!(store.journal_mode().unwrap(), "wal");
    assert!(live.with_extension("sqlite-wal").exists());
    store.snapshot_to(&archive).unwrap();
    // Subsequent writes never leak into the completed snapshot.
    store.rename_source(id, "Changed after backup").unwrap();
    store.set_all_playlists(false).unwrap();
    Store::validate_snapshot(&archive).unwrap();
    let restored = Store::open(&archive).unwrap();
    assert_eq!(restored.source(id).unwrap().unwrap().source, source);
    assert_eq!(restored.active_source().unwrap(), Some(id));
    assert!(restored.all_playlists().unwrap());
    assert_eq!(
        restored.combined_last_channel().unwrap(),
        Some(combined_key)
    );
    assert!(restored.is_favorite(id, &last).unwrap());
    let channels = restored
        .load_channels(id, &ChannelQuery::default())
        .unwrap();
    assert_eq!(channels.len(), 6);
    assert_eq!(channels[0].channel.id, last);
    assert_eq!(channels[0].name, "Generated custom name");
    assert_eq!(restored.custom_groups(id).unwrap()[0].id, group);
    let history = restored.recent(Some(id), None, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].position_secs, Some(17));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(store.snapshot_to(&archive).is_err());
    assert_eq!(
        restored.source(id).unwrap().unwrap().source.name,
        "Generated playlist"
    );
}

#[test]
fn snapshot_validation_rejects_corruption_newer_versions_and_unknown_schema_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.sqlite");
    std::fs::write(&path, b"not a database").unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(Store::validate_snapshot(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::remove_file(&path).unwrap();
    let store = Store::open_in_memory().unwrap();
    store.snapshot_to(&path).unwrap();
    {
        let db = Connection::open(&path).unwrap();
        db.execute(
            "INSERT INTO schema_migrations (version,name,applied_at) VALUES (?1,'future',0)",
            [SCHEMA_VERSION + 1],
        )
        .unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    assert!(Store::validate_snapshot(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::remove_file(&path).unwrap();
    store.snapshot_to(&path).unwrap();
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE unexpected (value TEXT)")
            .unwrap();
    }
    assert!(Store::validate_snapshot(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    store.snapshot_to(&path).unwrap();
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE channel_fts_unexpected (value TEXT)")
            .unwrap();
    }
    assert!(Store::validate_snapshot(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    store.snapshot_to(&path).unwrap();
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO source_credentials (source_id,name,value)
             VALUES (999,'location','http://127.0.0.1/generated.m3u')",
        )
        .unwrap();
    }
    assert!(Store::validate_snapshot(&path).is_err());
}

#[test]
fn local_playlist_paths_rebase_transactionally_and_network_sources_stay_unchanged() {
    let mut store = Store::open_in_memory().unwrap();
    let network = m3u_source("Generated network playlist");
    let network_id = store.add_source(&network).unwrap();
    let mut local = m3u_source("Generated local playlist");
    if let SourceConfig::M3u(config) = &mut local.config {
        config.location = SecretUrl::new("/old/home/playlists/123/list.m3u");
    }
    let local_id = store.add_source(&local).unwrap();
    store
        .rebase_local_playlists("/old/home".as_ref(), "/new/home".as_ref())
        .unwrap();
    assert_eq!(store.source(network_id).unwrap().unwrap().source, network);
    let SourceConfig::M3u(config) = store.source(local_id).unwrap().unwrap().source.config else {
        panic!()
    };
    assert_eq!(
        config.location.expose().replace('\\', "/"),
        "/new/home/playlists/123/list.m3u"
    );
    // The first location is valid but the second traverses out of the root:
    // neither location may be committed after that failure.
    let mut invalid = local.clone();
    if let SourceConfig::M3u(config) = &mut invalid.config {
        config.location = SecretUrl::new("/new/home/../unrelated.m3u");
    }
    store.add_source(&invalid).unwrap();
    assert!(
        store
            .rebase_local_playlists("/new/home".as_ref(), "/third/home".as_ref())
            .is_err()
    );
    let SourceConfig::M3u(config) = store.source(local_id).unwrap().unwrap().source.config else {
        panic!()
    };
    assert_eq!(
        config.location.expose().replace('\\', "/"),
        "/new/home/playlists/123/list.m3u"
    );
}

#[test]
fn validation_accepts_historical_schema_without_migrating_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("historical.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../fixtures/schema_v1.sql"))
        .unwrap();
    db.execute(
        "INSERT INTO schema_migrations (version,name,applied_at) VALUES (1,'provider_data',0)",
        [],
    )
    .unwrap();
    db.pragma_update(None, "user_version", 1).unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    Store::validate_snapshot(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        Store::open(&path).unwrap().schema_version().unwrap(),
        SCHEMA_VERSION
    );
}

#[test]
fn restore_confines_file_urls_and_validates_guide_locations() {
    use iptv_store_sqlite::GuideSource;
    let mut store = Store::open_in_memory().unwrap();
    let mut source = m3u_source("Synthetic local");
    if let SourceConfig::M3u(config) = &mut source.config {
        config.location = SecretUrl::new("file:///old/playlists/list.m3u");
    }
    let id = store.add_source(&source).unwrap();
    let guide = |url: &str| GuideSource {
        feed: iptv_epg::FeedId(1),
        provider_index: Some(0),
        url: SecretUrl::new(url),
        name: None,
        enabled: true,
        shift_minutes: 0,
    };
    for url in [
        "file:///etc/hosts",
        "/etc/hosts",
        "http://127.0.0.1/private",
        "http://10.0.0.1/private",
    ] {
        store.set_guide_sources(id, &[guide(url)]).unwrap();
        assert!(
            store
                .rebase_local_playlists("/old".as_ref(), "/new".as_ref())
                .is_err()
        );
        assert_eq!(
            store.source(id).unwrap().unwrap().source,
            source,
            "failed rebase is atomic"
        );
    }
    store
        .set_guide_sources(id, &[guide("file:///old/playlists/guide.xml")])
        .unwrap();
    store
        .rebase_local_playlists("/old".as_ref(), "/new".as_ref())
        .unwrap();
    assert_eq!(
        store.guide_sources(id).unwrap()[0]
            .url
            .expose()
            .replace('\\', "/"),
        "/new/playlists/guide.xml"
    );
    let SourceConfig::M3u(config) = store.source(id).unwrap().unwrap().source.config else {
        panic!()
    };
    assert_eq!(
        config.location.expose().replace('\\', "/"),
        "/new/playlists/list.m3u"
    );
}

#[test]
fn restore_keeps_explicit_lan_guides_and_derived_xtream_guides() {
    use iptv_store_sqlite::GuideSource;
    for xtream in [false, true] {
        let mut store = Store::open_in_memory().unwrap();
        let mut source = if xtream {
            xtream_source("Synthetic LAN")
        } else {
            m3u_source("Synthetic LAN")
        };
        let url = if let SourceConfig::Xtream(c) = &mut source.config {
            c.server_url = "http://10.0.0.1".into();
            iptv_providers::xtream::XtreamClient::from_credentials(c)
                .unwrap()
                .xmltv_url()
        } else {
            "http://192.168.1.2/guide.xml".into()
        };
        let id = store.add_source(&source).unwrap();
        store
            .set_guide_sources(
                id,
                &[GuideSource {
                    feed: iptv_epg::FeedId(1),
                    provider_index: xtream.then_some(0),
                    url: SecretUrl::new(&url),
                    name: None,
                    enabled: true,
                    shift_minutes: 0,
                }],
            )
            .unwrap();
        store
            .rebase_local_playlists("/old".as_ref(), "/new".as_ref())
            .unwrap();
        assert_eq!(
            store.guide_sources(id).unwrap()[0]
                .url
                .expose()
                .replace('\\', "/"),
            url
        );
    }
}
