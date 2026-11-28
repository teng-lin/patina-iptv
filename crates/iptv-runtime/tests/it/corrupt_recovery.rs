//! Startup corruption classification, wrong-key refusal, atomic set-aside,
//! backup restore into an unreadable library, and child-process crash safety.
//!
//! Synthetic data only.

use crate::support::block_on;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_runtime::{Ports, Runtime, RuntimeEvent, backup};
use iptv_settings::{Settings, Target, Value, keys};
use iptv_store_sqlite::{DatabaseKey, SCHEMA_VERSION, SourcePolicy, Store};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::Mutex,
    time::Duration,
};

const KEY_A: [u8; 32] = [0x41; 32];
const KEY_B: [u8; 32] = [0x42; 32];
const CRASH_ENV_ROOT: &str = "IPTV_TEST_CORRUPT_CRASH_ROOT";
const CRASH_ENV_MODE: &str = "IPTV_TEST_CORRUPT_CRASH_MODE";

fn ports_with_key(root: &Path, key: [u8; 32]) -> Ports {
    Ports::new(root.to_owned(), move |_| Ok(DatabaseKey::from_bytes(key)))
}

fn runtime_with_ports(ports: Ports) -> Rc<Runtime> {
    let owner = Rc::new(Runtime::new(ports));
    owner.start_services();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner
}

/// Seeds a multi-page encrypted SQLCipher store so page-2+ corruption leaves
/// page 1 unlockable with `key` (producing `SQLITE_CORRUPT`, not `NOTADB`).
fn seed_multipage_encrypted_store(root: &Path, key: [u8; 32], source_name: &str) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let db_path = root.join("iptv.sqlite");
    let mut store = Store::open_encrypted(&db_path, &DatabaseKey::from_bytes(key)).unwrap();
    let mut m3u = String::from("#EXTM3U\n");
    for index in 0..120 {
        m3u.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"ch-{index}\" group-title=\"Synthetic Group\",Synthetic Channel {index:03}\n\
             http://stream.example.test/live/{index}.ts\n"
        ));
    }
    let source = store
        .add_source(&Source {
            name: source_name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("http://playlist.example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(&m3u);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_favorite(source, &parsed.channels.items[0].id, true)
        .unwrap();
    drop(store);
    let len = fs::metadata(&db_path).unwrap().len();
    assert!(
        len >= 12 * 1024,
        "expected multi-page encrypted database, got {len} bytes"
    );
    db_path
}

/// Overwrites all pages past page 1 so SQLCipher unlocks page 1 with the right
/// key and positively reports `SQLITE_CORRUPT` (`is_corrupt_database()`).
fn corrupt_pages_after_header(db_path: &Path) -> Vec<u8> {
    let mut bytes = fs::read(db_path).unwrap();
    assert!(bytes.len() > 8192);
    for (idx, byte) in bytes[4096..].iter_mut().enumerate() {
        *byte ^= 0xA5u8.wrapping_add((idx % 251) as u8);
    }
    fs::write(db_path, &bytes).unwrap();
    bytes
}

fn run_startup_failure(ports: Ports) -> iptv_runtime::Error {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&executor, async {
        let owner = Rc::new(Runtime::new(ports));
        owner.bind_tasks(
            |task| drop(tokio::task::spawn_local(task)),
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        owner.bind(move |event| match event {
            RuntimeEvent::FirstRun => panic!("unreadable database must not report FirstRun"),
            RuntimeEvent::StoreStartupFailed { error, .. } => tx.send(error).unwrap(),
            _ => {}
        });
        owner.start_services();
        iptv_runtime::data::start(&owner);
        let error = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("startup failure timed out")
            .unwrap();
        assert!(owner.drain_services());
        error
    })
}

fn run_startup_expect_first_run(ports: Ports) {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&executor, async {
        let owner = Rc::new(Runtime::new(ports));
        owner.bind_tasks(
            |task| drop(tokio::task::spawn_local(task)),
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        owner.bind(move |event| match event {
            RuntimeEvent::FirstRun => tx.send("first-run").unwrap(),
            RuntimeEvent::StoreStartupFailed { error, .. } => {
                panic!("expected clean FirstRun, got failure: {error:?}")
            }
            _ => {}
        });
        owner.start_services();
        iptv_runtime::data::start(&owner);
        let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("FirstRun timed out")
            .unwrap();
        assert_eq!(event, "first-run");
        assert!(owner.drain_services());
    });
}

#[test]
fn corrupted_pages_and_schema_are_corrupt_while_wrong_key_is_not_database_and_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("library");
    let db_path = seed_multipage_encrypted_store(&root, KEY_A, "Synthetic Source");
    let intact = fs::read(&db_path).unwrap();

    // 1. Healthy database: set_aside_corrupt_library must refuse.
    let healthy_ports = ports_with_key(&root, KEY_A);
    let err = backup::set_aside_corrupt_library(&healthy_ports)
        .expect_err("healthy database must never be set aside");
    assert!(!err.is_corrupt_database());
    assert_eq!(fs::read(&db_path).unwrap(), intact);
    assert!(!root.join("recovered").exists());

    // 2. Wrong key: SQLITE_NOTADB, never corrupt, refused by set_aside_corrupt_library.
    let wrong_store_err = Store::open_encrypted(&db_path, &DatabaseKey::from_bytes(KEY_B))
        .expect_err("wrong key must fail to open");
    assert!(wrong_store_err.is_not_database());
    assert!(!wrong_store_err.is_corrupt_database());

    let wrong_ports = ports_with_key(&root, KEY_B);
    let startup_wrong = run_startup_failure(wrong_ports.clone());
    assert!(startup_wrong.is_not_database());
    assert!(startup_wrong.is_credential_unavailable());
    assert!(!startup_wrong.is_corrupt_database());

    let refused_wrong = backup::set_aside_corrupt_library(&wrong_ports)
        .expect_err("wrong key (SQLITE_NOTADB) must never be set aside");
    assert!(refused_wrong.is_not_database());
    assert!(!refused_wrong.is_corrupt_database());
    assert_eq!(fs::read(&db_path).unwrap(), intact);
    assert!(!root.join("recovered").exists());

    // 3. Unavailable key broker: refused, never corrupt.
    let missing_key_ports = Ports::new(root.clone(), |_| {
        Err(iptv_runtime::Error::unavailable(
            "platform credential broker is locked or unavailable",
        ))
    });
    let startup_missing = run_startup_failure(missing_key_ports.clone());
    assert!(startup_missing.is_credential_unavailable());
    assert!(!startup_missing.is_corrupt_database());
    assert!(backup::set_aside_corrupt_library(&missing_key_ports).is_err());
    assert_eq!(fs::read(&db_path).unwrap(), intact);

    // 4. Newer schema: refused, never corrupt.
    let future_root = temp.path().join("future");
    fs::create_dir_all(&future_root).unwrap();
    let future_db = future_root.join("iptv.sqlite");
    let future_store = Store::open_encrypted(&future_db, &DatabaseKey::from_bytes(KEY_A)).unwrap();
    drop(future_store);
    {
        let conn = rusqlite::Connection::open(&future_db).unwrap();
        let hex_key: String = KEY_A.iter().map(|b| format!("{b:02x}")).collect();
        conn.pragma_update(None, "key", format!("x'{hex_key}'"))
            .unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, 'future', 0)",
            [SCHEMA_VERSION + 1],
        )
        .unwrap();
    }
    let future_ports = ports_with_key(&future_root, KEY_A);
    let refused_future = backup::set_aside_corrupt_library(&future_ports)
        .expect_err("newer schema must never be treated as corrupt");
    assert_eq!(
        refused_future.newer_schema(),
        Some((SCHEMA_VERSION + 1, SCHEMA_VERSION))
    );
    assert!(!future_root.join("recovered").exists());

    // 5. Corrupted pages past header: positively classified is_corrupt_database().
    corrupt_pages_after_header(&db_path);
    let startup_corrupt = run_startup_failure(ports_with_key(&root, KEY_A));
    assert!(startup_corrupt.is_corrupt_database());
    assert!(!startup_corrupt.is_not_database());
    assert!(!startup_corrupt.is_credential_unavailable());

    // 6. SchemaCorrupt (missing middle row in schema_migrations) is also classified
    // corrupt and accepted by set_aside_corrupt_library.
    let schema_root = temp.path().join("schema-corrupt");
    fs::create_dir_all(&schema_root).unwrap();
    let schema_db = schema_root.join("iptv.sqlite");
    drop(Store::open_encrypted(&schema_db, &DatabaseKey::from_bytes(KEY_A)).unwrap());
    {
        let conn = rusqlite::Connection::open(&schema_db).unwrap();
        let hex_key: String = KEY_A.iter().map(|b| format!("{b:02x}")).collect();
        conn.pragma_update(None, "key", format!("x'{hex_key}'"))
            .unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 2", [])
            .unwrap();
    }
    let schema_ports = ports_with_key(&schema_root, KEY_A);
    let startup_schema = run_startup_failure(schema_ports.clone());
    assert!(startup_schema.is_corrupt_database());
    let moved = backup::set_aside_corrupt_library(&schema_ports).unwrap();
    assert!(moved.join("iptv.sqlite").is_file());
    assert!(moved.join("README.txt").is_file());

    // 7. Secondary-index B-tree corruption with valid SQLCipher page HMACs is caught
    // by `PRAGMA integrity_check(1)` inside `Store::verify_corrupt_encrypted`.
    let idx_root = temp.path().join("index-corrupt");
    let idx_db = seed_multipage_encrypted_store(&idx_root, KEY_A, "Index Corrupt Source");
    {
        let conn = rusqlite::Connection::open(&idx_db).unwrap();
        let hex_key: String = KEY_A.iter().map(|b| format!("{b:02x}")).collect();
        conn.pragma_update(None, "key", format!("x'{hex_key}'"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE synthetic_empty_holder (v TEXT);
             CREATE INDEX synthetic_empty_idx ON synthetic_empty_holder(v);",
        )
        .unwrap();
        let empty_root: i64 = conn
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name = 'synthetic_empty_idx'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.pragma_update(None, "writable_schema", true).unwrap();
        let updated = conn
            .execute(
                "UPDATE sqlite_schema SET rootpage = ?1
                 WHERE type = 'index' AND tbl_name = 'channels' AND rootpage != 0",
                [empty_root],
            )
            .unwrap();
        assert!(updated >= 1);
        conn.pragma_update(None, "writable_schema", false).unwrap();
    }
    Store::verify_corrupt_encrypted(&idx_db, &DatabaseKey::from_bytes(KEY_A))
        .expect("HMAC-valid secondary index corruption must verify as corrupt");
    let idx_moved = backup::set_aside_corrupt_library(&ports_with_key(&idx_root, KEY_A)).unwrap();
    assert!(idx_moved.join("iptv.sqlite").is_file());

    // 8. A damaged private playlist file under `playlists/` when `iptv.sqlite` itself is
    // healthy must never misclassify the catalog as corrupt or offer destructive reset.
    let doc_root = temp.path().join("healthy-db-corrupt-doc");
    let doc_owner = runtime_with_ports(ports_with_key(&doc_root, KEY_A));
    let doc_url =
        block_on(doc_owner.keep_picked_file("big.m3u", vec![b'A'; 16 * 1024].as_slice())).unwrap();
    assert!(doc_owner.drain_services());
    drop(doc_owner);
    let doc_file = iptv_providers::content_url::local_path(&doc_url)
        .unwrap()
        .unwrap();
    corrupt_pages_after_header(&doc_file);
    {
        let store = Store::open_encrypted(
            doc_root.join("iptv.sqlite"),
            &DatabaseKey::from_bytes(KEY_A),
        )
        .unwrap();
        store.mark_private_documents_unsealed().unwrap();
    }
    let doc_err = run_startup_failure(ports_with_key(&doc_root, KEY_A));
    assert!(
        !doc_err.is_corrupt_database(),
        "corrupt private document must not masquerade as catalog corruption"
    );
    assert!(backup::set_aside_corrupt_library(&ports_with_key(&doc_root, KEY_A)).is_err());
}

#[test]
fn set_aside_moves_wal_and_shm_preserves_settings_and_updates_and_prunes_to_three() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app-data");
    let db_path = seed_multipage_encrypted_store(&root, KEY_A, "Synthetic Before Corrupt");
    let corrupted_bytes = corrupt_pages_after_header(&db_path);

    let wal_bytes = b"synthetic-wal-frames-payload".to_vec();
    let shm_bytes = b"synthetic-shm-index-payload".to_vec();
    fs::write(root.join("iptv.sqlite-wal"), &wal_bytes).unwrap();
    fs::write(root.join("iptv.sqlite-shm"), &shm_bytes).unwrap();

    let mut settings = Settings::new();
    settings
        .set(
            "general.user_agent",
            Value::Text("SyntheticKeptAgent/1".into()),
        )
        .unwrap();
    let settings_json = settings.to_json_string();
    fs::write(root.join("settings.json"), &settings_json).unwrap();

    let update_dir = backup::sibling(&root, "app-update");
    fs::create_dir_all(&update_dir).unwrap();
    let update_schedule = b"{\"frequency\":\"weekly\"}";
    fs::write(update_dir.join("schedule.json"), update_schedule).unwrap();

    let ports = ports_with_key(&root, KEY_A);
    let first_recovered = backup::set_aside_corrupt_library(&ports).unwrap();
    assert!(first_recovered.starts_with(root.join("recovered")));
    assert!(!db_path.exists());
    assert!(!root.join("iptv.sqlite-wal").exists());
    assert!(!root.join("iptv.sqlite-shm").exists());
    assert_eq!(
        fs::read(first_recovered.join("iptv.sqlite")).unwrap(),
        corrupted_bytes
    );
    assert_eq!(
        fs::read(first_recovered.join("iptv.sqlite-wal")).unwrap(),
        wal_bytes
    );
    assert!(first_recovered.join("iptv.sqlite-shm").is_file());
    let readme = fs::read_to_string(first_recovered.join("README.txt")).unwrap();
    assert!(readme.contains("unreadable library database"));
    assert!(readme.contains("settings"));

    // Credentials (encryption marker + key), settings, and app-update state remain intact.
    assert!(root.join("iptv.sqlite.encrypted").is_file());
    assert_eq!(
        fs::read_to_string(root.join("settings.json")).unwrap(),
        settings_json
    );
    assert_eq!(
        fs::read(update_dir.join("schedule.json")).unwrap(),
        update_schedule
    );

    // Next open creates a clean, empty encrypted store using the same key.
    run_startup_expect_first_run(ports.clone());
    let reopened = Store::open_encrypted(&db_path, &DatabaseKey::from_bytes(KEY_A)).unwrap();
    assert!(reopened.sources().unwrap().is_empty());
    drop(reopened);

    // Perform four more set-aside recoveries; at most MAX_RECOVERED_LIBRARIES (3) remain,
    // and same-second timestamp collisions get distinct ordered suffixes.
    let mut slots = vec![first_recovered];
    for round in 1..5 {
        seed_multipage_encrypted_store(&root, KEY_A, &format!("Synthetic Round {round}"));
        corrupt_pages_after_header(&db_path);
        let slot = backup::set_aside_corrupt_library(&ports).unwrap();
        assert!(slot.join("iptv.sqlite").is_file());
        assert!(slot.join("README.txt").is_file());
        slots.push(slot);
    }

    let mut remaining: Vec<PathBuf> = fs::read_dir(root.join("recovered"))
        .unwrap()
        .filter_map(|entry| {
            entry
                .ok()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
        })
        .collect();
    remaining.sort();
    assert_eq!(remaining.len(), backup::MAX_RECOVERED_LIBRARIES);
    assert_eq!(remaining.as_slice(), &slots[slots.len() - 3..]);
    assert!(
        !slots[0].exists(),
        "oldest copy was pruned after new rename"
    );
    assert!(!slots[1].exists(), "second-oldest copy was pruned");

    // Simulate backward clock skew (future-dated slots already on disk): the newly
    // set-aside database must ALWAYS be preserved, never pruned on arrival.
    for future in ["20990101-000000Z", "20990101-000001Z", "20990101-000002Z"] {
        let dir = root.join("recovered").join(future);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("iptv.sqlite"), b"future-slot").unwrap();
        fs::write(dir.join("README.txt"), b"future\n").unwrap();
    }
    seed_multipage_encrypted_store(&root, KEY_A, "Clock Skew Round");
    let skew_bytes = corrupt_pages_after_header(&db_path);
    let skew_slot = backup::set_aside_corrupt_library(&ports).unwrap();
    assert_eq!(
        fs::read(skew_slot.join("iptv.sqlite")).unwrap(),
        skew_bytes,
        "newly set-aside library must never be pruned even under backward clock skew"
    );
}

#[test]
fn restore_backup_into_empty_library_validates_before_modifying_anything() {
    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("source-app");
    let origin = runtime_with_ports(ports_with_key(&source_root, KEY_B));
    let playlist_bytes =
        b"#EXTM3U\n#EXTINF:-1,Synthetic Restored Channel\nhttps://example.test/restored.ts\n";
    let local_doc = block_on(origin.keep_picked_file("restored.m3u", playlist_bytes)).unwrap();
    let source_id = {
        let mut store = Store::open_encrypted(
            source_root.join("iptv.sqlite"),
            &DatabaseKey::from_bytes(KEY_B),
        )
        .unwrap();
        let id = store
            .add_source(&Source {
                name: "Synthetic Restored Playlist".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new(&local_doc),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let parsed = iptv_providers::m3u::parse_m3u(std::str::from_utf8(playlist_bytes).unwrap());
        store
            .replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])
            .unwrap();
        store
            .set_favorite(id, &parsed.channels.items[0].id, true)
            .unwrap();
        id
    };
    origin
        .set_setting(
            "general.user_agent",
            Value::Text("SyntheticRestoredAgent/2".into()),
        )
        .unwrap();
    assert!(origin.flush_files(Duration::from_secs(5)));

    let archive_path = temp.path().join("valid.patinabackup");
    {
        let busy = origin.begin_backup().unwrap();
        let out = archive_path.clone();
        let settings_text = origin.settings().to_json_string();
        block_on(busy.worker(move |archive| archive.export(&out, &settings_text))).unwrap();
    }
    assert!(origin.drain_services());

    // Prepare target root with KEY_A, existing settings, and a corrupt library + WAL.
    let target_root = temp.path().join("target-app");
    let target_db = seed_multipage_encrypted_store(&target_root, KEY_A, "Damaged Target");
    let intact_bytes = fs::read(&target_db).unwrap();
    let target_ports = ports_with_key(&target_root, KEY_A);

    // Refuses restore_backup_into_empty_library when target DB is healthy or wrong-key!
    assert!(
        backup::restore_backup_into_empty_library(
            &target_ports,
            &mut fs::File::open(&archive_path).unwrap(),
        )
        .is_err(),
        "healthy library must not use corrupt-recovery restore"
    );
    assert_eq!(fs::read(&target_db).unwrap(), intact_bytes);
    assert!(
        backup::restore_backup_into_empty_library(
            &ports_with_key(&target_root, KEY_B),
            &mut fs::File::open(&archive_path).unwrap(),
        )
        .is_err(),
        "wrong-key library must never be overwritten by corrupt-recovery restore"
    );
    assert_eq!(fs::read(&target_db).unwrap(), intact_bytes);

    let corrupt_bytes = corrupt_pages_after_header(&target_db);
    let wal_bytes = b"damaged-wal-sidecar".to_vec();
    fs::write(target_root.join("iptv.sqlite-wal"), &wal_bytes).unwrap();
    let mut before_settings = Settings::new();
    before_settings
        .set("general.user_agent", Value::Text("BeforeRestore/1".into()))
        .unwrap();
    let before_json = before_settings.to_json_string();
    fs::write(target_root.join("settings.json"), &before_json).unwrap();

    // Invalid / truncated archives fail validation BEFORE touching anything on disk.
    let valid_bytes = fs::read(&archive_path).unwrap();
    for bad in [
        b"not a patina backup".as_slice(),
        &valid_bytes[..valid_bytes.len() / 2],
    ] {
        let mut cursor = std::io::Cursor::new(bad);
        assert!(backup::restore_backup_into_empty_library(&target_ports, &mut cursor).is_err());
        assert_eq!(fs::read(&target_db).unwrap(), corrupt_bytes);
        assert_eq!(
            fs::read(target_root.join("iptv.sqlite-wal")).unwrap(),
            wal_bytes
        );
        assert_eq!(
            fs::read_to_string(target_root.join("settings.json")).unwrap(),
            before_json
        );
        assert!(!target_root.join("recovered").exists());
        assert!(!backup::sibling(&target_root, "restore-pending").exists());
    }

    // Valid backup sets corrupt DB aside into `recovered/` (preserved across activation)
    // and activates the restored library re-encrypted under KEY_A.
    let recovered_slot = backup::restore_backup_into_empty_library(
        &target_ports,
        &mut fs::File::open(&archive_path).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fs::read(recovered_slot.join("iptv.sqlite")).unwrap(),
        corrupt_bytes
    );
    assert_eq!(
        fs::read(recovered_slot.join("iptv.sqlite-wal")).unwrap(),
        wal_bytes
    );
    assert!(recovered_slot.join("README.txt").is_file());
    assert_ne!(
        fs::read(target_root.join("iptv.sqlite-wal")).unwrap_or_default(),
        wal_bytes
    );

    let restored_store =
        Store::open_encrypted(&target_db, &DatabaseKey::from_bytes(KEY_A)).unwrap();
    let restored_source = restored_store.source(source_id).unwrap().unwrap().source;
    assert_eq!(restored_source.name, "Synthetic Restored Playlist");
    let SourceConfig::M3u(restored_m3u) = restored_source.config else {
        panic!("expected restored M3U source");
    };
    let rebased_doc = iptv_providers::content_url::local_path(restored_m3u.location.expose())
        .unwrap()
        .unwrap();
    assert!(rebased_doc.starts_with(target_root.join("playlists")));
    let mut decrypted_doc = Vec::new();
    restored_store
        .read_private_file(&rebased_doc)
        .unwrap()
        .read_to_end(&mut decrypted_doc)
        .unwrap();
    assert_eq!(decrypted_doc, playlist_bytes);
    let channels = restored_store
        .load_channels(source_id, &Default::default())
        .unwrap();
    assert_eq!(channels[0].name, "Synthetic Restored Channel");
    assert!(
        restored_store
            .is_favorite(source_id, &channels[0].channel.id)
            .unwrap()
    );
    let (loaded_settings, report) =
        Settings::from_json(&fs::read_to_string(target_root.join("settings.json")).unwrap())
            .unwrap();
    assert!(report.is_clean());
    assert_eq!(
        loaded_settings.text(keys::USER_AGENT, &Target::Global),
        "SyntheticRestoredAgent/2"
    );
}

#[test]
fn recovery_warnings_are_logged_once_and_redact_filesystem_paths() {
    struct Capture(Mutex<Vec<String>>);
    impl log::Log for Capture {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Warn
        }
        fn log(&self, record: &log::Record) {
            if record.level() <= log::Level::Warn {
                self.0.lock().unwrap().push(record.args().to_string());
            }
        }
        fn flush(&self) {}
    }
    static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

    if std::env::var_os("IPTV_TEST_CORRUPT_LOG_CHILD").is_none() {
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "corrupt_recovery::recovery_warnings_are_logged_once_and_redact_filesystem_paths",
                "--nocapture",
            ])
            .env("IPTV_TEST_CORRUPT_LOG_CHILD", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "logger child failed or skipped: {stdout}\n{stderr}",
        );
        return;
    }

    log::set_logger(&CAPTURE).expect("isolated child installs the only logger");
    log::set_max_level(log::LevelFilter::Warn);

    let temp = tempfile::tempdir().unwrap();
    let origin_root = temp.path().join("origin-secret-path-427");
    let archive_path = temp.path().join("logged.patinabackup");
    {
        let origin = runtime_with_ports(ports_with_key(&origin_root, KEY_B));
        let busy = origin.begin_backup().unwrap();
        let out = archive_path.clone();
        let settings_text = origin.settings().to_json_string();
        block_on(busy.worker(move |archive| archive.export(&out, &settings_text))).unwrap();
        assert!(origin.drain_services());
    }

    let root = temp.path().join("secret-path-marker-427");
    let db_path = seed_multipage_encrypted_store(&root, KEY_A, "Synthetic Logged");
    corrupt_pages_after_header(&db_path);
    let ports = ports_with_key(&root, KEY_A);
    let slot = backup::set_aside_corrupt_library(&ports).unwrap();
    assert!(slot.exists());

    // Seed and corrupt again, then exercise restore_backup_into_empty_library warning too.
    seed_multipage_encrypted_store(&root, KEY_A, "Synthetic Logged Restore");
    corrupt_pages_after_header(&db_path);
    backup::restore_backup_into_empty_library(&ports, &mut fs::File::open(&archive_path).unwrap())
        .unwrap();

    let captured = CAPTURE.0.lock().unwrap();
    for line in captured.iter() {
        assert!(
            !line.contains("secret-path-marker-427")
                && !line.contains("origin-secret-path-427")
                && !line.contains("iptv.sqlite"),
            "warn! must not leak filesystem paths: {line:?}"
        );
    }
    let recovery_lines: Vec<&String> = captured
        .iter()
        .filter(|line| line.starts_with("iptv recovery:"))
        .collect();
    assert_eq!(
        recovery_lines.len(),
        2,
        "expected one warn! per recovery op: {recovery_lines:?}"
    );
    assert!(recovery_lines[0].contains("set aside for a fresh start"));
    assert!(recovery_lines[1].contains("set aside and backup restored"));
}

#[test]
fn crash_child_entrypoint() {
    let Some(root) = std::env::var_os(CRASH_ENV_ROOT) else {
        return;
    };
    let root = PathBuf::from(root);
    let mode = std::env::var(CRASH_ENV_MODE).unwrap();
    let ports = ports_with_key(&root, KEY_A);
    match mode.as_str() {
        "after-rename" => {
            let slot = backup::set_aside_corrupt_library(&ports).unwrap();
            assert!(slot.join("iptv.sqlite").is_file());
            let _ = std::io::stdout().flush();
            // Simulate hard process exit right between rename and reopen.
            std::process::exit(0);
        }
        "before-sidecars" => {
            // Exercises the real `recovery_crash_point("before-sidecars")` inside
            // `move_corrupt_files_aside` right after moving `iptv.sqlite` and writing `.latest`.
            let _ = backup::set_aside_corrupt_library(&ports);
            panic!("expected process exit at before-sidecars crash point");
        }
        "restore-before-publish" | "restore-before-activate" => {
            let archive = PathBuf::from(std::env::var_os("IPTV_TEST_CORRUPT_ARCHIVE").unwrap());
            let _ = backup::restore_backup_into_empty_library(
                &ports,
                &mut fs::File::open(archive).unwrap(),
            );
            panic!("expected process exit at restore crash point");
        }
        other => panic!("unknown crash child mode: {other}"),
    }
}

#[test]
fn crash_between_rename_and_reopen_preserves_library_and_recovers_sidecars() {
    let exe = std::env::current_exe().unwrap();
    let temp = tempfile::tempdir().unwrap();

    // Case 1: Process exits immediately after set_aside_corrupt_library, before reopen.
    let root_one = temp.path().join("crash-after-rename");
    let db_one = seed_multipage_encrypted_store(&root_one, KEY_A, "Crash One");
    let corrupt_one = corrupt_pages_after_header(&db_one);
    fs::write(root_one.join("iptv.sqlite-wal"), b"wal-one").unwrap();

    let out = Command::new(&exe)
        .args([
            "--exact",
            "corrupt_recovery::crash_child_entrypoint",
            "--nocapture",
        ])
        .env(CRASH_ENV_ROOT, &root_one)
        .env(CRASH_ENV_MODE, "after-rename")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let recovered_copies: Vec<PathBuf> = fs::read_dir(root_one.join("recovered"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path().join("iptv.sqlite")))
        .filter(|p| p.is_file())
        .collect();
    let live_exists = db_one.is_file();
    assert!(
        live_exists || !recovered_copies.is_empty(),
        "crash must leave either original corrupt DB or recovered copy, never neither"
    );
    assert_eq!(recovered_copies.len(), 1);
    assert_eq!(fs::read(&recovered_copies[0]).unwrap(), corrupt_one);

    // Next launch cleanly initializes an empty store.
    run_startup_expect_first_run(ports_with_key(&root_one, KEY_A));
    assert!(db_one.is_file());

    #[cfg(feature = "test-support")]
    {
        // Case 2: Process crashes inside production `move_corrupt_files_aside` right after
        // renaming `iptv.sqlite` into `recovered/<ts>/` and before moving `-wal`/`-shm`.
        let root_two = temp.path().join("crash-before-sidecars");
        let db_two = seed_multipage_encrypted_store(&root_two, KEY_A, "Crash Two");
        let corrupt_two = corrupt_pages_after_header(&db_two);
        fs::write(root_two.join("iptv.sqlite-wal"), b"orphan-wal-bytes").unwrap();
        fs::write(root_two.join("iptv.sqlite-shm"), b"orphan-shm-bytes").unwrap();

        let out_two = Command::new(&exe)
            .args([
                "--exact",
                "corrupt_recovery::crash_child_entrypoint",
                "--nocapture",
            ])
            .env(CRASH_ENV_ROOT, &root_two)
            .env(CRASH_ENV_MODE, "before-sidecars")
            .env("IPTV_TEST_CORRUPT_CRASH_POINT", "before-sidecars")
            .output()
            .unwrap();
        assert!(out_two.status.success());

        assert!(root_two.join("iptv.sqlite-wal").exists());
        assert!(root_two.join("recovered/.latest").is_file());
        let slot_two: PathBuf = fs::read_dir(root_two.join("recovered"))
            .unwrap()
            .filter_map(|e| {
                e.ok()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.path())
            })
            .next()
            .unwrap();
        assert_eq!(fs::read(slot_two.join("iptv.sqlite")).unwrap(), corrupt_two);

        // Next launch finishes moving orphaned sidecars into the exact `.latest` slot before opening.
        run_startup_expect_first_run(ports_with_key(&root_two, KEY_A));
        assert!(!root_two.join("recovered/.latest").exists());
        assert_eq!(
            fs::read(slot_two.join("iptv.sqlite-wal")).unwrap(),
            b"orphan-wal-bytes"
        );
        assert!(slot_two.join("iptv.sqlite-shm").is_file());
        let fresh = Store::open_encrypted(&db_two, &DatabaseKey::from_bytes(KEY_A)).unwrap();
        assert!(fresh.sources().unwrap().is_empty());
        drop(fresh);

        // Case 3 & 4: Crash during `restore_backup_into_empty_library` (before publish and
        // before activate) never loses the corrupt database and recovers cleanly on restart.
        let origin_root = temp.path().join("crash-origin");
        fs::create_dir_all(&origin_root).unwrap();
        let archive = temp.path().join("crash-valid.patinabackup");
        {
            let origin = runtime_with_ports(ports_with_key(&origin_root, KEY_B));
            let mut store = Store::open_encrypted(
                origin_root.join("iptv.sqlite"),
                &DatabaseKey::from_bytes(KEY_B),
            )
            .unwrap();
            store
                .add_source(&Source {
                    name: "Crash Restored Source".into(),
                    config: SourceConfig::M3u(M3uSourceConfig {
                        location: SecretUrl::new("http://playlist.example.test/crash.m3u"),
                        user_agent: None,
                        referrer: None,
                    }),
                })
                .unwrap();
            drop(store);
            let busy = origin.begin_backup().unwrap();
            let out_path = archive.clone();
            let settings_text = origin.settings().to_json_string();
            block_on(busy.worker(move |b| b.export(&out_path, &settings_text))).unwrap();
            assert!(origin.drain_services());
        }

        let root_three = temp.path().join("crash-restore-before-publish");
        let db_three = seed_multipage_encrypted_store(&root_three, KEY_A, "Crash Three");
        let corrupt_three = corrupt_pages_after_header(&db_three);
        // Seed both a leftover `.restore-previous` and an older `.restore-pending` to verify
        // `reconcile_previous_restore` runs before set-aside, the older pending backup stays
        // staged until activation commits, and post-commit cleanup discards `.restore-prior`.
        let prior_pending = backup::sibling(&root_three, "restore-pending");
        fs::create_dir_all(&prior_pending).unwrap();
        fs::write(prior_pending.join("marker.txt"), b"older-pending").unwrap();
        let stale_prev = backup::sibling(&root_three, "restore-previous");
        fs::create_dir_all(stale_prev.join("recovered/20200101-000000Z")).unwrap();
        fs::write(
            stale_prev.join("recovered/20200101-000000Z/iptv.sqlite"),
            b"older-recovered-copy",
        )
        .unwrap();
        fs::write(
            stale_prev.join("recovered/20200101-000000Z/README.txt"),
            b"older\n",
        )
        .unwrap();
        let out_three = Command::new(&exe)
            .args([
                "--exact",
                "corrupt_recovery::crash_child_entrypoint",
                "--nocapture",
            ])
            .env(CRASH_ENV_ROOT, &root_three)
            .env(CRASH_ENV_MODE, "restore-before-publish")
            .env("IPTV_TEST_CORRUPT_ARCHIVE", &archive)
            .env(
                "IPTV_TEST_CORRUPT_CRASH_POINT",
                "after-set-aside-before-publish",
            )
            .output()
            .unwrap();
        assert!(out_three.status.success());
        // Verify that if the credential store is locked on restart after an interrupted
        // corrupt restore, opening fails closed instead of creating an empty database,
        // and unlocking + retrying completes the interrupted restore!
        let locked_ports = Ports::new(root_three.clone(), |_| {
            Err(iptv_runtime::Error::unavailable(
                "platform credential broker is locked or unavailable",
            ))
        });
        let locked_err = run_startup_failure(locked_ports);
        assert!(locked_err.is_credential_unavailable());
        assert!(!db_three.exists());
        assert!(
            backup::sibling(&root_three, "restore-prior").exists(),
            "older pending backup must survive locked-key failure before commit"
        );
        let reopened_three = runtime_with_ports(ports_with_key(&root_three, KEY_A));
        let _ = block_on(reopened_three.announce_retired_recordings()).unwrap();
        assert!(!reopened_three.restore_is_pending());
        assert!(!backup::sibling(&root_three, "restore-prior").exists());
        // Simulate a leftover `.restore-prior` after commit (crash right before deleting it):
        // next startup on a fresh process/path must discard `.restore-prior`, never promote it!
        assert!(reopened_three.drain_services());
        drop(reopened_three);
        let post_commit_root = temp.path().join("post-commit-leftover-prior");
        fs::create_dir_all(&post_commit_root).unwrap();
        fs::copy(&db_three, post_commit_root.join("iptv.sqlite")).unwrap();
        let leftover_prior = backup::sibling(&post_commit_root, "restore-prior");
        fs::create_dir_all(&leftover_prior).unwrap();
        fs::write(leftover_prior.join("marker.txt"), b"stale").unwrap();
        let verify_after_prior = runtime_with_ports(ports_with_key(&post_commit_root, KEY_A));
        verify_after_prior.restore_activation()().unwrap_or_else(|_| panic!("activation failed"));
        assert!(!leftover_prior.exists());
        assert!(!verify_after_prior.restore_is_pending());
        assert!(verify_after_prior.drain_services());
        let restored_three =
            Store::open_encrypted(&db_three, &DatabaseKey::from_bytes(KEY_A)).unwrap();
        assert_eq!(
            restored_three.sources().unwrap()[0].name,
            "Crash Restored Source"
        );
        let mut copies_three: Vec<PathBuf> = fs::read_dir(root_three.join("recovered"))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path().join("iptv.sqlite")))
            .filter(|p| p.is_file())
            .collect();
        copies_three.sort();
        assert_eq!(copies_three.len(), 2);
        assert!(
            copies_three
                .iter()
                .any(|p| fs::read(p).unwrap() == corrupt_three)
        );

        let root_four = temp.path().join("crash-restore-before-activate");
        let db_four = seed_multipage_encrypted_store(&root_four, KEY_A, "Crash Four");
        let corrupt_four = corrupt_pages_after_header(&db_four);
        let out_four = Command::new(&exe)
            .args([
                "--exact",
                "corrupt_recovery::crash_child_entrypoint",
                "--nocapture",
            ])
            .env(CRASH_ENV_ROOT, &root_four)
            .env(CRASH_ENV_MODE, "restore-before-activate")
            .env("IPTV_TEST_CORRUPT_ARCHIVE", &archive)
            .env(
                "IPTV_TEST_CORRUPT_CRASH_POINT",
                "after-publish-before-activate",
            )
            .output()
            .unwrap();
        assert!(out_four.status.success());
        let reopened_four = runtime_with_ports(ports_with_key(&root_four, KEY_A));
        reopened_four.restore_activation()()
            .unwrap_or_else(|_| panic!("restore activation failed"));
        assert!(!reopened_four.restore_is_pending());
        assert!(reopened_four.drain_services());
        let restored_four =
            Store::open_encrypted(&db_four, &DatabaseKey::from_bytes(KEY_A)).unwrap();
        assert_eq!(
            restored_four.sources().unwrap()[0].name,
            "Crash Restored Source"
        );
        let copies_four: Vec<PathBuf> = fs::read_dir(root_four.join("recovered"))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path().join("iptv.sqlite")))
            .filter(|p| p.is_file())
            .collect();
        assert_eq!(copies_four.len(), 1);
        assert_eq!(fs::read(&copies_four[0]).unwrap(), corrupt_four);
    }
}

#[test]
fn adopt_recordings_preserves_valid_completion_receipt_and_rejects_corrupt_cells() {
    use iptv_core::recording::{JobId, Request, Status};
    use iptv_store_sqlite::RecordingCompletion;

    let temp = tempfile::tempdir().unwrap();
    let host_root = temp.path().join("host-rec");
    let host_db = seed_multipage_encrypted_store(&host_root, KEY_A, "Recording Host Source");
    let target_db = temp.path().join("target.sqlite");
    drop(Store::open_encrypted(&target_db, &DatabaseKey::from_bytes(KEY_A)).unwrap());

    {
        let mut host = Store::open_encrypted(&host_db, &DatabaseKey::from_bytes(KEY_A)).unwrap();
        let src = host.sources().unwrap()[0].id;
        let ch = host.load_channels(src, &Default::default()).unwrap()[0]
            .channel
            .id
            .clone();
        let job_id = host
            .schedule_recording(
                JobId(7),
                &Request {
                    source: src.0,
                    channel: ch.to_string(),
                    channel_name: "Synthetic Channel 000".into(),
                    title: "Synthetic Show".into(),
                    start: 1_000,
                    end: 2_000,
                },
                900,
            )
            .unwrap();
        let scheduled = host.recording_job(job_id).unwrap().unwrap();
        assert!(host.claim_recording(&scheduled).unwrap());
        let running = host.recording_job(job_id).unwrap().unwrap();
        assert!(
            host.prepare_recording_completion(
                &running,
                &RecordingCompletion {
                    job: job_id,
                    status: Status::Partial,
                    bytes: 4096,
                    digest: "a".repeat(64),
                    message: "Cut short".into(),
                },
            )
            .unwrap()
        );
    }

    let mut target = Store::open_encrypted(&target_db, &DatabaseKey::from_bytes(KEY_A)).unwrap();
    target
        .adopt_host_recordings_encrypted(&host_db, &DatabaseKey::from_bytes(KEY_A))
        .expect("valid TEXT source_incarnation completion receipt must adopt cleanly");
    assert_eq!(target.recording_jobs().unwrap().len(), 1);
    assert!(target.recording_completion(JobId(7)).unwrap().is_some());

    // Corrupting a TEXT column into non-UTF-8 bytes in a discarded scheduled
    // zero-byte row (`status='scheduled' AND bytes=0`) or its progress row must
    // NOT abort adoption of valid kept recordings.
    let hex_key: String = KEY_A.iter().map(|b| format!("{b:02x}")).collect();
    {
        let conn = rusqlite::Connection::open(&host_db).unwrap();
        conn.pragma_update(None, "key", format!("x'{hex_key}'"))
            .unwrap();
        let src_id: i64 = conn
            .query_row("SELECT id FROM sources LIMIT 1", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,
                 ends_at,status,bytes,message,source_incarnation)
             VALUES(8,?1,'ch-0','Synthetic Channel 000','Scheduled Discarded',3000,4000,'scheduled',0,'','1')",
            [src_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recording_playback_progress(recording_id,position_ms,duration_ms,completed,updated_at)
             VALUES(8,'not-an-integer',NULL,0,3000)",
            [],
        )
        .unwrap();
    }
    target
        .adopt_host_recordings_encrypted(&host_db, &DatabaseKey::from_bytes(KEY_A))
        .expect("corrupt progress on a discarded scheduled row must not abort adoption");
    assert_eq!(target.recording_jobs().unwrap().len(), 1);

    // Corrupting a TEXT column into non-UTF-8 bytes in a KEPT `recording_jobs` row
    // must still reject adoption before mutating `target`.
    {
        let conn = rusqlite::Connection::open(&host_db).unwrap();
        conn.pragma_update(None, "key", format!("x'{hex_key}'"))
            .unwrap();
        conn.execute(
            "UPDATE recording_jobs SET title = x'fffe00' WHERE id = 7",
            [],
        )
        .unwrap();
    }
    assert!(
        target
            .adopt_host_recordings_encrypted(&host_db, &DatabaseKey::from_bytes(KEY_A))
            .is_err()
    );
    assert_eq!(
        target.recording_jobs().unwrap()[0].request.title,
        "Synthetic Show"
    );
}

#[test]
fn verify_leaves_no_live_sidecars_and_distinguishes_inaccessible_from_missing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("clean-wal-live");
    let db_path = seed_multipage_encrypted_store(&root, KEY_A, "No Live Sidecars");
    // Ensure clean WAL-mode DB has no `-wal` or `-shm` on disk before verification.
    let _ = fs::remove_file(root.join("iptv.sqlite-wal"));
    let _ = fs::remove_file(root.join("iptv.sqlite-shm"));
    assert!(!root.join("iptv.sqlite-wal").exists());
    assert!(!root.join("iptv.sqlite-shm").exists());

    // Healthy verification returns `is_library_not_corrupt()` without creating live sidecars.
    let healthy_err =
        Store::verify_corrupt_encrypted(&db_path, &DatabaseKey::from_bytes(KEY_A)).unwrap_err();
    assert!(healthy_err.is_library_not_corrupt());
    assert!(!root.join("iptv.sqlite-wal").exists());
    assert!(!root.join("iptv.sqlite-shm").exists());

    // Corrupt DB with rejected backup leaves zero new files in `root`.
    corrupt_pages_after_header(&db_path);
    let mut bad_archive = std::io::Cursor::new(b"PATINABK\x01truncated".as_slice());
    assert!(
        backup::restore_backup_into_empty_library(&ports_with_key(&root, KEY_A), &mut bad_archive)
            .is_err()
    );
    assert!(!root.join("iptv.sqlite-wal").exists());
    assert!(!root.join("iptv.sqlite-shm").exists());

    // Missing file reports `is_library_not_corrupt()`.
    let missing_err = Store::verify_corrupt_encrypted(
        root.join("does-not-exist.sqlite"),
        &DatabaseKey::from_bytes(KEY_A),
    )
    .unwrap_err();
    assert!(missing_err.is_library_not_corrupt());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // If running as non-root, making parent directory 0o000 causes symlink_metadata
        // to fail with PermissionDenied, which must NOT report `is_library_not_corrupt()`.
        let locked_dir = temp.path().join("locked-dir");
        fs::create_dir_all(&locked_dir).unwrap();
        let locked_db = locked_dir.join("iptv.sqlite");
        fs::write(&locked_db, b"nonempty").unwrap();
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o000)).unwrap();
        let meta_res = fs::symlink_metadata(&locked_db);
        let verify_res =
            Store::verify_corrupt_encrypted(&locked_db, &DatabaseKey::from_bytes(KEY_A));
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).unwrap();
        if meta_res.is_err() {
            let err = verify_res.unwrap_err();
            assert!(
                !err.is_library_not_corrupt(),
                "inaccessible file must not masquerade as missing/healthy: {err:?}"
            );
        }
    }
}

#[test]
fn failed_restore_rollback_keeps_three_recovered_copies_and_second_owner_closes_handles() {
    let temp = tempfile::tempdir().unwrap();
    let origin_root = temp.path().join("origin-valid");
    fs::create_dir_all(&origin_root).unwrap();
    let archive_path = temp.path().join("valid-restore.patinabackup");
    {
        let origin = runtime_with_ports(ports_with_key(&origin_root, KEY_B));
        let mut store = Store::open_encrypted(
            origin_root.join("iptv.sqlite"),
            &DatabaseKey::from_bytes(KEY_B),
        )
        .unwrap();
        store
            .add_source(&Source {
                name: "Restored After Rollback".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("http://playlist.example.test/ok.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        drop(store);
        let busy = origin.begin_backup().unwrap();
        let out = archive_path.clone();
        let settings_text = origin.settings().to_json_string();
        block_on(busy.worker(move |b| b.export(&out, &settings_text))).unwrap();
        assert!(origin.drain_services());
    }

    let root = temp.path().join("shared-root");
    let ports = ports_with_key(&root, KEY_A);

    // 1. Open warm connections on `owner_a`, then corrupt the DB on disk and run
    // `recover_corrupt_start_fresh` from a second `owner_b` sharing `root`.
    let db_path = seed_multipage_encrypted_store(&root, KEY_A, "Shared Root Initial");
    let owner_a = runtime_with_ports(ports.clone());
    let _ = block_on(owner_a.announce_retired_recordings()).unwrap();
    corrupt_pages_after_header(&db_path);
    let owner_b = runtime_with_ports(ports.clone());
    let moved_slot = block_on(owner_b.recover_corrupt_start_fresh()).unwrap();
    #[cfg(feature = "test-support")]
    assert!(
        !owner_a.host_handle().has_open_connections(),
        "recovery must close warmed connections on sibling owners before moving files"
    );
    assert!(moved_slot.join("iptv.sqlite").is_file());
    assert!(!db_path.exists());
    assert!(owner_a.drain_services());
    assert!(owner_b.drain_services());
    drop(owner_a);
    drop(owner_b);

    // 2. Seed two more recovered copies so `recovered/` holds 3 copies (`slot0..2`).
    let mut prior_slots = vec![moved_slot];
    for idx in 1..3 {
        seed_multipage_encrypted_store(&root, KEY_A, &format!("Prior Copy {idx}"));
        corrupt_pages_after_header(&db_path);
        prior_slots.push(backup::set_aside_corrupt_library(&ports).unwrap());
    }
    assert_eq!(prior_slots.len(), backup::MAX_RECOVERED_LIBRARIES);
    let prior_snapshots: Vec<(PathBuf, Vec<u8>)> = prior_slots
        .iter()
        .map(|slot| (slot.clone(), fs::read(slot.join("iptv.sqlite")).unwrap()))
        .collect();

    // Seed a 4th corrupt live DB and simulate a pre-install rejection during
    // `activate_locked` (corrupt `staged_corrupt` resumed when an older `restore-pending`
    // was also waiting): all 3 prior recovered copies AND the older `restore-pending`
    // must remain intact, and `resume` must return Err!
    seed_multipage_encrypted_store(&root, KEY_A, "Fourth Corrupt Live");
    let fourth_bytes = corrupt_pages_after_header(&db_path);
    let fourth_slot = root.join("recovered/20991231-235959Z");
    fs::create_dir_all(&fourth_slot).unwrap();
    fs::rename(&db_path, fourth_slot.join("iptv.sqlite")).unwrap();
    fs::write(fourth_slot.join("README.txt"), b"fourth\n").unwrap();
    fs::write(
        backup::sibling(&root, "restore-corrupt-active"),
        b"20991231-235959Z",
    )
    .unwrap();
    let older_pending = backup::sibling(&root, "restore-pending");
    fs::create_dir_all(&older_pending).unwrap();
    fs::write(older_pending.join("older.txt"), b"keep-older-pending").unwrap();
    let bad_staged = backup::sibling(&root, "restore-corrupt-staged");
    fs::create_dir_all(&bad_staged).unwrap();
    fs::write(bad_staged.join("iptv.sqlite"), &fourth_bytes).unwrap();
    fs::write(bad_staged.join("settings.json"), b"{}").unwrap();

    // Verify on an independent root that `restore_activation()` with a locked key
    // moves an older `restore-pending` into `restore-prior` before publishing `staged_corrupt`.
    let locked_root = temp.path().join("locked-prior-preserve");
    fs::create_dir_all(locked_root.join("recovered/20991231-235959Z")).unwrap();
    fs::write(
        locked_root.join("recovered/20991231-235959Z/iptv.sqlite"),
        b"corrupt-moved",
    )
    .unwrap();
    fs::write(
        backup::sibling(&locked_root, "restore-corrupt-active"),
        b"20991231-235959Z",
    )
    .unwrap();
    fs::create_dir_all(backup::sibling(&locked_root, "restore-pending")).unwrap();
    fs::write(
        backup::sibling(&locked_root, "restore-pending").join("older.txt"),
        b"keep-older-pending",
    )
    .unwrap();
    let locked_staged = backup::sibling(&locked_root, "restore-corrupt-staged");
    fs::create_dir_all(&locked_staged).unwrap();
    fs::write(locked_staged.join("iptv.sqlite"), b"encrypted-staged-bytes").unwrap();
    fs::write(locked_staged.join("settings.json"), b"{}").unwrap();
    let locked_owner = runtime_with_ports(Ports::new(locked_root.clone(), |_| {
        Err(iptv_runtime::Error::key_unavailable(
            "platform credential broker is locked or unavailable",
        ))
    }));
    assert!(locked_owner.restore_activation()().is_err());
    assert!(locked_owner.drain_services());
    assert!(
        backup::sibling(&locked_root, "restore-prior")
            .join("older.txt")
            .is_file(),
        "locked-key activation must preserve older pending restore in restore-prior"
    );

    // On `root` with valid key, `restore_activation()` rejects `bad_staged`, rolls
    // `fourth_slot` back to `iptv.sqlite`, restores `restore-prior` -> `restore-pending`,
    // and keeps all 3 prior slots!
    let verifier = runtime_with_ports(ports.clone());
    assert!(
        verifier.restore_activation()().is_err(),
        "rejected resumed restore must return Err after rolling back"
    );
    assert!(verifier.drain_services());
    assert_eq!(fs::read(&db_path).unwrap(), fourth_bytes);
    assert!(older_pending.join("older.txt").is_file());
    for (slot, expected_bytes) in &prior_snapshots {
        assert!(
            slot.join("iptv.sqlite").is_file(),
            "rollback must keep all 3 prior recovered copies: {slot:?}"
        );
        assert_eq!(
            fs::read(slot.join("iptv.sqlite")).unwrap(),
            *expected_bytes,
            "rollback must preserve the exact contents of prior copy {slot:?}"
        );
    }
    fs::remove_dir_all(&older_pending).unwrap();

    // Finally, a valid restore commits and prunes `recovered/` down to 3 copies!
    let committed_slot = backup::restore_backup_into_empty_library(
        &ports,
        &mut fs::File::open(&archive_path).unwrap(),
    )
    .unwrap();
    assert!(committed_slot.join("iptv.sqlite").is_file());
    let remaining_count = fs::read_dir(root.join("recovered"))
        .unwrap()
        .filter_map(|e| e.ok().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())))
        .count();
    assert_eq!(remaining_count, backup::MAX_RECOVERED_LIBRARIES);

    // Verify interrupted rollback with a future-dated older slot and no `.latest` marker
    // keeps an already-returned `-wal` in `root` rather than moving it into the future slot.
    let partial_root = temp.path().join("partial-rollback-wal");
    let future_slot = partial_root.join("recovered/20991231-235959Z");
    let target_slot = partial_root.join("recovered/20260101-000001Z");
    fs::create_dir_all(&future_slot).unwrap();
    fs::create_dir_all(&target_slot).unwrap();
    fs::write(future_slot.join("iptv.sqlite"), b"older-future-copy").unwrap();
    fs::write(target_slot.join("iptv.sqlite"), b"rolling-back-db").unwrap();
    fs::write(partial_root.join("iptv.sqlite-wal"), b"returned-wal").unwrap();
    fs::write(
        backup::sibling(&partial_root, "restore-corrupt-active"),
        b"rollback:20260101-000001Z",
    )
    .unwrap();
    let partial_owner = runtime_with_ports(ports_with_key(&partial_root, KEY_A));
    assert!(partial_owner.restore_activation()().is_ok());
    assert!(partial_owner.drain_services());
    assert_eq!(
        fs::read(partial_root.join("iptv.sqlite")).unwrap(),
        b"rolling-back-db"
    );
    assert_eq!(
        fs::read(partial_root.join("iptv.sqlite-wal")).unwrap(),
        b"returned-wal",
        "resumed rollback must keep the returned WAL alongside the restored database"
    );
    assert!(!future_slot.join("iptv.sqlite-wal").exists());
}
