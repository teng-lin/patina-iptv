//! Corrupt-library startup recovery in the headless shell:
//! - Wrong key (`SQLITE_NOTADB`) keeps Retry + Details, points to the credential
//!   store, and never offers Start fresh or Restore backup.
//! - Positively corrupt library (`is_corrupt_database()`) offers Restore backup,
//!   Start fresh, and Details (never Retry), requires confirmation with initial
//!   focus on Cancel, preserves settings and update state, and lands on the
//!   first-run setup wizard.
//! - Restore backup rejects invalid archives without changing state, then
//!   restores a valid `.patinabackup` directly into the guide while keeping the
//!   corrupt database in `recovered/`.
//!
//! Synthetic data only.

use crate::headless::Headless;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::{ChannelQuery, DatabaseKey, SourcePolicy, Store};
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    task::{Context, Poll, Wake},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(20);

fn step(result: Result<(), String>) {
    result.unwrap_or_else(|error| panic!("{error}"));
}

fn start_app() -> Headless {
    let size = Size::new(px(1280.0), px(720.0));
    let mut config = EngineConfig {
        viewport: size,
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.window = size;
    config.environment.idiom = nsh_core::platform::Idiom::Tv;
    Headless::start(config).unwrap()
}

fn seed_encrypted_library(root: &Path, key: [u8; 32], source_name: &str) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let db_path = root.join("iptv.sqlite");
    let mut store = Store::open_encrypted(&db_path, &DatabaseKey::from_bytes(key)).unwrap();
    let mut playlist = String::from("#EXTM3U\n");
    for index in 0..120 {
        playlist.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"ch-{index}\" group-title=\"Generated\",Generated Channel {index:03}\n\
             http://stream.example.test/ch/{index}.m3u8\n"
        ));
    }
    let source = store
        .add_source(&Source {
            name: source_name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("http://playlist.example.test/list.m3u"),
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
    let parsed = iptv_providers::m3u::parse_m3u(&playlist);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    drop(store);
    db_path
}

fn corrupt_after_first_page(db_path: &Path) -> Vec<u8> {
    let mut bytes = fs::read(db_path).unwrap();
    assert!(bytes.len() > 8192);
    for (idx, byte) in bytes[4096..].iter_mut().enumerate() {
        *byte ^= 0x5Au8.wrapping_add((idx % 239) as u8);
    }
    fs::write(db_path, &bytes).unwrap();
    bytes
}

#[test]
fn wrong_key_shows_credential_store_retry_and_never_offers_start_fresh() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    // Test harness installs [0x54; 32]; encrypting with [0x99; 32] produces SQLITE_NOTADB.
    let db_path = seed_encrypted_library(&root, [0x99; 32], "Intact Wrong Key Library");
    let intact_bytes = fs::read(&db_path).unwrap();

    let mut app = start_app();
    step(app.wait_for("id:store-startup-failure"));
    step(
        app.wait_for(
            "text:Patina couldn’t unlock your library from the device’s credential store.",
        ),
    );
    assert!(app.screen().matches("id:store-recovery-retry").unwrap());
    assert!(
        app.screen()
            .matches("id:store-recovery-details-button")
            .unwrap()
    );
    assert!(
        !app.screen()
            .matches("id:store-recovery-start-fresh")
            .unwrap(),
        "wrong key must never offer Start fresh"
    );
    assert!(
        !app.screen().matches("id:store-recovery-restore").unwrap(),
        "wrong key must never offer Restore backup"
    );
    assert!(!app.screen().shows("damaged"));
    assert!(!app.screen().shows("SQLite"));
    assert!(!app.screen().shows("NOTADB"));

    step(app.press("id:store-recovery-details-button"));
    step(app.wait_for("id:store-recovery-details"));
    step(app.wait_for(
        "text:The saved library could not be unlocked with the key from the credential store.",
    ));
    assert!(!app.screen().shows("not a database"));

    // Pressing Retry re-runs startup, reports that the library still cannot open,
    // and continues to withhold Start fresh and Restore backup.
    step(app.press("id:store-recovery-retry"));
    step(app.wait_for_within("text:Patina still can’t open your library.", WAIT));
    assert!(
        !app.screen()
            .matches("id:store-recovery-start-fresh")
            .unwrap(),
        "wrong key retry must still never offer Start fresh"
    );
    assert!(
        !app.screen().matches("id:store-recovery-restore").unwrap(),
        "wrong key retry must still never offer Restore backup"
    );
    assert_eq!(fs::read(&db_path).unwrap(), intact_bytes);
    assert!(!root.join("recovered").exists());
}

#[test]
fn corrupt_library_start_fresh_requires_confirmation_defaults_to_cancel_and_opens_wizard() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    let db_path = seed_encrypted_library(&root, [0x54; 32], "Damaged Library");
    corrupt_after_first_page(&db_path);

    let mut settings = Settings::new();
    settings
        .set(
            "general.user_agent",
            Value::Text("PreservedSettings/1".into()),
        )
        .unwrap();
    let settings_json = settings.to_json_string();
    fs::write(root.join("settings.json"), &settings_json).unwrap();

    let update_dir = iptv_runtime::backup::sibling(&root, "app-update");
    fs::create_dir_all(&update_dir).unwrap();
    fs::write(
        update_dir.join("schedule.json"),
        b"{\"frequency\":\"daily\"}",
    )
    .unwrap();

    let mut app = start_app();
    step(app.wait_for("id:store-startup-failure"));
    let db_at_recovery = fs::read(&db_path).unwrap();
    let wal_at_recovery = b"uncheckpointed-wal-sidecar".to_vec();
    fs::write(root.join("iptv.sqlite-wal"), &wal_at_recovery).unwrap();
    fs::write(root.join("iptv.sqlite-shm"), b"synthetic-shm").unwrap();
    step(app.wait_for("text:The saved library file is damaged and can’t be read."));
    assert!(app.screen().matches("id:store-recovery-restore").unwrap());
    assert!(
        app.screen()
            .matches("id:store-recovery-start-fresh")
            .unwrap()
    );
    assert!(
        app.screen()
            .matches("id:store-recovery-details-button")
            .unwrap()
    );
    assert!(
        !app.screen().matches("id:store-recovery-retry").unwrap(),
        "Retry must not be offered for corruption"
    );

    // Focus starts on Restore backup -> Right moves to Start fresh -> Right moves to Details.
    step(app.press_key("right"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(app.wait_for("id:store-recovery-details"));
    step(app.wait_for("text:The library database failed an integrity check after unlocking."));
    assert!(!app.screen().shows("SQLITE_CORRUPT"));
    step(app.back());
    step(app.wait_for_gone("id:store-recovery-details", Duration::from_secs(3)));

    // Move left from Details to Start fresh and activate it.
    step(app.press_key("left"));
    step(app.press_key("enter"));
    step(app.wait_for("id:store-recovery-confirm-fresh"));
    step(app.wait_for("text:Start with an empty library?"));
    step(app.wait_for(
        "text:Your current library will be kept in a backup folder; the app will start empty. \
         Your app settings and device credentials will be kept, and you can add your playlists again.",
    ));

    // Initial focus on confirmation MUST be Cancel: pressing Enter cancels without moving files.
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:store-recovery-confirm-fresh", Duration::from_secs(3)));
    assert!(app.screen().matches("id:store-startup-failure").unwrap());
    assert_eq!(fs::read(&db_path).unwrap(), db_at_recovery);
    assert!(!root.join("recovered").exists());

    // Open confirmation again, move Right to Start fresh, and confirm.
    step(app.press_key("enter"));
    step(app.wait_for("id:store-recovery-confirm-fresh"));
    step(app.press_key("right"));
    step(app.press_key("enter"));

    step(app.wait_for_within("id:wizard-welcome", WAIT));
    step(app.wait_for_gone("id:store-startup-failure", WAIT));

    let recovered_entries: Vec<PathBuf> = fs::read_dir(root.join("recovered"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(recovered_entries.len(), 1);
    let slot = &recovered_entries[0];
    assert_eq!(fs::read(slot.join("iptv.sqlite")).unwrap(), db_at_recovery);
    assert!(
        Store::verify_corrupt_encrypted(
            slot.join("iptv.sqlite"),
            &DatabaseKey::from_bytes([0x54; 32]),
        )
        .is_ok()
    );
    assert!(slot.join("iptv.sqlite-wal").is_file());
    assert!(slot.join("iptv.sqlite-shm").is_file());
    assert!(slot.join("README.txt").is_file());
    assert_eq!(
        fs::read_to_string(root.join("settings.json")).unwrap(),
        settings_json
    );
    assert_eq!(
        fs::read(update_dir.join("schedule.json")).unwrap(),
        b"{\"frequency\":\"daily\"}"
    );
    let fresh_store = crate::isolated::open_database(&db_path).unwrap();
    assert!(fresh_store.sources().unwrap().is_empty());
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Arc::new(Unpark(std::thread::current())).into();
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

#[test]
fn corrupt_library_restore_backup_rejects_bad_archive_then_restores_valid_library() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();

    // Build a valid `.patinabackup` from an independent synthetic origin directory.
    let origin_root = home.path().join("origin-data");
    let valid_archive = home.path().join("valid.patinabackup");
    {
        let origin = Rc::new(iptv_runtime::Runtime::new(iptv_runtime::Ports::new(
            origin_root.clone(),
            |_| Ok(DatabaseKey::from_bytes([0x77; 32])),
        )));
        origin.start_services();
        origin.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        let playlist_bytes = b"#EXTM3U\n\
            #EXTINF:-1 group-title=\"Recovered\",Recovered Channel One\n\
            http://stream.example.test/recovered-one.m3u8\n";
        let local_doc = block_on(origin.keep_picked_file("recovered.m3u", playlist_bytes)).unwrap();
        let mut store = Store::open_encrypted(
            origin_root.join("iptv.sqlite"),
            &DatabaseKey::from_bytes([0x77; 32]),
        )
        .unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated Recovered Playlist".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new(&local_doc),
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
        let parsed = iptv_providers::m3u::parse_m3u(std::str::from_utf8(playlist_bytes).unwrap());
        store
            .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
            .unwrap();
        drop(store);
        let busy = origin.begin_backup().unwrap();
        let out = valid_archive.clone();
        let settings = origin.settings().to_json_string();
        block_on(busy.worker(move |archive| archive.export(&out, &settings))).unwrap();
        drop(busy);
        assert!(origin.drain_services());
    }
    assert!(valid_archive.is_file());

    // Seed the app's live store with a multi-page corrupt database.
    let db_path = seed_encrypted_library(&root, [0x54; 32], "Corrupt Before Restore");
    let corrupt_bytes = corrupt_after_first_page(&db_path);

    let bad_archive = home.path().join("corrupt.patinabackup");
    fs::write(&bad_archive, b"PATINABK\x01truncated").unwrap();

    let mut app = start_app();
    step(app.wait_for("id:store-startup-failure"));

    // 1. Invalid backup fails validation, reports an error, and leaves everything untouched.
    step(app.on_pick_paths(vec![bad_archive]));
    // Initial focus for Cause::Corrupt is Restore backup; Enter activates it.
    step(app.press_key("enter"));
    step(app.wait_for_within("id:store-recovery-error", WAIT));
    assert!(app.screen().matches("id:store-startup-failure").unwrap());
    assert!(!root.join("recovered").exists());
    assert_eq!(fs::read(&db_path).unwrap(), corrupt_bytes);

    // 2. Valid backup sets corrupt DB aside into `recovered/` and opens the restored library.
    step(app.on_pick_paths(vec![valid_archive]));
    step(app.press("id:store-recovery-restore"));
    step(app.wait_for_within("text:Recovered Channel One", WAIT));
    step(app.wait_for_gone("id:store-startup-failure", WAIT));

    let recovered_entries: Vec<PathBuf> = fs::read_dir(root.join("recovered"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(recovered_entries.len(), 1);
    assert_eq!(
        fs::read(recovered_entries[0].join("iptv.sqlite")).unwrap(),
        corrupt_bytes
    );
    assert!(recovered_entries[0].join("README.txt").is_file());

    let store = crate::isolated::open_database(&db_path).unwrap();
    let sources = store.sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].name, "Generated Recovered Playlist");
    let full_source = store.source(sources[0].id).unwrap().unwrap().source;
    let SourceConfig::M3u(m3u_cfg) = full_source.config else {
        panic!("expected restored M3U source");
    };
    let rebased_path = iptv_providers::content_url::local_path(m3u_cfg.location.expose())
        .unwrap()
        .unwrap();
    assert!(rebased_path.starts_with(root.join("playlists")));
    let mut decrypted = Vec::new();
    std::io::Read::read_to_end(
        &mut store.read_private_file(&rebased_path).unwrap(),
        &mut decrypted,
    )
    .unwrap();
    assert!(
        decrypted
            .windows(b"Recovered Channel One".len())
            .any(|w| w == b"Recovered Channel One")
    );
    let channels = store
        .load_channels(sources[0].id, &ChannelQuery::default())
        .unwrap();
    assert_eq!(channels[0].name, "Recovered Channel One");
}
