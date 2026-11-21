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
        assert!(iptv_core::i18n::is_cataloged(failure.message()));
    }
}

fn fixture(
    home: &crate::isolated::Home,
    idiom: Idiom,
    media_url: &str,
    with_guide: bool,
) -> (
    Headless,
    std::path::PathBuf,
    iptv_library::SourceId,
    Option<(i64, i64)>,
) {
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("playlists.max_connections", Value::Int(3))
        .unwrap();
    if with_guide {
        settings
            .set("epg.update_on_start", Value::Bool(true))
            .unwrap();
    }
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
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"generated\" group-title=\"Generated group\",A\n{media_url}\n"
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
    let mut expected = None;
    if with_guide {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let future = now / 60 * 60 + 1200;
        expected = Some((future, future + 2700));
        let stamp = iptv_epg::format_xmltv_time;
        let xml = format!(
            "<tv><channel id=\"generated\"><display-name>A</display-name></channel>\
             <programme channel=\"generated\" start=\"{}\" stop=\"{}\"><title>Generated live program</title></programme>\
             <programme channel=\"generated\" start=\"{}\" stop=\"{}\"><title>Generated future program</title></programme></tv>",
            stamp(now - 1200),
            stamp(future),
            stamp(future),
            stamp(future + 2700)
        );
        let guide = home.path().join("generated-guide.xml");
        std::fs::write(&guide, xml).unwrap();
        store
            .set_guide_sources(
                source,
                &[iptv_store_sqlite::GuideSource {
                    feed: iptv_epg::FeedId(1001),
                    provider_index: None,
                    url: SecretUrl::new(guide.to_string_lossy()),
                    name: Some("Generated guide".into()),
                    enabled: true,
                    shift_minutes: 0,
                }],
            )
            .unwrap();
    }
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
    (app, path, source, expected)
}

fn exercise(home: crate::isolated::Home, idiom: Idiom) {
    let (mut app, path, source, _) =
        fixture(&home, idiom, "https://example.test/generated.ts", false);
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
        for _ in 0..5 {
            step(app.press_key("up"));
        }
        step(app.press_key("right"));
        for _ in 0..5 {
            step(app.press_key("down"));
        }
        step(app.press_key("enter"));
    } else {
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
        step(app.press("id:recording-options"));
        for minutes in ["5", "0"] {
            step(app.press("label:/^Start early ·/"));
            step(app.wait_for("id:recording-entry"));
            step(app.clear("id:recording-entry"));
            step(app.type_text("id:recording-entry", minutes));
            step(app.press("id:recording-entry >> text:Done"));
        }
        step(app.wait_for("label:Start early · 0 min"));
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
        step(app.press_key("m"));
        step(app.wait_for("text:Cancel recording"));
        step(app.press_key("down"));
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Cancel recording ·/"));
    }
    step(app.wait_for_within("text:/Canceled ·/", Duration::from_secs(10)));
    if idiom == Idiom::Tv {
        step(app.press_key("m"));
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Delete recording ·/"));
    }
    step(app.wait_for("id:recording-delete-confirmation"));
    // Back cancels and a later attempt needs its own confirmation.
    step(app.back());
    step(app.wait_for_gone("id:recording-delete-confirmation", Duration::from_secs(2)));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .recording_jobs()
            .unwrap()
            .len(),
        1
    );
    if idiom == Idiom::Tv {
        step(app.press_key("m"));
        step(app.press_key("enter"));
        // Destructive confirmation defaults to Cancel for remote users.
        step(app.press_key("right"));
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Delete recording ·/"));
        step(app.press("id:recording-delete-confirmation >> text:Delete recording"));
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

fn future_program(home: crate::isolated::Home, idiom: Idiom) {
    let (mut app, path, source, expected) =
        fixture(&home, idiom, "https://example.test/generated.ts", true);
    step(app.wait_for("text:Generated future program"));
    step(app.press_key("right"));
    let selected = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| {
            node.test_id.as_deref() == Some("guide-program")
                && app
                    .screen()
                    .accessibility_state(&format!("view:{}", node.id))
                    .unwrap()
                    .selected
        })
        .expect("focused future program")
        .id;
    if idiom == Idiom::Desktop {
        step(app.right_click(&format!("view:{selected}")));
    } else {
        step(app.long_press(&format!("view:{selected}")));
    }
    step(app.wait_for("id:menu-panel >> text:Schedule recording"));
    assert!(
        !app.screen()
            .matches("id:menu-panel >> text:Record")
            .unwrap()
    );
    step(app.press("id:menu-panel >> text:Schedule recording"));
    assert!(
        !app.screen().matches("text:Save recording").unwrap(),
        "guide scheduling must skip the custom form"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    step(app.wait_for_within("text:/Scheduled ·/", Duration::from_secs(15)));
    let jobs = crate::isolated::open_database(&path)
        .unwrap()
        .recording_jobs()
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].request.source, source.0);
    assert_eq!(jobs[0].request.title, "Generated future program");
    assert!(
        jobs[0].request.start > now,
        "future program must not become record-now"
    );
    assert_eq!(
        (jobs[0].request.start, jobs[0].request.end),
        expected.unwrap()
    );
    step(app.press("id:recording-job"));
    step(app.wait_for("text:Save recording"));
    step(app.press("text:Close"));
    // Repeating the same guide action opens the existing job, never a duplicate.
    if idiom == Idiom::Desktop {
        step(app.right_click(&format!("view:{selected}")));
    } else {
        step(app.long_press(&format!("view:{selected}")));
    }
    step(app.press("id:menu-panel >> text:Schedule recording"));
    step(app.wait_for("text:Cancel recording"));
    assert_eq!(
        crate::isolated::open_database(path)
            .unwrap()
            .recording_jobs()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn right_click_on_a_future_program_schedules_without_a_form() {
    let Some(home) = isolated!() else { return };
    future_program(home, Idiom::Desktop);
}

#[test]
fn long_press_on_a_future_program_schedules_without_a_form() {
    let Some(home) = isolated!() else { return };
    future_program(home, Idiom::Pad);
}

#[test]
fn long_press_on_a_current_program_offers_record_without_starting_playback() {
    let Some(home) = isolated!() else { return };
    let (mut app, _, _, _) = fixture(&home, Idiom::Pad, "https://example.test/generated.ts", true);
    step(app.wait_for("text:Generated live program"));
    let opens = iptv_app::player_opens(0);
    step(app.long_press("role:cell[Generated live program]"));
    step(app.wait_for("id:menu-panel >> text:/^Generated live program ·/"));
    step(app.wait_for("id:menu-panel >> label:Record"));
    assert!(
        !app.screen()
            .accessibility_state("id:menu-panel >> label:Record")
            .unwrap()
            .disabled
    );
    assert!(
        !app.screen()
            .matches("id:menu-panel >> text:Schedule recording")
            .unwrap()
    );
    assert_eq!(iptv_app::player_opens(0), opens);
    assert!(!app.screen().matches("id:recordings-screen").unwrap());
    step(app.back());
    step(app.wait_for_gone("id:menu-panel >> label:Record", Duration::from_secs(2)));
}

struct CaptureServer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl CaptureServer {
    fn close(&mut self) -> std::thread::Result<()> {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.thread.take().map_or(Ok(()), |thread| thread.join())
    }
    fn join(mut self) -> std::thread::Result<()> {
        self.close()
    }
}
impl Drop for CaptureServer {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn local_capture() -> (String, CaptureServer) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/generated.ts", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let thread = std::thread::spawn(move || {
        let mut handlers = Vec::new();
        while !stopping.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let stop = stopping.clone();
                    handlers.push(std::thread::spawn(move || serve_capture(stream, stop)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("synthetic capture listener failed: {error}"),
            }
        }
        for handler in handlers {
            handler.join().unwrap();
        }
    });
    (
        url,
        CaptureServer {
            stop,
            thread: Some(thread),
        },
    )
}

fn serve_capture(
    mut stream: std::net::TcpStream,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::io::{Read, Write};
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut head = [0; 4096];
    if !matches!(stream.read(&mut head), Ok(n) if n > 0) {
        return;
    }
    if stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return;
    }
    let mut packets = [0u8; 18800];
    for packet in packets.as_chunks_mut::<188>().0 {
        packet[0] = 0x47;
        packet[3] = 0x10;
    }
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        if stream.write_all(&packets).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn local_capture_serves_two_streams_without_waiting_for_either_to_close() {
    use std::io::{BufRead, Read, Write};
    let (url, server) = local_capture();
    let address = url
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut clients = Vec::new();
    for _ in 0..2 {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .write_all(b"GET /generated.ts HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut reader = std::io::BufReader::new(stream);
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
        }
        let mut packet = [0u8; 188];
        reader.read_exact(&mut packet).unwrap();
        assert_eq!((packet[0], packet[3]), (0x47, 0x10));
        clients.push(reader);
    }
    let mut packet = [0u8; 188];
    clients[0].read_exact(&mut packet).unwrap();
    assert_eq!(packet[0], 0x47, "the first stream is still live");
    drop(clients);
    server.join().unwrap();
}

#[test]
fn play_bar_records_immediately_without_a_form_or_reopening_the_player() {
    let Some(home) = isolated!() else { return };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(&home, Idiom::Pad, &url, false);
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
    step(
        app.wait_until("active record toggle", Duration::from_secs(15), |screen| {
            screen
                .accessibility_state("id:play-bar-record")
                .is_ok_and(|s| s.pressed == Some(nsh_core::Checked::True) && !s.disabled)
        }),
    );
    assert!(!app.screen().matches("id:recording-indicator").unwrap());
    assert_eq!(iptv_app::player_opens(0), opens);
    assert!(
        app.screen()
            .layout_nodes()
            .iter()
            .any(|n| n.id == surface && n.kind == nsh_core::ViewKind::Native)
    );
    step(app.press("id:play-bar-record"));
    step(
        app.wait_until("idle record toggle", Duration::from_secs(15), |screen| {
            screen
                .accessibility_state("id:play-bar-record")
                .is_ok_and(|s| s.pressed == Some(nsh_core::Checked::False) && !s.disabled)
        }),
    );
    let jobs = crate::isolated::open_database(&path)
        .unwrap()
        .recording_jobs()
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert!(matches!(
        jobs[0].status,
        iptv_core::recording::Status::Completed | iptv_core::recording::Status::Canceled
    ));
    assert!(!app.screen().matches("id:recordings-screen").unwrap());
    assert_eq!(iptv_app::player_opens(0), opens);
    server.join().unwrap();
}

#[test]
fn a_recordings_screen_opened_during_record_now_stays_open_after_save() {
    let Some(home) = isolated!() else { return };
    let (url, server) = local_capture();
    let (mut app, _, _, _) = fixture(&home, Idiom::Pad, &url, false);
    step(app.press("text:A"));
    step(app.press("text:A"));
    step(app.wait_for_within("text:▶", Duration::from_secs(15)));
    step(app.press("label:Video"));
    step(app.wait_for("label:Record"));
    let pending = iptv_app::hold_writer();
    assert!(pending.wait_entered(Duration::from_secs(5)));
    step(app.press("label:Record"));
    assert!(!app.screen().matches("id:recordings-screen").unwrap());
    step(app.press_key("m"));
    step(app.wait_for("id:player-controls >> text:Recordings"));
    step(app.press("id:player-controls >> text:Recordings"));
    step(app.wait_for("id:recordings-screen"));
    assert!(!app.screen().matches("id:recording-job").unwrap());
    drop(pending);
    step(app.wait_for_within("id:recording-job", Duration::from_secs(15)));
    assert!(app.screen().matches("id:recordings-screen").unwrap());
    step(app.wait_for_within("label:/^Recording ·/", Duration::from_secs(15)));
    step(app.press("label:/^Stop recording ·/"));
    step(app.wait_for_within("text:/Completed ·/", Duration::from_secs(10)));
    server.join().unwrap();
}

#[test]
fn one_recording_list_keeps_scheduled_and_finished_jobs_visible() {
    let Some(home) = isolated!() else { return };
    let (mut app, path, source, _) =
        fixture(&home, Idiom::Tv, "https://example.test/generated.ts", false);
    use iptv_core::recording::{JobId, Request, Status};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut store = crate::isolated::open_database(&path).unwrap();
    let channel = store.load_channels(source, &Default::default()).unwrap()[0]
        .channel
        .id
        .as_str()
        .to_owned();
    for (n, status) in [Status::Scheduled, Status::Completed, Status::Failed]
        .into_iter()
        .enumerate()
    {
        let request = Request {
            source: source.0,
            channel: channel.clone(),
            channel_name: "Generated channel".into(),
            title: format!("Generated library job {}", n + 1),
            start: now + 3600 * (n as i64 + 1),
            end: now + 3600 * (n as i64 + 2),
        };
        let id = store
            .schedule_recording(JobId(n as i64 + 1), &request, now)
            .unwrap();
        if status != Status::Scheduled {
            assert!(
                store
                    .transition_recording(id, Status::Scheduled, status, 0, "")
                    .unwrap()
            );
        }
    }
    drop(store);
    step(app.press("text:/^☰  /"));
    step(app.press("label:Recordings"));
    step(app.wait_for("id:recording-job >> nth=2"));
    assert_eq!(app.screen().count("id:recording-status").unwrap(), 3);
    for label in ["Scheduled", "Completed", "Failed"] {
        assert!(
            app.screen()
                .matches(&format!("id:recording-job >> label:/^{label} ·/"))
                .unwrap_or(false)
                || app
                    .screen()
                    .matches(&format!("label:/^{label} · Generated library job/"))
                    .unwrap()
        );
    }
    for label in ["All", "Active", "Finished"] {
        assert_eq!(app.screen().count(&format!("text:{label}")).unwrap(), 0);
    }
    step(app.press_key("right"));
    step(app.press_key("left"));
    assert_eq!(app.screen().count("id:recording-job").unwrap(), 3);
}
