//! A restarted native host cannot advertise the previous run's OS permission.
mod support;

use iptv_core::recording::WakeCapability;
use iptv_runtime::{Ports, Runtime, model::DatabaseKey};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SyncSender, sync_channel},
};
use std::time::Duration;
use support::block_on;

type RegistrationGate = (SyncSender<()>, Receiver<()>);
static GATE: Mutex<Option<RegistrationGate>> = Mutex::new(None);
static DENIED: AtomicBool = AtomicBool::new(false);

fn register(_: i64, _: bool) -> WakeCapability {
    let gate = GATE.lock().unwrap().take();
    if let Some((entered, release)) = gate {
        entered.send(()).unwrap();
        release.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    if DENIED.load(Ordering::SeqCst) {
        WakeCapability::Denied
    } else {
        WakeCapability::Exact
    }
}

#[test]
fn restart_waits_for_its_new_registration_before_exposing_exact_wake() {
    let dir = tempfile::tempdir().unwrap();
    let mut ports = Ports::new(dir.path().to_owned(), |_| {
        Ok(DatabaseKey::from_bytes([0x79; 32]))
    });
    ports.background_recordings = true;
    ports.recording_schedule = register;
    let owner = Runtime::new(ports);
    owner.start_services();
    let (entered, seen) = sync_channel(1);
    let (release, parked) = sync_channel(1);
    *GATE.lock().unwrap() = Some((entered, parked));
    owner.start_recordings();
    seen.recv_timeout(Duration::from_secs(10)).unwrap();
    release.send(()).unwrap();
    block_on(owner.refresh_recording_wake()).unwrap();
    assert!(owner.recording_capabilities().exact_start());
    assert!(owner.shutdown_recording_host());
    assert_eq!(owner.recording_capabilities().wake, None);

    DENIED.store(true, Ordering::SeqCst);
    let (entered, seen) = sync_channel(1);
    let (release, parked) = sync_channel(1);
    *GATE.lock().unwrap() = Some((entered, parked));
    owner.start_recordings();
    seen.recv_timeout(Duration::from_secs(10)).unwrap();
    let pending = owner.recording_capabilities();
    // Always release the native worker before asserting, including on a regression.
    release.send(()).unwrap();
    block_on(owner.refresh_recording_wake()).unwrap();
    assert_eq!(
        pending.wake, None,
        "old exact support must not cross native run generations"
    );
    assert_eq!(
        owner.recording_capabilities().wake,
        Some(WakeCapability::Denied)
    );
    assert!(owner.shutdown_recording_host());
    assert!(owner.drain_services());
}
