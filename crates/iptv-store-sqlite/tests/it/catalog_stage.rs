//! A private import cannot disturb the saved catalog until publication.
use crate::common;
use iptv_import::{CatalogRow, CatalogSink};
use iptv_store_sqlite::{CatalogStage, ChannelQuery, Store};

#[test]
fn incomplete_and_failed_steps_preserve_saved_rows_and_overlays() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = common::open(dir.path().join("store.sqlite"));
    let source = store.add_source(&common::m3u_source("Synthetic")).unwrap();
    let old = common::m3u_channel(1, 2, common::XT_PASSWORD);
    store
        .replace_catalog(source, &[], std::slice::from_ref(&old), &[], &[])
        .unwrap();
    store.set_favorite(source, &old.id, true).unwrap();
    let mut incomplete = common::catalog_stage(&store, dir.path());
    incomplete
        .push(CatalogRow::Channel(common::m3u_channel(
            2,
            2,
            common::XT_PASSWORD,
        )))
        .unwrap();
    assert!(store.replace_catalog_stage(source, incomplete).is_err());
    assert_eq!(
        store
            .load_channels(source, &ChannelQuery::default())
            .unwrap()[0]
            .channel,
        old
    );

    let mut stage = common::catalog_stage(&store, dir.path());
    stage.begin_step().unwrap();
    // Cross multiple flush boundaries, then roll back the entire failed step.
    for i in 2..1300 {
        stage
            .push(CatalogRow::Channel(common::m3u_channel(
                i,
                2,
                common::XT_PASSWORD,
            )))
            .unwrap();
    }
    stage.end_step(false).unwrap();
    stage.begin_step().unwrap();
    stage.push(CatalogRow::Channel(old.clone())).unwrap();
    stage.push(CatalogRow::Channel(old.clone())).unwrap();
    stage.end_step(true).unwrap();
    stage.finish().unwrap();
    assert_eq!(stage.counts().channels, 1);
    assert_eq!(stage.counts().duplicates, 1);
    assert_eq!(stage.load_channels().unwrap(), vec![old.clone()]);
    assert_eq!(
        store.replace_catalog_stage(source, stage).unwrap().channels,
        1
    );
    let rows = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].channel, old);
    assert!(rows[0].favorite);
    // The browser's stage is a memory file, gone with its connection.
    #[cfg(not(target_family = "wasm"))]
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("catalog-stage"))
            .count(),
        0
    );
}

#[test]
#[cfg_attr(target_family = "wasm", ignore = "needs a file system")]
fn startup_removes_only_abandoned_catalog_stage_files() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "catalog-stage-123-0.sqlite",
        "catalog-stage-123-0.sqlite-journal",
        "iptv.sqlite",
        "playlist.m3u",
    ] {
        std::fs::write(dir.path().join(name), b"synthetic").unwrap();
    }
    CatalogStage::remove_stale(dir.path());
    assert!(!dir.path().join("catalog-stage-123-0.sqlite").exists());
    assert!(
        !dir.path()
            .join("catalog-stage-123-0.sqlite-journal")
            .exists()
    );
    assert!(dir.path().join("iptv.sqlite").exists());
    assert!(dir.path().join("playlist.m3u").exists());
}

/// A failure after credentials and guide rows are written must roll back the
/// entire edit, including a failure late in the catalog copy itself.
#[test]
#[cfg_attr(target_family = "wasm", ignore = "needs a second connection")]
fn edited_details_guides_and_catalog_publish_as_one_transaction() {
    use iptv_epg::FeedId;
    use iptv_providers::SecretUrl;
    use iptv_store_sqlite::GuideSource;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite");
    let mut store = common::open(&path);
    let source = store.add_source(&common::m3u_source("Original")).unwrap();
    let old_channel = common::m3u_channel(1, 2, common::XT_PASSWORD);
    store
        .replace_catalog(source, &[], std::slice::from_ref(&old_channel), &[], &[])
        .unwrap();
    let old_guide = GuideSource {
        feed: FeedId(1),
        provider_index: None,
        url: SecretUrl::new("https://guide.example.test/original.xml"),
        name: Some("Generated original guide".into()),
        enabled: true,
        shift_minutes: 0,
    };
    store
        .set_guide_sources(source, std::slice::from_ref(&old_guide))
        .unwrap();
    let original = store.source(source).unwrap();
    let original_status = store.import_status(source).unwrap();
    let mut edited = common::m3u_source("Edited");
    let iptv_providers::model::SourceConfig::M3u(config) = &mut edited.config else {
        unreachable!()
    };
    config.location = SecretUrl::new("https://playlist.example.test/edited.m3u?token=generated");
    config.user_agent = Some("GeneratedEdit/1".into());
    let new_channel = common::m3u_channel(2, 2, common::XT_PASSWORD);
    let new_guide = GuideSource {
        url: SecretUrl::new("https://guide.example.test/edited.xml"),
        ..old_guide.clone()
    };
    let stage = |store: &Store| {
        let mut stage = common::catalog_stage(store, dir.path());
        stage
            .push(CatalogRow::Channel(new_channel.clone()))
            .unwrap();
        stage.finish().unwrap();
        stage
    };
    // Invalid metadata fails after the source update, before catalog copying.
    assert!(
        store
            .publish_catalog_stage(
                source,
                stage(&store),
                Some(&edited),
                &[new_guide.clone(), new_guide.clone()]
            )
            .is_err()
    );
    assert_eq!(store.source(source).unwrap(), original);
    assert_eq!(
        store.guide_sources(source).unwrap(),
        vec![old_guide.clone()]
    );
    assert_eq!(store.import_status(source).unwrap(), original_status);
    assert_eq!(
        store
            .load_channels(source, &ChannelQuery::default())
            .unwrap()[0]
            .channel,
        old_channel
    );

    // A real SQLite abort during the final copy must undo the earlier metadata.
    let faults = rusqlite::Connection::open(&path).unwrap();
    faults
        .execute_batch(
            "CREATE TRIGGER synthetic_reject_catalog BEFORE INSERT ON channels
        BEGIN SELECT RAISE(ABORT, 'synthetic publication failure'); END;",
        )
        .unwrap();
    assert!(
        store
            .publish_catalog_stage(
                source,
                stage(&store),
                Some(&edited),
                std::slice::from_ref(&new_guide)
            )
            .is_err()
    );
    assert_eq!(store.source(source).unwrap(), original);
    assert_eq!(store.guide_sources(source).unwrap(), vec![old_guide]);
    assert_eq!(store.import_status(source).unwrap(), original_status);
    assert_eq!(
        store
            .load_channels(source, &ChannelQuery::default())
            .unwrap()[0]
            .channel,
        old_channel
    );
    faults
        .execute_batch("DROP TRIGGER synthetic_reject_catalog")
        .unwrap();
    store
        .publish_catalog_stage(
            source,
            stage(&store),
            Some(&edited),
            std::slice::from_ref(&new_guide),
        )
        .unwrap();
    assert_eq!(store.source(source).unwrap().unwrap().source, edited);
    assert_eq!(store.guide_sources(source).unwrap(), vec![new_guide]);
    assert_eq!(
        store
            .load_channels(source, &ChannelQuery::default())
            .unwrap()[0]
            .channel,
        new_channel
    );
}
