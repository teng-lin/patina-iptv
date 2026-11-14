//! Guide work that cannot read the store still says when to look again, so
//! the periodic check survives; with automatic updates off it stays silent.
//!
//! That holds for a store that is unavailable as a whole and for one that
//! reads in part: a guide check whose catalogue reads while its guide
//! sources or its guide's state do not must not take the missing part for
//! "no guide configured" (which deletes the stored guide) or for "never
//! downloaded" (which downloads it and makes up a check).
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

const KEY: [u8; 32] = [0x69; 32];

/// A runtime over a store holding one playlist with a guide source whose
/// last download failed just now (the refresh policy backs off for it).
fn stored(directory: &Path) -> (Rc<Runtime>, SourceId) {
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, Source, SourceConfig},
    };
    let key = iptv_runtime::model::DatabaseKey::from_bytes(KEY);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(directory.join("iptv.sqlite"), &key).unwrap();
    let source = store
        .add_source(&Source {
            name: "Synthetic".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://synthetic.invalid/list.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let guide = iptv_runtime::guides::user_source(
        iptv_runtime::guides::FIRST_USER_FEED,
        // Nothing listens there: a download that is tried fails at once.
        SecretUrl::new("http://127.0.0.1:9/guide.xml"),
        None,
    );
    store.set_guide_sources(source, &[guide]).unwrap();
    store
        .record_epg_failure(source, iptv_runtime::model::ImportErrorKind::Network)
        .unwrap();
    drop(store);
    let owner = Rc::new(Runtime::new(Ports::new(directory.to_owned(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes(KEY))
    })));
    (owner, source)
}

/// The store's file through a connection of the test's own.
fn raw(directory: &Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(directory.join("iptv.sqlite")).unwrap();
    let key: String = KEY.iter().map(|byte| format!("{byte:02x}")).collect();
    connection
        .pragma_update(None, "key", format!("x'{key}'"))
        .unwrap();
    connection
}

/// What one timer check of `source` sends: its next checks, and how many
/// other messages (guide data, "no check").
fn timer_check(owner: &Rc<Runtime>, source: SourceId) -> (Vec<i64>, usize) {
    let worker = owner.scope_worker(move || {
        let (tx, mut rx) = unbounded_channel();
        block_on(guide_pipeline(
            source,
            &tx,
            GuideRead::Check,
            &CancelToken::new(),
        ));
        drop(tx);
        let (mut checks, mut others) = (Vec::new(), 0);
        while let Some(message) = rx.blocking_recv() {
            match message {
                Msg::NextCheck { at, .. } => checks.push(at),
                _ => others += 1,
            }
        }
        (checks, others)
    });
    std::thread::spawn(worker).join().unwrap()
}

#[test]
fn unreadable_guide_sources_are_not_taken_for_no_guide_configured() {
    let directory = tempfile::tempdir().unwrap();
    let (owner, source) = stored(directory.path());
    with_runtime(owner.clone(), || {
        owner.start_services();
        let before = iptv_runtime::imports::now_unix();
        // Readable: the policy backs off after the failed download.
        let (checks, others) = timer_check(&owner, source);
        assert_eq!((checks.len(), others), (1, 0), "{checks:?}");
        assert!(checks[0] > before);
        // The guide sources cannot be read any more; the catalogue can.
        raw(directory.path())
            .execute_batch("DROP TABLE guide_sources")
            .unwrap();
        let (checks, others) = timer_check(&owner, source);
        assert_eq!(
            (checks.len(), others),
            (1, 0),
            "a retry, and neither guide data nor \"no check\": {checks:?}"
        );
        assert!(checks[0] > before, "the retry is in the future");
        let kept: i64 = raw(directory.path())
            .query_row("SELECT count(*) FROM epg_guides", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, 1, "the stored guide's state was not deleted");
        owner.drain_services();
    });
}

#[test]
fn an_unreadable_guide_state_makes_up_no_check_when_guide_updates_are_manual() {
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
    let (owner, source) = stored(directory.path());
    with_runtime(owner.clone(), || {
        owner.start_services();
        iptv_runtime::settings::load();
        // Readable: the back-off is owed whatever the interval.
        let (checks, others) = timer_check(&owner, source);
        assert_eq!((checks.len(), others), (1, 0), "{checks:?}");
        // The guide's state cannot be read any more (its feeds are gone
        // from under it); the catalogue and the guide sources can.
        raw(directory.path())
            .execute_batch("DROP TABLE epg_feeds")
            .unwrap();
        let (checks, others) = timer_check(&owner, source);
        assert_eq!(
            (checks.len(), others),
            (0, 0),
            "silent: no download and no check made up: {checks:?}"
        );
        owner.drain_services();
    });
}
