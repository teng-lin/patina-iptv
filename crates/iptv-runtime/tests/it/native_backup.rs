//! Public backup service against encrypted stores and managed documents.
use crate::support;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_runtime::{Ports, Runtime, RuntimeEvent, backup, commands};
use iptv_store_sqlite::{DatabaseKey, SourceId, Store};
use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};
use support::block_on;

const KEY: [u8; 32] = [0x69; 32];
const TEXT: &[u8] =
    b"#EXTM3U\n#EXTINF:-1,Generated backed up channel\nhttps://example.test/generated.ts\n";

fn runtime(root: &Path) -> Rc<Runtime> {
    let owner = Rc::new(Runtime::new(Ports::new(root.into(), |_| {
        Ok(DatabaseKey::from_bytes(KEY))
    })));
    owner.start_services();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner
}

fn seed(owner: &Runtime, name: &str) -> (SourceId, PathBuf) {
    let document = block_on(owner.keep_picked_file("generated.m3u", TEXT)).unwrap();
    let mut store = Store::open_encrypted(
        owner.data_dir().join("iptv.sqlite"),
        &DatabaseKey::from_bytes(KEY),
    )
    .unwrap();
    let id = store
        .add_source(&Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(&document),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(std::str::from_utf8(TEXT).unwrap());
    store
        .replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_favorite(id, &parsed.channels.items[0].id, true)
        .unwrap();
    (id, PathBuf::from(document))
}

fn name(owner: &Runtime, id: SourceId) -> String {
    Store::open_encrypted(
        owner.data_dir().join("iptv.sqlite"),
        &DatabaseKey::from_bytes(KEY),
    )
    .unwrap()
    .source(id)
    .unwrap()
    .unwrap()
    .source
    .name
}

#[test]
fn native_backup_round_trip_reencrypts_documents_and_preserves_other_owner_events() {
    let directory = tempfile::tempdir().unwrap();
    let original = runtime(&directory.path().join("generated-original"));
    let (source, document) = seed(&original, "Generated original");
    let guide_text =
        b"<tv><channel id=\"generated\"><display-name>Generated</display-name></channel></tv>";
    let guide = block_on(original.keep_picked_file("generated.xml", guide_text)).unwrap();
    let orphan = block_on(original.keep_picked_file("generated-unused.m3u", b"#EXTM3U\n")).unwrap();
    {
        let mut store = Store::open_encrypted(
            original.data_dir().join("iptv.sqlite"),
            &DatabaseKey::from_bytes(KEY),
        )
        .unwrap();
        store
            .set_guide_sources(
                source,
                &[iptv_store_sqlite::GuideSource {
                    feed: iptv_epg::FeedId(1),
                    provider_index: None,
                    url: SecretUrl::new(&guide),
                    name: None,
                    enabled: true,
                    shift_minutes: 0,
                }],
            )
            .unwrap();
    }
    original
        .set_setting(
            "general.user_agent",
            iptv_settings::Value::Text("GeneratedBackup/1".into()),
        )
        .unwrap();
    assert!(original.flush_files(Duration::from_secs(5)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    original.bind(move |event| {
        if let RuntimeEvent::BackupProgress(progress) = event {
            observed.borrow_mut().push(progress);
        }
    });
    let target_root = directory.path().join("generated-target");
    let target = runtime(&target_root);
    let (target_id, _) = seed(&target, "Generated target");
    // A recording this device made: its media stays beside the catalog
    // under this job id, so the restored library keeps the row.
    let kept = iptv_core::recording::JobId(931);
    {
        let mut store = Store::open_encrypted(
            target_root.join("iptv.sqlite"),
            &DatabaseKey::from_bytes(KEY),
        )
        .unwrap();
        let request = iptv_core::recording::Request {
            source: target_id.0,
            channel: iptv_providers::m3u::parse_m3u(std::str::from_utf8(TEXT).unwrap())
                .channels
                .items[0]
                .id
                .to_string(),
            channel_name: "Generated target channel".into(),
            title: "Generated kept recording".into(),
            start: 1_800_000_000,
            end: 1_800_000_060,
        };
        store
            .schedule_recording(kept, &request, 1_799_999_000)
            .unwrap();
        assert!(
            store
                .transition_recording(
                    kept,
                    iptv_core::recording::Status::Scheduled,
                    iptv_core::recording::Status::Completed,
                    376,
                    "",
                )
                .unwrap()
        );
    }
    let media = iptv_runtime::recordings::media_directory(&target_root);
    fs::create_dir_all(&media).unwrap();
    fs::write(media.join(format!("{}.ts", kept.0)), [0x47; 376]).unwrap();
    let wrong_events = Rc::new(RefCell::new(0));
    let wrong = wrong_events.clone();
    target.bind(move |event| {
        if matches!(event, RuntimeEvent::BackupProgress(_)) {
            *wrong.borrow_mut() += 1;
        }
    });
    let busy = original.begin_backup().unwrap();
    assert!(
        original.begin_backup().is_none(),
        "one owner must not start overlapping archives"
    );
    let destination = directory.path().join("generated.patinabackup");
    let output = destination.clone();
    let settings = original.settings().to_json_string();
    let expected_root = original.data_dir().to_owned();
    block_on(busy.worker(move |archive| {
        assert_eq!(archive.root(), expected_root);
        archive.export(&output, &settings)
    }))
    .unwrap();
    drop(busy);
    let exported = fs::read(&destination).unwrap();
    let header_length = u64::from_le_bytes(exported[9..17].try_into().unwrap()) as usize;
    let manifest = iptv_import::backup_manifest::parse(&exported[17..17 + header_length]).unwrap();
    let relative = |path: &str| {
        Path::new(path)
            .strip_prefix(original.data_dir())
            .unwrap()
            .to_str()
            .unwrap()
            .replace(std::path::MAIN_SEPARATOR, "/")
    };
    assert!(
        manifest
            .files
            .iter()
            .any(|entry| entry.name == relative(&guide))
    );
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.name == relative(&orphan))
    );
    assert_eq!(
        &*events.borrow(),
        &[
            backup::Progress::Selecting,
            backup::Progress::Working,
            backup::Progress::Idle
        ]
    );
    assert_eq!(*wrong_events.borrow(), 0);
    assert!(
        !fs::read(document).unwrap().starts_with(TEXT),
        "live managed documents must stay encrypted"
    );
    let busy = target.begin_backup().unwrap();
    let input = destination.clone();
    block_on(busy.worker(move |archive| archive.stage(&mut fs::File::open(input).unwrap())))
        .unwrap();
    assert!(target.restore_is_pending());
    assert_eq!(
        name(&target, target_id),
        "Generated target",
        "staging cannot replace the live owner"
    );
    let pending = backup::sibling(&target_root, "restore-pending");
    let encrypted = fs::read(pending.join("iptv.sqlite")).unwrap();
    assert!(!encrypted.starts_with(b"SQLite format 3\0"));
    // A failed new staging request must leave the previously accepted restore exact.
    assert!(
        block_on(busy.worker(|archive| archive.stage(&mut &b"generated malformed archive"[..])))
            .is_err()
    );
    assert_eq!(fs::read(pending.join("iptv.sqlite")).unwrap(), encrypted);
    drop(busy);
    assert!(target.drain_services());
    drop(target);
    let restarted = runtime(&target_root);
    std::thread::spawn(restarted.restore_activation())
        .join()
        .unwrap()
        .unwrap_or_else(|_| panic!("generated restore activation failed"));
    assert!(!restarted.restore_is_pending());
    restarted.load_settings();
    assert_eq!(
        restarted.settings().text(
            iptv_settings::keys::USER_AGENT,
            &iptv_settings::Target::Global
        ),
        "GeneratedBackup/1"
    );
    let store = Store::open_encrypted(
        target_root.join("iptv.sqlite"),
        &DatabaseKey::from_bytes(KEY),
    )
    .unwrap();
    assert_eq!(
        store.source(source).unwrap().unwrap().source.name,
        "Generated original"
    );
    let channels = store.load_channels(source, &Default::default()).unwrap();
    assert_eq!(channels[0].name, "Generated backed up channel");
    assert!(store.is_favorite(source, &channels[0].channel.id).unwrap());
    // Its media, beside the catalog directory, still opens.
    assert_eq!(
        fs::read(block_on(restarted.recording_file(kept)).unwrap()).unwrap(),
        [0x47; 376]
    );
    let recording = store.recording_job(kept).unwrap().unwrap();
    assert_eq!(
        (recording.status, recording.bytes),
        (iptv_core::recording::Status::Completed, 376),
        "this device's recording survives the restore"
    );
    let SourceConfig::M3u(config) = store.source(source).unwrap().unwrap().source.config else {
        unreachable!()
    };
    let restored = Path::new(config.location.expose());
    assert!(restored.starts_with(&target_root));
    use std::io::Read;
    let mut restored_bytes = Vec::new();
    store
        .read_private_file(restored)
        .unwrap()
        .read_to_end(&mut restored_bytes)
        .unwrap();
    assert_eq!(restored_bytes, TEXT);
    assert!(!fs::read(restored).unwrap().starts_with(TEXT));
    let guides = store.guide_sources(source).unwrap();
    let restored_guide = Path::new(guides[0].url.expose());
    assert!(restored_guide.starts_with(&target_root));
    let mut restored_guide_bytes = Vec::new();
    store
        .read_private_file(restored_guide)
        .unwrap()
        .read_to_end(&mut restored_guide_bytes)
        .unwrap();
    assert_eq!(restored_guide_bytes, guide_text);
    assert!(!fs::read(restored_guide).unwrap().starts_with(guide_text));
    assert!(!target_root.join(relative(&orphan)).exists());
    assert!(original.drain_services());
    assert!(restarted.drain_services());
}

#[test]
fn cancellation_and_parental_denial_do_not_mutate_live_or_pending_data() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    let owner = runtime(&root);
    let (source, _) = seed(&owner, "Generated retained source");
    let busy = owner.begin_backup().unwrap();
    let destination = directory.path().join("generated.patinabackup");
    let output = destination.clone();
    block_on(busy.worker(move |archive| {
        archive.export(&output, &iptv_settings::Settings::new().to_json_string())
    }))
    .unwrap();
    block_on(busy.worker(move |archive| archive.stage(&mut fs::File::open(destination).unwrap())))
        .unwrap();
    assert!(owner.restore_is_pending());
    block_on(busy.worker(|archive| archive.cancel())).unwrap();
    assert!(!owner.restore_is_pending());
    block_on(busy.worker(|archive| archive.cancel())).unwrap();
    drop(busy);
    block_on(owner.parental_change_pin(None, iptv_core::parental::Pin::parse("2468").unwrap()))
        .unwrap();
    owner.parental_lock();
    let busy = owner.begin_backup().unwrap();
    let denied = block_on(busy.worker(|_| -> backup::error::Result<()> {
        panic!("locked backup worker must not enter")
    }))
    .unwrap_err();
    assert_eq!(
        denied.message(),
        "Unlock parental controls before changing settings"
    );
    assert_eq!(name(&owner, source), "Generated retained source");
    assert!(!owner.restore_is_pending());
    assert!(!format!("{denied}").contains("2468"));
    drop(busy);
    assert!(owner.drain_services());
}

#[test]
fn failed_private_scratch_admission_preserves_the_destination_and_cleans_its_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    let mut ports = Ports::new(root.clone(), |_| Ok(DatabaseKey::from_bytes(KEY)));
    ports.exclude_from_backup = |_| Err(std::io::ErrorKind::PermissionDenied.into());
    let owner = Rc::new(Runtime::new(ports));
    owner.start_services();
    block_on(commands::remember_combined_channel(
        &owner,
        "generated-retained".into(),
    ))
    .unwrap();
    let destination = directory.path().join("generated-existing.patinabackup");
    fs::write(&destination, b"generated retained archive").unwrap();
    let output = destination.clone();
    let busy = owner.begin_backup().unwrap();
    let error = block_on(busy.worker(move |archive| {
        archive.export(&output, &iptv_settings::Settings::new().to_json_string())
    }))
    .unwrap_err();
    assert_eq!(
        fs::read(destination).unwrap(),
        b"generated retained archive"
    );
    assert!(!format!("{error}").contains(root.to_str().unwrap()));
    assert!(
        !fs::read_dir(directory.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("generated-app.backup-work-")),
        "refused private scratch must be removed immediately"
    );
    drop(busy);
    assert!(owner.drain_services());
}
