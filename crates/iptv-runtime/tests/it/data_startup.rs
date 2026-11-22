//! Data orchestration runs through the runtime's task and event ports alone.
use iptv_runtime::{LocalTask, Ports, Runtime, RuntimeEvent};
use std::{
    cell::RefCell,
    rc::Rc,
    task::{Context, Waker},
    time::{Duration, Instant},
};

#[test]
fn empty_library_startup_finishes_on_its_accepting_owner_without_a_ui() {
    let directory = tempfile::tempdir().unwrap();
    let make = |name: &str| {
        let path = directory.path().join(name);
        std::fs::create_dir(&path).unwrap();
        Rc::new(Runtime::new(Ports::new(path, |_| {
            Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x63; 32]))
        })))
    };
    let first = make("first");
    let tasks = Rc::new(RefCell::new(Vec::<LocalTask>::new()));
    let queue = tasks.clone();
    first.bind_tasks(
        move |task| queue.borrow_mut().push(task),
        |_| Box::pin(std::future::pending()),
    );
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    first.bind(move |event| {
        if matches!(event, RuntimeEvent::FirstRun) {
            received.borrow_mut().push("first run");
        }
    });
    first.start_services();
    iptv_runtime::data::start(&first);
    // A recreated shell constructs and starts its own owner while the old
    // one's asynchronous reads are still returning.
    let second = make("second");
    second.bind(|_| panic!("first owner's startup reached the second owner"));
    second.bind_tasks(
        |_| panic!("first owner's startup spawned on the second owner"),
        |_| Box::pin(std::future::pending()),
    );
    second.start_services();
    let until = Instant::now() + Duration::from_secs(10);
    let mut context = Context::from_waker(Waker::noop());
    while events.borrow().is_empty() {
        assert!(
            Instant::now() < until,
            "startup never presented its empty library"
        );
        let pending = std::mem::take(&mut *tasks.borrow_mut());
        for mut task in pending {
            if task.as_mut().poll(&mut context).is_pending() {
                tasks.borrow_mut().push(task);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(&*events.borrow(), &["first run"]);
    tasks.borrow_mut().clear();
    assert!(first.drain_services());
    assert!(second.drain_services());
    assert!(first.data_dir().join("iptv.sqlite").exists());
    assert!(!second.data_dir().join("iptv.sqlite").exists());
}

#[test]
fn a_retired_startup_does_not_report_to_a_restarted_owner() {
    let directory = tempfile::tempdir().unwrap();
    let owner = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| {
            Err(iptv_runtime::Error::unavailable(
                "synthetic database key is unavailable",
            ))
        },
    )));
    let tasks = Rc::new(RefCell::new(Vec::<LocalTask>::new()));
    let queue = tasks.clone();
    owner.bind_tasks(
        move |task| queue.borrow_mut().push(task),
        |_| Box::pin(std::future::pending()),
    );
    owner.bind(|_| panic!("the retired startup published a result"));
    owner.start_services();
    iptv_runtime::data::start(&owner);
    let mut startup = tasks.borrow_mut().pop().unwrap();
    assert!(owner.drain_services());
    owner.start_services();

    // Draining the writer completed the old read before its UI task ran.
    let mut context = Context::from_waker(Waker::noop());
    assert!(startup.as_mut().poll(&mut context).is_ready());
    assert!(owner.drain_services());
}

#[test]
fn startup_retry_reopens_the_reader_and_loads_parental_controls_before_reporting_success() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&executor, async {
        let directory = tempfile::tempdir().unwrap();
        let key_ready = Arc::new(AtomicBool::new(false));
        let broker = key_ready.clone();
        let owner = Rc::new(Runtime::new(Ports::new(
            directory.path().to_owned(),
            move |_| {
                if broker.load(Ordering::SeqCst) {
                    Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x63; 32]))
                } else {
                    Err(iptv_runtime::Error::unavailable(
                        "synthetic key is unavailable",
                    ))
                }
            },
        )));
        owner.bind_tasks(
            |task| drop(tokio::task::spawn_local(task)),
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let observer = Rc::downgrade(&owner);
        owner.bind(move |event| match event {
            RuntimeEvent::StoreStartupFailed { .. } => sender.send("failed").unwrap(),
            RuntimeEvent::StoreStartupRecovered => {
                assert!(observer.upgrade().unwrap().parental_is_ready());
                sender.send("recovered").unwrap();
            }
            RuntimeEvent::FirstRun => sender.send("welcome").unwrap(),
            _ => {}
        });
        owner.start_services();
        assert!(owner.parental_refresh().await.is_err());
        iptv_runtime::data::start(&owner);
        tokio::time::timeout(Duration::from_secs(10), async {
            assert_eq!(events.recv().await, Some("failed"));
            key_ready.store(true, Ordering::SeqCst);
            iptv_runtime::data::retry_startup(&owner);
            assert_eq!(events.recv().await, Some("recovered"));
            assert_eq!(events.recv().await, Some("welcome"));
        })
        .await
        .unwrap();
        assert!(owner.drain_services());
    });
}

#[test]
fn newer_schema_and_unavailable_store_are_reported_at_startup_and_block_import() {
    use iptv_core::wizard::{FailureKind, ImportFailure};
    use iptv_runtime::data::{self, Job, Purpose, Update};
    use iptv_store_sqlite::{DatabaseKey, SCHEMA_VERSION, Store};

    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&executor, async {
        for broker_available in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("iptv.sqlite");
            let found = SCHEMA_VERSION + 1;
            if broker_available {
                Store::open_in_memory().unwrap().snapshot_to(&path).unwrap();
                let connection = rusqlite::Connection::open(&path).unwrap();
                connection
                    .execute(
                        "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, 'future', 0)",
                        [found],
                    )
                    .unwrap();
            }
            let reason = "synthetic database key is unavailable";
            let owner = Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), move |_| {
                if broker_available {
                    Ok(DatabaseKey::from_bytes([0x63; 32]))
                } else {
                    Err(iptv_runtime::Error::unavailable(reason))
                }
            })));
            owner.bind_tasks(
                |task| drop(tokio::task::spawn_local(task)),
                |duration| Box::pin(tokio::time::sleep(duration)),
            );
            let (sender, mut startup) = tokio::sync::mpsc::unbounded_channel();
            owner.bind(move |event| match event {
                RuntimeEvent::FirstRun => panic!("an unreadable store is not an empty library"),
                RuntimeEvent::StoreStartupFailed { path, error } => {
                    sender.send((path, error)).unwrap();
                }
                _ => {}
            });
            owner.start_services();
            // The shell reads parental controls before loading the library.
            assert!(owner.parental_refresh().await.is_err());
            data::start(&owner);
            let (reported_path, error) =
                tokio::time::timeout(Duration::from_secs(10), startup.recv())
                    .await
                    .expect("startup did not report the storage failure")
                    .unwrap();
            assert_eq!(reported_path, Some(path));
            if broker_available {
                assert_eq!(error.newer_schema(), Some((found, SCHEMA_VERSION)));
            } else {
                assert_eq!(error.unavailable_reason(), Some(reason));
            }

            for expected in [FailureKind::Storage, FailureKind::Canceled] {
                let (sender, mut import) = tokio::sync::mpsc::unbounded_channel();
                let cancel = data::start_import(
                    &owner,
                    Job {
                        source: crate::common::m3u_source("Generated playlist"),
                        epg_url: None,
                        stored: None,
                        purpose: Purpose::Add,
                        edit: false,
                    },
                    move |update| {
                        if let Update::Done(result) = update {
                            sender.send(result).unwrap();
                        }
                    },
                );
                if expected == FailureKind::Canceled {
                    cancel.cancel();
                }
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(10), import.recv())
                        .await
                        .expect("import did not report the outcome"),
                    Some(Err(ImportFailure::new(expected)))
                );
            }
            assert!(owner.drain_services());
        }
    });
}
