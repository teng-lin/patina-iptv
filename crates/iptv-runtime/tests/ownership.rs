//! Public runtime API exercised without a UI or platform credential broker.
use iptv_runtime::{Ports, Runtime, RuntimeEvent, current, in_runtime, with_runtime};
use std::{
    cell::Cell,
    future::Future,
    rc::Rc,
    task::{Context, Poll, Waker},
};

fn owner(directory: &std::path::Path) -> Rc<Runtime> {
    Rc::new(Runtime::new(Ports::new(directory.to_owned(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x61; 32]))
    })))
}

#[test]
fn a_suspended_task_delivers_its_event_to_the_accepting_owner() {
    let directory = tempfile::tempdir().unwrap();
    let first = owner(&directory.path().join("alpha"));
    let second = owner(&directory.path().join("beta"));
    let seen_first = Rc::new(Cell::new(None));
    let seen_second = Rc::new(Cell::new(None));
    for (owner, seen) in [(&first, seen_first.clone()), (&second, seen_second.clone())] {
        owner.bind(move |event| {
            if let RuntimeEvent::AccountStreams(limit) = event {
                seen.set(limit);
            }
        });
    }
    let (release, wait) = futures_channel::oneshot::channel();
    let task = with_runtime(first.clone(), || {
        in_runtime(async move {
            let limit = wait.await.unwrap();
            current().emit(RuntimeEvent::AccountStreams(Some(limit)));
        })
    });
    let mut task = std::pin::pin!(task);
    let mut context = Context::from_waker(Waker::noop());
    with_runtime(second.clone(), || {
        assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
        assert!(Rc::ptr_eq(&current(), &second));
        release.send(3).unwrap();
        assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
        assert!(Rc::ptr_eq(&current(), &second));
    });
    assert_eq!(seen_first.get(), Some(3));
    assert_eq!(seen_second.get(), None);
    assert!(iptv_runtime::try_current().is_none());
}

#[test]
fn unwinding_restores_the_outer_owner_and_context_does_not_retain_it() {
    let directory = tempfile::tempdir().unwrap();
    let outer = owner(&directory.path().join("outer"));
    let inner = owner(&directory.path().join("inner"));
    let weak = Rc::downgrade(&outer);
    with_runtime(outer.clone(), || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_runtime(inner, || panic!("synthetic interrupted callback"));
        }));
        assert!(result.is_err());
        assert!(Rc::ptr_eq(&current(), &outer));
    });
    iptv_runtime::set_current(&outer);
    drop(outer);
    assert!(weak.upgrade().is_none());
    assert!(iptv_runtime::try_current().is_none());
}

#[test]
fn a_worker_keeps_its_accepting_directory_and_key_broker() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let directory = tempfile::tempdir().unwrap();
    let first_directory = directory.path().join("alpha");
    let second_directory = directory.path().join("beta");
    std::fs::create_dir(&first_directory).unwrap();
    let first_calls = Arc::new(AtomicUsize::new(0));
    let second_calls = Arc::new(AtomicUsize::new(0));
    let make = |path, calls: Arc<AtomicUsize>, byte| {
        Rc::new(Runtime::new(Ports::new(path, move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(iptv_runtime::model::DatabaseKey::from_bytes([byte; 32]))
        })))
    };
    let first = make(first_directory.clone(), first_calls.clone(), 0x62);
    let second = make(second_directory.clone(), second_calls.clone(), 0x63);
    let worker = with_runtime(first.clone(), || {
        first.start_services();
        first.scope_worker(|| {
            iptv_runtime::executor::block_on(iptv_runtime::commands::remember_combined_channel(
                "synthetic-selection".into(),
            ))
            .unwrap();
        })
    });
    with_runtime(second.clone(), || {
        second.activate();
        std::thread::spawn(worker).join().unwrap();
    });
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
    assert!(!second_directory.exists());
    with_runtime(first.clone(), || assert!(first.drain_services()));
    let store = iptv_store_sqlite::Store::open_encrypted(
        first_directory.join("iptv.sqlite"),
        &iptv_runtime::model::DatabaseKey::from_bytes([0x62; 32]),
    )
    .unwrap();
    assert_eq!(
        store.combined_last_channel().unwrap().as_deref(),
        Some("synthetic-selection")
    );
}
