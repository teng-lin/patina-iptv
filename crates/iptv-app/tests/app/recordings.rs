//! Synthetic scheduling through the real shell: one workflow for keys and touch.
use crate::headless::Headless;
use crate::personalization::{self, step};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value};
use nsh_core::platform::Idiom;
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::time::Duration;

#[test]
fn recording_capture_failures_have_translations() {
    use iptv_net::recording::Failure;
    for failure in [
        Failure::Network,
        Failure::Unsupported,
        Failure::Storage,
        Failure::Limit,
        Failure::Gap,
    ] {
        assert!(iptv_core::i18n::is_catalogued(failure.message()));
    }
}

fn fixture(
    home: &crate::isolated::Home,
    idiom: Idiom,
    media_url: &str,
) -> (Headless, std::path::PathBuf, iptv_library::SourceId) {
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("playlists.max_connections", Value::Int(3))
        .unwrap();
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated DVR source".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(&format!(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",A\n{media_url}\n"
    ));
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = idiom;
    config.environment.os = if idiom == Idiom::Desktop {
        nsh_core::platform::Os::MacOs
    } else if idiom == Idiom::Pad {
        nsh_core::platform::Os::Ios
    } else {
        nsh_core::platform::Os::Android
    };
    let mut app = Headless::start(config).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    step(app.press("text:/^☰  /"));
    step(app.press_key("right"));
    (app, path, source)
}

fn exercise(home: crate::isolated::Home, idiom: Idiom) {
    let (mut app, path, source) = fixture(&home, idiom, "https://example.test/generated.ts");
    // The custom form works without guide data, with the selected channel retained.
    if idiom == Idiom::Tv {
        step(app.press_key("m"));
        for _ in 0..30 {
            if app
                .screen()
                .accessibility_state("label:Schedule a recording…")
                .unwrap()
                .selected
            {
                break;
            }
            step(app.press_key("down"));
        }
        assert!(
            app.screen()
                .accessibility_state("label:Schedule a recording…")
                .unwrap()
                .selected
        );
        step(app.press_key("enter"));
    } else {
        personalization::menu(&mut app, "Schedule a recording…");
    }
    step(app.wait_for("id:recordings-screen"));
    step(app.wait_for("text:Save recording"));
    // Calendar and clock values are separate, and touch opens an exact editor.
    step(app.wait_for("label:/^Start day · [^:]+$/"));
    step(app.wait_for("label:/^Start time · .*:/"));
    // Move the recording to tomorrow through a visible touch/pointer control.
    if idiom == Idiom::Tv {
        for _ in 0..iptv_core::recording::Draft::SAVE {
            step(app.press_key("up"));
        }
        step(app.press_key("right"));
        for _ in 0..iptv_core::recording::Draft::SAVE {
            step(app.press_key("down"));
        }
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Increase Start day/"));
        step(app.press("label:/^Start day ·/"));
        step(app.wait_for("id:recording-day-option"));
        step(app.press("id:recording-day-option >> nth=1"));
        step(app.press("label:/^Start time ·/"));
        step(app.wait_for("id:recording-entry"));
        step(app.clear("id:recording-entry"));
        step(app.type_text("id:recording-entry", "18:05"));
        step(app.press("id:recording-entry >> text:Done"));
        step(app.wait_for("label:/^Start time · (18:05|6:05 PM)$/"));
        step(app.press("label:/^Duration ·/"));
        step(app.wait_for("id:recording-entry"));
        step(app.clear("id:recording-entry"));
        step(app.type_text("id:recording-entry", "1:10"));
        step(app.press("id:recording-entry >> text:Done"));
        step(app.press("label:/^Name ·/"));
        step(app.wait_for("id:recording-entry"));
        step(app.clear("id:recording-entry"));
        step(app.type_text("id:recording-entry", "Generated scheduled recording"));
        step(app.press("id:recording-entry >> text:Done"));
        step(app.press("text:Save recording"));
    }
    step(app.wait_for_within("text:/Scheduled ·/", Duration::from_secs(15)));
    let store = crate::isolated::open_database(&path).unwrap();
    let jobs = store.recording_jobs().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].request.source, source.0);
    assert_eq!(jobs[0].status, iptv_core::recording::Status::Scheduled);
    if idiom != Idiom::Tv {
        assert_eq!(jobs[0].request.title, "Generated scheduled recording");
        assert_eq!(jobs[0].request.end - jobs[0].request.start, 70 * 60);
        assert_eq!(jobs[0].request.start.rem_euclid(60), 0);
    }
    drop(store);
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("id:recording-job"));
    }
    step(app.wait_for("text:Cancel recording"));
    if idiom == Idiom::Tv {
        // Remote: second action cancels the schedule, then Back returns to the guide.
        step(app.press_key("down"));
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Cancel recording"));
    }
    step(app.wait_for_within("text:/Cancelled ·/", Duration::from_secs(10)));
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("id:recording-job"));
    }
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Delete recording"));
    }
    step(app.wait_for("text:/Delete this recording file permanently/"));
    // Closing the whole screen also cancels the pending confirmation. Reopening
    // and choosing Delete must ask again instead of reusing the previous consent.
    if idiom != Idiom::Tv {
        step(app.press("text:Close"));
        step(app.press("text:/^☰  /"));
        step(app.press("label:Recordings"));
        step(app.press("id:recording-job"));
        step(app.press("text:Delete recording"));
        step(app.wait_for("text:/Delete this recording file permanently/"));
    }
    // Cancelling a delete never loses the job.
    step(app.back());
    step(app.wait_for_gone(
        "text:/Delete this recording file permanently/",
        Duration::from_secs(2),
    ));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .recording_jobs()
            .unwrap()
            .len(),
        1
    );
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Delete recording"));
    }
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Delete recording"));
    }
    step(app.wait_for_within("text:/No recordings here/", Duration::from_secs(10)));
    step(app.back());
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(2)));
}

#[test]
fn tv_schedules_and_cancels_without_a_keyboard_or_touch_requirement() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Tv);
}
#[test]
fn tablet_schedules_cancels_and_confirms_deletion() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Pad);
}
#[test]
fn desktop_schedules_cancels_and_confirms_deletion() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Desktop);
}

fn future_programme(home: crate::isolated::Home, idiom: Idiom) {
    let (mut app, path, source) = fixture(&home, idiom, "https://example.test/generated.ts");
    step(app.press_key("right"));
    let selected = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| {
            node.test_id.as_deref() == Some("guide-programme")
                && app
                    .screen()
                    .accessibility_state(&format!("view:{}", node.id))
                    .unwrap()
                    .selected
        })
        .expect("focused future programme")
        .id;
    if idiom == Idiom::Desktop {
        step(app.right_click(&format!("view:{selected}")));
    } else {
        step(app.long_press(&format!("view:{selected}")));
    }
    step(app.wait_for("id:menu-panel >> text:Schedule a recording…"));
    assert!(
        !app.screen()
            .matches("id:menu-panel >> text:Record")
            .unwrap()
    );
    step(app.press("id:menu-panel >> text:Schedule a recording…"));
    step(app.wait_for("text:Save recording"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    step(app.press("text:Save recording"));
    step(app.wait_for_within("text:/Scheduled ·/", Duration::from_secs(15)));
    let jobs = crate::isolated::open_database(path)
        .unwrap()
        .recording_jobs()
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].request.source, source.0);
    assert!(
        jobs[0].request.start > now,
        "future programme must not become record-now"
    );
    assert!(jobs[0].request.end > jobs[0].request.start);
}

#[test]
fn right_click_on_a_future_programme_prefills_its_schedule() {
    let Some(home) = isolated!() else { return };
    future_programme(home, Idiom::Desktop);
}

#[test]
fn long_press_on_a_future_programme_prefills_its_schedule() {
    let Some(home) = isolated!() else { return };
    future_programme(home, Idiom::Pad);
}

#[test]
fn play_bar_records_immediately_without_a_form_or_reopening_the_player() {
    let Some(home) = isolated!() else { return };
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/generated.ts", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let until = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < until {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut head = [0; 4096];
            if stream.read(&mut head).is_err() {
                return;
            }
            if stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n",
                )
                .is_err()
            {
                return;
            }
            let mut packets = [0u8; 18800];
            for packet in packets.as_chunks_mut::<188>().0 {
                packet[0] = 0x47;
                packet[3] = 0x10;
            }
            while std::time::Instant::now() < until {
                if stream.write_all(&packets).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            return;
        }
    });
    let (mut app, path, _) = fixture(&home, Idiom::Pad, &url);
    step(app.press("text:A"));
    step(app.press("text:A"));
    step(app.wait_for_within("text:▶", Duration::from_secs(15)));
    step(app.wait_for("label:Video"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Record"));
    let surface = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.kind == nsh_core::ViewKind::Native)
        .unwrap()
        .id;
    let opens = iptv_app::player_opens(0);
    step(app.press("label:Record"));
    assert!(!app.screen().matches("id:recordings-screen").unwrap());
    if let Err(error) = app.wait_for_within("id:recording-indicator", Duration::from_secs(15)) {
        let jobs = crate::isolated::open_database(&path)
            .unwrap()
            .recording_jobs()
            .unwrap();
        eprintln!(
            "GENERATED_RECORD_JOBS {:?}; server_finished={}",
            jobs,
            server.is_finished()
        );
        panic!("{error}");
    }
    assert_eq!(iptv_app::player_opens(0), opens);
    assert!(
        app.screen()
            .layout_nodes()
            .iter()
            .any(|n| n.id == surface && n.kind == nsh_core::ViewKind::Native)
    );
    // Repeated Record is idempotent rather than starting another capture.
    if !app.screen().matches("label:Record").unwrap() {
        step(app.press("label:Video"));
    }
    step(app.press("label:Record"));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .recording_jobs()
            .unwrap()
            .len(),
        1
    );
    step(app.press("id:recording-indicator"));
    step(app.press("id:recording-job"));
    step(app.press("text:Stop recording"));
    step(app.wait_for_within("text:/Completed ·/", Duration::from_secs(10)));
    assert_eq!(iptv_app::player_opens(0), opens);
    server.join().unwrap();
}
