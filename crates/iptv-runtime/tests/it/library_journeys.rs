//! Cached aggregate browsing retains source identity through policy and deletion.
use crate::common;

use iptv_library::SourcePolicy;
use iptv_runtime::{Ports, Runtime, RuntimeEvent, data};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::{DatabaseKey, Store};
use std::{rc::Rc, time::Duration};

async fn displayed(
    owner: &Rc<Runtime>,
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<usize>,
    expected: usize,
) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(channels) = receiver.recv().await {
            if channels == expected {
                assert_eq!(owner.schedule().channel_count(), expected);
                return;
            }
        }
        panic!("display event stream ended before the expected catalogue");
    })
    .await
    .expect("cached library did not publish the expected catalogue");
}

#[test]
fn aggregate_policy_and_deletion_never_reassign_another_sources_channel_identity() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&executor, async {
        let dir = tempfile::tempdir().unwrap();
        let key = DatabaseKey::from_bytes([0x75; 32]);
        let mut store = Store::open_encrypted(dir.path().join("iptv.sqlite"), &key).unwrap();
        let catalog = common::catalog(4, 2, common::XT_PASSWORD);
        let mut sources = Vec::new();
        for name in ["Generated A", "Generated B"] {
            let id = store.add_source(&common::m3u_source(name)).unwrap();
            common::replace(&mut store, id, &catalog);
            store
                .set_source_policy(
                    id,
                    SourcePolicy {
                        refresh_on_start: false,
                        ..Default::default()
                    },
                )
                .unwrap();
            sources.push(id);
        }
        store.set_all_playlists(true).unwrap();
        drop(store);
        let mut settings = Settings::new();
        settings
            .set("epg.update_on_start", Value::Bool(false))
            .unwrap();
        settings
            .set("epg.update_on_playlist_change", Value::Bool(false))
            .unwrap();
        std::fs::write(dir.path().join("settings.json"), settings.to_json_string()).unwrap();
        let owner = Rc::new(Runtime::new(Ports::new(dir.path().into(), move |_| {
            Ok(key.clone())
        })));
        owner.bind_tasks(
            |task| drop(tokio::task::spawn_local(task)),
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let weak = Rc::downgrade(&owner);
        owner.bind(move |event| {
            if let RuntimeEvent::ScheduleReady(schedule) = event {
                let owner = weak.upgrade().unwrap();
                let count = schedule.channel_count();
                owner.set_schedule(schedule.clone());
                owner.favorites_catalog_changed(schedule.as_ref());
                sender.send(count).unwrap();
            }
        });
        owner.start_services();
        owner.load_settings();
        data::start(&owner);
        displayed(&owner, &mut events, 8).await;
        assert!(data::combined::has_multiple_enabled(&owner));
        let keys: Vec<_> = (0..8)
            .map(|i| owner.schedule().channel_key(i).unwrap())
            .collect();
        let a = keys
            .iter()
            .find(|key| data::combined::owner(&owner, key).unwrap().0 == sources[0])
            .unwrap()
            .clone();
        let b = keys
            .iter()
            .find(|key| data::combined::owner(&owner, key).unwrap().0 == sources[1])
            .unwrap()
            .clone();
        assert_ne!(a, b, "equal provider IDs have source-qualified identities");
        data::library::rename(&owner, sources[1], "Renamed generated B")
            .await
            .unwrap();
        assert!(
            data::library::playlists(&owner)
                .unwrap()
                .iter()
                .any(|p| p.id == sources[1] && p.name == "Renamed generated B")
        );
        data::library::set_policy(
            &owner,
            sources[0],
            SourcePolicy {
                enabled: false,
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        displayed(&owner, &mut events, 4).await;
        assert!(!data::combined::includes(&owner, sources[0]));
        assert!(owner.schedule().find_channel(&a).is_none());
        assert!(owner.schedule().find_channel(&b).is_some());
        assert!(!data::combined::has_multiple_enabled(&owner));
        data::library::set_policy(
            &owner,
            sources[0],
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        displayed(&owner, &mut events, 8).await;
        assert!(owner.schedule().find_channel(&a).is_some());
        assert!(owner.schedule().find_channel(&b).is_some());
        data::library::delete(&owner, sources[0]).await.unwrap();
        displayed(&owner, &mut events, 4).await;
        assert!(owner.schedule().find_channel(&a).is_none());
        assert!(owner.schedule().find_channel(&b).is_some());
        // A playlist's own movie and series import: switching back to the
        // inherited value pins nothing, and deleting it drops its own value,
        // so a playlist given the same id later starts from the global one.
        let key = "playlists.include_vod";
        let at = iptv_settings::Target::playlist(sources[1].0.to_string());
        owner
            .set_playlist_setting(sources[1], key, Value::Bool(false))
            .unwrap();
        assert_eq!(owner.settings().stored(key, &at), Some(&Value::Bool(false)));
        owner
            .set_playlist_setting(sources[1], key, Value::Bool(true))
            .unwrap();
        assert_eq!(owner.settings().stored(key, &at), None, "inherits again");
        owner
            .set_playlist_setting(sources[1], key, Value::Bool(false))
            .unwrap();
        data::library::delete(&owner, sources[1]).await.unwrap();
        displayed(&owner, &mut events, 0).await;
        assert_eq!(owner.settings().stored(key, &at), None);
        assert!(
            owner
                .settings()
                .bool(iptv_settings::keys::PLAYLISTS_INCLUDE_VOD, &at)
        );
        assert!(
            data::combined::active(&owner),
            "explicit aggregate mode remains available for adding a source"
        );
        assert!(data::library::playlists(&owner).unwrap().is_empty());
        assert!(owner.drain_services());
    });
}
