//! Real Linux GTK/GL surfaces and a generated stream. Run in the explicit Xvfb
//! lane, separate from the display-independent shell scenarios.
#![cfg(all(target_os = "linux", debug_assertions))]
use gtk::{glib, prelude::*};
use nsh_core::prelude::*;
use nsh_platform_linux::{AppOptions, AppView, Root, gtk4 as gtk};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

static URL: OnceLock<String> = OnceLock::new();
static STEP: AtomicUsize = AtomicUsize::new(0);
static READY: AtomicUsize = AtomicUsize::new(0);
static POSITIONS: AtomicUsize = AtomicUsize::new(0);
static IDLE: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicUsize = AtomicUsize::new(0);

fn policy(public_only: bool) -> iptv_net::playback::PlaybackPolicy {
    iptv_net::playback::PlaybackPolicy {
        public_only,
        allow_private_fixture: true,
        ..Default::default()
    }
}
fn components(components: &nsh_platform_linux::Components) {
    iptv_player_mpv::register(components, policy);
}
#[component]
fn PlayerProbe() -> impl IntoElement {
    let step = signal(0usize);
    let _timer = set_interval(Duration::from_millis(10), move || {
        step.set(STEP.load(Ordering::SeqCst))
    });
    native(iptv_player_mpv::PLAYER_VIEW)
        .flex_1()
        .prop("popupSurface", || json!(7))
        .prop("source", move || {
            if step.get() >= 6 {
                Value::Null
            } else {
                json!({"url": URL.get().unwrap(), "token":1, "live":false,
                "headers":{}, "publicOnly":false, "reconnect":false})
            }
        })
        .prop("muted", || json!(true))
        .prop("pip", move || json!({"window":matches!(step.get(),1|2)}))
        .on_native_event(|event: &NativeEvent| {
            if event.name.as_ref() == "state" && event.data["state"] == "ready" {
                READY.fetch_add(1, Ordering::SeqCst);
            }
            if event.name.as_ref() == "size"
                && event.data["width"] == 160
                && event.data["height"] == 90
            {
                FRAMES.fetch_add(1, Ordering::SeqCst);
            }
            if event.name.as_ref() == "position"
                && event.data["ms"].as_u64().is_some_and(|ms| ms > 0)
            {
                POSITIONS.fetch_add(1, Ordering::SeqCst);
            }
            if event.name.as_ref() == "state" && event.data["state"] == "idle" {
                IDLE.store(true, Ordering::SeqCst);
            }
            assert_ne!(
                event.name.as_ref(),
                "error",
                "native generated playback reported an error"
            );
        })
}
#[component]
fn PipProbe() -> impl IntoElement {
    let step = signal(0usize);
    let _timer = set_interval(Duration::from_millis(10), move || {
        step.set(STEP.load(Ordering::SeqCst))
    });
    native(iptv_player_mpv::PIP_VIEW)
        .flex_1()
        .prop("surface", move || {
            if step.get() >= 4 {
                json!(7)
            } else {
                Value::Null
            }
        })
        .prop("compact", move || json!(step.get() == 4))
}
fn wait(reason: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let context = glib::MainContext::default();
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "missing native acknowledgement: {reason}"
        );
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn areas(widget: &gtk::Widget) -> Vec<gtk::GLArea> {
    let mut found = Vec::new();
    if let Ok(area) = widget.clone().downcast::<gtk::GLArea>() {
        found.push(area);
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        found.extend(areas(&widget));
        child = widget.next_sibling();
    }
    found
}
struct Server {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || {
            'connections: while let Ok((mut stream, _)) = listener.accept() {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut byte = [0];
                    if stream.read_exact(&mut byte).is_err() {
                        continue 'connections;
                    }
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                count.fetch_add(1, Ordering::SeqCst);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream
                    .write_all(header.as_bytes())
                    .and_then(|_| stream.write_all(&body));
            }
        });
        Self {
            address,
            requests,
            stopped,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Assertion unwinding closes native engines and GL contexts while GTK's
/// private display and the generated HTTP fixture still exist.
#[derive(Default)]
struct Cleanup(Vec<(gtk::Window, std::rc::Rc<AppView>)>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for (window, view) in self.0.iter().rev() {
            view.stop();
            window.destroy();
        }
        let context = glib::MainContext::default();
        for _ in 0..100 {
            if !context.pending() {
                break;
            }
            context.iteration(false);
        }
    }
}
fn window(root: Root, directory: &std::path::Path) -> (gtk::Window, std::rc::Rc<AppView>) {
    let view = AppView::new(AppOptions {
        title: "Generated native player".into(),
        data_dir: Some(directory.to_path_buf()),
        app_id: "com.hantekllc.patina.generated-native-test".into(),
        ..Default::default()
    });
    let window = gtk::Window::builder()
        .title("Generated native player")
        .default_width(320)
        .default_height(180)
        .build();
    window.set_child(Some(&view.widget()));
    view.load(nsh_engine::App::Rust(root));
    window.present();
    (window, view)
}
#[test]
#[ignore = "requires an isolated Xvfb display and software GL; coverage.sh runs it explicitly"]
fn native_gtk_player_shares_pip_frames_and_recreates_only_after_context_loss() {
    if !isolated_native("native_gtk_player_shares_pip_frames_and_recreates_only_after_context_loss")
    {
        return;
    }
    let (directory, server) = generated_environment();
    let mut cleanup = Cleanup::default();
    let (main, view) = window(Root::of::<PlayerProbe>(), directory.path());
    cleanup.0.push((main.clone(), view.clone()));
    wait("mounted primary GTK area", || {
        !areas(&view.widget()).is_empty()
    });
    wait("decoded frame and native play event", || {
        READY.load(Ordering::SeqCst) > 0
            && POSITIONS.load(Ordering::SeqCst) > 0
            && FRAMES.load(Ordering::SeqCst) > 0
    });
    let primary = areas(&view.widget()).pop().expect("persistent player area");
    assert!(primary.is_realized());
    let context = primary.context().expect("native GL context");
    let original_requests = server.requests.load(Ordering::SeqCst);
    assert!(original_requests > 0);
    let (popup, pip) = window(Root::of::<PipProbe>(), directory.path());
    cleanup.0.push((popup.clone(), pip.clone()));
    STEP.store(1, Ordering::SeqCst);
    wait("PiP shared GL area", || {
        areas(&pip.widget())
            .iter()
            .any(|a| a.is_realized() && a.context().is_some())
    });
    let shared = areas(&pip.widget()).pop().unwrap();
    assert_eq!(shared.context().unwrap(), context);
    assert_eq!(areas(&view.widget())[0], primary);
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        original_requests,
        "PiP must not open another stream"
    );
    STEP.store(3, Ordering::SeqCst);
    wait("PiP area retired", || areas(&pip.widget()).is_empty());
    assert!(primary.is_realized());
    STEP.store(4, Ordering::SeqCst);
    wait("explicit popup surface GL area", || {
        areas(&pip.widget()).iter().any(|a| a.is_realized())
    });
    let explicit = areas(&pip.widget()).pop().unwrap();
    assert_eq!(explicit.context().unwrap(), context);
    assert_eq!(server.requests.load(Ordering::SeqCst), original_requests);
    STEP.store(5, Ordering::SeqCst);
    pip.stop();
    popup.destroy();
    wait("popup teardown", || !explicit.is_realized());
    assert!(
        primary.is_realized(),
        "closing PiP must keep the main video surface realized"
    );
    let ready = READY.load(Ordering::SeqCst);
    let positions = POSITIONS.load(Ordering::SeqCst);
    main.set_child(None::<&gtk::Widget>);
    wait("native context loss", || !primary.is_realized());
    main.set_child(Some(&view.widget()));
    wait("context recreation and resumed decoding", || {
        primary.is_realized()
            && READY.load(Ordering::SeqCst) > ready
            && POSITIONS.load(Ordering::SeqCst) > positions
            && server.requests.load(Ordering::SeqCst) > original_requests
    });
    assert_eq!(areas(&view.widget())[0], primary);
    IDLE.store(false, Ordering::SeqCst);
    STEP.store(6, Ordering::SeqCst);
    wait("explicit source stop", || IDLE.load(Ordering::SeqCst));
    view.stop();
    main.destroy();
    wait("main surface teardown", || !primary.is_realized());
}

fn generated_environment() -> (tempfile::TempDir, Server) {
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();
    gtk::init().expect("mandatory native GTK initialization");
    nsh_platform_linux::set_components(components);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generated-native.ts");
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=160x90:r=10",
                "-t",
                "120",
                "-an",
                "-c:v",
                "mpeg2video",
                "-f",
                "mpegts"
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg")
            .success()
    );
    let server = Server::new(std::fs::read(path).unwrap());
    URL.set(format!("http://{}/generated.ts", server.address))
        .unwrap();
    (directory, server)
}

#[test]
#[cfg(feature = "test-support")]
#[ignore = "explicit sequential journey runner uses its own Xvfb display"]
fn native_selection_reaches_a_correlated_render_submission() {
    if !isolated_native("native_selection_reaches_a_correlated_render_submission") {
        return;
    }
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, Source, SourceConfig},
    };
    use iptv_store_sqlite::{DatabaseKey, SourcePolicy, Store};
    let home = tempfile::tempdir().unwrap();
    let sample = std::env::var_os("IPTV_PERF_SAMPLE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.path().join("selection.json"));
    if sample.exists() {
        std::fs::remove_file(&sample).unwrap();
    }
    // SAFETY: this exact native test runs in a fresh process; configure it before
    // GTK, the real application, or its workers can read the environment.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("IPTV_PERF_NATIVE_SAMPLE", &sample);
        std::env::set_var("IPTV_RS_TEST_LOCALE", "en-US");
    }
    let ready = home.path().join("native-ready");
    // SAFETY: GTK and application workers have not started in this isolated child.
    unsafe {
        std::env::set_var("IPTV_PERF_NATIVE_READY", &ready);
    }
    iptv_app::install_test_key([0x54; 32]);
    let (directory, server) = generated_environment();
    let data = home.path().join(".local/share/iptv-rs");
    std::fs::create_dir_all(&data).unwrap();
    let _recording_host = RecordingHost(data.clone());
    let playlist = format!(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Synthetic\",Generated channel\nhttp://{}/generated.ts\n",
        server.address
    );
    let file = home.path().join("generated.m3u");
    std::fs::write(&file, &playlist).unwrap();
    let mut store = Store::open_encrypted(
        data.join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x54; 32]),
    )
    .unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated native journey".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(file.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(&playlist);
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
    drop(store);
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("epg.update_on_start", iptv_settings::Value::Bool(false))
        .unwrap();
    settings
        .set("epg.update_interval", iptv_settings::Value::Duration(0))
        .unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let mut cleanup = Cleanup::default();
    let (main, view) = window(Root::of::<iptv_app::App>(), directory.path());
    main.set_default_size(1280, 720);
    wait("persistent native surface realized before Enter", || {
        areas(&view.widget())
            .iter()
            .any(|area| area.is_realized() && area.context().is_some())
    });
    std::fs::write(&ready, "ready").unwrap();
    cleanup.0.push((main, view));
    wait(
        "actual App Enter selection reaches its current-source render",
        || sample.exists(),
    );
    let evidence: Value = serde_json::from_slice(&std::fs::read(sample).unwrap()).unwrap();
    let events = evidence["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "one start and one renderer terminal");
    assert_eq!(
        events[0]["token"], events[1]["token"],
        "originating selection token"
    );
    assert_eq!(events[1]["kind"], "first-render");
    assert_eq!(
        evidence["counters"]["player_opens"], 1,
        "one actual App player open"
    );
    drop(cleanup);
}

// GTK/NSH state is process-local. Even `--ignored` without an exact filter must
// give each scenario a fresh process rather than initialize GTK twice.
fn isolated_native(name: &str) -> bool {
    const CHILD: &str = "IPTV_NATIVE_TEST_CHILD";
    if let Ok(child) = std::env::var(CHILD) {
        assert_eq!(child, name, "native scenario identity");
        std::fs::write(std::env::var_os("IPTV_NATIVE_TEST_MARKER").unwrap(), name).unwrap();
        return true;
    }
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("ran");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            name,
            "--ignored",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, name)
        .env("IPTV_NATIVE_TEST_MARKER", &marker)
        .status()
        .unwrap();
    assert!(status.success(), "native scenario failed: {name}");
    assert!(marker.exists(), "native scenario did not run: {name}");
    false
}

// Declared before the view cleanup so unwinding stops the UI before its native
// recording executor, and both stop before the synthetic HOME is removed.
struct RecordingHost(std::path::PathBuf);
impl Drop for RecordingHost {
    fn drop(&mut self) {
        let stopped = iptv_runtime::recordings::host::stop_background_host(&self.0);
        assert!(
            stopped || std::thread::panicking(),
            "synthetic native host did not stop"
        );
    }
}
