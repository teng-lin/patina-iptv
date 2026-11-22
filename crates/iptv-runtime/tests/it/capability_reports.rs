//! Inspection is I/O-free; a loaded native host refreshes permission without reopening credentials.
use crate::support;

use iptv_core::recording::WakeCapability;
use iptv_runtime::{Ports, Runtime, model::DatabaseKey};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    time::Duration,
};
use support::block_on;

static ALLOWED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static KEYS: AtomicUsize = AtomicUsize::new(0);
static FIRST: Mutex<Option<SyncSender<()>>> = Mutex::new(None);

fn register(_: i64, _: bool) -> WakeCapability {
    CALLS.fetch_add(1, Ordering::SeqCst);
    if let Some(sender) = FIRST.lock().unwrap().take() {
        sender.send(()).unwrap();
    }
    if ALLOWED.load(Ordering::SeqCst) {
        WakeCapability::Exact
    } else {
        WakeCapability::Denied
    }
}

#[test]
fn registration_permission_changes_update_capabilities_without_reopening_the_store() {
    let unopened = tempfile::tempdir().unwrap();
    let mut ports = Ports::new(unopened.path().to_owned(), |_| {
        panic!("capability inspection must not unlock the credential store")
    });
    ports.background_recordings = true;
    ports.recording_schedule = register;
    let owner = Runtime::new(ports);
    let initial = owner.recording_capabilities();
    assert!(initial.background);
    assert_eq!(initial.wake, None);
    assert!(!initial.exact_start());
    owner.start_services();
    block_on(owner.resume_recording_wake()).unwrap();
    assert_eq!(
        CALLS.load(Ordering::SeqCst),
        0,
        "unknown durable intent must not replace an OS alarm"
    );
    assert_eq!(owner.recording_capabilities().wake, None);
    assert!(!unopened.path().join("iptv.sqlite").exists());
    assert!(owner.drain_services());
    assert!(block_on(owner.refresh_recording_wake()).is_err());
    assert_eq!(CALLS.load(Ordering::SeqCst), 0);

    let dir = tempfile::tempdir().unwrap();
    let mut ports = Ports::new(dir.path().to_owned(), |_| {
        KEYS.fetch_add(1, Ordering::SeqCst);
        Ok(DatabaseKey::from_bytes([0x76; 32]))
    });
    ports.background_recordings = true;
    ports.recording_schedule = register;
    let owner = Runtime::new(ports);
    owner.start_services();
    let (sender, first) = sync_channel(1);
    *FIRST.lock().unwrap() = Some(sender);
    owner.start_recordings();
    first.recv_timeout(Duration::from_secs(10)).unwrap();
    block_on(owner.refresh_recording_wake()).unwrap();
    assert_eq!(
        owner.recording_capabilities().wake,
        Some(WakeCapability::Denied)
    );
    assert!(!owner.recording_capabilities().exact_start());
    let key_calls = KEYS.load(Ordering::SeqCst);
    assert!(key_calls > 0);
    ALLOWED.store(true, Ordering::SeqCst);
    block_on(owner.refresh_recording_wake()).unwrap();
    assert!(owner.recording_capabilities().exact_start());
    ALLOWED.store(false, Ordering::SeqCst);
    block_on(owner.refresh_recording_wake()).unwrap();
    assert_eq!(
        owner.recording_capabilities().wake,
        Some(WakeCapability::Denied)
    );
    assert!(!owner.recording_capabilities().exact_start());
    assert_eq!(
        KEYS.load(Ordering::SeqCst),
        key_calls,
        "permission refresh must not reopen credentials"
    );
    assert!(owner.shutdown_recording_host());
    assert!(owner.drain_services());
    assert_eq!(owner.recording_capabilities().wake, None);
    let calls = CALLS.load(Ordering::SeqCst);
    assert!(block_on(owner.refresh_recording_wake()).is_err());
    assert_eq!(
        CALLS.load(Ordering::SeqCst),
        calls,
        "retired refresh must not call the OS"
    );
}
