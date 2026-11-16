//! Dynamic host reports must replace stale assumptions without touching provider data.
mod support;

use iptv_core::recording::WakeCapability;
use iptv_runtime::{Ports, Runtime};
use std::sync::atomic::{AtomicBool, Ordering};
use support::block_on;

static ALLOWED: AtomicBool = AtomicBool::new(false);

#[test]
fn registration_permission_changes_update_capabilities_without_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut ports = Ports::new(dir.path().to_owned(), |_| {
        panic!("capability inspection must not unlock the credential store")
    });
    ports.background_recordings = true;
    ports.recording_schedule = |_, _| {
        if ALLOWED.load(Ordering::SeqCst) {
            WakeCapability::Exact
        } else {
            WakeCapability::Denied
        }
    };
    let owner = Runtime::new(ports);
    let initial = owner.recording_capabilities();
    assert!(initial.background);
    assert_eq!(initial.wake, None);
    assert!(!initial.exact_start());
    assert!(!dir.path().join("iptv.sqlite").exists());
    owner.start_services();
    block_on(owner.refresh_recording_wake()).unwrap();
    assert_eq!(
        owner.recording_capabilities().wake,
        Some(WakeCapability::Denied)
    );
    assert!(owner.recording_capabilities().notice().is_some());
    ALLOWED.store(true, Ordering::SeqCst);
    block_on(owner.refresh_recording_wake()).unwrap();
    assert!(owner.recording_capabilities().exact_start());
    ALLOWED.store(false, Ordering::SeqCst);
    block_on(owner.refresh_recording_wake()).unwrap();
    assert!(!owner.recording_capabilities().exact_start());
    assert_eq!(
        owner.recording_capabilities().wake,
        Some(WakeCapability::Denied)
    );
    assert!(!dir.path().join("iptv.sqlite").exists());
    assert!(owner.drain_services());
    assert_eq!(
        owner.recording_capabilities().wake,
        None,
        "a retired owner cannot advertise its old registration"
    );
}
