//! Scheduled recording is retired. An earlier version's login item or task
//! still launches Patina with the real `--background-recordings` flag; such a
//! launch removes those registrations and exits. The first ordinary launch
//! removes them too, runs nothing an earlier version scheduled and tells the
//! viewer once. Registrations live in each scenario's temporary directory
//! (`IPTV_RS_TEST_REGISTRATIONS`), never the real user's.
use crate::{headless::Headless, personalization::step};
use iptv_core::recording::{JobId, Request, Status};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::Store;
use nsh_core::{
    element::Root,
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::EngineConfig;
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REGISTRATIONS: &str = "IPTV_RS_TEST_REGISTRATIONS";

/// The entries an earlier version wrote, under the scenario's directory.
fn registrations(home: &Path) -> [PathBuf; 2] {
    let root = home.join("registrations");
    [
        root.join("autostart/patina-recorder.desktop"),
        root.join("LaunchAgents/com.hantekllc.patina.recorder.plist"),
    ]
}

fn register(home: &Path, data: &Path) {
    for entry in registrations(home) {
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(entry, "generated registration").unwrap();
    }
    std::fs::create_dir_all(data).unwrap();
    std::fs::write(data.join("recording-login.enabled"), b"1").unwrap();
}

fn assert_retired(home: &Path, data: &Path) {
    for entry in registrations(home) {
        assert!(!entry.exists(), "{} remains", entry.display());
    }
    assert!(!data.join("recording-login.enabled").exists());
    assert!(data.join("recording-registrations.retired").exists());
}

fn app() -> Headless {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let mut config = EngineConfig::new(Root::of::<iptv_app::App>());
    config.viewport = size(px(1920.0), px(1080.0));
    config.environment.os = if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };
    config.environment.idiom = Idiom::Desktop;
    Headless::start(config).unwrap()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The real flag reaches the app as a launch argument; the exact test
/// filter still selects this scenario in the child.
#[test]
fn a_background_recorder_launch_removes_registrations_and_exits() {
    let Some(home) = isolated!() else { return };
    let name = "recorder_startup::a_background_recorder_launch_removes_registrations_and_exits";
    if std::env::args().any(|arg| arg == "--background-recordings") {
        // The child: the app's root ends the process before any storage.
        let mut app = app();
        let _ = app.idle(Duration::from_secs(20));
        panic!("a recorder launch kept running");
    }
    let data = home.data_dir();
    register(home.path(), &data);
    let marker = home.path().join("recorder-argument-ran");
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            name,
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "--",
            "--background-recordings",
        ])
        .env(crate::isolated::CHILD, name)
        .env("IPTV_RS_TEST_RAN", &marker)
        .env(REGISTRATIONS, home.path().join("registrations"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(marker.exists(), "the child reached the scenario");
    assert_retired(home.path(), &data);
    assert!(
        !data.join("iptv.sqlite").exists(),
        "no storage opened before exiting"
    );
}

/// A library as 0.1.8 left it, schema 27, with a pending job and a
/// repeating rule. Written in plaintext; the app encrypts it on opening.
fn library_from_0_1_8(path: &Path) {
    let mut store = Store::open(path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated retired source".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated channel\nhttps://example.test/generated.ts\n",
    );
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store.set_active_source(source).unwrap();
    let request = Request {
        source: source.0,
        channel: parsed.channels.items[0].id.to_string(),
        channel_name: "Generated channel".into(),
        title: "Generated pending program".into(),
        // Due now: a recorder that ran it would claim it at once.
        start: now() - 5,
        end: now() + 600,
    };
    store
        .schedule_recording(JobId(4711), &request, now())
        .unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO recording_rules(id,source_id,source_incarnation,channel_id,channel_name,
             title,starts_at,ends_at,local_start,repeat,enabled)
         SELECT 4712,source_id,source_incarnation,channel_id,channel_name,
             'Generated weekly program',starts_at+86400,ends_at+86400,starts_at+86400,'weekly',1
         FROM recording_jobs WHERE id=4711",
        [],
    )
    .unwrap();
    // Undo schema 28, which this build applied on creating the file.
    conn.execute_batch(
        "DELETE FROM schema_migrations WHERE version=28;
         DROP TABLE retired_recording_schedules;
         PRAGMA user_version=27;",
    )
    .unwrap();
}

#[test]
fn the_first_launch_after_upgrading_runs_nothing_scheduled_and_tells_the_viewer_once() {
    let Some(home) = isolated!() else { return };
    // Point the app at this scenario's own registrations directory.
    // SAFETY: this process runs this one scenario; nothing reads the
    // environment concurrently before the app starts.
    unsafe { std::env::set_var(REGISTRATIONS, home.path().join("registrations")) };
    let data = home.data_dir();
    register(home.path(), &data);
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let database = data.join("iptv.sqlite");
    library_from_0_1_8(&database);

    let mut app = app();
    step(app.wait_for_within(
        "text:/^Scheduled recording was removed. Open Recordings/",
        Duration::from_secs(20),
    ));
    assert_retired(home.path(), &data);
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.press("label:Recordings"));
    step(app.wait_for("id:retired-schedules"));
    step(app.wait_for("text:/Generated weekly program · Generated channel · .* · Every week/"));
    step(app.wait_for("text:/Generated pending program · Generated channel · .* · Once/"));
    step(app.wait_for("text:/^Canceled ·/"));
    // The recorder runs, yet the due job never starts.
    step(app.idle(Duration::from_secs(2)).map(|_| ()));
    let store = crate::isolated::open_database(&database).unwrap();
    let job = store.recording_job(JobId(4711)).unwrap().unwrap();
    assert_eq!(job.status, Status::Canceled);
    assert_eq!(job.message, "Scheduled recording was removed");
    assert!(store.recording_rules().unwrap().iter().all(|r| !r.enabled));
    drop(store);
    // Closing Recordings means the viewer has seen the list.
    step(app.press("text:Close"));
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(5)));
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !crate::isolated::open_database(&database)
        .unwrap()
        .retired_recording_schedules()
        .unwrap()
        .is_empty()
    {
        assert!(
            std::time::Instant::now() < until,
            "the list was not dismissed"
        );
        step(app.idle(Duration::from_millis(100)).map(|_| ()));
    }
    step(app.open_guide_groups());
    step(app.press("label:Recordings"));
    step(app.wait_for("text:/^Canceled ·/"));
    assert!(!app.screen().matches("id:retired-schedules").unwrap());
}
