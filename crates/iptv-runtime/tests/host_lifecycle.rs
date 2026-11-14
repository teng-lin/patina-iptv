//! The native host can flush accepted writes while the local UI executor is stopped.
use iptv_runtime::{Ports, Runtime, flush_pending_writes, with_runtime};
use std::{cell::RefCell, rc::Rc, time::Duration};

#[test]
fn stopping_the_host_flushes_settings_without_polling_the_local_reply() {
    assert!(flush_pending_writes(Duration::ZERO));
    assert!(iptv_runtime::try_current().is_none());
    let directory = tempfile::tempdir().unwrap();
    let owner = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| panic!("a settings-only host must not open the credential broker"),
    )));
    let weak = Rc::downgrade(&owner);
    let pending = Rc::new(RefCell::new(Vec::new()));
    let queued = pending.clone();
    owner.bind_tasks(
        move |task| queued.borrow_mut().push(iptv_runtime::in_runtime(task)),
        |_| Box::pin(async {}),
    );
    with_runtime(owner.clone(), || {
        owner.activate();
        owner.start_services();
        iptv_runtime::settings::set(
            "general.user_agent",
            iptv_settings::Value::Text("SyntheticAgent/1".into()),
        )
        .unwrap();
    });
    assert_eq!(pending.borrow().len(), 1);
    assert!(
        std::thread::spawn(|| flush_pending_writes(Duration::from_secs(1)))
            .join()
            .unwrap()
    );
    let saved = std::fs::read_to_string(directory.path().join("settings.json")).unwrap();
    let (settings, report) = iptv_settings::Settings::from_json(&saved).unwrap();
    assert!(report.is_clean());
    assert_eq!(
        settings.text(
            iptv_settings::keys::USER_AGENT,
            &iptv_settings::Target::Global
        ),
        "SyntheticAgent/1"
    );
    with_runtime(owner.clone(), || assert!(owner.drain_services()));
    pending.borrow_mut().clear();
    drop(owner);
    assert!(weak.upgrade().is_none());
    assert!(flush_pending_writes(Duration::ZERO));
}
