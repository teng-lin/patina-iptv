//! Acknowledged persistence work survives teardown and failed publication.
#![cfg(feature = "test-support")]
mod support;
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
fn restart_cleans_interrupted_catalogue_work_without_touching_committed_content() {
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
    iptv_runtime::data::start(&restarted);
    // Drive startup through database admission, then stop the local UI executor.
    let mut startup = tasks.borrow_mut().pop().unwrap();
    assert!(
        startup
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
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
