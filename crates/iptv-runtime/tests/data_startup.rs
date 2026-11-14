//! Data orchestration runs through the runtime's task and event ports alone.
use iptv_runtime::{LocalTask, Ports, Runtime, RuntimeEvent, in_runtime, with_runtime};
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
    let second = make("second");
    let tasks = Rc::new(RefCell::new(Vec::<LocalTask>::new()));
    let queue = tasks.clone();
    first.bind_tasks(
        move |task| queue.borrow_mut().push(Box::pin(in_runtime(task))),
        |_| Box::pin(std::future::pending()),
    );
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    first.bind(move |event| {
        if matches!(event, RuntimeEvent::FirstRun) {
            received.borrow_mut().push("first run");
        }
    });
    second.bind(|_| panic!("first owner's startup reached the second owner"));
    with_runtime(first.clone(), || {
        first.start_services();
        iptv_runtime::data::start();
    });
    // Poll with a different active runtime, as a recreated shell can do while
    // old asynchronous reads are still returning.
    let until = Instant::now() + Duration::from_secs(10);
    with_runtime(second.clone(), || {
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
        assert!(Rc::ptr_eq(&iptv_runtime::current(), &second));
    });
    assert_eq!(&*events.borrow(), &["first run"]);
    tasks.borrow_mut().clear();
    with_runtime(first.clone(), || assert!(first.drain_services()));
    assert!(!second.data_dir().join("iptv.sqlite").exists());
}
