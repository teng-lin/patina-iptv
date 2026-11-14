//! Guide work that cannot read the store still says when to look again, so
//! the periodic check survives; with automatic updates off it stays silent.
use iptv_import::CancelToken;
use iptv_runtime::{
    Ports, Runtime,
    executor::block_on,
    imports::{GuideRead, Msg, guide_pipeline},
    model::{SourceId, StoreError},
    stored::download_now,
    with_runtime,
};
use std::{path::Path, rc::Rc};
use tokio::sync::mpsc::unbounded_channel;

/// A runtime whose store cannot be opened (its key is refused), as while
/// the device's credential storage is locked.
fn locked(directory: &Path) -> Rc<Runtime> {
    Rc::new(Runtime::new(Ports::new(directory.to_owned(), |_| {
        Err(StoreError::Unavailable("synthetic locked credential store"))
    })))
}

/// The next checks one timer check and one requested download ask for.
fn next_checks(owner: &Rc<Runtime>) -> Vec<(SourceId, i64)> {
    let source = SourceId(7);
    let worker = owner.scope_worker(move || {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        block_on(guide_pipeline(source, &tx, GuideRead::Check, &cancel));
        // No configs could be read either: the retry must not depend on them.
        block_on(download_now(source, Vec::new(), &tx, &cancel));
        drop(tx);
        let mut checks = Vec::new();
        while let Some(message) = rx.blocking_recv() {
            match message {
                Msg::NextCheck { source, at } => checks.push((source, at)),
                _ => panic!("an unreadable store produced guide data"),
            }
        }
        checks
    });
    std::thread::spawn(worker).join().unwrap()
}

#[test]
fn an_unreadable_store_keeps_automatic_guide_checks() {
    let directory = tempfile::tempdir().unwrap();
    let owner = locked(directory.path());
    with_runtime(owner.clone(), || {
        owner.start_services();
        let interval = iptv_import::RefreshPolicy::from_settings(
            &iptv_runtime::settings::get(),
            &iptv_settings::Target::Global,
        )
        .interval;
        assert!(interval.is_some(), "automatic guide updates are on");
        let before = iptv_runtime::imports::now_unix();
        let checks = next_checks(&owner);
        assert_eq!(
            checks.len(),
            2,
            "the timer check and the download: {checks:?}"
        );
        for (source, at) in checks {
            assert_eq!(source, SourceId(7));
            assert!(at > before, "the retry is in the future");
        }
        owner.drain_services();
    });
}

#[test]
fn an_unreadable_store_arms_no_check_when_guide_updates_are_manual() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("epg.update_interval", iptv_settings::Value::ms(0))
        .unwrap();
    std::fs::write(
        directory.path().join("settings.json"),
        settings.to_json_string(),
    )
    .unwrap();
    let owner = locked(directory.path());
    with_runtime(owner.clone(), || {
        owner.start_services();
        iptv_runtime::settings::load();
        assert!(next_checks(&owner).is_empty());
        owner.drain_services();
    });
}
