//! Whole-store snapshots use generated sources and catalogs only.

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

/// Stores a guide whose programs (with descriptions) run from an hour ago
/// for three days, bound to playlist channel `k1`, fetched at `now`.
fn stored_guide(store: &mut Store, source: iptv_store_sqlite::SourceId, now: i64) {
    use iptv_epg::{
        ChannelKey, EpgRef, FeedId, GuideStore, MatchKind, SourceBuilder, StoreOptions,
        XmltvProgram,
    };
    const H: i64 = 3600;
    let mut feed = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), now);
    for hour in -1..72 {
        let start = now + hour * H;
        feed.push_program(XmltvProgram {
            channel: "generated.example".into(),
            start,
            stop: Some(start + H),
            title: format!("Generated program {hour}"),
            desc: Some("A generated description that takes some room. ".repeat(8)),
            ..XmltvProgram::default()
        });
    }
    let mut guide = GuideStore::new();
    guide.replace_source(feed.finish());
    guide.set_matches(
        [(
            ChannelKey::from("k1"),
            vec![EpgRef {
                source: FeedId(1),
                epg_id: "generated.example".into(),
                kind: MatchKind::TvgId,
            }],
        )]
        .into_iter()
        .collect(),
    );
    let meta = iptv_store_sqlite::EpgFeedMeta {
        config: iptv_import::EpgSourceConfig::new(
            FeedId(1),
            SecretUrl::new("http://epg.example.com/generated.xml"),
        ),
        fetched_at: now,
        etag: None,
        last_modified: None,
        bytes: 1,
    };
    crate::staged::replace_epg(store, source, &guide, &[meta], 7).unwrap();
}

fn rows(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn snapshot_drops_downloaded_programs_but_keeps_guide_setup_and_reads_as_never_downloaded() {
    use iptv_import::{Decision, RefreshPolicy, RefreshReason, Trigger, decide};
    use iptv_store_sqlite::GuideSource;
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.sqlite");
    let archive = dir.path().join("archive.sqlite");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut store = Store::open(&live).unwrap();
    let id = store.add_source(&m3u_source("Generated playlist")).unwrap();
    store
        .set_guide_sources(
            id,
            &[GuideSource {
                feed: iptv_epg::FeedId(1),
                provider_index: None,
                url: SecretUrl::new("http://epg.example.com/generated.xml"),
                name: Some("Generated guide".into()),
                enabled: true,
                shift_minutes: 30,
            }],
        )
        .unwrap();
    stored_guide(&mut store, id, now);
    Connection::open(&live)
        .unwrap()
        .execute(
            "INSERT INTO guide_repair_commits (id, source_id, source_incarnation, created_at,
                 expires_at, payload)
             VALUES ('generated-repair', ?1, 'generated', ?2, ?3, '{}')",
            rusqlite::params![id.0, now, now + 86_400],
        )
        .unwrap();
    let before = store.epg_guide(id).unwrap().unwrap();
    assert!(before.replaced_at.is_some_and(|at| at >= now));
    // A full copy (the live file after a checkpoint) for the size comparison.
    let full = dir.path().join("full.sqlite");
    Connection::open(&live)
        .unwrap()
        .execute("VACUUM INTO ?1", [full.to_str().unwrap()])
        .unwrap();
    store.snapshot_to(&archive).unwrap();
    Store::validate_snapshot(&archive).unwrap();

    let db = Connection::open(&archive).unwrap();
    let live_db = Connection::open(&live).unwrap();
    assert_eq!(rows(&db, "epg_programs"), 0);
    assert_eq!(rows(&db, "epg_descriptions"), 0);
    assert_eq!(rows(&live_db, "epg_programs"), 73, "the live guide stays");
    assert_eq!(rows(&live_db, "epg_descriptions"), 73);
    for table in [
        "guide_sources",
        "guide_feed_incarnations",
        "guide_repair_commits",
        "epg_guides",
        "epg_feeds",
        "epg_channels",
        "epg_bindings",
        "epg_feed_channels",
        "epg_feed_results",
        "store_meta",
    ] {
        assert_eq!(rows(&db, table), rows(&live_db, table), "{table}");
    }
    for table in [
        "guide_sources",
        "guide_feed_incarnations",
        "guide_repair_commits",
        "epg_bindings",
    ] {
        assert_eq!(rows(&db, table), 1, "{table}");
    }
    drop((db, live_db));

    let restored = Store::open(&archive).unwrap();
    assert_eq!(
        restored.guide_sources(id).unwrap(),
        store.guide_sources(id).unwrap()
    );
    let guide = restored.epg_guide(id).unwrap().unwrap();
    assert_eq!((guide.replaced_at, guide.ends_at), (None, None));
    assert_eq!(guide.attempted_at, None);
    assert_eq!(guide.match_fingerprint, before.match_fingerprint);
    assert_eq!(guide.feeds, before.feeds, "the feeds' settings stay");
    // Downloaded again at the first start, even with updates only on request.
    let manual_only = RefreshPolicy {
        interval: None,
        on_start: false,
        ..RefreshPolicy::default()
    };
    for (policy, state) in [(RefreshPolicy::default(), &before), (manual_only, &before)] {
        assert!(matches!(
            decide(
                &policy,
                Trigger::Startup,
                &state.configs(),
                7,
                Some(&state.refresh_state()),
                now
            ),
            Decision::Keep { .. }
        ));
    }
    for policy in [RefreshPolicy::default(), manual_only] {
        assert_eq!(
            decide(
                &policy,
                Trigger::Startup,
                &guide.configs(),
                7,
                Some(&guide.refresh_state()),
                now
            ),
            Decision::Refresh(RefreshReason::NeverFetched)
        );
    }
    // The programs' pages leave the file, not only their rows.
    let size = |path: &std::path::Path| std::fs::metadata(path).unwrap().len();
    assert!(
        size(&archive) < size(&full),
        "{} >= {}",
        size(&archive),
        size(&full)
    );
}

#[test]
fn older_untitled_imports_keep_personalization_on_upgrade_restore_and_refresh() {
    use iptv_providers::m3u::parse_m3u;
    use iptv_providers::model::StreamLocator;

    let directory = tempfile::tempdir().unwrap();
    let live = directory.path().join("generated-live.sqlite");
    let backup = directory.path().join("generated-older-backup.sqlite");
    let text = "#EXTM3U\n\
http://streams.example.com/12345.ts\n\
http://streams.example.com/12345.ts\n\
#EXTINF:-1,Explicit title\n\
http://streams.example.com/named.ts\n\
#EXTINF:-1,http://label.example.com/title\n\
http://streams.example.com/url-label.ts\n\
#EXTINF:-1 tvg-name=\"56789\",\n\
http://streams.example.com/56789.ts\n\
";
    let channels = parse_m3u(text).channels.items;
    let mut store = Store::open(&live).unwrap();
    let source = store.add_source(&m3u_source("Generated labels")).unwrap();
    // Recreate the labels an older parser wrote, keeping its original IDs.
    let mut old_channels = channels.clone();
    for channel in old_channels.iter_mut().take(2) {
        let StreamLocator::Url { url, .. } = &channel.stream else {
            panic!()
        };
        channel.name = iptv_providers::m3u::legacy_fallback_name(url);
        channel.epg_name = None;
    }
    store
        .replace_catalog(source, &[], &old_channels, &[], &[])
        .unwrap();
    for (index, channel) in old_channels.iter().take(2).enumerate() {
        store.set_favorite(source, &channel.id, true).unwrap();
        store
            .record_watch(
                source,
                &channel.id,
                ContentKind::Live,
                12345 + index as i64,
                Some(17),
            )
            .unwrap();
    }
    store
        .rename_channel(source, &old_channels[1].id, Some("My generated label"))
        .unwrap();
    store.snapshot_to(&backup).unwrap();
    drop(store);
    // This data-only migration leaves schema 26 unchanged, so removing its
    // ledger entry exactly reconstructs a pre-correction store and snapshot.
    for path in [&live, &backup] {
        let db = Connection::open(path).unwrap();
        db.execute("DELETE FROM schema_migrations WHERE version=27", [])
            .unwrap();
        db.pragma_update(None, "user_version", 26).unwrap();
    }
    Store::validate_snapshot(&backup).unwrap();
    for path in [&live, &backup] {
        let mut restored = Store::open(path).unwrap();
        let check = |store: &Store| {
            let rows = store
                .load_channels(source, &ChannelQuery::default())
                .unwrap();
            assert_eq!(rows.len(), channels.len());
            for (row, channel) in rows.iter().zip(&channels) {
                assert_eq!(row.channel.id, channel.id);
                assert_eq!(row.channel.name, channel.name);
                assert_eq!(row.channel.epg_name, channel.epg_name);
            }
            assert_eq!(rows[1].name, "My generated label");
            let neutral = store.search_channels(Some(source), "Unnamed", 10).unwrap();
            assert_eq!(neutral.len(), 2);
            assert!(
                store
                    .search_channels(Some(source), "12345", 10)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                store
                    .search_channels(Some(source), "Explicit", 10)
                    .unwrap()
                    .len(),
                1
            );
            for channel in channels.iter().take(2) {
                assert!(store.is_favorite(source, &channel.id).unwrap());
            }
            let history = store.recent(Some(source), None, 10).unwrap();
            assert_eq!(history.len(), 2);
            assert!(history.iter().all(|row| row.position_secs == Some(17)));
            assert_eq!(history[0].item_id, channels[1].id);
            assert_eq!(history[1].item_id, channels[0].id);
        };
        check(&restored);
        // Token rotation and reimport apply the neutral label without changing
        // either the old identity or the user-owned overlay/history rows.
        let refreshed = parse_m3u(&text.replace("12345.ts", "12345.ts?token=generated-new"))
            .channels
            .items;
        restored
            .replace_catalog(source, &[], &refreshed, &[], &[])
            .unwrap();
        check(&restored);
    }
}
