//! Import-driven display reads must keep using committed data while writes wait.
#![cfg(feature = "test-support")]

use iptv_runtime::{Ports, Runtime, executor::block_on, with_runtime};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

#[test]
fn display_snapshot_and_catalogue_do_not_queue_behind_a_busy_writer() {
    let directory = tempfile::tempdir().unwrap();
    let owner = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x65; 32])),
    )));
    with_runtime(owner.clone(), || {
        owner.start_services();
        // Open the reader before holding the writer: opening a reader itself
        // requires the writer to provide its already-validated database key.
        block_on(iptv_runtime::library::snapshot()).unwrap();
        iptv_runtime::occupy_writer(Duration::from_secs(3));
        let started = Instant::now();
        block_on(iptv_runtime::library::snapshot()).unwrap();
        let worker = owner.scope_worker(|| {
            iptv_runtime::stored::catalog(iptv_runtime::model::SourceId(1)).unwrap()
        });
        let (groups, channels) = std::thread::spawn(worker).join().unwrap();
        assert!(groups.is_empty() && channels.is_empty());
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "display reads waited for writer: {elapsed:?}"
        );
        assert!(owner.drain_services());
    });
}
