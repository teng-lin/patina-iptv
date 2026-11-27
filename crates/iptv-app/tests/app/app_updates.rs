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
struct Api {
    state: Arc<Mutex<Response>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Api {
    fn start() -> Self {
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
                let mut buffer = [0; 8192];
                let count = stream.read(&mut buffer).unwrap();
                let headers = String::from_utf8_lossy(&buffer[..count]).to_lowercase();
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
    fn serve(&self, status: u16, body: String) {
        let mut state = self.state.lock().unwrap();
        state.status = status;
        state.body = body;
        state.held = false;
    }
    fn requests(&self) -> usize {
        self.state.lock().unwrap().requests
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn release(version: &str, assets: bool) -> String {
    let suffix = if cfg!(target_os = "macos") {
        "macos-arm64.dmg"
    } else if cfg!(windows) {
        "windows-x86_64-setup.exe"
    } else {
        "linux-x86_64.AppImage"
    };
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
    step(app.press("id:app-update-panel >> text:Check for updates"));
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
    // Review is the initial remote focus; no touch or browser is required.
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
