use super::*;

/// Each test owns an independent synthetic root with its own services.
fn export(root: &Path, destination: &Path, settings: &str) -> Result<()> {
    super::export(&Services::for_tests(root), destination, settings)
}
fn stage(root: &Path, input: &mut impl Read) -> Result<()> {
    super::stage(&Services::for_tests(root), input)
}
fn activate(root: &Path) -> Result<()> {
    super::activate(&crate::Ports::for_tests(), root)
}
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::{ChannelQuery, OrderScope};

#[test]
fn startup_outcomes_and_replay_protection_are_independent_per_root() {
    let directory = tempfile::tempdir().unwrap();
    let alpha = directory.path().join("alpha");
    let beta = directory.path().join("beta");
    let gamma = directory.path().join("gamma");
    for root in [&alpha, &beta, &gamma] {
        fs::create_dir(root).unwrap();
        let mut store = Store::open(root.join("iptv.sqlite")).unwrap();
        store
            .set_combined_last_channel("generated-original")
            .unwrap();
    }
    let start = |root: &Path| {
        let mut ports = crate::Ports::for_tests();
        ports.directory = root.to_owned();
        startup(&ports)
    };
    assert!(start(&alpha).is_ok());
    let pending_beta = sibling(&beta, "restore-pending");
    fs::create_dir(&pending_beta).unwrap();
    fs::write(
        pending_beta.join("iptv.sqlite"),
        b"generated invalid database",
    )
    .unwrap();
    fs::write(
        pending_beta.join("settings.json"),
        Settings::new().to_json_string(),
    )
    .unwrap();
    let failure = start(&beta).expect_err("beta must validate its own pending restore");
    assert!(!failure.fatal);
    fs::remove_dir_all(&pending_beta).unwrap();
    let repeated = start(&beta).expect_err("beta keeps its first outcome until restart");
    assert_eq!(repeated.error, failure.error);
    assert_eq!(repeated.fatal, failure.fatal);

    let replacement = directory.path().join("replacement.sqlite");
    let mut store = Store::open(&replacement).unwrap();
    store
        .set_combined_last_channel("generated-replacement")
        .unwrap();
    drop(store);
    let archive = packed(vec![
        ("iptv.sqlite", fs::read(replacement).unwrap()),
        (
            "settings.json",
            Settings::new().to_json_string().into_bytes(),
        ),
    ]);
    stage(&gamma, &mut archive.as_slice()).unwrap();
    assert!(start(&gamma).is_ok());
    assert!(!sibling(&gamma, "restore-pending").exists());
    let store = Store::open(gamma.join("iptv.sqlite")).unwrap();
    assert_eq!(
        store.combined_last_channel().unwrap().as_deref(),
        Some("generated-replacement")
    );
    drop(store);

    stage(&alpha, &mut archive.as_slice()).unwrap();
    assert!(start(&alpha).is_ok());
    assert!(
        sibling(&alpha, "restore-pending").exists(),
        "root recreation must not activate a newly staged restore"
    );
    let store = Store::open(alpha.join("iptv.sqlite")).unwrap();
    assert_eq!(
        store.combined_last_channel().unwrap().as_deref(),
        Some("generated-original")
    );
}

fn seed(root: &Path, name: &str) {
    fs::create_dir_all(root.join("playlists/generated")).unwrap();
    let playlist = root.join("playlists/generated/list.m3u");
    let text = "#EXTM3U\n#EXTINF:-1 group-title=\"Generated\",Channel One\nhttp://example.test/one\n#EXTINF:-1 group-title=\"Generated\",Channel Two\nhttp://example.test/two\n";
    fs::write(&playlist, text).unwrap();
    let mut store = Store::open(root.join("iptv.sqlite")).unwrap();
    let id = store
        .add_source(&Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(playlist.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(text);
    store
        .replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    let key = &parsed.channels.items[1].id;
    store.set_favorite(id, key, true).unwrap();
    store.move_channel(id, &OrderScope::All, key, 0).unwrap();
    let group = store
        .create_custom_group(id, "Generated personal group")
        .unwrap();
    store.add_to_custom_group(group, key).unwrap();
    store.set_active_source(id).unwrap();
    fs::write(root.join("settings.json"), Settings::new().to_json_string()).unwrap();
    fs::write(root.join(format!("last_channel-{}", id.0)), key.as_str()).unwrap();
    fs::write(root.join("last_channel_combined"), "generated-combined-key").unwrap();
    fs::write(root.join("search-history.json"), "[\"Generated query\"]").unwrap();
}

#[test]
fn whole_app_round_trip_rebases_local_files_and_activates_only_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    let target = dir.path().join("target");
    seed(&original, "Generated original");
    seed(&target, "Generated target");
    let archive = dir.path().join("snapshot.patinabackup");
    export(&original, &archive, &Settings::new().to_json_string()).unwrap();
    stage(&target, &mut File::open(&archive).unwrap()).unwrap();
    let current_name = |root: &Path| {
        Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .sources()
            .unwrap()[0]
            .name
            .clone()
    };
    assert_eq!(current_name(&target), "Generated target");
    activate(&target).unwrap();
    assert_eq!(current_name(&target), "Generated original");
    let store = Store::open(target.join("iptv.sqlite")).unwrap();
    let id = store.active_source().unwrap().unwrap();
    let channels = store.load_channels(id, &ChannelQuery::default()).unwrap();
    assert_eq!(channels[0].name, "Channel Two");
    assert!(store.is_favorite(id, &channels[0].channel.id).unwrap());
    assert_eq!(
        store.custom_groups(id).unwrap()[0].name,
        "Generated personal group"
    );
    let source = store.source(id).unwrap().unwrap();
    let SourceConfig::M3u(config) = source.source.config else {
        panic!()
    };
    assert_eq!(
        config.location.expose(),
        target
            .join("playlists/generated/list.m3u")
            .to_str()
            .unwrap()
    );
    assert!(Path::new(config.location.expose()).is_file());
    assert_eq!(
        fs::read_to_string(target.join(format!("last_channel-{}", id.0))).unwrap(),
        channels[0].channel.id.as_str()
    );
    assert_eq!(
        fs::read_to_string(target.join("last_channel_combined")).unwrap(),
        "generated-combined-key"
    );
    assert_eq!(
        fs::read_to_string(target.join("search-history.json")).unwrap(),
        "[\"Generated query\"]"
    );
    assert!(!sibling(&target, "restore-pending").exists());
    assert!(!sibling(&target, "restore-previous").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(archive).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn invalid_truncated_and_unsupported_archives_preserve_current_and_pending_state() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("live");
    seed(&root, "Generated live");
    let archive = dir.path().join("snapshot");
    export(&root, &archive, &Settings::new().to_json_string()).unwrap();
    let bytes = fs::read(archive).unwrap();
    stage(&root, &mut bytes.as_slice()).unwrap();
    let pending = sibling(&root, "restore-pending");
    let before = fs::read(pending.join("iptv.sqlite")).unwrap();
    for cut in [0, 8, 16, bytes.len() / 2, bytes.len() - 1] {
        assert!(stage(&root, &mut &bytes[..cut]).is_err());
        assert_eq!(fs::read(pending.join("iptv.sqlite")).unwrap(), before);
    }
    let mut changed = bytes.clone();
    changed[8] = 2;
    assert!(stage(&root, &mut changed.as_slice()).is_err());
    assert_eq!(fs::read(pending.join("iptv.sqlite")).unwrap(), before);
    assert_eq!(
        Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .sources()
            .unwrap()[0]
            .name,
        "Generated live"
    );
    cancel(&root).unwrap();
    activate(&root).unwrap();
    assert!(!pending.exists());
}

fn packed(files: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let listed: Vec<_> = files
        .iter()
        .map(|(name, bytes)| json!({"name": name, "length": bytes.len()}))
        .collect();
    let manifest = serde_json::to_vec(&json!({
        "format": "patina-whole-app",
        "version": 1,
        "root": "/synthetic",
        "files": listed,
    }))
    .unwrap();
    let mut bytes = MAGIC.to_vec();
    bytes.extend((manifest.len() as u64).to_le_bytes());
    bytes.extend(manifest);
    for (_, content) in files {
        bytes.extend(content);
    }
    bytes
}

#[test]
fn unsafe_paths_invalid_settings_and_invalid_database_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("live");
    seed(&root, "Generated");
    for name in [
        "../outside",
        "/outside",
        "playlists/../../outside",
        "playlists/..\\outside",
    ] {
        assert!(stage(&root, &mut packed(vec![(name, vec![1])]).as_slice()).is_err());
    }
    let db = dir.path().join("snapshot.sqlite");
    Store::open(root.join("iptv.sqlite"))
        .unwrap()
        .snapshot_to(&db)
        .unwrap();
    let bytes = fs::read(db).unwrap();
    assert!(
        stage(
            &root,
            &mut packed(vec![
                ("iptv.sqlite", bytes),
                ("settings.json", b"invalid settings".to_vec())
            ])
            .as_slice()
        )
        .is_err()
    );
    assert!(
        stage(
            &root,
            &mut packed(vec![
                ("iptv.sqlite", b"invalid db".to_vec()),
                (
                    "settings.json",
                    Settings::new().to_json_string().into_bytes()
                )
            ])
            .as_slice()
        )
        .is_err()
    );
    assert!(!sibling(&root, "restore-pending").exists());
    assert!(!dir.path().join("outside").exists());
}

#[test]
fn interrupted_swap_recovers_current_data_before_rejecting_invalid_pending() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("live");
    seed(&root, "Generated original");
    let archive = dir.path().join("archive");
    export(&root, &archive, &Settings::new().to_json_string()).unwrap();
    stage(&root, &mut File::open(archive).unwrap()).unwrap();
    fs::rename(&root, sibling(&root, "restore-previous")).unwrap();
    // App initializes its private directory before the recovery worker runs.
    crate::storage::protect_directory(&root, |_| Ok(())).unwrap();
    fs::write(
        sibling(&root, "restore-pending").join("iptv.sqlite"),
        b"damaged",
    )
    .unwrap();
    assert!(activate(&root).is_err());
    assert_eq!(
        Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .sources()
            .unwrap()[0]
            .name,
        "Generated original"
    );
    cancel(&root).unwrap();
    activate(&root).unwrap();
}

#[test]
fn interrupted_valid_swap_finishes_and_export_cannot_replace_live_database() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("live");
    seed(&root, "Generated original");
    assert!(
        export(
            &root,
            &root.join("iptv.sqlite"),
            &Settings::new().to_json_string()
        )
        .is_err()
    );
    let archive = dir.path().join("archive");
    export(&root, &archive, &Settings::new().to_json_string()).unwrap();
    stage(&root, &mut File::open(archive).unwrap()).unwrap();
    fs::rename(&root, sibling(&root, "restore-previous")).unwrap();
    // App initializes its private directory before the recovery worker runs.
    crate::storage::protect_directory(&root, |_| Ok(())).unwrap();
    activate(&root).unwrap();
    assert!(root.join("iptv.sqlite").exists());
    assert!(!sibling(&root, "restore-pending").exists());
    // A crash immediately after installing the new directory only leaves old
    // private storage to remove; activation must not revert the new directory.
    seed(&sibling(&root, "restore-previous"), "Generated old");
    activate(&root).unwrap();
    assert_eq!(
        Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .sources()
            .unwrap()[0]
            .name,
        "Generated original"
    );
}

#[test]
fn interrupted_pending_replacement_is_recovered_and_cancellation_cannot_resurrect_it() {
    for should_cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("live");
        seed(&root, "Generated backed up");
        let archive = directory.path().join("snapshot");
        export(&root, &archive, &Settings::new().to_json_string()).unwrap();
        stage(&root, &mut File::open(&archive).unwrap()).unwrap();
        let source = Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .sources()
            .unwrap()[0]
            .id;
        Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .rename_source(source, "Generated changed")
            .unwrap();
        fs::rename(
            sibling(&root, "restore-pending"),
            sibling(&root, "restore-replaced"),
        )
        .unwrap();
        if should_cancel {
            cancel(&root).unwrap();
        }
        activate(&root).unwrap();
        let name = Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .source(source)
            .unwrap()
            .unwrap()
            .source
            .name;
        assert_eq!(
            name,
            if should_cancel {
                "Generated changed"
            } else {
                "Generated backed up"
            }
        );
        assert!(!sibling(&root, "restore-replaced").exists());
    }
}

#[test]
fn structured_entry_budgets_preserve_current_and_pending_data() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("current");
    seed(&root, "Synthetic retained");
    let saved = dir.path().join("saved.patinabackup");
    export(&root, &saved, &Settings::new().to_json_string()).unwrap();
    stage(&root, &mut File::open(&saved).unwrap()).unwrap();
    let pending = sibling(&root, "restore-pending");
    let before = fs::read(pending.join("iptv.sqlite")).unwrap();
    for name in ["settings.json", "search-history.json", "last_channel-1"] {
        assert_eq!(
            entry_limit(name, 2 << 20),
            Err(iptv_import::backup_manifest::EntryLimit)
        );
        let manifest = serde_json::to_vec(&json!({"format":"patina-whole-app", "version":1, "root":"/synthetic", "files":[{"name":name,"length":2 << 20}]})).unwrap();
        let mut bytes = MAGIC.to_vec();
        bytes.extend((manifest.len() as u64).to_le_bytes());
        bytes.extend(manifest);
        assert_eq!(
            stage(&root, &mut bytes.as_slice()).map_err(|error| error.message()),
            Err("Backup structured entry is too large")
        );
        assert_eq!(fs::read(pending.join("iptv.sqlite")).unwrap(), before);
    }
    // A staged file modified between shutdown and startup is revalidated
    // before the current working directory is moved aside.
    File::create(pending.join("settings.json"))
        .unwrap()
        .set_len((1 << 20) + 1)
        .unwrap();
    assert_eq!(
        activate(&root).map_err(|error| error.message()),
        Err("Backup structured entry is too large")
    );
    assert_eq!(
        Store::open(root.join("iptv.sqlite"))
            .unwrap()
            .sources()
            .unwrap()[0]
            .name,
        "Synthetic retained"
    );
}

#[test]
fn pending_restore_seals_library_and_documents_before_publication() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.sqlite");
    let key = iptv_store_sqlite::DatabaseKey::from_bytes([0x51; 32]);
    let store = Store::open_encrypted(&live, &key).unwrap();
    let pending = dir.path().join("pending");
    seed(&pending, "Synthetic pending");
    let document = pending.join("playlists/generated/list.m3u");
    let original = fs::read(&document).unwrap();
    seal_restore_with_store(&store, &pending).unwrap();
    Store::validate_encrypted_snapshot(&pending.join("iptv.sqlite"), &key).unwrap();
    assert!(Store::validate_snapshot(&pending.join("iptv.sqlite")).is_err());
    let encrypted = fs::read(&document).unwrap();
    assert!(
        !encrypted
            .windows(b"http://example.test".len())
            .any(|w| w == b"http://example.test")
    );
    assert!(!encrypted.starts_with(b"#EXTM3U"));
    let mut reader = store.read_private_file(&document).unwrap();
    assert_eq!(reader.len(), original.len() as u64);
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded).unwrap();
    assert_eq!(
        decoded, original,
        "portable export streams the original plaintext bytes"
    );
}

#[test]
fn startup_removes_abandoned_plaintext_backup_work_only_for_this_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("synthetic");
    fs::create_dir(&root).unwrap();
    let stale = sibling(&root, "backup-work-123-0");
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("iptv.sqlite"), b"synthetic plaintext snapshot").unwrap();
    let unrelated = dir.path().join("other.backup-work-123-0");
    fs::create_dir(&unrelated).unwrap();
    activate(&root).unwrap();
    assert!(!stale.exists());
    assert!(unrelated.exists());
    assert!(root.exists());
}

#[test]
fn backup_collection_skips_interrupted_atomic_document_writes() {
    let directory = tempfile::tempdir().unwrap();
    let documents = directory.path().join("playlists");
    fs::create_dir(&documents).unwrap();
    for name in [".iptv-sealing-new", ".iptv-sealing-new-journal"] {
        fs::write(documents.join(name), b"interrupted encrypted write").unwrap();
    }
    fs::write(documents.join("valid.m3u"), b"#EXTM3U\n").unwrap();
    let mut names = Vec::new();
    collect(directory.path(), &documents, &mut names).unwrap();
    assert_eq!(names, ["playlists/valid.m3u"]);
}

#[test]
fn a_failed_operation_keeps_its_cause_for_the_log_without_paths() {
    use super::error::Cause;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("current");
    seed(&root, "Synthetic retained");

    let truncated = stage(&root, &mut &MAGIC[..5]).unwrap_err();
    assert_eq!(truncated.message(), "This is not a complete Patina backup");
    assert_eq!(
        truncated.cause(),
        Cause::Io(std::io::ErrorKind::UnexpectedEof)
    );

    let nowhere = dir.path().join("missing").join("saved.patinabackup");
    let unsaved = export(&root, &nowhere, &Settings::new().to_json_string()).unwrap_err();
    assert_eq!(unsaved.message(), "Could not open backup destination");
    assert_eq!(unsaved.cause(), Cause::Io(std::io::ErrorKind::NotFound));
    let line = unsaved.to_string();
    assert_eq!(line, "Could not open backup destination (I/O: NotFound)");
    assert!(!line.contains(dir.path().to_str().unwrap()));

    // A rule of the backup has no cause underneath.
    let inside = export(
        &root,
        &root.join("saved.patinabackup"),
        &Settings::new().to_json_string(),
    )
    .unwrap_err();
    assert_eq!(inside.cause(), Cause::Rule);
    assert_eq!(
        inside.to_string(),
        "Choose a backup destination outside application storage"
    );
}

/// Exits without destructors only in the explicitly selected child test.
pub(super) fn crash_boundary(boundary: &str) {
    if std::env::var("IPTV_TEST_RESTORE_CRASH").as_deref() == Ok(boundary) {
        std::process::exit(73);
    }
}

#[test]
fn restore_crash_child() {
    let Some(root) = std::env::var_os("IPTV_TEST_RESTORE_ROOT") else {
        return;
    };
    activate(Path::new(&root)).unwrap();
    panic!("the requested restore boundary was not reached");
}

#[test]
fn process_death_at_each_restore_swap_boundary_preserves_the_committed_snapshot() {
    use std::process::Command;
    use std::time::{Duration, Instant};
    for boundary in ["validated", "previous", "installed", "synced"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("live");
        let replacement = directory.path().join("replacement");
        seed(&root, "Generated current");
        seed(&replacement, "Generated restored");
        let archive = directory.path().join("snapshot");
        export(&replacement, &archive, &Settings::new().to_json_string()).unwrap();
        stage(&root, &mut File::open(archive).unwrap()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backup::tests::restore_crash_child",
                "--nocapture",
            ])
            .env("IPTV_TEST_RESTORE_ROOT", &root)
            .env("IPTV_TEST_RESTORE_CRASH", boundary)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("restore child did not acknowledge {boundary}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(73), "boundary {boundary}");
        // Reopen through actual startup recovery, not a manually reconstructed
        // directory layout. Repeated recovery must remain idempotent.
        activate(&root).unwrap();
        activate(&root).unwrap();
        assert_eq!(
            Store::open(root.join("iptv.sqlite"))
                .unwrap()
                .sources()
                .unwrap()[0]
                .name,
            "Generated restored",
            "boundary {boundary}"
        );
        assert!(!sibling(&root, "restore-pending").exists());
        assert!(!sibling(&root, "restore-previous").exists());
    }
}

/// A schema-24 library as a build before US spelling (#139) wrote it, from
/// the store's synthetic historical fixtures.
fn v24_library(path: &Path) {
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../iptv-store-sqlite/tests/fixtures");
    let fixture = |name: &str| fs::read_to_string(fixtures.join(name)).unwrap();
    let ledger = tempfile::tempdir().unwrap();
    let fresh = ledger.path().join("ledger.sqlite");
    drop(Store::open(&fresh).unwrap());
    let names: Vec<(u32, String)> = rusqlite::Connection::open(&fresh)
        .unwrap()
        .prepare("SELECT version,name FROM schema_migrations WHERE version<=24 ORDER BY version")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(&fixture("schema_v24.sql")).unwrap();
    for (version, name) in names {
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, 0)",
            rusqlite::params![version, name],
        )
        .unwrap();
    }
    conn.pragma_update(None, "user_version", 24).unwrap();
    for seed in 1..=24 {
        conn.execute_batch(&fixture(&format!("data_v{seed}.sql")))
            .unwrap();
    }
}

#[test]
fn a_backup_from_before_us_spelling_restores_favorites_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("live");
    seed(&root, "Generated");
    let old = dir.path().join("v24.sqlite");
    v24_library(&old);
    let settings = json!({
        "format": iptv_settings::backup::FORMAT,
        "version": 3,
        "global": {"appearance.accent": "blue_grey", "guide.show_programmes": false}
    })
    .to_string();
    stage(
        &root,
        &mut packed(vec![
            ("iptv.sqlite", fs::read(&old).unwrap()),
            ("settings.json", settings.into_bytes()),
        ])
        .as_slice(),
    )
    .unwrap();
    activate(&root).unwrap();

    let store = Store::open(root.join("iptv.sqlite")).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        iptv_store_sqlite::SCHEMA_VERSION
    );
    let favorites = store
        .load_channels(
            iptv_store_sqlite::SourceId(1),
            &ChannelQuery {
                favorites_only: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        favorites
            .iter()
            .map(|row| row.channel.id.as_str())
            .collect::<Vec<_>>(),
        ["xt:live:102", "xt:live:101"]
    );
    let (restored, report) =
        Settings::from_json(&fs::read_to_string(root.join("settings.json")).unwrap()).unwrap();
    assert!(report.unknown.is_empty() && report.rejected.is_empty());
    let global = &iptv_settings::Target::Global;
    assert_eq!(
        restored.color(iptv_settings::keys::ACCENT, global),
        "blue_gray"
    );
    assert_eq!(
        restored.get("guide.show_programs").unwrap(),
        iptv_settings::Value::Bool(false)
    );
}
