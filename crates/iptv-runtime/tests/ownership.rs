//! Public runtime API exercised without a UI or platform credential broker.
//! Every owner is reached through the handle the caller holds: there is no
//! ambient owner for a second runtime to replace.
mod support;

use iptv_runtime::{Ports, Runtime, RuntimeEvent, commands};
use std::{
    cell::Cell,
    future::Future,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

fn owner(directory: &std::path::Path) -> Rc<Runtime> {
    Rc::new(Runtime::new(Ports::new(directory.to_owned(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x61; 32]))
    })))
}

/// An owner whose key broker counts its calls.
fn counted(directory: &std::path::Path, calls: &Arc<AtomicUsize>, byte: u8) -> Rc<Runtime> {
    let calls = calls.clone();
    Rc::new(Runtime::new(Ports::new(directory.to_owned(), move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([byte; 32]))
    })))
}

fn combined_channel(directory: &std::path::Path, byte: u8) -> Option<String> {
    iptv_store_sqlite::Store::open_encrypted(
        directory.join("iptv.sqlite"),
        &iptv_runtime::model::DatabaseKey::from_bytes([byte; 32]),
    )
    .unwrap()
    .combined_last_channel()
    .unwrap()
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
    let accepted = first.clone();
    let task = async move {
        let limit = wait.await.unwrap();
        accepted.emit(RuntimeEvent::AccountStreams(Some(limit)));
    };
    let mut task = std::pin::pin!(task);
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
    // A newer owner is constructed and started while the task is suspended.
    let third = owner(&directory.path().join("gamma"));
    third.start_services();
    release.send(3).unwrap();
    assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
    assert_eq!(seen_first.get(), Some(3));
    assert_eq!(seen_second.get(), None);
    assert_eq!(first.account_limit(), Some(3));
    assert_eq!(second.account_limit(), None);
    assert_eq!(third.account_limit(), None);
}

#[test]
fn constructing_and_starting_another_runtime_does_not_change_an_open_owners_store() {
    let directory = tempfile::tempdir().unwrap();
    let (alpha, beta, gamma) = (
        directory.path().join("alpha"),
        directory.path().join("beta"),
        directory.path().join("gamma"),
    );
    let calls: [Arc<AtomicUsize>; 3] = Default::default();
    let first = counted(&alpha, &calls[0], 0x62);
    first.start_services();
    support::block_on(commands::remember_combined_channel(
        &first,
        "alpha-before".into(),
    ))
    .unwrap();
    assert_eq!(calls[0].load(Ordering::SeqCst), 1);

    // Constructed only: no thread, no directory, no key request, and the
    // first owner's next request still reaches its own library.
    let second = counted(&beta, &calls[1], 0x63);
    assert!(!beta.exists());
    assert_eq!(
        support::block_on(commands::combined_resume_key(&first))
            .unwrap()
            .as_deref(),
        Some("alpha-before")
    );

    // Started and used: each owner writes to its own library.
    second.start_services();
    support::block_on(commands::remember_combined_channel(&second, "beta".into())).unwrap();
    support::block_on(commands::remember_combined_channel(
        &first,
        "alpha-after".into(),
    ))
    .unwrap();

    // A third owner that is never started is refused instead of borrowing
    // an open owner's store.
    let third = counted(&gamma, &calls[2], 0x64);
    assert!(
        support::block_on(commands::remember_combined_channel(&third, "gamma".into())).is_err()
    );
    assert!(!gamma.exists());

    // Closing the second owner leaves the first one open.
    assert!(second.drain_services());
    assert!(
        support::block_on(commands::remember_combined_channel(&second, "late".into())).is_err()
    );
    assert_eq!(
        support::block_on(commands::combined_resume_key(&first))
            .unwrap()
            .as_deref(),
        Some("alpha-after")
    );
    assert!(first.drain_services());
    assert_eq!(
        calls.each_ref().map(|calls| calls.load(Ordering::SeqCst)),
        [1, 1, 0]
    );
    assert_eq!(
        combined_channel(&alpha, 0x62).as_deref(),
        Some("alpha-after")
    );
    assert_eq!(combined_channel(&beta, 0x63).as_deref(), Some("beta"));
}

#[test]
fn a_request_keeps_its_accepting_directory_and_key_broker_on_another_thread() {
    let directory = tempfile::tempdir().unwrap();
    let (alpha, beta) = (
        directory.path().join("alpha"),
        directory.path().join("beta"),
    );
    let calls: [Arc<AtomicUsize>; 2] = Default::default();
    let first = counted(&alpha, &calls[0], 0x62);
    first.start_services();
    // Accepted by the first owner; driven later, elsewhere.
    let accepted = commands::remember_combined_channel(&first, "synthetic-selection".into());
    let second = counted(&beta, &calls[1], 0x63);
    second.start_services();
    std::thread::spawn(move || support::block_on(accepted))
        .join()
        .unwrap()
        .unwrap();
    assert_eq!(calls[0].load(Ordering::SeqCst), 1);
    assert_eq!(calls[1].load(Ordering::SeqCst), 0);
    assert!(!beta.exists());
    assert!(first.drain_services());
    assert!(second.drain_services());
    assert_eq!(
        combined_channel(&alpha, 0x62).as_deref(),
        Some("synthetic-selection")
    );
}

#[test]
fn small_files_and_settings_belong_to_the_owner_that_was_asked() {
    let directory = tempfile::tempdir().unwrap();
    let (alpha, beta) = (
        directory.path().join("alpha"),
        directory.path().join("beta"),
    );
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    let first = owner(&alpha);
    let second = owner(&beta);
    for owner in [&first, &second] {
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        owner.load_settings();
    }
    first
        .set_setting(
            "general.user_agent",
            iptv_settings::Value::Text("SyntheticAlpha/1".into()),
        )
        .unwrap();
    first.save_last_channel(None, "alpha-channel");
    second.save_combined_channel("beta-combined");
    let agent = |owner: &Runtime| {
        owner.settings().text(
            iptv_settings::keys::USER_AGENT,
            &iptv_settings::Target::Global,
        )
    };
    assert_eq!(agent(&first), "SyntheticAlpha/1");
    assert_eq!(agent(&second), "");
    assert_eq!(
        first.load_last_channel(None).as_deref(),
        Some("alpha-channel")
    );
    assert_eq!(second.load_last_channel(None), None);
    assert_eq!(first.load_combined_channel(), None);
    assert_eq!(
        second.load_combined_channel().as_deref(),
        Some("beta-combined")
    );
    assert!(first.flush_files(Duration::from_secs(2)));
    assert!(second.flush_files(Duration::from_secs(2)));
    assert!(alpha.join("settings.json").is_file());
    assert!(alpha.join("last_channel").is_file());
    assert!(!alpha.join("last_channel_combined").exists());
    assert!(!beta.join("settings.json").exists());
    assert!(!beta.join("last_channel").exists());
    assert!(beta.join("last_channel_combined").is_file());
    assert!(first.drain_services());
    assert!(second.drain_services());
}

#[test]
fn no_handle_retains_its_owner() {
    let directory = tempfile::tempdir().unwrap();
    let first = owner(directory.path());
    let weak = Rc::downgrade(&first);
    let flusher = first.host_handle();
    first.start_services();
    assert!(first.drain_services());
    drop(first);
    assert!(weak.upgrade().is_none());
    // Nothing is left to wait for.
    assert!(flusher.flush_files(Duration::ZERO));
}
