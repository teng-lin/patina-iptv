//! Recovery rotates one-way credentials and revokes content grants across owners.
use crate::common;
use crate::support;
use iptv_core::parental::{Pin, Target};
use iptv_runtime::{Ports, Runtime, parental::Error};
use iptv_settings::Value;
use iptv_store_sqlite::{DatabaseKey, Store};
use std::{path::Path, rc::Rc, time::Duration};
use support::block_on;

fn owner(root: &Path) -> Rc<Runtime> {
    let owner = Rc::new(Runtime::new(Ports::new(root.into(), |_| {
        Ok(DatabaseKey::from_bytes([0x71; 32]))
    })));
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    owner.load_settings();
    owner
}

#[test]
fn recovery_rotates_the_code_preserves_blocks_and_revokes_previous_content_grants() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x71; 32]),
    )
    .unwrap();
    let source = store
        .add_source(&common::m3u_source("Generated recovery source"))
        .unwrap();
    let catalog = common::catalog(2, 2, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalog);
    drop(store);
    let runtime = owner(directory.path());
    let target = Target::live(source.0, catalog.channels[0].id.as_str());
    assert_eq!(
        block_on(runtime.parental_recover_pin(
            zeroize::Zeroizing::new("generated-no-code".into()),
            Pin::parse("8642").unwrap()
        )),
        Err(Error::NoPin)
    );
    let first_code =
        block_on(runtime.parental_change_pin(None, Pin::parse("2468").unwrap())).unwrap();
    runtime
        .set_setting("parental.live_enabled", Value::Bool(true))
        .unwrap();
    runtime
        .set_setting(
            "parental.unlock_window",
            Value::Enum("until_restart".into()),
        )
        .unwrap();
    block_on(runtime.parental_set_blocked(target.clone(), true)).unwrap();
    block_on(runtime.parental_authorize(target.clone(), Pin::parse("2468").unwrap())).unwrap();
    assert!(runtime.parental_allowed(&target));
    assert!(runtime.parental_explicitly_blocked(&target));
    assert!(runtime.parental_is_blocked(&target));
    assert_eq!(
        block_on(runtime.parental_recover_pin(
            zeroize::Zeroizing::new("generated-invalid-code".into()),
            Pin::parse("8642").unwrap()
        )),
        Err(Error::Incorrect)
    );
    assert!(
        runtime.parental_allowed(&target),
        "failed recovery cannot revoke a valid existing content grant"
    );
    let second_code =
        block_on(runtime.parental_recover_pin(first_code.clone(), Pin::parse("8642").unwrap()))
            .unwrap();
    assert!(
        first_code.as_str() != second_code.as_str(),
        "successful recovery must rotate its one-time code"
    );
    assert!(
        !runtime.parental_allowed(&target),
        "the previous PIN's content grant must be revoked"
    );
    assert!(runtime.parental_explicitly_blocked(&target));
    assert_eq!(
        block_on(runtime.parental_authorize(target.clone(), Pin::parse("2468").unwrap())),
        Err(Error::Incorrect)
    );
    block_on(runtime.parental_authorize(target.clone(), Pin::parse("8642").unwrap())).unwrap();
    assert_eq!(
        block_on(runtime.parental_recover_pin(first_code, Pin::parse("1357").unwrap())),
        Err(Error::Incorrect)
    );
    assert!(runtime.flush_files(Duration::from_secs(5)));
    assert!(runtime.drain_services());
    drop(runtime);
    let restarted = owner(directory.path());
    block_on(restarted.parental_refresh()).unwrap();
    assert!(!restarted.parental_allowed(&target));
    assert!(restarted.parental_explicitly_blocked(&target));
    block_on(restarted.parental_authorize(target.clone(), Pin::parse("8642").unwrap())).unwrap();
    // Session-scoped grants survive content close/background, then explicit lock
    // revokes them; every-time policy revokes on those lifecycle observations.
    restarted.parental_content_left(&target);
    restarted.parental_content_closed();
    restarted.parental_backgrounded();
    assert!(restarted.parental_allowed(&target));
    block_on(restarted.parental_authorize(Target::settings(), Pin::parse("8642").unwrap()))
        .unwrap();
    restarted
        .set_setting("parental.unlock_window", Value::Enum("every_time".into()))
        .unwrap();
    block_on(restarted.parental_authorize(target.clone(), Pin::parse("8642").unwrap())).unwrap();
    restarted.parental_content_closed();
    assert!(!restarted.parental_allowed(&target));
    block_on(restarted.parental_authorize(target.clone(), Pin::parse("8642").unwrap())).unwrap();
    restarted.parental_backgrounded();
    assert!(!restarted.parental_allowed(&target));
    assert!(restarted.drain_services());
}
