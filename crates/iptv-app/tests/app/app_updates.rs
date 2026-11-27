//! Actual Settings -> HTTP discovery -> inline release notes, cancellation and retry.
//! The only network endpoint is a generated loopback GitHub API fixture.
use super::personalization::{self, step};
use iptv_settings::{Settings, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

struct Response {
    status: u16,
    body: String,
    held: bool,
    requests: usize,
}
pub(super) struct Api {
    state: Arc<Mutex<Response>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Api {
    pub(super) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        // SAFETY: isolated! gives this scenario a fresh process. No app or
        // fixture worker has started; the environment stays fixed thereafter.
        unsafe {
            std::env::set_var(
                "IPTV_TEST_UPDATE_API",
                format!("http://{}/releases", listener.local_addr().unwrap()),
            );
        }
        let state = Arc::new(Mutex::new(Response {
            status: 200,
            body: release("9.8.7", true),
            held: true,
            requests: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let work = Arc::clone(&state);
        let stopped = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let headers = read_headers(&mut stream).to_lowercase();
                assert!(headers.starts_with("get /releases?"));
                assert!(!headers.contains("authorization:"));
                assert!(!headers.contains("cookie:"));
                assert!(headers.contains("user-agent: patina"));
                work.lock().unwrap().requests += 1;
                while work.lock().unwrap().held && !stopped.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(5));
                }
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                let state = work.lock().unwrap();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    state.status,
                    state.body.len(),
                    state.body
                );
            }
        });
        Self {
            state,
            stop,
            thread: Some(thread),
        }
    }
    pub(super) fn serve(&self, status: u16, body: String) {
        let mut state = self.state.lock().unwrap();
        state.status = status;
        state.body = body;
        state.held = false;
    }
    pub(super) fn requests(&self) -> usize {
        self.state.lock().unwrap().requests
    }
    fn hold(&self) {
        self.state.lock().unwrap().held = true;
    }
}

fn read_headers(stream: &mut impl Read) -> String {
    let mut headers = Vec::new();
    let mut chunk = [0; 1024];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !headers.windows(4).any(|part| part == b"\r\n\r\n") {
        let count = stream
            .read(&mut chunk)
            .expect("complete request headers before deadline");
        assert!(
            std::time::Instant::now() < deadline,
            "request headers exceeded deadline"
        );
        assert_ne!(count, 0, "request ended before complete headers");
        headers.extend_from_slice(&chunk[..count]);
        assert!(
            headers.len() <= 16 * 1024,
            "request headers exceed fixture limit"
        );
    }
    String::from_utf8(headers).expect("UTF-8 fixture headers")
}

#[test]
fn fixture_checks_headers_that_arrive_in_separate_reads() {
    struct Chunks(std::collections::VecDeque<&'static [u8]>);
    impl Read for Chunks {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let chunk = self.0.pop_front().unwrap_or_default();
            bytes[..chunk.len()].copy_from_slice(chunk);
            Ok(chunk.len())
        }
    }
    let mut chunks = Chunks(
        [
            b"GET /releases?per_page=100 HTTP/1.1\r\n".as_slice(),
            b"User-Agent: Patina\r\n".as_slice(),
            b"Authorization: synthetic\r\nCookie: synthetic\r\n\r\n".as_slice(),
        ]
        .into(),
    );
    let headers = read_headers(&mut chunks);
    assert!(headers.contains("User-Agent: Patina"));
    assert!(headers.contains("Authorization: synthetic"));
    assert!(headers.contains("Cookie: synthetic"));
}
impl Drop for Api {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
pub(super) fn release(version: &str, assets: bool) -> String {
    let suffix = if cfg!(target_os = "macos") {
        "macos-arm64.dmg"
    } else if cfg!(windows) {
        "windows-x86_64-setup.exe"
    } else {
        "linux-x86_64.AppImage"
    };
    release_for_suffix(version, assets, suffix)
}

fn release_for_suffix(version: &str, assets: bool, suffix: &str) -> String {
    let name = format!("Patina-{version}-{suffix}");
    serde_json::json!([{"tag_name":format!("v{version}"),"draft":false,"prerelease":false,
        "html_url":format!("https://github.com/teng-lin/patina-iptv/releases/tag/v{version}"),
        "body":"Generated release notes\n\nA generated playback correction.\nThe complete ending of the notes.",
        "published_at":"2026-11-27T12:00:00Z",
        "assets":if assets {vec![serde_json::json!({"name":name,"size":9,"browser_download_url":format!("https://github.com/teng-lin/patina-iptv/releases/download/v{version}/{name}")})]} else {vec![]}
    }]).to_string()
}

fn about(app: &mut super::Headless) {
    step(app.open_guide_groups());
    step(app.press("id:navigation-rail >> role:tab[Settings]"));
    step(app.press("id:settings-content >> text:About >> nth=0"));
    step(app.press("id:settings-content >> text:Check for updates"));
    step(app.wait_for("id:app-update-panel"));
}
fn refresh(app: &mut super::Headless) {
    // Retained release notes can be visible during an earlier check. Wait for
    // that operation to finish before asking the enabled control to retry.
    step(app.wait_for_gone(
        "id:app-update-panel >> text:Checking…",
        Duration::from_secs(10),
    ));
    step(app.press("id:app-update-panel >> text:Check for updates"));
}

#[test]
fn a_held_release_check_disables_cached_downloads_and_keeps_remote_focus_on_enabled_actions() {
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    api.serve(200, release("9.8.7", true));
    let mut settings = Settings::new();
    settings
        .set("general.app_update_frequency", Value::choice("never"))
        .unwrap();
    let mut fixture = personalization::launch(home, settings, false, |_| {}, |_| {});
    let app = &mut fixture.app;
    about(app);
    step(app.wait_for("id:app-update-panel >> text:/Latest version: 9.8.7/"));
    step(
        app.wait_for("id:app-update-panel >> text:/Update available|A newer version is available/"),
    );
    let requests = api.requests();
    api.hold();
    refresh(app);
    step(
        app.wait_until("manual HTTP check is held", Duration::from_secs(10), |_| {
            api.requests() > requests
        }),
    );
    step(app.wait_for("id:app-update-panel >> text:Checking…"));
    step(app.wait_for("id:app-update-panel >> text:/Latest version: 9.8.7/"));
    for label in [
        "Download package",
        "Release notes and downloads",
        "Download and update",
    ] {
        let selector = format!("id:app-update-panel >> text:{label}");
        if app.screen().matches(&selector).unwrap() {
            let error = app.press(&selector).unwrap_err();
            assert!(error.contains("disabled"), "{error}");
        }
    }
    assert!(app.screen().opened_urls().is_empty());
    step(app.press_key("right"));
    let focused = nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff);
    let later = "id:app-update-panel >> text:Later";
    assert_eq!(app.screen().span_style(later).unwrap().color, focused);
    // Left visits Skip and Right returns to Later; disabled Check/Download
    // controls never take remote focus while the cached version is stale.
    step(app.press_key("left"));
    assert_eq!(
        app.screen()
            .span_style("id:app-update-panel >> text:Skip this version")
            .unwrap()
            .color,
        focused
    );
    step(app.press_key("right"));
    assert_eq!(app.screen().span_style(later).unwrap().color, focused);
    api.serve(200, release("9.8.8", true));
    step(app.wait_for("id:app-update-panel >> text:/Latest version: 9.8.8/"));
    step(
        app.wait_for("id:app-update-panel >> text:/Update available|A newer version is available/"),
    );
    assert_eq!(app.screen().span_style(later).unwrap().color, focused);
    assert!(
        app.screen().opened_urls().is_empty(),
        "no old-version download was accepted"
    );
    for _ in 0..4 {
        step(app.press_key("left"));
    }
    assert_eq!(
        app.screen()
            .span_style("id:app-update-panel >> text:Check for updates")
            .unwrap()
            .color,
        focused
    );
}

fn android_distribution(app: &mut super::Headless, data: serde_json::Value) {
    app.engine().control().dispatch_event(
        super::device_signals::node(app, "device-signals"),
        nsh_core::EventKind::NativeEvent,
        nsh_core::EventPayload::Native {
            name: "device".into(),
            data,
        },
    );
    app.settle();
    step(app.wait(Duration::ZERO));
}

#[test]
fn delayed_android_distribution_does_not_cache_the_wrong_package_channel() {
    fn here() {}
    let Some(home) = super::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[
            ("IPTV_TEST_DEVICE_SIGNALS", "1"),
            ("IPTV_TEST_PLATFORM", "android"),
        ],
    ) else {
        return;
    };
    let api = Api::start();
    api.serve(
        200,
        release_for_suffix("9.8.7", true, "android-tablet-tv.apk"),
    );
    let mut settings = Settings::new();
    settings
        .set("general.app_update_frequency", Value::choice("never"))
        .unwrap();
    let mut fixture = personalization::launch(home, settings, false, |_| {}, |_| {});
    let app = &mut fixture.app;
    super::device_signals::report(app, "tv", false, true);
    about(app);
    step(app.wait_for("id:app-update-panel >> text:Checking…"));
    step(app.wait(Duration::from_millis(250)));
    assert_eq!(api.requests(), 0, "unknown is not a store distribution");
    android_distribution(app, serde_json::json!({"directApk":true}));
    step(app.wait(Duration::from_millis(250)));
    assert_eq!(
        api.requests(),
        0,
        "direct package identity is still incomplete"
    );
    android_distribution(
        app,
        serde_json::json!({"directApk":true,"versionCode":42,"androidApi":36}),
    );
    step(app.wait_for("id:app-update-panel >> text:Update available"));
    assert_eq!(
        api.requests(),
        1,
        "the waiting manual request resumes even with Never selected"
    );
    step(app.wait_for("id:app-update-panel >> text:Download package"));
    assert!(!app.screen().matches("text:Patina is up to date.").unwrap());
}

#[test]
fn android_store_report_releases_startup_check_without_offering_a_direct_apk() {
    fn here() {}
    let Some(home) = super::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[
            ("IPTV_TEST_DEVICE_SIGNALS", "1"),
            ("IPTV_TEST_PLATFORM", "android"),
        ],
    ) else {
        return;
    };
    let api = Api::start();
    api.serve(
        200,
        release_for_suffix("9.8.7", true, "android-tablet-tv.apk"),
    );
    let mut fixture = personalization::launch(home, Settings::new(), false, |_| {}, |_| {});
    let app = &mut fixture.app;
    super::device_signals::report(app, "tv", false, true);
    step(app.wait(Duration::from_millis(250)));
    assert_eq!(
        api.requests(),
        0,
        "startup waits for an explicit channel report"
    );
    android_distribution(app, serde_json::json!({"directApk":false}));
    step(app.wait_for("id:app-update-notice"));
    step(app.press_key("enter"));
    step(app.wait_for("id:app-update-panel >> text:A newer version is available, but there is no compatible package for this installation."));
    assert_eq!(api.requests(), 1);
    assert!(
        !app.screen()
            .matches("id:app-update-panel >> text:Download package")
            .unwrap()
    );
}

#[test]
fn actual_about_check_shows_notes_retains_result_and_recovers_from_failed_check() {
    let Some(home) = isolated!() else {
        return;
    };
    let api = Api::start();
    let mut settings = Settings::new();
    settings
        .set("general.app_update_frequency", Value::choice("never"))
        .unwrap();
    let mut fixture = personalization::launch(home, settings, false, |_| {}, |_| {});
    let app = &mut fixture.app;
    about(app);
    step(app.wait_until(
        "update HTTP request started",
        Duration::from_secs(10),
        |_| api.requests() > 0,
    ));
    step(app.wait_for("id:app-update-panel >> text:Checking…"));
    step(app.press("id:app-update-panel >> text:Later"));
    step(app.wait_for_gone("id:app-update-panel", Duration::from_secs(5)));
    api.serve(200, release("9.8.7", true));
    // Reopening joins or refreshes the owner's operation, rather than losing it
    // when the first update screen was dismissed.
    step(app.press("id:settings-content >> text:Check for updates"));
    step(app.wait_for("id:app-update-notes >> text:/The complete ending of the notes/"));
    step(app.wait_for(&format!(
        "text:/Installed version: {}/",
        env!("CARGO_PKG_VERSION")
    )));
    step(app.wait_for("text:/Latest version: 9.8.7/"));
    step(app.press("id:app-update-panel >> text:Skip this version"));
    step(app.wait_for_gone("id:app-update-panel", Duration::from_secs(5)));
    step(app.wait_until(
        "skipped version is durable",
        Duration::from_secs(10),
        |_| {
            let directory = fixture.path.parent().unwrap();
            let parent = directory.parent().unwrap();
            let name = directory.file_name().unwrap().to_string_lossy();
            std::fs::read(
                parent
                    .join(format!("{name}.app-update"))
                    .join("schedule.json"),
            )
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|state| state["skipped_version"] == "9.8.7")
        },
    ));
    step(app.press("id:settings-content >> text:Check for updates"));
    step(app.wait_for("text:/Latest version: 9.8.7/"));
    api.serve(503, "{}".into());
    refresh(app);
    step(app.wait_for("text:Update check failed. Try again."));
    assert!(!app.screen().matches("text:Patina is up to date.").unwrap());
    api.serve(200, release(env!("CARGO_PKG_VERSION"), true));
    refresh(app);
    step(app.wait_for("text:Patina is up to date."));
    assert!(!app.screen().matches("text:Skip this version").unwrap());
    // With no update, Right goes directly to the visible Later control.
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:app-update-panel", Duration::from_secs(5)));
    assert!(api.requests() >= 3);
}

#[test]
fn newer_release_without_matching_package_is_not_reported_as_up_to_date() {
    let Some(home) = isolated!() else {
        return;
    };
    let api = Api::start();
    api.serve(200, release("9.8.7", false));
    let mut settings = Settings::new();
    settings
        .set("general.app_update_frequency", Value::choice("never"))
        .unwrap();
    let mut fixture = personalization::launch(home, settings, false, |_| {}, |_| {});
    let app = &mut fixture.app;
    about(app);
    step(app.wait_for("text:A newer version is available, but there is no compatible package for this installation."));
    step(app.wait_for("id:app-update-notes >> text:/The complete ending of the notes/"));
    assert!(!app.screen().matches("text:Patina is up to date.").unwrap());
}

#[test]
fn default_weekly_notice_is_remote_actionable_and_monthly_choice_is_saved() {
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    api.serve(200, release("9.8.7", true));
    let mut fixture = personalization::launch(home, Settings::new(), false, |_| {}, |_| {});
    let app = &mut fixture.app;
    step(app.wait_for("id:app-update-notice"));
    // Physical left/right must route through the notice's input context, not
    // merely work when its action handler is called directly.
    step(app.press_key("right"));
    assert_eq!(
        app.screen()
            .span_style("id:app-update-notice >> text:Later")
            .unwrap()
            .color,
        nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff),
        "Right reaches the visible Later action"
    );
    step(app.press_key("left"));
    assert_eq!(
        app.screen()
            .span_style("id:app-update-notice >> text:View update")
            .unwrap()
            .color,
        nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff),
        "Left returns to Review"
    );
    step(app.press_key("enter"));
    step(app.wait_for("id:app-update-panel"));
    step(app.wait_for("id:app-update-notes >> text:/The complete ending of the notes/"));
    step(app.press("id:app-update-panel >> text:Later"));
    step(app.wait_for_gone("id:app-update-notice", Duration::from_secs(5)));
    about(app);
    step(app.wait_for("id:settings-content >> text:Weekly"));
    step(app.press("id:app-update-panel >> text:Later"));
    step(app.press("id:settings-content >> text:Automatically check for app updates"));
    step(app.wait_for("role:option[/Monthly/]"));
    step(app.press("role:option[/Monthly/]"));
    personalization::saved_settings(
        app,
        &fixture.path.parent().unwrap().join("settings.json"),
        |settings| {
            settings.choice(
                iptv_settings::keys::APP_UPDATE_FREQUENCY,
                &iptv_settings::Target::Global,
            ) == "monthly"
        },
    );
}

#[test]
fn default_weekly_notice_physical_right_selects_later_without_opening_or_downloading() {
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    api.serve(200, release("9.8.7", true));
    let mut fixture = personalization::launch(home, Settings::new(), false, |_| {}, |_| {});
    let app = &mut fixture.app;
    step(app.wait_for("id:app-update-notice"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:app-update-notice", Duration::from_secs(5)));
    assert!(
        !app.screen().matches("id:app-update-panel").unwrap(),
        "Later dismisses rather than reviewing the update"
    );
    assert!(
        app.screen().opened_urls().is_empty(),
        "Later never opens a browser or starts a download"
    );
}

#[test]
fn closing_a_joined_manual_check_keeps_the_unseen_result_and_notice_remote_focus() {
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    let mut fixture = personalization::launch(home, Settings::new(), false, |_| {}, |_| {});
    let app = &mut fixture.app;
    about(app);
    step(app.wait_for("id:app-update-panel >> text:Checking…"));
    step(app.press_key("enter"));
    api.serve(200, release("9.8.7", true));
    for _ in 0..6 {
        if !app.screen().matches("id:settings-screen").unwrap() {
            break;
        }
        step(app.back());
    }
    step(app.wait_for("id:app-update-notice"));
    assert_eq!(
        api.requests(),
        1,
        "opening during the scheduled fetch joins it"
    );
    let focused = nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff);
    assert_eq!(
        app.screen()
            .span_style("id:app-update-notice >> text:View update")
            .unwrap()
            .color,
        focused
    );
    step(app.press_key("enter"));
    step(app.wait_for("id:app-update-panel"));
    step(app.wait_for("id:app-update-notes >> text:/The complete ending of the notes/"));
    step(app.press("id:app-update-panel >> text:Later"));
    step(app.wait_for_gone("id:app-update-notice", Duration::from_secs(5)));
    assert!(app.screen().opened_urls().is_empty());
}

#[test]
fn compact_large_text_update_panel_keeps_actions_reachable_and_notes_scrollable() {
    use nsh_core::{ApproxTextEngine, Size, TextEngine, platform::Idiom, prelude::px};
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    let mut published: serde_json::Value = serde_json::from_str(&release("9.8.7", true)).unwrap();
    published[0]["body"] = serde_json::json!(format!(
        "Beginning of generated release notes.\n{}\nComplete ending of generated release notes.",
        (0..40)
            .map(|line| format!(
                "Generated change {line}: preserve playback and existing library data.\n"
            ))
            .collect::<String>()
    ));
    api.serve(200, published.to_string());
    let mut settings = Settings::new();
    settings
        .set("general.app_update_frequency", Value::choice("never"))
        .unwrap();
    settings
        .set("appearance.font_size", Value::choice("extra_large"))
        .unwrap();
    let mut fixture = personalization::launch(home, settings, false, |_| {}, |_| {});
    let app = &mut fixture.app;
    about(app);
    step(app.wait_for("text:/Latest version: 9.8.7/"));
    step(app.wait_for("id:app-update-notes >> text:/Beginning of generated release notes/"));
    let check_sizes = |app: &mut super::Headless| {
        for (width, height, idiom) in [(960.0, 540.0, Idiom::Tv), (915.0, 412.0, Idiom::Phone)] {
            step(app.set_environment("large system text", move |env| {
                env.idiom = idiom;
                env.font_scale = 1.3;
            }));
            step(app.set_viewport(Size::new(px(width), px(height))));
            app.idle(Duration::from_millis(100)).unwrap();
            let nodes = app.screen().layout_nodes();
            let bounds = |selector: &str| {
                let id = app.screen().id(selector).unwrap();
                nodes.iter().find(|node| node.id == id).unwrap().bounds
            };
            let notes = bounds("id:app-update-notes");
            let text = nodes
                .iter()
                .find_map(|node| {
                    node.text.as_ref().filter(|text| {
                        text.text
                            .text
                            .starts_with("Beginning of generated release notes")
                    })
                })
                .unwrap();
            let mut one_line = (*text.text).clone();
            one_line.text = "Generated change".into();
            one_line.spans.truncate(1);
            one_line.spans[0].end = one_line.text.len() as u32;
            let engine = ApproxTextEngine;
            let line_height = engine.measure(&engine.prepare(&one_line), None).height.0;
            let mut actions = vec!["Close", "Check for updates", "Skip this version", "Later"];
            for offered in [
                "Download package",
                "Download and update",
                "Release notes and downloads",
            ] {
                if app
                    .screen()
                    .matches(&format!("id:app-update-panel >> role:button[{offered}]"))
                    .unwrap()
                {
                    actions.push(offered);
                }
            }
            for action in actions {
                let control = bounds(&format!("id:app-update-panel >> role:button[{action}]"));
                assert!(
                    control.origin.x.0 >= 0.0
                        && control.origin.y.0 >= 0.0
                        && control.origin.x.0 + control.size.width.0 <= width + 1.0
                        && control.origin.y.0 + control.size.height.0 <= height + 1.0,
                    "{width}x{height} large text: {action} must remain inside the viewport; control={control:?}, notes={notes:?}"
                );
            }
            assert!(
                notes.size.height.0 >= line_height,
                "{width}x{height} large text: release notes need at least one readable line; notes={notes:?}, line_height={line_height}"
            );
            let start = text.content.origin.y.0;
            step(app.press_key("down"));
            step(app.wait_until(
                "remote scroll moves release notes",
                Duration::from_secs(5),
                |screen| {
                    screen
                        .layout_nodes()
                        .iter()
                        .filter_map(|node| node.text.as_ref())
                        .find(|text| {
                            text.text
                                .text
                                .starts_with("Beginning of generated release notes")
                        })
                        .is_some_and(|text| text.content.origin.y.0 < start)
                },
            ));
        }
    };
    check_sizes(app);
    // The missing-package status is longer and wraps even on a wide phone.
    // Exercise the same worst case on macOS as on a development Linux build.
    published[0]["assets"] = serde_json::json!([]);
    api.serve(200, published.to_string());
    refresh(app);
    step(app.wait_for("id:app-update-panel >> text:A newer version is available, but there is no compatible package for this installation."));
    check_sizes(app);
    // Up/down scroll the notes without moving the action focus; Right selects
    // the offered update, Skip, then Later. Select must dismiss this panel.
    let has_manual = app
        .screen()
        .matches("id:app-update-panel >> text:Download package")
        .unwrap()
        && app
            .screen()
            .matches("id:app-update-panel >> text:Download and update")
            .unwrap();
    for _ in 0..if has_manual { 4 } else { 3 } {
        step(app.press_key("right"));
    }
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:app-update-panel", Duration::from_secs(5)));
    assert!(
        app.screen().opened_urls().is_empty(),
        "remote dismissal does not activate a download"
    );
}

#[test]
fn closing_a_manual_check_before_android_metadata_does_not_discard_it_under_never() {
    fn here() {}
    let Some(home) = super::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[
            ("IPTV_TEST_DEVICE_SIGNALS", "1"),
            ("IPTV_TEST_PLATFORM", "android"),
        ],
    ) else {
        return;
    };
    let api = Api::start();
    api.serve(
        200,
        release_for_suffix("9.8.7", true, "android-tablet-tv.apk"),
    );
    let mut settings = Settings::new();
    settings
        .set("general.app_update_frequency", Value::choice("never"))
        .unwrap();
    let mut fixture = personalization::launch(home, settings, false, |_| {}, |_| {});
    let app = &mut fixture.app;
    super::device_signals::report(app, "tv", false, true);
    about(app);
    step(app.wait_for("id:app-update-panel >> text:Checking…"));
    step(app.press("id:app-update-panel >> text:Later"));
    assert_eq!(api.requests(), 0);
    android_distribution(
        app,
        serde_json::json!({"directApk":true,"versionCode":42,"androidApi":36}),
    );
    for _ in 0..6 {
        if !app.screen().matches("id:settings-screen").unwrap() {
            break;
        }
        step(app.back());
    }
    step(app.wait_for("id:app-update-notice"));
    assert_eq!(
        api.requests(),
        1,
        "the accepted manual request survives closing the waiting panel"
    );
    step(app.press_key("enter"));
    step(app.wait_for("id:app-update-panel >> text:Update available"));
    step(app.wait_for("id:app-update-notes >> text:/The complete ending of the notes/"));
}

#[test]
fn first_discovery_failure_keeps_the_public_releases_browser_route() {
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    api.serve(503, "{}".into());
    let mut settings = Settings::new();
    settings
        .set("general.app_update_frequency", Value::choice("never"))
        .unwrap();
    let mut fixture = personalization::launch(home, settings, false, |_| {}, |_| {});
    let app = &mut fixture.app;
    about(app);
    step(app.wait_for("id:app-update-panel >> text:Update check failed. Try again."));
    step(app.press("id:app-update-panel >> text:Release notes and downloads"));
    assert_eq!(
        app.screen().opened_urls(),
        ["https://github.com/teng-lin/patina-iptv/releases"]
    );
}

#[test]
fn update_notice_review_requires_the_settings_pin_and_cancellation_keeps_the_notice() {
    let Some(home) = isolated!() else { return };
    let api = Api::start();
    api.serve(200, release("9.8.7", true));
    let mut settings = Settings::new();
    settings
        .set(
            "parental.require_pin_for",
            Value::choice("blocked_content_and_settings"),
        )
        .unwrap();
    let mut fixture = personalization::launch_prepared(
        home,
        settings,
        false,
        |path, _| {
            crate::isolated::open_database(path)
                .unwrap()
                .set_parental_credential(&iptv_store_sqlite::parental::ParentalCredential {
                    salt: [0x63; 16],
                    verifier: [
                        108, 253, 222, 238, 40, 38, 77, 8, 100, 135, 138, 42, 133, 48, 171, 236,
                        123, 169, 134, 85, 199, 28, 212, 46, 152, 192, 13, 230, 58, 195, 156, 119,
                    ],
                    recovery_verifier: [0; 32],
                    failures: 0,
                    retry_after: 0,
                })
                .unwrap();
        },
        |_| {},
        |_| {},
    );
    let app = &mut fixture.app;
    step(app.wait_for("id:app-update-notice"));
    step(app.press_key("enter"));
    step(app.wait_for("id:parental-layer >> text:Enter parental PIN"));
    assert!(!app.screen().matches("id:app-update-panel").unwrap());
    step(app.back());
    step(app.wait_for("id:app-update-notice"));
    step(app.press_key("enter"));
    step(app.wait_for("id:parental-layer >> text:Enter parental PIN"));
    for digit in ["2", "4", "6", "8"] {
        step(app.press_key(digit));
    }
    step(app.press("id:parental-layer >> text:Continue"));
    step(app.wait_for("id:app-update-panel"));
    step(app.wait_for("id:app-update-notes >> text:/The complete ending of the notes/"));
    step(app.press("id:app-update-panel >> text:Later"));
    step(app.wait_for_gone("id:app-update-notice", Duration::from_secs(5)));
}
