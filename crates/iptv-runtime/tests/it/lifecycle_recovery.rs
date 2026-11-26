//! Acknowledged persistence work survives teardown and failed publication.
#![cfg(feature = "test-support")]
use crate::support;
use iptv_runtime::{Ports, Runtime, commands};
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};
use std::{path::Path, rc::Rc, time::Duration};

const LIMIT: Duration = Duration::from_secs(5);

fn owner(path: &Path) -> Rc<Runtime> {
    let owner = Rc::new(Runtime::new(Ports::new(path.into(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x68; 32]))
    })));
    owner.start_services();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner
}

#[test]
fn dropping_the_last_ui_owner_acknowledges_closed_lanes_and_keeps_accepted_writes() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = owner(directory.path());
    support::block_on(commands::combined_resume_key(&runtime)).unwrap();
    let gate = runtime.host_handle().hold_writer().unwrap();
    assert!(gate.wait_entered(LIMIT));
    let observed = runtime.host_handle().observe_drain().unwrap();
    let mut accepted = Box::pin(commands::remember_combined_channel(
        &runtime,
        "generated-drop-commit".into(),
    ));
    assert!(
        accepted
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(accepted);
    runtime.save_last_channel(None, "generated-drop-file");
    let closed = runtime.host_handle().observe_admission_close().unwrap();
    let release = std::thread::spawn(move || {
        assert!(closed.wait_entered(LIMIT));
        gate.release();
    });
    drop(runtime);
    release.join().unwrap();
    assert!(
        observed.wait_entered(Duration::ZERO),
        "Drop returned before its native connections were drained"
    );
    let restarted = owner(directory.path());
    assert_eq!(
        support::block_on(commands::combined_resume_key(&restarted))
            .unwrap()
            .as_deref(),
        Some("generated-drop-commit")
    );
    assert_eq!(
        restarted.load_last_channel(None).as_deref(),
        Some("generated-drop-file")
    );
    assert!(restarted.drain_services());
}

#[test]
fn host_flush_acknowledges_the_actual_file_worker_and_dropped_reply() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = owner(directory.path());
    let host = runtime.host_handle();
    let gate = host.hold_file_write().unwrap();
    runtime.save_combined_channel("generated-accepted-selection");
    assert!(gate.wait_entered(LIMIT));
    assert_eq!(
        runtime.load_combined_channel().as_deref(),
        Some("generated-accepted-selection")
    );
    assert!(!directory.path().join("last_channel_combined").exists());
    assert!(!host.flush_files(Duration::ZERO));
    gate.release();
    assert!(host.flush_files(LIMIT));
    assert!(runtime.drain_services());
    drop(runtime);
    let restarted = owner(directory.path());
    assert_eq!(
        restarted.load_combined_channel().as_deref(),
        Some("generated-accepted-selection")
    );
    assert!(restarted.drain_services());
}

#[test]
fn failed_file_publication_keeps_committed_state_after_owner_restart() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = owner(directory.path());
    runtime.save_combined_channel("generated-committed");
    assert!(runtime.flush_files(LIMIT));
    let gate = runtime.host_handle().hold_file_write().unwrap();
    runtime.save_combined_channel("generated-failed");
    assert!(gate.wait_entered(LIMIT));
    // A real filesystem failure, independent of root privileges or disk speed.
    let blocked = directory.path().join("last_channel_combined.tmp");
    std::fs::create_dir(&blocked).unwrap();
    gate.release();
    assert!(runtime.flush_files(LIMIT));
    assert_eq!(
        runtime.load_combined_channel().as_deref(),
        Some("generated-committed")
    );
    assert!(runtime.drain_services());
    drop(runtime);
    let restarted = owner(directory.path());
    assert_eq!(
        restarted.load_combined_channel().as_deref(),
        Some("generated-committed")
    );
    assert!(
        blocked.is_dir(),
        "failed writes must not delete unrelated paths"
    );
    assert!(restarted.drain_services());
}

#[test]
fn failed_playlist_sort_publication_restores_the_combined_guide_order() {
    use crate::common;
    use iptv_runtime::{RuntimeEvent, data};
    use iptv_settings::{Settings, Value, keys};
    use iptv_store_sqlite::{DatabaseKey, SourcePolicy, Store};

    async fn displayed(
        events: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<String>>,
        expected: [&str; 2],
    ) {
        tokio::time::timeout(LIMIT, async {
            while let Some(names) = events.recv().await {
                if names == expected {
                    return;
                }
            }
            panic!("the combined guide closed before showing {expected:?}");
        })
        .await
        .unwrap_or_else(|_| panic!("the combined guide did not show {expected:?}"));
    }

    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&executor, async {
        let directory = tempfile::tempdir().unwrap();
        let key = DatabaseKey::from_bytes([0x68; 32]);
        let mut store = Store::open_encrypted(directory.path().join("iptv.sqlite"), &key).unwrap();
        let catalog = common::catalog(2, 1, common::XT_PASSWORD);
        for name in ["Generated Z", "Generated A"] {
            let source = store.add_source(&common::m3u_source(name)).unwrap();
            common::replace(&mut store, source, &catalog);
            store
                .set_source_policy(
                    source,
                    SourcePolicy {
                        refresh_on_start: false,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        store.set_all_playlists(true).unwrap();
        drop(store);
        let mut settings = Settings::new();
        settings
            .set("epg.update", Value::choice("on_request"))
            .unwrap();
        let saved = settings.to_json_string();
        let path = directory.path().join("settings.json");
        std::fs::write(&path, &saved).unwrap();
        let owner = Rc::new(Runtime::new(Ports::new(
            directory.path().into(),
            move |_| Ok(key.clone()),
        )));
        owner.bind_tasks(
            |task| drop(tokio::task::spawn_local(task)),
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        owner.bind(move |event| {
            if let RuntimeEvent::ScheduleChanged { current, .. } = event {
                let names = (0..current.group_count())
                    .filter(|&index| current.group_is_playlist(index))
                    .filter_map(|index| match current.group_label(index) {
                        iptv_core::guide::GroupName::Named(name) => Some(name),
                        _ => None,
                    })
                    .collect();
                let _ = sender.send(names);
            }
        });
        owner.start_services();
        owner.load_settings();
        data::start(&owner);
        let original = ["Generated Z", "Generated A"];
        displayed(&mut events, original).await;
        assert!(owner.flush_files(LIMIT));

        let gate = owner.host_handle().hold_file_write().unwrap();
        owner
            .set_setting(keys::PLAYLIST_SORT.as_str(), Value::choice("name"))
            .unwrap();
        assert!(gate.wait_entered(LIMIT));
        displayed(&mut events, ["Generated A", "Generated Z"]).await;
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        gate.release();

        displayed(&mut events, original).await;
        assert_eq!(
            owner
                .settings()
                .choice(keys::PLAYLIST_SORT, &iptv_settings::Target::Global),
            "manual"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), saved);
        assert!(owner.drain_services());
    });
}

#[test]
fn queued_playlist_writes_recheck_settings_authority() {
    use crate::common;
    use iptv_core::parental::{Pin, Target};
    use iptv_runtime::data;
    use iptv_settings::{Settings, Value, keys};
    use iptv_store_sqlite::{DatabaseKey, Store};

    for (mode, protects_settings) in [("manual", true), ("name", true), ("manual", false)] {
        let directory = tempfile::tempdir().unwrap();
        let key = DatabaseKey::from_bytes([0x68; 32]);
        let mut store = Store::open_encrypted(directory.path().join("iptv.sqlite"), &key).unwrap();
        let first = store
            .add_source(&common::m3u_source("Generated A"))
            .unwrap();
        let second = store
            .add_source(&common::m3u_source("Generated Z"))
            .unwrap();
        let stored_order = || {
            store
                .sources()
                .unwrap()
                .into_iter()
                .map(|source| source.id)
                .collect::<Vec<_>>()
        };
        let mut settings = Settings::new();
        settings
            .set(keys::PLAYLIST_SORT.as_str(), Value::choice(mode))
            .unwrap();
        settings
            .set(
                keys::PARENTAL_REQUIRE_PIN_FOR.as_str(),
                Value::choice(if protects_settings {
                    "blocked_content_and_settings"
                } else {
                    "blocked_content"
                }),
            )
            .unwrap();
        settings
            .set(
                keys::PARENTAL_UNLOCK_WINDOW.as_str(),
                Value::choice("until_restart"),
            )
            .unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            settings.to_json_string(),
        )
        .unwrap();
        let owner = owner(directory.path());
        owner.load_settings();
        support::block_on(owner.parental_change_pin(None, Pin::parse("2468").unwrap())).unwrap();
        assert!(owner.parental_settings_allowed());
        let gate = owner.host_handle().hold_writer().unwrap();
        assert!(gate.wait_entered(LIMIT));
        let mut reorder = Box::pin(data::reorder(&owner, vec![second, first]));
        assert!(
            reorder
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        let mut hide = Box::pin(data::update_policy(&owner, first, |policy| {
            policy.enabled = false;
        }));
        assert!(
            hide.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        owner.parental_lock();
        gate.release();
        let reordered = support::block_on(reorder);
        let hidden = support::block_on(hide);

        if protects_settings {
            assert!(reordered.unwrap_err().settings_locked(), "{mode}");
            assert!(hidden.unwrap_err().settings_locked(), "{mode}");
            assert_eq!(stored_order(), [first, second], "{mode}");
            assert!(store.source_policy(first).unwrap().enabled, "{mode}");
            support::block_on(
                owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()),
            )
            .unwrap();
            support::block_on(data::reorder(&owner, vec![second, first])).unwrap();
            support::block_on(data::update_policy(&owner, first, |policy| {
                policy.enabled = false;
            }))
            .unwrap();
        } else {
            reordered.unwrap();
            hidden.unwrap();
        }
        assert_eq!(stored_order(), [second, first], "{mode}");
        assert!(!store.source_policy(first).unwrap().enabled, "{mode}");
        assert!(owner.drain_services());
    }
}

#[test]
fn shutdown_drains_accepted_database_work_even_when_its_reply_is_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = owner(directory.path());
    support::block_on(commands::combined_resume_key(&runtime)).unwrap();
    let gate = runtime.host_handle().hold_writer().unwrap();
    assert!(gate.wait_entered(LIMIT));
    // Poll through admission, then drop only the local reply future.
    let mut accepted = Box::pin(commands::remember_combined_channel(
        &runtime,
        "generated-accepted".into(),
    ));
    assert!(matches!(
        accepted
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    drop(accepted);
    let closed = runtime.host_handle().observe_admission_close().unwrap();
    let release = std::thread::spawn(move || {
        assert!(closed.wait_entered(LIMIT));
        gate.release();
    });
    assert!(runtime.drain_services());
    release.join().unwrap();
    assert!(
        support::block_on(commands::remember_combined_channel(
            &runtime,
            "generated-late".into()
        ))
        .is_err()
    );
    drop(runtime);
    let restarted = owner(directory.path());
    assert_eq!(
        support::block_on(commands::combined_resume_key(&restarted))
            .unwrap()
            .as_deref(),
        Some("generated-accepted")
    );
    assert!(restarted.drain_services());
}

#[test]
fn restart_cleans_interrupted_catalog_work_without_touching_committed_content() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = owner(directory.path());
    support::block_on(commands::remember_combined_channel(
        &runtime,
        "generated-retained".into(),
    ))
    .unwrap();
    assert!(runtime.drain_services());
    drop(runtime);
    let key = iptv_runtime::model::DatabaseKey::from_bytes([0x68; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(directory.path().join("iptv.sqlite"), &key)
            .unwrap();
    let id = store
        .add_source(&iptv_providers::model::Source {
            name: "Generated committed source".into(),
            config: iptv_providers::model::SourceConfig::M3u(
                iptv_providers::model::M3uSourceConfig {
                    location: iptv_providers::SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                },
            ),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1,Generated committed\nhttps://example.test/committed.ts\n",
    );
    store
        .replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    let incarnation = store.source_incarnation(id).unwrap();
    let mut stage = iptv_store_sqlite::CatalogStage::create_in_with(directory.path(), |path| {
        store.open_staging_connection(path)
    })
    .unwrap();
    use iptv_import::CatalogSink;
    stage
        .push(iptv_import::CatalogRow::Channel(
            parsed.channels.items[0].clone(),
        ))
        .unwrap();
    stage.finish().unwrap();
    let staged = std::fs::read_dir(directory.path())
        .unwrap()
        .find_map(|entry| {
            let path = entry.unwrap().path();
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog-stage-")
                .then_some(path)
        })
        .unwrap();
    // Copy the exact encrypted stage as a crash would leave it; drop the live
    // handle normally so this test also works on hosts that lock open files.
    let abandoned = directory.path().join("catalog-stage-generated.sqlite");
    std::fs::copy(staged, &abandoned).unwrap();
    assert!(
        !std::fs::read(&abandoned)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    drop(stage);
    drop(store);
    for name in [
        "catalog-stage-generated.sqlite-journal",
        "epg-stage-generated.sqlite",
    ] {
        std::fs::write(
            directory.path().join(name),
            b"generated interrupted encrypted stage",
        )
        .unwrap();
    }
    let retained = directory.path().join("generated-unrelated");
    std::fs::write(&retained, b"generated unrelated file").unwrap();
    let restarted = owner(directory.path());
    // This is the same startup operation that the shell invokes, including cleanup.
    support::block_on(restarted.read_library_metadata()).unwrap();
    // Metadata reads deliberately do not perform startup cleanup. Starting data does.
    let tasks = Rc::new(std::cell::RefCell::new(Vec::new()));
    let queued = tasks.clone();
    restarted.bind_tasks(
        move |task| queued.borrow_mut().push(task),
        |_| Box::pin(std::future::pending()),
    );
    let writer = restarted.host_handle().hold_writer().unwrap();
    assert!(writer.wait_entered(LIMIT));
    iptv_runtime::data::start(&restarted);
    // Drive startup through database admission, then stop the local UI executor.
    let mut startup = tasks.borrow_mut().pop().unwrap();
    assert!(
        startup
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    // Keep the first poll pending deterministically, then let native cleanup
    // finish without polling the local UI executor again.
    writer.release();
    assert!(restarted.drain_services());
    drop(startup);
    tasks.borrow_mut().clear();
    assert_eq!(
        std::fs::read(retained).unwrap(),
        b"generated unrelated file"
    );
    assert!(
        !directory
            .path()
            .join("catalog-stage-generated.sqlite")
            .exists()
    );
    assert!(
        !directory
            .path()
            .join("catalog-stage-generated.sqlite-journal")
            .exists()
    );
    assert!(!directory.path().join("epg-stage-generated.sqlite").exists());
    drop(restarted);
    let again = owner(directory.path());
    assert_eq!(
        support::block_on(commands::combined_resume_key(&again))
            .unwrap()
            .as_deref(),
        Some("generated-retained")
    );
    assert!(again.drain_services());
    let store =
        iptv_store_sqlite::Store::open_encrypted(directory.path().join("iptv.sqlite"), &key)
            .unwrap();
    assert_eq!(store.source_incarnation(id).unwrap(), incarnation);
    assert_eq!(
        store.source(id).unwrap().unwrap().source.name,
        "Generated committed source"
    );
    assert_eq!(
        store.load_channels(id, &Default::default()).unwrap()[0].name,
        "Generated committed"
    );
}
