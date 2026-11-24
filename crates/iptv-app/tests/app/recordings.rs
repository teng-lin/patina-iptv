//! Synthetic recording through the real shell: one workflow for keys and touch.
use crate::headless::Headless;
use crate::personalization::step;
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
        assert!(iptv_presentation::i18n::is_cataloged(failure.message()));
    }
}

#[derive(Clone, Copy)]
pub(crate) enum GuideFixture {
    Listing(&'static str),
    Gap,
}

pub(crate) fn fixture(
    home: &crate::isolated::Home,
    idiom: Idiom,
    media_url: &str,
    guide_fixture: Option<GuideFixture>,
) -> (
    Headless,
    std::path::PathBuf,
    iptv_library::SourceId,
    Option<(i64, i64)>,
) {
    fixture_in(home, idiom, media_url, guide_fixture, "en")
}

fn fixture_in(
    home: &crate::isolated::Home,
    idiom: Idiom,
    media_url: &str,
    guide_fixture: Option<GuideFixture>,
    locale: &str,
) -> (
    Headless,
    std::path::PathBuf,
    iptv_library::SourceId,
    Option<(i64, i64)>,
) {
    let mut settings = Settings::new();
    settings
        .set("general.language", Value::text(locale))
        .unwrap();
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
    if let Some(guide_fixture) = guide_fixture {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let (future, end) = match guide_fixture {
            GuideFixture::Listing(_) => {
                let start = now / 60 * 60 + 1200;
                (start, start + 2700)
            }
            GuideFixture::Gap => {
                let start = now.div_euclid(60) * 60 + 1200;
                let end = start.div_euclid(3600) * 3600 + 3600;
                (start, end)
            }
        };
        expected = Some((future, end));
        let stamp = iptv_epg::format_xmltv_time;
        let mut xml = format!(
            "<tv><channel id=\"generated\"><display-name>A</display-name></channel>\
             <programme channel=\"generated\" start=\"{}\" stop=\"{}\"><title>Generated live program</title></programme>",
            stamp(now - 1200),
            stamp(future),
        );
        if let GuideFixture::Listing(title) = guide_fixture {
            xml.push_str(&format!(
                "<programme channel=\"generated\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
                stamp(future), stamp(end),
            ));
        }
        xml.push_str("</tv>");
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
    step(app.open_guide_groups());
    step(app.press_key("right"));
    (app, path, source, expected)
}

/// Two jobs for later, stored the way the runtime stores them, as an
/// earlier version scheduled them before scheduling was retired (#273).
fn seed_scheduled(path: &std::path::Path, source: iptv_library::SourceId) {
    use iptv_core::recording::{JobId, Request};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut store = crate::isolated::open_database(path).unwrap();
    let channel = store.load_channels(source, &Default::default()).unwrap()[0]
        .channel
        .id
        .as_str()
        .to_owned();
    let request = |title: &str, start: i64| Request {
        source: source.0,
        channel: channel.clone(),
        channel_name: "Generated channel".into(),
        title: title.into(),
        start,
        end: start + 1800,
    };
    store
        .schedule_recording(
            JobId(1),
            &request("Generated scheduled recording", now + 3600),
            now,
        )
        .unwrap();
    store
        .schedule_recording(
            JobId(2),
            &request("Generated later recording", now + 7200),
            now,
        )
        .unwrap();
}

fn jobs(path: &std::path::Path) -> Vec<iptv_core::recording::Job> {
    crate::isolated::open_database(path)
        .unwrap()
        .recording_jobs()
        .unwrap()
}

#[track_caller]
fn wait_for_jobs(
    app: &mut Headless,
    path: &std::path::Path,
    what: &str,
    done: impl Fn(&[iptv_core::recording::Job]) -> bool,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !done(&jobs(path)) {
        assert!(std::time::Instant::now() < deadline, "{what}");
        app.idle(Duration::from_millis(200)).unwrap();
    }
}

/// Scheduling is retired (#273): no menu offers it, and jobs scheduled
/// before stay listed and can be canceled, but not edited.
fn exercise(home: crate::isolated::Home, idiom: Idiom) {
    use iptv_core::recording::Status;
    let (mut app, path, source, _) =
        fixture(&home, idiom, "https://example.test/generated.ts", None);
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favorites…"));
    assert!(!app.shows("text:Schedule a recording…"));
    assert!(!app.shows("text:Schedule recording"));
    step(app.back());
    step(app.wait_for_gone("text:Manage favorites…", Duration::from_secs(2)));

    seed_scheduled(&path, source);
    crate::browse_fixture::open_rail_item(&mut app, "Recordings");
    step(app.wait_for("id:recordings-screen"));
    // Newest start first.
    step(app.wait_for_within("id:recording-job >> nth=1", Duration::from_secs(15)));
    assert_eq!(app.screen().count("text:/Scheduled ·/").unwrap(), 2);
    assert!(!app.shows("label:/^Edit recording ·/"));
    if idiom == Idiom::Tv {
        // Select on a scheduled job lists its actions; none edits it.
        step(app.press_key("enter"));
        step(app.wait_for("text:Cancel recording"));
        assert!(!app.shows("text:Edit recording"));
        assert!(!app.shows("text:Cancel repeating schedule"));
        step(app.press_key("enter"));
    } else {
        step(app.press("role:button[/^More options for Generated later recording/]"));
        step(app.wait_for("text:Cancel recording"));
        assert!(!app.shows("text:Edit recording"));
        assert!(!app.shows("text:Cancel repeating schedule"));
        step(app.press("text:Cancel recording"));
    }
    wait_for_jobs(&mut app, &path, "the later job is canceled", |jobs| {
        jobs.iter()
            .any(|j| j.id.0 == 2 && j.status == Status::Canceled)
    });
    if idiom == Idiom::Tv {
        step(app.wait_for_gone("text:Cancel recording", Duration::from_secs(5)));
        step(app.press_key("down"));
        step(app.press_key("enter"));
        step(app.wait_for("text:Cancel recording"));
        assert!(!app.shows("text:Edit recording"));
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Cancel recording · Generated scheduled recording/"));
    }
    wait_for_jobs(&mut app, &path, "the one-off job is canceled", |jobs| {
        jobs.iter()
            .any(|j| j.id.0 == 1 && j.status == Status::Canceled)
    });
    step(app.wait_until(
        "both jobs show as canceled",
        Duration::from_secs(10),
        |screen| screen.count("text:/Canceled ·/").is_ok_and(|n| n == 2),
    ));
    if idiom == Idiom::Tv {
        step(app.press_key("m"));
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Delete recording · Generated scheduled recording/"));
    }
    step(app.wait_for("id:recording-delete-confirmation"));
    // Back cancels and a later attempt needs its own confirmation.
    step(app.back());
    step(app.wait_for_gone("id:recording-delete-confirmation", Duration::from_secs(2)));
    assert_eq!(jobs(&path).len(), 2);
    if idiom == Idiom::Tv {
        step(app.press_key("m"));
        step(app.press_key("enter"));
        // Destructive confirmation defaults to Cancel for remote users.
        step(app.press_key("right"));
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Delete recording · Generated scheduled recording/"));
        step(app.press("id:recording-delete-confirmation >> text:Delete recording"));
    }
    wait_for_jobs(&mut app, &path, "the one-off job is deleted", |jobs| {
        jobs.len() == 1 && jobs[0].id.0 == 2
    });
    step(app.back());
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(2)));
}

#[test]
fn tv_lists_and_cancels_scheduled_jobs_without_editing_them() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Tv);
}
#[test]
fn tablet_lists_cancels_and_deletes_scheduled_jobs_without_editing_them() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Pad);
}
#[test]
fn desktop_lists_cancels_and_deletes_scheduled_jobs_without_editing_them() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Desktop);
}

/// A future program offers no way to record it: not its menu, not the
/// Record key, not its details.
fn future_program(home: crate::isolated::Home, idiom: Idiom, listing: GuideFixture) {
    let (mut app, path, _, _) = fixture(
        &home,
        idiom,
        "https://example.test/generated.ts",
        Some(listing),
    );
    step(app.wait_for("text:Generated live program"));
    let title = match listing {
        GuideFixture::Listing(title) => title,
        GuideFixture::Gap => "No information",
    };
    step(app.wait_for(&format!("text:{title}")));
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
    let cell = format!("view:{selected}");
    // The channel menu of a future program neither records nor schedules it.
    if idiom == Idiom::Desktop {
        step(app.right_click(&cell));
    } else {
        step(app.long_press(&cell));
    }
    let favorite = "id:menu-panel >> text:Add to favorites";
    step(app.wait_for(favorite));
    for item in ["Schedule recording", "Record now"] {
        assert!(
            !app.screen()
                .matches(&format!("id:menu-panel >> label:/^{item}/"))
                .unwrap(),
            "{item}"
        );
    }
    step(app.back());
    step(app.wait_for_gone(favorite, Duration::from_secs(2)));
    // Nor does its program menu.
    step(app.press_key("m"));
    let details = "id:menu-panel >> text:Details";
    step(app.wait_for(details));
    for item in ["Schedule recording", "Schedule a recording…", "Record"] {
        assert!(
            !app.screen()
                .matches(&format!("id:menu-panel >> text:{item}"))
                .unwrap(),
            "{item}"
        );
    }
    // The details offer no Record button, and Select and Record do nothing.
    step(app.press(details));
    step(app.wait_for(&format!("id:description-layer >> text:{title}")));
    assert!(!app.shows("id:description-layer >> text:Record"));
    step(app.press_remote(nsh_tv::RemoteKey::Record));
    step(app.press_key("enter"));
    step(app.back());
    // The Record key on a future program explains instead of scheduling.
    step(app.press_remote(nsh_tv::RemoteKey::Record));
    step(app.wait_for_within(
        "id:app-notice >> text:Only a program on now can be recorded",
        Duration::from_secs(5),
    ));
    assert!(!app.screen().matches("id:recordings-screen").unwrap());
    assert!(jobs(&path).is_empty(), "no job is scheduled");
}

#[test]
fn right_click_on_a_future_program_offers_no_scheduling() {
    let Some(home) = isolated!() else { return };
    future_program(
        home,
        Idiom::Desktop,
        GuideFixture::Listing("No information"),
    );
}

#[test]
fn long_press_on_a_future_program_offers_no_scheduling() {
    let Some(home) = isolated!() else { return };
    future_program(home, Idiom::Pad, GuideFixture::Listing("Loading…"));
}

#[test]
fn a_future_guide_gap_offers_no_scheduling() {
    let Some(home) = isolated!() else { return };
    future_program(home, Idiom::Desktop, GuideFixture::Gap);
}

fn stored_title_in_japanese_guide(home: crate::isolated::Home, title: &'static str) {
    let (mut app, _, _, _) = fixture_in(
        &home,
        Idiom::Pad,
        "https://example.test/generated.ts",
        Some(GuideFixture::Listing(title)),
        "ja",
    );
    step(app.wait_for(&format!("role:cell[{title}]")));
    step(app.wait_for("role:cell[情報なし]"));
}

#[test]
fn japanese_guide_preserves_the_providers_loading_title() {
    let Some(home) = isolated!() else { return };
    stored_title_in_japanese_guide(home, "Loading…");
}

#[test]
fn japanese_guide_preserves_the_providers_no_information_title() {
    let Some(home) = isolated!() else { return };
    stored_title_in_japanese_guide(home, "No information");
}

#[test]
fn long_press_on_a_current_program_offers_record_now_in_the_channel_menu() {
    let Some(home) = isolated!() else { return };
    let (mut app, _, _, _) = fixture(
        &home,
        Idiom::Pad,
        "https://example.test/generated.ts",
        Some(GuideFixture::Listing("Generated future program")),
    );
    step(app.wait_for("text:Generated live program"));
    let opens = iptv_app::player_opens(0);
    step(app.long_press("role:cell[Generated live program]"));
    let record_now = "id:menu-panel >> label:/^Record now · Generated live program/";
    step(app.wait_for(record_now));
    assert!(!app.shows("id:menu-panel >> text:Manage favorites…"));
    assert!(
        !app.screen()
            .accessibility_state(record_now)
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
    step(app.wait_for_gone(record_now, Duration::from_secs(2)));
}

#[test]
fn details_record_on_a_current_program_records_now_without_a_form() {
    let Some(home) = isolated!() else { return };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(
        &home,
        Idiom::Pad,
        &url,
        Some(GuideFixture::Listing("Generated future program")),
    );
    step(app.wait_for("text:Generated live program"));
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    step(app.press_key("m"));
    step(app.press("id:menu-panel >> text:Details"));
    let record = "id:description-layer >> text:Record";
    step(app.wait_for(record));
    step(app.press(record));
    assert!(!app.screen().matches("text:Save recording").unwrap());
    wait_for_jobs(&mut app, &path, "Record starts a job now", |jobs| {
        jobs.len() == 1 && jobs[0].status.active()
    });
    let job = jobs(&path).remove(0);
    assert_eq!(job.request.title, "Generated live program");
    assert!(job.request.start <= started + 60, "it records now");
    assert!(!app.screen().matches("id:recordings-screen").unwrap());
    crate::browse_fixture::open_rail_item(&mut app, "Recordings");
    step(app.wait_for_within("label:/^Stop recording ·/", Duration::from_secs(15)));
    step(app.press("label:/^Stop recording ·/"));
    wait_for_jobs(&mut app, &path, "the recording stops", |jobs| {
        !jobs[0].status.active()
    });
    server.join().unwrap();
}

pub(crate) struct CaptureServer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl CaptureServer {
    fn close(&mut self) -> std::thread::Result<()> {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.thread.take().map_or(Ok(()), |thread| thread.join())
    }
    pub(crate) fn join(mut self) -> std::thread::Result<()> {
        self.close()
    }
}
impl Drop for CaptureServer {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub(crate) fn local_capture() -> (String, CaptureServer) {
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
    let (mut app, path, _, _) = fixture(&home, Idiom::Pad, &url, None);
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
    let (mut app, _, _, _) = fixture(&home, Idiom::Pad, &url, None);
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
        fixture(&home, Idiom::Tv, "https://example.test/generated.ts", None);
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
    step(app.open_guide_groups());
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

/// Opens the channel menu of `cell` until it offers `item`. The menu reads
/// the recorder's job snapshot, which follows the store within seconds.
#[track_caller]
fn channel_menu_offering(app: &mut Headless, cell: &str, item: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let open = "id:menu-panel >> text:Add to favorites";
    loop {
        step(app.long_press(cell));
        step(app.wait_for(open));
        if app.screen().matches(item).unwrap() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the channel menu never offered {item}"
        );
        step(app.back());
        step(app.wait_for_gone(open, Duration::from_secs(2)));
        app.idle(Duration::from_millis(250)).unwrap();
    }
}

/// Record now from the guide: the Record key and the channel menu start a
/// program on now, a repeated Record does not start it twice, and the
/// channel menu stops it.
#[test]
fn record_key_and_channel_menu_start_and_stop_a_program_on_now() {
    let Some(home) = isolated!() else { return };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(
        &home,
        Idiom::Pad,
        &url,
        Some(GuideFixture::Listing("Generated future program")),
    );
    // A badge prefixes the title while a job covers it.
    let live = "role:cell[/Generated live program$/]";
    let stop = "id:menu-panel >> label:/^Stop recording · Generated live program/";
    step(app.wait_for(live));
    step(app.press_remote(nsh_tv::RemoteKey::Record));
    wait_for_jobs(&mut app, &path, "the Record key starts a job", |jobs| {
        jobs.len() == 1 && jobs[0].status.active()
    });
    assert_eq!(jobs(&path)[0].request.title, "Generated live program");
    assert!(!app.screen().matches("id:recordings-screen").unwrap());

    // Once the recorder lists the job, Record again only confirms it.
    channel_menu_offering(&mut app, live, stop);
    step(app.back());
    step(app.wait_for_gone(stop, Duration::from_secs(2)));
    step(app.press_remote(nsh_tv::RemoteKey::Record));
    step(app.wait_for_within(
        "id:app-notice >> text:Recording started",
        Duration::from_secs(5),
    ));
    app.idle(Duration::from_millis(500)).unwrap();
    assert_eq!(
        jobs(&path).len(),
        1,
        "a repeated Record starts no second job"
    );

    channel_menu_offering(&mut app, live, stop);
    step(app.press(stop));
    wait_for_jobs(&mut app, &path, "the channel menu stops it", |jobs| {
        jobs.len() == 1 && !jobs[0].status.active()
    });

    let record_now = "id:menu-panel >> label:/^Record now · Generated live program/";
    channel_menu_offering(&mut app, live, record_now);
    step(app.press(record_now));
    wait_for_jobs(&mut app, &path, "Record now starts a new job", |jobs| {
        jobs.len() == 2 && jobs.iter().any(|j| j.status.active())
    });
    channel_menu_offering(&mut app, live, stop);
    step(app.press(stop));
    wait_for_jobs(&mut app, &path, "the second job stops", |jobs| {
        jobs.iter().all(|j| !j.status.active())
    });
    server.join().unwrap();
}

/// A job scheduled before scheduling was retired (#273) is offered for
/// cancellation from its program's channel menu.
#[test]
fn channel_menu_cancels_a_job_scheduled_before_scheduling_was_retired() {
    use iptv_core::recording::{JobId, Request, Status};
    let Some(home) = isolated!() else { return };
    let (mut app, path, source, expected) = fixture(
        &home,
        Idiom::Pad,
        "https://example.test/generated.ts",
        Some(GuideFixture::Listing("Generated future program")),
    );
    let (start, end) = expected.unwrap();
    let future = "role:cell[Generated future program]";
    step(app.wait_for(future));
    let mut store = crate::isolated::open_database(&path).unwrap();
    let channel = store.load_channels(source, &Default::default()).unwrap()[0]
        .channel
        .id
        .as_str()
        .to_owned();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    store
        .schedule_recording(
            JobId(1),
            &Request {
                source: source.0,
                channel,
                channel_name: "Generated channel".into(),
                title: "Generated future program".into(),
                start,
                end,
            },
            now,
        )
        .unwrap();
    drop(store);
    let cancel = "id:menu-panel >> label:/^Cancel recording · Generated future program/";
    channel_menu_offering(&mut app, future, cancel);
    assert!(
        !app.screen()
            .matches("id:menu-panel >> label:/^Record now/")
            .unwrap()
    );
    step(app.press(cancel));
    wait_for_jobs(&mut app, &path, "the scheduled job is canceled", |jobs| {
        jobs.len() == 1 && jobs[0].status == Status::Canceled
    });
}
