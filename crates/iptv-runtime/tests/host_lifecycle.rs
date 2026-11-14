//! The native host can flush accepted writes while the local UI executor is stopped.
use iptv_runtime::{HostHandle, Ports, Runtime};
use std::{cell::RefCell, rc::Rc, time::Duration};

#[test]
fn stopping_the_host_flushes_settings_without_polling_the_local_reply() {
    let directory = tempfile::tempdir().unwrap();
    let owner = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| panic!("a settings-only host must not open the credential broker"),
    )));
    let weak = Rc::downgrade(&owner);
    let pending = Rc::new(RefCell::new(Vec::new()));
    let queued = pending.clone();
    owner.bind_tasks(
        move |task| queued.borrow_mut().push(task),
        |_| Box::pin(async {}),
    );
    // The host keeps this for its off-thread stop callback.
    let flusher: HostHandle = owner.host_handle();
    // Nothing accepted yet: nothing to wait for.
    assert!(flusher.flush_files(Duration::ZERO));
    owner.start_services();
    owner
        .set_setting(
            "general.user_agent",
            iptv_settings::Value::Text("SyntheticAgent/1".into()),
        )
        .unwrap();
    assert_eq!(pending.borrow().len(), 1);
    // Another owner constructed meanwhile does not become the one flushed.
    let other = tempfile::tempdir().unwrap();
    let _newer = Runtime::new(Ports::new(other.path().to_owned(), |_| {
        panic!("an unused owner must not open the credential broker")
    }));
    let stop = flusher.clone();
    assert!(
        std::thread::spawn(move || stop.flush_files(Duration::from_secs(1)))
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
    assert!(!other.path().join("settings.json").exists());
    assert!(owner.drain_services());
    pending.borrow_mut().clear();
    drop(owner);
    assert!(weak.upgrade().is_none());
    assert!(flusher.flush_files(Duration::ZERO));
}
