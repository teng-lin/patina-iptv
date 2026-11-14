//! The test-support view of registered guide checks belongs to one runtime:
//! two owners in one process do not see each other's checks.
#![cfg(feature = "test-support")]

use iptv_runtime::{GuideWork, Ports, Runtime, model::SourceId};
use std::rc::Rc;

fn owner(directory: &std::path::Path) -> Rc<Runtime> {
    Rc::new(Runtime::new(Ports::new(directory.to_owned(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x6f; 32]))
    })))
}

#[test]
fn two_owners_do_not_share_registered_guide_checks() {
    let (first_home, second_home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (first, second) = (owner(first_home.path()), owner(second_home.path()));
    // The same id in both libraries, as two installations have.
    let source = SourceId(1);
    let registered = |owner: &Rc<Runtime>| owner.host_handle().guide_check_registered(source);
    let generation = |owner: &Rc<Runtime>| owner.browser_generation();

    let timer = GuideWork::Timer(1_800_000_000);
    first
        .schedule_guide_work(source, generation(&first), timer)
        .unwrap();
    assert!(registered(&first));
    assert!(
        !registered(&second),
        "another owner's check is not this one's"
    );

    second
        .schedule_guide_check(source, generation(&second))
        .unwrap();
    // Work that takes the first one's guide over ends its timer only.
    first.cancel_guide_check(source);
    assert!(!registered(&first));
    assert!(
        registered(&second),
        "ending one owner's check leaves the other's"
    );

    // A closed owner's checks end with it, and only its own.
    first
        .schedule_guide_check(source, generation(&first))
        .unwrap();
    second.cancel_guide_checks();
    assert!(registered(&first) && !registered(&second));
    drop(second);
    assert!(registered(&first));
    // The active owner answers a thread without a context of its own
    // (the headless tests' thread).
    let host = first.host_handle();
    assert!(
        std::thread::spawn(move || host.guide_check_registered(source))
            .join()
            .unwrap()
    );
}
