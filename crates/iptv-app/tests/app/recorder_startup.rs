//! Recorder login launches use the real CLI flag, never hidden viewer streams.
use crate::{headless::Headless, personalization::step};
use iptv_core::recording::{JobId, Request, Status};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::{SourcePolicy, Store};
use nsh_core::{
    element::Root,
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::EngineConfig;
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static REOPEN: AtomicBool = AtomicBool::new(false);

#[component]
fn RecorderHarness() -> impl IntoElement {
    let window = nsh_core::element::desktop::main_window();
    let _timer = set_interval(Duration::from_millis(10), move || {
        if REOPEN.swap(false, Ordering::SeqCst) {
            window.focus();
        }
    });
    <iptv_app::App as nsh_core::element::Component>::element(())
}

fn with_cli_argument(home: &crate::isolated::Home, name: &str) -> bool {
    if std::env::args().any(|arg| arg == "--background-recordings") {
        return false;
    }
    // Rust's harness accepts literal filters after `--`; the app sees the real
    // launch argument while the exact test filter still selects this scenario.
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
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(marker.exists());
    true
}

fn app() -> Headless {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let mut config = EngineConfig::new(Root::of::<RecorderHarness>());
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

fn stream() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for connection in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut connection = connection;
                connection
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 2048];
                let _ = connection.read(&mut request);
                if connection
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n",
                    )
                    .is_err()
                {
                    return;
                }
                let mut packet = [0; 188 * 7];
                for transport in packet.as_chunks_mut::<188>().0 {
                    transport[0] = 0x47;
                }
                for _ in 0..1200 {
                    if connection.write_all(&packet).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
        }
    });
    format!("http://{address}/generated.ts")
}

fn source(store: &mut Store, name: &str, stream: &str) -> (iptv_store_sqlite::SourceId, String) {
    let source = store
        .add_source(&Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(&format!(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Recorder channel\n{stream}\n"
    ));
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
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
    (source, parsed.channels.items[0].id.to_string())
}

fn cached_start(home: crate::isolated::Home, combined: bool) {
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let settings = Settings::new(); // Resume-last-channel and guide autoplay are enabled.
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let database = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&database).unwrap();
    let (source_id, channel) = source(&mut store, "Generated primary", &stream());
    store.set_active_source(source_id).unwrap();
    std::fs::write(root.join(format!("last_channel-{}", source_id.0)), &channel).unwrap();
    if combined {
        source(
            &mut store,
            "Generated secondary",
            "https://example.test/secondary.ts",
        );
        store.set_all_playlists(true).unwrap();
        store
            .set_combined_last_channel(&format!("source:{}:{channel}", source_id.0))
            .unwrap();
    }
    let job = JobId(4711);
    let mut request = Request {
        source: source_id.0,
        channel,
        channel_name: "Recorder channel".into(),
        title: "Generated scheduled capture".into(),
        start: now() + 3600,
        end: now() + 3660,
    };
    store.schedule_recording(job, &request, now()).unwrap();
    drop(store);
    let opens = iptv_app::player_opens(0);
    let mut app = app();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    step(app.idle(Duration::from_millis(800)).map(|_| ()));
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "no automatic hidden viewer"
    );
    assert!(!iptv_app::player_has_source(0));
    assert!(!app.window_state("main").unwrap().unwrap().visible);
    // Start the already accepted job after the startup checks, so slow CI does
    // not change which side of the one-connection admission race runs first.
    request.start = now();
    request.end = now() + 60;
    let mut store = crate::isolated::open_database(&database).unwrap();
    assert!(store.update_recording(job, &request, now()).unwrap());
    drop(store);
    let partial = iptv_runtime::recordings::media_directory(&root).join("4711.part");
    let captured = crate::isolated::wait_outside(
        &mut app,
        "one-stream background capture",
        Duration::from_secs(20),
        || {
            let store = crate::isolated::open_database(&database).unwrap();
            store
                .recording_job(job)
                .unwrap()
                .is_some_and(|job| job.status == Status::Recording)
                && std::fs::metadata(&partial).is_ok_and(|metadata| metadata.len() > 0)
        },
    );
    if captured.is_err() {
        let store = crate::isolated::open_database(&database).unwrap();
        eprintln!(
            "synthetic capture state: {:?}; partial bytes: {:?}",
            store.recording_job(job).unwrap(),
            std::fs::metadata(&partial).map(|m| m.len())
        );
    }
    step(captured);
    assert!(
        !iptv_app::player_has_source(0),
        "the recorder owns the only provider lease"
    );
    REOPEN.store(true, Ordering::SeqCst);
    step(
        app.wait_until("reopened recorder window", Duration::from_secs(5), |_| {
            // The command is observed on the UI owner before the next screen batch.
            !REOPEN.load(Ordering::SeqCst)
        }),
    );
    step(app.press_key("left"));
    assert!(app.window_state("main").unwrap().unwrap().visible);
    step(app.press("label:Recordings"));
    step(app.wait_for("id:recording-job"));
    step(app.press("id:recording-job"));
    step(app.press("text:Stop recording"));
    step(app.wait_for_within("text:/Completed ·/", Duration::from_secs(10)));
    step(app.press("text:Close"));
    step(app.press_key("right"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(app.wait_until(
        "explicit foreground viewing",
        Duration::from_secs(10),
        |_| iptv_app::player_opens(0) == opens + 1,
    ));
}

#[test]
fn recorder_launch_skips_early_resume_and_leaves_the_provider_slot_for_capture() {
    let Some(home) = isolated!() else { return };
    if with_cli_argument(
        &home,
        "recorder_startup::recorder_launch_skips_early_resume_and_leaves_the_provider_slot_for_capture",
    ) {
        return;
    }
    cached_start(home, false);
}

#[test]
fn recorder_launch_skips_combined_resume_and_automatic_preview() {
    let Some(home) = isolated!() else { return };
    if with_cli_argument(
        &home,
        "recorder_startup::recorder_launch_skips_combined_resume_and_automatic_preview",
    ) {
        return;
    }
    cached_start(home, true);
}

#[test]
fn recorder_launch_skips_source_ready_portal_resume() {
    let Some(home) = isolated!() else { return };
    if with_cli_argument(
        &home,
        "recorder_startup::recorder_launch_skips_source_ready_portal_resume",
    ) {
        return;
    }
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set("playlists.include_vod", Value::Bool(false))
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let portal = crate::combined_resume::Portal::start();
    portal.recover_auth();
    let mut store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    let (source, channel) = crate::combined_resume::seed_portal(&mut store, &portal);
    std::fs::write(root.join(format!("last_channel-{}", source.0)), channel).unwrap();
    drop(store);
    let opens = iptv_app::player_opens(0);
    let mut app = app();
    step(app.wait_for_within("text:/^Aut/", Duration::from_secs(20)));
    step(app.idle(Duration::from_millis(800)).map(|_| ()));
    assert_eq!(
        portal.link_requests(),
        0,
        "no viewing create_link in a recorder launch"
    );
    assert_eq!(iptv_app::player_opens(0), opens);
    assert!(!iptv_app::player_has_source(0));
    assert!(!app.window_state("main").unwrap().unwrap().visible);
}
