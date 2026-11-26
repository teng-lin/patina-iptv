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
        panic!("display event stream ended before the expected catalog");
    })
    .await
    .expect("cached library did not publish the expected catalog");
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
            .set("epg.update", Value::choice("on_request"))
            .unwrap();
        // A playlist deleted while its settings could not be saved left its
        // own value behind; a kept playlist has one too.
        let gone = iptv_settings::Target::playlist("9");
        let kept = iptv_settings::Target::playlist(sources[0].0.to_string());
        for at in [&gone, &kept] {
            settings
                .set_in("playlists.include_vod", at, Value::Bool(false))
                .unwrap();
        }
        settings
            .set_in("playlists.max_connections", &kept, Value::Int(4))
            .unwrap();
        // Before #282 one streams-at-once value applied to every playlist:
        // a playlist without its own value takes it, then it goes.
        settings
            .set("playlists.max_connections", Value::Int(3))
            .unwrap();
        let other = iptv_settings::Target::playlist(sources[1].0.to_string());
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
        let last = std::cell::RefCell::new(owner.schedule());
        owner.bind(move |event| {
            if let RuntimeEvent::ScheduleChanged {
                previous, current, ..
            } = event
            {
                let owner = weak.upgrade().unwrap();
                assert!(Rc::ptr_eq(&last.replace(current.clone()), &previous));
                assert!(Rc::ptr_eq(&owner.schedule(), &current));
                for channel in 0..current.channel_count() {
                    let key = current.channel_key(channel).unwrap();
                    assert_eq!(owner.favorites_channel_of(&key), Some(channel));
                }
                sender.send(current.channel_count()).unwrap();
            }
        });
        owner.start_services();
        owner.load_settings();
        data::start(&owner);
        displayed(&owner, &mut events, 8).await;
        // Two quick edits use the latest stored fields without reopening the guide.
        let schedule = owner.schedule();
        let source = sources[1];
        let interval = tokio::task::spawn_local({
            let owner = owner.clone();
            async move {
                data::update_policy(&owner, source, |policy| policy.refresh_seconds = 21600).await
            }
        });
        data::update_policy(&owner, source, |policy| policy.refresh_on_start = true)
            .await
            .unwrap();
        interval.await.unwrap().unwrap();
        assert_eq!(
            data::library::policy(&owner, source).unwrap(),
            SourcePolicy {
                enabled: true,
                refresh_seconds: 21600,
                refresh_on_start: true
            }
        );
        assert!(
            !data::library::policy(&owner, sources[0])
                .unwrap()
                .refresh_on_start
        );
        assert!(Rc::ptr_eq(&schedule, &owner.schedule()));
        // Startup drops the settings of playlists the store no longer has.
        assert_eq!(
            owner.settings().stored("playlists.include_vod", &gone),
            None
        );
        assert_eq!(
            owner.settings().stored("playlists.include_vod", &kept),
            Some(&Value::Bool(false))
        );
        let streams = |at| {
            owner
                .settings()
                .stored("playlists.max_connections", at)
                .cloned()
        };
        assert_eq!(streams(&kept), Some(Value::Int(4)), "its own value stays");
        assert_eq!(streams(&other), Some(Value::Int(3)), "the shared value");
        assert_eq!(streams(&iptv_settings::Target::Global), None);
        assert!(data::has_multiple_enabled(&owner));
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
        data::rename(&owner, sources[1], "Renamed generated B")
            .await
            .unwrap();
        assert!(
            data::library::playlists(&owner)
                .unwrap()
                .iter()
                .any(|p| p.id == sources[1] && p.name == "Renamed generated B")
        );
        data::set_policy(
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
        assert!(!data::combined_includes(&owner, sources[0]));
        assert!(owner.schedule().find_channel(&a).is_none());
        assert!(owner.schedule().find_channel(&b).is_some());
        assert!(!data::has_multiple_enabled(&owner));
        data::set_policy(
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
        data::delete(&owner, sources[0]).await.unwrap();
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
        data::delete(&owner, sources[1]).await.unwrap();
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
