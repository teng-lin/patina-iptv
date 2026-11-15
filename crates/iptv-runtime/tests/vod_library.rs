//! Synthetic direct-ID access, source fences and durable watch progress.
#[path = "../../iptv-store-sqlite/tests/common/mod.rs"]
mod common;
mod support;
use iptv_core::{
    parental::{Pin, Target},
    vod::{Key, Kind, Progress, Query},
};
use iptv_providers::model::StreamLocator;
use iptv_runtime::{Ports, Runtime};
use iptv_settings::Value;
use std::rc::Rc;
#[test]
fn movie_playback_progress_survives_restart_and_locked_direct_ids_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x63; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &database_key)
            .unwrap();
    let source = store
        .add_source(&common::m3u_source("Generated on-demand source"))
        .unwrap();
    let mut catalog = common::catalog(1, 1, common::XT_PASSWORD);
    catalog.vod[0].stream = StreamLocator::Url {
        url: "https://example.test/generated-movie.mp4".into(),
        headers: vec![("X-Generated".into(), "fixture".into())],
        properties: vec![],
    };
    common::replace(&mut store, source, &catalog);
    drop(store);
    let key = Key {
        source: source.0,
        kind: Kind::Movie,
        id: catalog.vod[0].id.as_str().into(),
    };
    let make = || {
        Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), {
            let key = database_key.clone();
            move |_| Ok(key.clone())
        })))
    };
    let owner = make();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    let details = support::block_on(owner.vod_details(key.clone())).unwrap();
    let (playing, stream) =
        support::block_on(owner.vod_play(details.clone(), key.clone(), false)).unwrap();
    assert_eq!(playing.key, key);
    assert_eq!(stream.url, "https://example.test/generated-movie.mp4");
    assert_eq!(stream.header("X-Generated"), Some("fixture"));
    let incarnation = details.incarnation.clone();
    let point = Progress::observed(37_000, Some(120_000), false);
    support::block_on(owner.vod_save_progress(key.clone(), None, point, incarnation)).unwrap();
    assert!(owner.drain_services());
    drop(owner);
    let owner = make();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    let details = support::block_on(owner.vod_details(key.clone())).unwrap();
    assert_eq!(details.item.progress, point);
    let (playing, _) =
        support::block_on(owner.vod_play(details.clone(), key.clone(), false)).unwrap();
    assert_eq!(playing.progress.resume_ms(), 37_000);
    let (playing, _) =
        support::block_on(owner.vod_play(details.clone(), key.clone(), true)).unwrap();
    assert_eq!(playing.progress.resume_ms(), 0);
    support::block_on(owner.parental_change_pin(None, Pin::parse("2468").unwrap())).unwrap();
    owner
        .set_setting("parental.vod_enabled", Value::Bool(true))
        .unwrap();
    let target = iptv_runtime::vod::target(&key);
    support::block_on(owner.parental_set_blocked(target.clone(), true)).unwrap();
    owner.parental_lock();
    assert!(support::block_on(owner.vod_details(key.clone())).is_err());
    assert!(support::block_on(owner.vod_play(details.clone(), key.clone(), false)).is_err());
    support::block_on(owner.parental_authorize(target, Pin::parse("2468").unwrap())).unwrap();
    assert!(support::block_on(owner.vod_play(details, key.clone(), false)).is_ok());
    let (cards, _, _, _) = support::block_on(owner.vod_catalogue(
        Query {
            continue_watching: true,
            ..Default::default()
        },
        0,
    ))
    .unwrap();
    assert!(cards.iter().any(|c| c.key == key));
    support::block_on(owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()))
        .unwrap();
    owner
        .set_setting("parental.vod_enabled", Value::Bool(false))
        .unwrap();
    assert!(owner.drain_services());
}

#[test]
fn stale_observation_cannot_write_to_a_reused_source_id() {
    let dir = tempfile::tempdir().unwrap();
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x64; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &database_key)
            .unwrap();
    let source_data = common::m3u_source("Generated reused source");
    let source = store.add_source(&source_data).unwrap();
    let catalog = common::catalog(1, 1, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalog);
    let key = Key {
        source: source.0,
        kind: Kind::Movie,
        id: catalog.vod[0].id.to_string(),
    };
    let old = store.source_incarnation(source).unwrap();
    store.delete_source(source).unwrap();
    let replacement = store.add_source(&source_data).unwrap();
    assert_eq!(source, replacement);
    common::replace(&mut store, replacement, &catalog);
    drop(store);
    let owner = Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), move |_| {
        Ok(database_key.clone())
    })));
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    let point = Progress::observed(40_000, Some(100_000), false);
    assert!(
        support::block_on(owner.vod_save_progress(key.clone(), None, point, old.clone())).is_err()
    );
    owner.queue_vod_progress(key.clone(), None, point, old);
    assert!(owner.drain_services());
    let key = iptv_runtime::model::DatabaseKey::from_bytes([0x64; 32]);
    let store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &key).unwrap();
    assert_eq!(
        store
            .vod_progress(&iptv_core::vod::Key {
                source: source.0,
                kind: Kind::Movie,
                id: catalog.vod[0].id.to_string()
            })
            .unwrap(),
        Progress::default()
    );
}
