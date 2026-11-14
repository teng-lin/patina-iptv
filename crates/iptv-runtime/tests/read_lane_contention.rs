//! Import-driven display reads must keep using committed data while writes wait.
#![cfg(feature = "test-support")]
mod support;

use iptv_runtime::{Ports, Runtime};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};
use support::block_on;

#[test]
fn display_snapshot_and_catalogue_do_not_queue_behind_a_busy_writer() {
    let directory = tempfile::tempdir().unwrap();
    let owner = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x65; 32])),
    )));
    owner.start_services();
    // Open the reader before holding the writer: opening a reader itself
    // requires the writer to provide its already-validated database key.
    block_on(iptv_runtime::library::snapshot(&owner)).unwrap();
    owner.occupy_writer(Duration::from_secs(3));
    let started = Instant::now();
    block_on(iptv_runtime::library::snapshot(&owner)).unwrap();
    let worker = owner.worker_catalog_size(iptv_runtime::model::SourceId(1));
    let (groups, channels) = std::thread::spawn(worker).join().unwrap().unwrap();
    assert_eq!((groups, channels), (0, 0));
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "display reads waited for writer: {elapsed:?}"
    );
    assert!(owner.drain_services());
}
