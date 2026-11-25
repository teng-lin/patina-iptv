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
    io::{self, Read},
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

/// Rewrites only generated archives, retaining their file ordering and lengths.
fn rewrite_generated_archive(
    archive: &[u8],
    edit: impl FnOnce(&mut serde_json::Value, &mut Vec<Vec<u8>>),
) -> Vec<u8> {
    let size = u64::from_le_bytes(archive[9..17].try_into().unwrap()) as usize;
    let mut manifest: serde_json::Value = serde_json::from_slice(&archive[17..17 + size]).unwrap();
    let mut offset = 17 + size;
    let mut payloads = Vec::new();
    for entry in manifest["files"].as_array().unwrap() {
        let length = entry["length"].as_u64().unwrap() as usize;
        payloads.push(archive[offset..offset + length].to_vec());
        offset += length;
    }
    edit(&mut manifest, &mut payloads);
    if let Some(entries) = manifest
        .get_mut("files")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (entry, payload) in entries.iter_mut().zip(&payloads) {
            entry["length"] = serde_json::json!(payload.len());
        }
    }
    let header = serde_json::to_vec(&manifest).unwrap();
    let mut bytes = b"PATINABK\x01".to_vec();
    bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&header);
    for payload in payloads {
        bytes.extend_from_slice(&payload);
    }
    bytes
}

fn generated_tree(path: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(path, path, &mut files);
    files
}

/// Delivers a valid archive header, then an explicit reader error inside the
/// first payload. Ordinary file truncation instead returns a short byte count.
struct PayloadReadFailure {
    input: io::Cursor<Vec<u8>>,
    fail_at: u64,
    kind: io::ErrorKind,
}

impl Read for PayloadReadFailure {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let remaining = self.fail_at.saturating_sub(self.input.position()) as usize;
        if remaining == 0 {
            return Err(self.kind.into());
        }
        let count = buffer.len().min(remaining);
        self.input.read(&mut buffer[..count])
    }
}

#[test]
fn compatibility_rejections_preserve_current_data_and_the_exact_queued_restore() {
    use backup::error::Rejection;
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let original = runtime(&directory.path().join("generated-backup-owner"));
    let (archived_source, _) = seed(&original, "Generated queued source");
    let settings = iptv_settings::Settings::new().to_json_string();
    let destination = directory.path().join("generated-accepted.patinabackup");
    let output = destination.clone();
    let export = original.begin_backup().unwrap();
    block_on(export.worker(move |archive| archive.export(&output, &settings))).unwrap();
    drop(export);
    let accepted = fs::read(destination).unwrap();

    let root = directory.path().join("generated-current-owner");
    let current = runtime(&root);
    let (live_source, document) = seed(&current, "Generated current source");
    current
        .set_setting(
            "general.user_agent",
            iptv_settings::Value::text("GeneratedCurrent/1"),
        )
        .unwrap();
    assert!(current.flush_files(Duration::from_secs(5)));
    let live_settings = fs::read(root.join("settings.json")).unwrap();
    let live_document = fs::read(&document).unwrap();
    let busy = current.begin_backup().unwrap();
    let bytes = accepted.clone();
    block_on(busy.worker(move |archive| archive.stage(&mut bytes.as_slice()))).unwrap();
    let pending = backup::sibling(&root, "restore-pending");
    let queued = generated_tree(&pending);
    assert!(!queued.is_empty());

    let mut cases = Vec::new();
    let mut future_envelope = accepted.clone();
    future_envelope[8] = 2;
    cases.push((
        "future envelope",
        future_envelope,
        Rejection::Format {
            found: Some(2),
            supported: 1,
        },
    ));
    cases.push((
        "future manifest shape",
        rewrite_generated_archive(&accepted, |header, _| {
            header["version"] = json!(2);
            header.as_object_mut().unwrap().remove("root");
            header.as_object_mut().unwrap().remove("files");
        }),
        Rejection::Format {
            found: Some(2),
            supported: 1,
        },
    ));
    cases.push((
        "foreign format",
        rewrite_generated_archive(&accepted, |header, _| {
            header["format"] = json!("generated-private-format-token");
        }),
        Rejection::Format {
            found: None,
            supported: 1,
        },
    ));
    for version in [serde_json::Value::Null, json!("2"), json!(0)] {
        cases.push((
            "malformed manifest version",
            rewrite_generated_archive(&accepted, |header, _| {
                if version.is_null() {
                    header.as_object_mut().unwrap().remove("version");
                } else {
                    header["version"] = version;
                }
            }),
            Rejection::Damaged,
        ));
    }
    for (label, version, changed_shape, expected) in [
        (
            "future settings shape",
            json!(iptv_settings::backup::VERSION + 1),
            true,
            Rejection::Settings {
                found: Some(u64::from(iptv_settings::backup::VERSION + 1)),
                supported: u64::from(iptv_settings::backup::VERSION),
            },
        ),
        (
            "missing settings version",
            serde_json::Value::Null,
            false,
            Rejection::Damaged,
        ),
        (
            "nonnumeric settings version",
            json!("7"),
            false,
            Rejection::Damaged,
        ),
        ("zero settings version", json!(0), false, Rejection::Damaged),
    ] {
        cases.push((
            label,
            rewrite_generated_archive(&accepted, |header, payloads| {
                let index = header["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|e| e["name"] == "settings.json")
                    .unwrap();
                let mut settings: serde_json::Value =
                    serde_json::from_slice(&payloads[index]).unwrap();
                if version.is_null() {
                    settings.as_object_mut().unwrap().remove("version");
                } else {
                    settings["version"] = version;
                }
                if changed_shape {
                    settings["global"] = json!(["generated future layer"]);
                }
                payloads[index] = serde_json::to_vec(&settings).unwrap();
            }),
            expected,
        ));
    }
    let database = directory.path().join("generated-future.sqlite");
    cases.push(("future database", rewrite_generated_archive(&accepted, |header, payloads| {
        let index = header["files"].as_array().unwrap().iter().position(|e| e["name"] == "iptv.sqlite").unwrap();
        fs::write(&database, &payloads[index]).unwrap();
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES (?1,'generated_future',0)", [iptv_store_sqlite::SCHEMA_VERSION + 1]).unwrap();
        db.pragma_update(None, "user_version", iptv_store_sqlite::SCHEMA_VERSION + 1).unwrap();
        drop(db);
        payloads[index] = fs::read(&database).unwrap();
    }), Rejection::Database { found: iptv_store_sqlite::SCHEMA_VERSION + 1, supported: iptv_store_sqlite::SCHEMA_VERSION }));
    for name in ["settings.json", "iptv.sqlite"] {
        cases.push((
            "damaged payload",
            rewrite_generated_archive(&accepted, |header, payloads| {
                let index = header["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|e| e["name"] == name)
                    .unwrap();
                payloads[index] = b"generated damaged payload".to_vec();
            }),
            Rejection::Damaged,
        ));
    }

    let mut zero_envelope = accepted.clone();
    zero_envelope[8] = 0;
    cases.push(("zero envelope", zero_envelope, Rejection::Damaged));
    cases.push((
        "truncated envelope",
        b"PATINABK".to_vec(),
        Rejection::Damaged,
    ));
    cases.push((
        "large future manifest",
        rewrite_generated_archive(&accepted, |header, _| {
            header["version"] = json!(u64::MAX);
        }),
        Rejection::Format {
            found: Some(u64::MAX),
            supported: 1,
        },
    ));
    for malformed in [
        serde_json::Value::Null,
        json!([]),
        json!({}),
        json!({"format": 17, "version": 1}),
    ] {
        cases.push((
            "malformed manifest shape",
            rewrite_generated_archive(&accepted, |header, _| {
                *header = malformed;
            }),
            Rejection::Damaged,
        ));
    }
    for (label, replacement, expected) in [
        (
            "foreign settings format",
            json!({"format": "generated-private-format-token", "version": 1}),
            Rejection::Settings {
                found: None,
                supported: u64::from(iptv_settings::backup::VERSION),
            },
        ),
        (
            "large future settings",
            json!({"format": iptv_settings::backup::FORMAT, "version": u64::MAX}),
            Rejection::Settings {
                found: Some(u64::MAX),
                supported: u64::from(iptv_settings::backup::VERSION),
            },
        ),
        (
            "unknown settings",
            json!({"format": iptv_settings::backup::FORMAT, "version": iptv_settings::backup::VERSION, "global": {"generated.future_setting": true}}),
            Rejection::Settings {
                found: Some(u64::from(iptv_settings::backup::VERSION)),
                supported: u64::from(iptv_settings::backup::VERSION),
            },
        ),
        ("null settings", serde_json::Value::Null, Rejection::Damaged),
        ("array settings", json!([]), Rejection::Damaged),
        (
            "missing settings format",
            json!({"version": 1}),
            Rejection::Damaged,
        ),
        (
            "nonstrings settings format",
            json!({"format": 17, "version": 1}),
            Rejection::Damaged,
        ),
    ] {
        cases.push((
            label,
            rewrite_generated_archive(&accepted, |header, payloads| {
                let index = header["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|e| e["name"] == "settings.json")
                    .unwrap();
                payloads[index] = serde_json::to_vec(&replacement).unwrap();
            }),
            expected,
        ));
    }
    cases.push((
        "invalid UTF8 settings",
        rewrite_generated_archive(&accepted, |header, payloads| {
            let index = header["files"]
                .as_array()
                .unwrap()
                .iter()
                .position(|e| e["name"] == "settings.json")
                .unwrap();
            payloads[index] = vec![0xff, 0xfe];
        }),
        Rejection::Damaged,
    ));

    cases.push((
        "missing referenced local playlist",
        rewrite_generated_archive(&accepted, |header, payloads| {
            let entries = header["files"].as_array_mut().unwrap();
            let index = entries
                .iter()
                .position(|entry| entry["name"].as_str().unwrap().starts_with("playlists/"))
                .unwrap();
            entries.remove(index);
            payloads.remove(index);
        }),
        Rejection::Damaged,
    ));

    cases.push((
        "referenced local playlist parent is a file",
        rewrite_generated_archive(&accepted, |header, _| {
            let entry = header["files"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|entry| entry["name"].as_str().unwrap().starts_with("playlists/"))
                .unwrap();
            let parent = entry["name"]
                .as_str()
                .unwrap()
                .rsplit_once('/')
                .unwrap()
                .0
                .to_owned();
            assert!(parent.starts_with("playlists/"));
            entry["name"] = json!(parent);
        }),
        Rejection::Damaged,
    ));

    // These are valid versioned documents whose values this build cannot
    // restore faithfully. An older release may allow another enum, range or
    // override scope, so refusal must not call the archive damaged.
    for version in [1, iptv_settings::backup::VERSION] {
        for (label, global, playlists) in [
            (
                "unsupported enum",
                json!({"appearance.font_size": "generated_larger"}),
                json!({}),
            ),
            (
                "unsupported numeric range",
                json!({"player.info_panel_timeout": 120_000}),
                json!({}),
            ),
            (
                "unsupported override scope",
                json!({}),
                json!({"generated-playlist": {"appearance.accent": "red"}}),
            ),
        ] {
            cases.push((
                label,
                rewrite_generated_archive(&accepted, |header, payloads| {
                    let index = header["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .position(|entry| entry["name"] == "settings.json")
                        .unwrap();
                    payloads[index] = serde_json::to_vec(&json!({
                        "format": iptv_settings::backup::FORMAT, "version": version,
                        "global": global, "playlists": playlists
                    }))
                    .unwrap();
                }),
                Rejection::Settings {
                    found: Some(u64::from(version)),
                    supported: u64::from(iptv_settings::backup::VERSION),
                },
            ));
        }
    }
    for (label, bytes, expected) in cases {
        let error =
            block_on(busy.worker(move |archive| archive.stage(&mut bytes.as_slice()))).unwrap_err();
        assert_eq!(error.rejection(), Some(expected), "{label}: {error}");
        assert!(!error.to_string().contains("generated-private-format-token"));
        assert_eq!(
            generated_tree(&pending),
            queued,
            "{label}: queued restore changed"
        );
        assert_eq!(
            name(&current, live_source),
            "Generated current source",
            "{label}"
        );
        assert_eq!(
            fs::read(root.join("settings.json")).unwrap(),
            live_settings,
            "{label}"
        );
        assert_eq!(fs::read(&document).unwrap(), live_document, "{label}");
        assert!(current.restore_is_pending(), "{label}");
    }

    let header_length = u64::from_le_bytes(accepted[9..17].try_into().unwrap());
    let fail_at = 17 + header_length + 1;
    assert!(fail_at < accepted.len() as u64);
    for (kind, rejection) in [
        (io::ErrorKind::UnexpectedEof, Some(Rejection::Damaged)),
        (io::ErrorKind::PermissionDenied, None),
    ] {
        let bytes = accepted.clone();
        let error = block_on(busy.worker(move |archive| {
            archive.stage(&mut PayloadReadFailure {
                input: io::Cursor::new(bytes),
                fail_at,
                kind,
            })
        }))
        .unwrap_err();
        assert_eq!(error.rejection(), rejection, "{kind:?}: {error}");
        assert_eq!(
            error.to_string(),
            format!("Could not read backup files (I/O: {kind:?})")
        );
        assert_eq!(generated_tree(&pending), queued, "{kind:?}");
        assert_eq!(name(&current, live_source), "Generated current source");
        assert_eq!(fs::read(root.join("settings.json")).unwrap(), live_settings);
        assert_eq!(fs::read(&document).unwrap(), live_document);
        assert!(current.restore_is_pending());
    }
    drop(busy);
    assert!(current.drain_services());
    drop(current);
    let restarted = runtime(&root);
    std::thread::spawn(restarted.restore_activation())
        .join()
        .unwrap()
        .unwrap_or_else(|_| panic!("generated queued restore activation failed"));
    assert!(!restarted.restore_is_pending());
    assert_eq!(name(&restarted, archived_source), "Generated queued source");
    assert!(original.drain_services());
    assert!(restarted.drain_services());
}
