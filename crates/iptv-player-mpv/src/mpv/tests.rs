//! `tv.player` against the real libmpv, without a display: the platform
//! side is [`test_host`]'s, mpv's render API draws into a framebuffer of a
//! surfaceless EGL context ([`Gpu`]: no window, no display connection),
//! and the streams are synthetic clips ffmpeg makes at test time, served
//! by a mock server on 127.0.0.1. No sound plays (`ao=null`).
//!
//! The players run one at a time ([`serial`]): what mpv's threads queue for
//! the main loop is one queue for the process.

use std::ffi::{CStr, c_char, c_void};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::test_host::{self as host, ComponentContext, Video};
use super::worker::Note;
use super::*;

static SERIAL: Mutex<()> = Mutex::new(());

/// How long a test waits for mpv at most: long, for a machine under heavy
/// load (nothing waits for it unless something is wrong).
const PATIENCE: Duration = Duration::from_secs(60);

/// One test's players at a time.
pub(super) fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// CI opts into mandatory native coverage; missing prerequisites cannot pass.
fn unavailable(reason: &str) {
    assert!(
        std::env::var("IPTV_REQUIRE_PLAYER_TESTS").as_deref() != Ok("1"),
        "Required player test unavailable: {reason}"
    );
    eprintln!("skipped: {reason}");
}

#[test]
fn required_player_tests_fail_without_their_prerequisites() {
    const CHILD: &str = "IPTV_PLAYER_PREREQUISITE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let dir = tempfile::tempdir().unwrap();
        clip(dir.path(), "black", 0.1);
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mpv::tests::required_player_tests_fail_without_their_prerequisites",
        ])
        .env(CHILD, "1")
        .env("IPTV_REQUIRE_PLAYER_TESTS", "1")
        .env("PATH", "/synthetic/no-programs")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("Required player test unavailable"));
}

// ---- A GL context without a display ----

const W: i32 = 64;
const H: i32 = 36;

type EglPtr = *mut c_void;

/// A surfaceless EGL context (Mesa's and NVIDIA's EGL have the platform),
/// current on the test's thread, and a `W`×`H` framebuffer bound in it.
struct Gpu {
    display: EglPtr,
    context: EglPtr,
    make_current: unsafe extern "C" fn(EglPtr, EglPtr, EglPtr, EglPtr) -> u32,
    destroy_context: unsafe extern "C" fn(EglPtr, EglPtr) -> u32,
    bind_framebuffer: unsafe extern "C" fn(u32, u32),
    read_pixels: unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void),
    fbo: u32,
}

impl Gpu {
    /// `None` (the caller skips) where EGL or its surfaceless platform is
    /// missing.
    fn new() -> Option<Gpu> {
        // SAFETY: create checks dynamically loaded EGL entry points and context creation.
        let gpu = unsafe { Gpu::create() };
        if gpu.is_none() {
            unavailable("no surfaceless EGL context on this machine");
        }
        gpu
    }

    unsafe fn create() -> Option<Gpu> {
        // SAFETY: EGL's and GL's documented C signatures, called in
        // the order the specifications ask for.
        unsafe {
            let lib = libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
            if lib.is_null() {
                return None;
            }
            let proc_ = libc::dlsym(lib, c"eglGetProcAddress".as_ptr());
            if proc_.is_null() {
                return None;
            }
            let get_proc = std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(*const c_char) -> *mut c_void,
            >(proc_);
            let f = |name: &CStr| Some(get_proc(name.as_ptr())).filter(|p| !p.is_null());
            macro_rules! load {
                ($name:literal, $ty:ty) => {
                    std::mem::transmute::<*mut c_void, $ty>(f($name)?)
                };
            }
            let get_display = load!(
                c"eglGetPlatformDisplayEXT",
                unsafe extern "C" fn(u32, EglPtr, *const i32) -> EglPtr
            );
            let initialize = load!(
                c"eglInitialize",
                unsafe extern "C" fn(EglPtr, *mut i32, *mut i32) -> u32
            );
            let bind_api = load!(c"eglBindAPI", unsafe extern "C" fn(u32) -> u32);
            let choose = load!(
                c"eglChooseConfig",
                unsafe extern "C" fn(EglPtr, *const i32, *mut EglPtr, i32, *mut i32) -> u32
            );
            let create_context = load!(
                c"eglCreateContext",
                unsafe extern "C" fn(EglPtr, EglPtr, EglPtr, *const i32) -> EglPtr
            );
            let make_current = load!(
                c"eglMakeCurrent",
                unsafe extern "C" fn(EglPtr, EglPtr, EglPtr, EglPtr) -> u32
            );
            let destroy_context = load!(
                c"eglDestroyContext",
                unsafe extern "C" fn(EglPtr, EglPtr) -> u32
            );
            const PLATFORM_SURFACELESS_MESA: u32 = 0x31DD;
            const OPENGL_API: u32 = 0x30A2;
            const NONE: i32 = 0x3038;
            let display = get_display(PLATFORM_SURFACELESS_MESA, std::ptr::null_mut(), &NONE);
            let (mut major, mut minor) = (0, 0);
            if display.is_null() || initialize(display, &mut major, &mut minor) == 0 {
                return None;
            }
            if bind_api(OPENGL_API) == 0 {
                return None;
            }
            // EGL_SURFACE_TYPE: PBUFFER, EGL_RENDERABLE_TYPE: OPENGL.
            let attrs = [0x3033, 0x0001, 0x3040, 0x0008, NONE];
            let (mut config, mut n) = (std::ptr::null_mut(), 0);
            if choose(display, attrs.as_ptr(), &mut config, 1, &mut n) == 0 || n < 1 {
                return None;
            }
            let context = create_context(display, config, std::ptr::null_mut(), &NONE);
            if context.is_null() {
                return None;
            }
            let none = std::ptr::null_mut();
            if make_current(display, none, none, context) == 0 {
                destroy_context(display, context);
                return None;
            }
            let gen_framebuffers = load!(c"glGenFramebuffers", unsafe extern "C" fn(i32, *mut u32));
            let bind_framebuffer = load!(c"glBindFramebuffer", unsafe extern "C" fn(u32, u32));
            let gen_renderbuffers =
                load!(c"glGenRenderbuffers", unsafe extern "C" fn(i32, *mut u32));
            let bind_renderbuffer = load!(c"glBindRenderbuffer", unsafe extern "C" fn(u32, u32));
            let storage = load!(
                c"glRenderbufferStorage",
                unsafe extern "C" fn(u32, u32, i32, i32)
            );
            let attach = load!(
                c"glFramebufferRenderbuffer",
                unsafe extern "C" fn(u32, u32, u32, u32)
            );
            let status = load!(
                c"glCheckFramebufferStatus",
                unsafe extern "C" fn(u32) -> u32
            );
            let viewport = load!(c"glViewport", unsafe extern "C" fn(i32, i32, i32, i32));
            let read_pixels = load!(
                c"glReadPixels",
                unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void)
            );
            const FRAMEBUFFER: u32 = 0x8D40;
            const RENDERBUFFER: u32 = 0x8D41;
            let (mut fbo, mut rb) = (0, 0);
            gen_framebuffers(1, &mut fbo);
            gen_renderbuffers(1, &mut rb);
            bind_renderbuffer(RENDERBUFFER, rb);
            storage(RENDERBUFFER, 0x8058 /* RGBA8 */, W, H);
            bind_framebuffer(FRAMEBUFFER, fbo);
            attach(
                FRAMEBUFFER,
                0x8CE0, /* COLOR_ATTACHMENT0 */
                RENDERBUFFER,
                rb,
            );
            viewport(0, 0, W, H);
            let gpu = Gpu {
                display,
                context,
                make_current,
                destroy_context,
                bind_framebuffer,
                read_pixels,
                fbo,
            };
            (status(FRAMEBUFFER) == 0x8CD5/* COMPLETE */).then_some(gpu)
        }
    }

    /// Binds the framebuffer for drawing, as GTK binds the area's before
    /// it asks for a frame.
    fn bind(&self) {
        // SAFETY: the context is current.
        unsafe { (self.bind_framebuffer)(0x8D40, self.fbo) };
    }

    /// The framebuffer's middle pixel, RGB.
    fn pixel(&self) -> [u8; 3] {
        let mut px = [0_u8; 4];
        // SAFETY: the context is current; room for one RGBA pixel.
        unsafe {
            (self.bind_framebuffer)(0x8D40, self.fbo);
            (self.read_pixels)(W / 2, H / 2, 1, 1, 0x1908, 0x1401, px.as_mut_ptr().cast());
        }
        [px[0], px[1], px[2]]
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        let none = std::ptr::null_mut();
        // SAFETY: ours; released before it is destroyed.
        unsafe {
            (self.make_current)(self.display, none, none, none);
            (self.destroy_context)(self.display, self.context);
        }
    }
}

fn black(px: [u8; 3]) -> bool {
    px.iter().all(|c| *c < 24)
}

fn red(px: [u8; 3]) -> bool {
    px[0] > 180 && px[1] < 70 && px[2] < 70
}

fn blue(px: [u8; 3]) -> bool {
    px[2] > 180 && px[0] < 70 && px[1] < 70
}

// ---- Synthetic streams ----

/// An MPEG-TS clip of `secs` s: `color` at 320×180, 25 fps, an English
/// stereo and a French mono tone. `None` (the caller skips) without ffmpeg.
fn clip(dir: &Path, color: &str, secs: f32) -> Option<Vec<u8>> {
    let out = dir.join(format!("{color}-{secs}.ts"));
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s=320x180:r=25"),
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-f", "lavfi", "-i", "sine=frequency=660:sample_rate=48000"])
        .args(["-map", "0:v", "-map", "1:a", "-map", "2:a"])
        .args(["-ac:a:0", "2", "-ac:a:1", "1"])
        .args([
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:1",
            "language=fra",
        ])
        .args(["-c:v", "mpeg2video", "-q:v", "2", "-c:a", "mp2"])
        .args(["-t", &secs.to_string(), "-f", "mpegts"])
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() => Some(std::fs::read(&out).expect("ffmpeg produced its output")),
        _ => {
            unavailable("ffmpeg could not make a synthetic clip");
            None
        }
    }
}

#[derive(Clone)]
enum Answer {
    Body(Vec<u8>),
    Status(u16),
    Redirect(String),
    /// The first body until the flag is set, then the second.
    Switch(Arc<AtomicBool>, Vec<u8>, Vec<u8>),
}

/// A mock server on 127.0.0.1: each path answers a body or a status; the
/// request heads it received are kept.
struct Server {
    port: u16,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(routes: Vec<(&'static str, Answer)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let routes = Arc::new(routes);
        let kept = heads.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (routes, heads) = (routes.clone(), kept.clone());
                std::thread::spawn(move || answer(conn, &routes, &heads));
            }
        });
        Server { port, heads }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// The request heads for `path`.
    fn heads(&self, path: &str) -> Vec<String> {
        let line = format!("GET {path} ");
        let heads = self.heads.lock().unwrap();
        heads
            .iter()
            .filter(|h| h.starts_with(&line))
            .cloned()
            .collect()
    }
}

fn answer(mut conn: TcpStream, routes: &[(&'static str, Answer)], heads: &Mutex<Vec<String>>) {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let path = head.split(' ').nth(1).unwrap_or("").to_owned();
    heads.lock().unwrap().push(head);
    let found = routes
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, a)| a.clone());
    let found = match found {
        Some(Answer::Switch(switched, before, after)) => {
            Some(Answer::Body(if switched.load(Ordering::Acquire) {
                after
            } else {
                before
            }))
        }
        other => other,
    };
    let _ = match found.unwrap_or(Answer::Status(404)) {
        Answer::Switch(..) => unreachable!("answered above"),
        Answer::Body(body) => conn
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .and_then(|()| conn.write_all(&body)),
        Answer::Redirect(url) => conn.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()),
        Answer::Status(code) => conn.write_all(
            format!("HTTP/1.1 {code} Synthetic\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        ),
    };
}

// ---- The player ----

/// A player as GTK's component holds it, and what it emitted.
struct Player {
    inner: Rc<Inner>,
    events: ComponentContext,
    video: Video,
    drawn: u32,
}

impl Player {
    fn new() -> Player {
        let events = ComponentContext::default();
        let video = Video::default();
        let view = video.clone();
        let inner = Inner::new(events.clone(), move |_| view);
        // No sound in tests: mpv opens its audio output with each file.
        inner.mpv.as_ref().expect("libmpv").set("ao", "null");
        // Exercise playback and frame retention with the basic renderer.
        // Ubuntu's older mpv renders black through its optional scaling
        // shaders on software Mesa; this tiny framebuffer needs none of them.
        inner.mpv.as_ref().unwrap().set("gpu-dumb-mode", "yes");
        Player {
            inner,
            events,
            video,
            drawn: 0,
        }
    }

    fn set(&self, name: &str, value: Value) {
        self.inner.set_prop(name, Some(&value));
    }

    /// The events named `name` so far.
    fn events(&self, name: &str) -> Vec<Value> {
        let events = self.events.0.borrow();
        events
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, d)| d.clone())
            .collect()
    }

    fn states(&self) -> Vec<String> {
        self.events("state")
            .iter()
            .map(|s| s["state"].as_str().unwrap().to_owned())
            .collect()
    }

    fn clear(&self) {
        self.events.0.borrow_mut().clear();
    }

    /// Runs the main loop's share once: mpv's notes, and a draw where GTK
    /// would draw.
    fn turn(&mut self, gpu: Option<&Gpu>) {
        host::pump();
        if self.video.0.get() != self.drawn {
            self.drawn = self.video.0.get();
            if let Some(gpu) = gpu {
                gpu.bind();
                self.inner.draw(W, H);
            }
        }
    }

    /// Turns the main loop until `done`, at most [`PATIENCE`].
    fn wait(&mut self, gpu: Option<&Gpu>, what: &str, done: impl Fn(&Player) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < PATIENCE,
                "timed out waiting for {what}; events: {:?}",
                self.events.0.borrow()
            );
            self.turn(gpu);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Turns the main loop for `ms`.
    fn idle(&mut self, gpu: Option<&Gpu>, ms: u64) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(ms) {
            self.turn(gpu);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_state(&mut self, gpu: Option<&Gpu>, state: &str) {
        self.wait(gpu, state, |p| {
            p.states().last().map(String::as_str) == Some(state)
        });
    }

    /// mpv's property `name` as a string.
    fn prop(&self, name: &CStr) -> Option<String> {
        self.inner.mpv.as_ref().unwrap().get(name)
    }

    /// Waits until the control thread set mpv's `name` to `value`.
    fn wait_prop(&mut self, name: &CStr, value: &str) {
        let what = format!("{name:?} = {value}");
        self.wait(None, &what, |p| p.prop(name).as_deref() == Some(value));
    }

    /// Draws now, as GTK would, and reads the picture.
    fn draw(&self, gpu: &Gpu) -> [u8; 3] {
        gpu.bind();
        self.inner.draw(W, H);
        gpu.pixel()
    }

    /// Draws as GTK would until the picture is `want`.
    fn wait_picture(&mut self, gpu: &Gpu, what: &str, want: fn([u8; 3]) -> bool) {
        let start = Instant::now();
        while !want(self.draw(gpu)) {
            assert!(
                start.elapsed() < PATIENCE,
                "timed out waiting for a {what} picture (got {:?})",
                gpu.pixel()
            );
            self.turn(Some(gpu));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Makes the player see `entry` as the current file of `url`, as if mpv
    /// had loaded and started it.
    fn current(&self, url: &str, entry: i64) {
        let mut st = self.inner.state.borrow_mut();
        st.url = Some(url.to_owned());
        st.expected = Some(entry);
        st.entry = Some(entry);
    }

    fn note(&self, note: Note) -> Out {
        let mut out = Out::new();
        self.inner.note(note, &mut out);
        out
    }
}

/// The states were `buffering`, then `ready` (a rebuffer may come
/// between), ending in `last`.
fn assert_played_to(p: &Player, last: &str) {
    let states = p.states();
    assert_eq!(
        states.first().map(String::as_str),
        Some("buffering"),
        "{states:?}"
    );
    assert!(states.iter().any(|s| s == "ready"), "{states:?}");
    assert_eq!(states.last().map(String::as_str), Some(last), "{states:?}");
    let ends = states
        .iter()
        .filter(|s| *s == "ended" || *s == "idle")
        .count();
    assert_eq!(ends, usize::from(last == "ended"), "{states:?}");
}

/// Everything a test needs for real playback, or `None` to skip.
fn rig() -> Option<(tempfile::TempDir, Gpu)> {
    let dir = tempfile::tempdir().unwrap();
    // ffmpeg first: it is the likelier one missing.
    clip(dir.path(), "black", 0.2)?;
    Some((dir, Gpu::new()?))
}

// ---- Playback ----

/// A source waits for the render context, then plays: `buffering`, then
/// `ready` with its size, tracks and format; black until it plays, then its
/// picture; `position` once a second while it plays (not paused); a retry
/// of the same stream keeps the last frame, a new channel goes black; the
/// end is `ended`, no source `idle`. The stream's requests carry the
/// source's headers, and its user agent or else the player's own.
#[test]
fn a_source_plays_through_the_render_api() {
    let _serial = serial();
    let Some((dir, gpu)) = rig() else { return };
    let (Some(red_ts), Some(blue_ts)) =
        (clip(dir.path(), "red", 6.0), clip(dir.path(), "blue", 3.0))
    else {
        return;
    };
    let server = Server::start(vec![
        ("/red.ts", Answer::Body(red_ts)),
        ("/blue.ts", Answer::Body(blue_ts)),
    ]);
    let mut p = Player::new();
    let headers = json!({ "User-Agent": "SyntheticAgent/1.0", "X-Synthetic": "one" });
    p.set(
        "source",
        json!({ "url": server.url("/red.ts"), "headers": headers }),
    );
    p.idle(None, 200);
    assert!(p.states().is_empty(), "no load before the render context");
    assert!(p.inner.state.borrow().deferred);
    assert!(server.heads("/red.ts").is_empty());

    assert!(
        p.inner.create_render(),
        "a render context in the EGL context"
    );
    assert_eq!(p.states(), ["buffering"]);
    assert!(black(p.draw(&gpu)), "black until the stream plays");
    p.wait_state(Some(&gpu), "ready");
    p.wait_picture(&gpu, "red", red);

    p.wait(Some(&gpu), "size", |p| !p.events("size").is_empty());
    assert_eq!(p.events("size"), [json!({ "width": 320, "height": 180 })]);
    p.wait(Some(&gpu), "format", |p| {
        let f = p.events("format");
        f.iter().any(|f| f["fps"] == json!(25.0)) && f.iter().any(|f| f["channels"] == json!(2))
    });
    p.wait(Some(&gpu), "tracks", |p| !p.events("tracks").is_empty());
    let tracks = p.events("tracks").pop().unwrap();
    assert_eq!(tracks["video"].as_array().unwrap().len(), 1);
    let languages: Vec<&str> = tracks["audio"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["language"].as_str().unwrap())
        .collect();
    assert_eq!(languages, ["eng", "fra"]);
    assert_eq!(tracks["selected"]["video"], "auto");
    assert_eq!(tracks["selected"]["text"], "off");
    assert_eq!(p.inner.state.borrow().audio_out.as_deref(), Some("null"));
    let head = server.heads("/red.ts").remove(0);
    assert!(
        head.to_ascii_lowercase()
            .contains("user-agent: syntheticagent/1.0"),
        "{head}"
    );
    assert!(
        head.to_ascii_lowercase().contains("x-synthetic: one"),
        "{head}"
    );

    // Once a second while playing: where it is.
    host::tick(&p.inner);
    p.wait(Some(&gpu), "position", |p| !p.events("position").is_empty());
    assert!(p.events("position")[0]["ms"].as_u64().unwrap() < 6000);
    p.set("paused", json!(true));
    p.wait_prop(c"pause", "yes");
    p.clear();
    host::tick(&p.inner);
    p.idle(Some(&gpu), 100);
    assert!(p.events("position").is_empty(), "no position while paused");
    p.set("paused", json!(false));
    p.wait_prop(c"pause", "no");

    // A retry of the same stream keeps its last frame until it plays.
    p.set(
        "source",
        json!({ "url": server.url("/red.ts"), "token": 1 }),
    );
    assert_eq!(p.inner.state.borrow().last_state, PlayerState::Buffering);
    assert!(!p.inner.state.borrow().shutter, "the last frame stays");
    assert!(red(p.draw(&gpu)), "the last frame stays");
    p.wait_state(Some(&gpu), "ready");
    // This source names no agent: the player's own, not the last source's.
    let head = server.heads("/red.ts").pop().unwrap();
    let own = format!("user-agent: {}", DEFAULT_UA.to_ascii_lowercase());
    assert!(head.to_ascii_lowercase().contains(&own), "{head}");
    assert!(
        !head.to_ascii_lowercase().contains("x-synthetic"),
        "headers are the source's: {head}"
    );

    // The render context goes with the view, and the source opens again
    // with the next one.
    p.inner.free_render();
    assert!(p.inner.render.get().is_null());
    assert!(p.inner.state.borrow().deferred, "waits for the next one");
    p.clear();
    assert!(p.inner.create_render());
    assert_eq!(p.states(), ["buffering"]);
    p.wait_state(Some(&gpu), "ready");

    // Another channel: black until it plays; then its end.
    p.clear();
    p.set("source", json!({ "url": server.url("/blue.ts") }));
    assert!(black(p.draw(&gpu)), "a new channel starts black");
    p.wait_state(Some(&gpu), "ready");
    p.wait_picture(&gpu, "blue", blue);
    p.wait_state(Some(&gpu), "ended");
    assert_played_to(&p, "ended");

    p.set("source", Value::Null);
    assert_eq!(p.states().last().unwrap(), "idle");
    p.wait_prop(c"idle-active", "yes");
}

/// A failed stream: `error` with Media3's code (and the server's status),
/// then `idle`, black.
#[test]
fn failures_report_the_cause_then_idle() {
    let _serial = serial();
    let Some((_dir, gpu)) = rig() else { return };
    let server = Server::start(vec![
        ("/gone.ts", Answer::Status(404)),
        (
            "/text.ts",
            Answer::Body(b"synthetic text, not a stream\n".repeat(64)),
        ),
    ]);
    let mut p = Player::new();
    assert!(p.inner.create_render());

    p.set("source", json!({ "url": server.url("/gone.ts") }));
    p.wait_state(Some(&gpu), "idle");
    assert_eq!(p.states(), ["buffering", "idle"]);
    assert_eq!(
        p.events("error"),
        [json!({ "code": map::HTTP_STATUS, "http": 404 })]
    );
    assert!(black(p.draw(&gpu)));

    p.clear();
    p.set("source", json!({ "url": server.url("/text.ts") }));
    p.wait_state(Some(&gpu), "idle");
    assert_eq!(
        p.events("error"),
        [json!({ "code": map::CONTAINER_UNSUPPORTED })]
    );

    // Nothing listens there any more.
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    p.clear();
    p.set(
        "source",
        json!({ "url": format!("http://127.0.0.1:{closed}/synthetic.ts"), "live": true }),
    );
    p.wait_state(Some(&gpu), "idle");
    assert_eq!(
        p.events("error"),
        [json!({ "code": map::CONNECTION_FAILED })]
    );
}

/// The control thread ending (here a panic in it) is noticed: what plays
/// fails as a player error the app retries, and the next open starts a new
/// thread on the same handle, which plays the stream again.
#[test]
fn a_control_thread_that_ends_fails_the_stream_until_the_next_open() {
    let _serial = serial();
    let Some((dir, gpu)) = rig() else { return };
    let Some(red_ts) = clip(dir.path(), "red", 6.0) else {
        return;
    };
    let server = Server::start(vec![("/red.ts", Answer::Body(red_ts))]);
    let mut p = Player::new();
    assert!(p.inner.create_render());
    p.set(
        "source",
        json!({ "url": server.url("/red.ts"), "token": 1 }),
    );
    p.wait_state(Some(&gpu), "ready");

    p.clear();
    p.inner.send(worker::Cmd::Panic);
    p.wait(Some(&gpu), "the player error", |p| {
        !p.events("error").is_empty()
    });
    assert_eq!(p.events("error"), [json!({ "code": map::UNSPECIFIED })]);
    assert_eq!(p.states().last().map(String::as_str), Some("idle"));
    assert!(p.inner.worker.borrow().is_none());

    // The app's retry: the same stream with a new token.
    p.clear();
    p.set(
        "source",
        json!({ "url": server.url("/red.ts"), "token": 2 }),
    );
    assert!(p.inner.worker.borrow().is_some(), "a new control thread");
    p.wait_state(Some(&gpu), "ready");
    p.wait_picture(&gpu, "red", red);
}

/// A live stream the server closes after it played a while plays on with
/// a spare connection of the same address: no end reaches the app.
#[test]
fn a_live_stream_plays_on_across_a_closed_connection() {
    let _serial = serial();
    let Some((dir, gpu)) = rig() else { return };
    let Some(live) = clip(dir.path(), "red", 6.0) else {
        return;
    };
    let server = Server::start(vec![("/live.ts", Answer::Body(live))]);
    let mut p = Player::new();
    assert!(p.inner.create_render());
    p.set(
        "source",
        json!({ "url": server.url("/live.ts"), "live": true }),
    );
    p.wait_state(Some(&gpu), "ready");
    // Where it plays, as the app asks once a second, until the position
    // goes back: the first connection played to its end (6 s of stream)
    // and the spare plays from its start.
    let start = Instant::now();
    loop {
        assert!(
            start.elapsed() < PATIENCE,
            "no bridge: {:?}",
            p.events("position")
        );
        host::tick(&p.inner);
        p.idle(Some(&gpu), 200);
        let ms: Vec<u64> = p
            .events("position")
            .iter()
            .map(|e| e["ms"].as_u64().unwrap())
            .collect();
        let highest = ms.iter().copied().max().unwrap_or(0);
        if highest >= 5000 && ms.last().is_some_and(|last| last + 2000 < highest) {
            break;
        }
    }
    assert!(server.heads("/live.ts").len() >= 2, "a spare connection");
    assert_played_to(&p, "ready");
    assert!(p.events("error").is_empty());
    assert!(red(p.draw(&gpu)));
}

/// A live stream the server drops keeps its last picture, and so does the
/// app's retry of it, until the new stream's first frame: never black in
/// between, in the view or at the picture-in-picture window's size.
#[test]
fn a_dropped_live_stream_keeps_its_picture_until_the_retry_plays() {
    let _serial = serial();
    let Some((dir, gpu)) = rig() else { return };
    let (Some(red_ts), Some(blue_ts)) =
        (clip(dir.path(), "red", 1.5), clip(dir.path(), "blue", 3.0))
    else {
        return;
    };
    let switched = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![(
        "/live.ts",
        Answer::Switch(switched.clone(), red_ts, blue_ts),
    )]);
    let mut p = Player::new();
    assert!(p.inner.create_render());
    let source =
        |token: i64| json!({ "url": server.url("/live.ts"), "live": true, "token": token });
    // Establish a rendered picture before this short clip can reach EOF:
    // software renderer startup can take longer than the clip on CI. This
    // test needs a real end for the app to retry, regardless of paused wall
    // time (which would otherwise count toward automatic live bridging).
    p.inner.send(Cmd::DisableBridge);
    p.set("paused", json!(true));
    p.wait_prop(c"pause", "yes");
    p.set("source", source(0));
    p.wait_state(Some(&gpu), "ready");
    p.wait_picture(&gpu, "red", red);
    // The view and the picture-in-picture window draw it at two sizes.
    p.inner.draw(W / 2, H / 2);
    assert!(red(p.draw(&gpu)));

    // The server drops it (a live end the app retries).
    p.set("paused", json!(false));
    p.wait_prop(c"pause", "no");
    p.wait_state(Some(&gpu), "ended");
    p.idle(Some(&gpu), 300);
    assert!(red(p.draw(&gpu)), "the picture stays after the drop");
    p.inner.draw(W / 2, H / 2);
    assert!(red(p.draw(&gpu)), "at either size");

    // The retry: the same stream, now blue; red until its first frame.
    // Keep that first frame available until the test thread has sampled it.
    p.set("paused", json!(true));
    p.wait_prop(c"pause", "yes");
    switched.store(true, Ordering::Release);
    p.set("source", source(1));
    let start = Instant::now();
    loop {
        assert!(start.elapsed() < PATIENCE, "the retry never played");
        p.turn(Some(&gpu));
        let px = p.draw(&gpu);
        if blue(px) {
            break;
        }
        assert!(
            red(px),
            "the last picture until the retry plays, not {px:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    p.wait_state(Some(&gpu), "ready");
    p.set("paused", json!(false));
    p.wait_prop(c"pause", "no");

    // Another channel still starts black.
    p.set(
        "source",
        json!({ "url": server.url("/other.ts"), "live": true }),
    );
    assert!(black(p.draw(&gpu)), "a new channel starts black");
}

/// A live stream that ends soon after it started is an end the app sees
/// (its retries back off); the spare is dropped instead of played.
#[test]
fn a_short_live_play_ends() {
    let _serial = serial();
    let Some((dir, gpu)) = rig() else { return };
    let Some(live) = clip(dir.path(), "red", 1.0) else {
        return;
    };
    let server = Server::start(vec![("/live.ts", Answer::Body(live))]);
    let mut p = Player::new();
    assert!(p.inner.create_render());
    p.set(
        "source",
        json!({ "url": server.url("/live.ts"), "live": true }),
    );
    p.wait_state(Some(&gpu), "ended");
    assert_played_to(&p, "ended");
    // The spare is gone (the ended entry stays listed).
    p.wait_prop(c"playlist-count", "1");
    p.idle(Some(&gpu), 300);
    assert_eq!(p.states().last().unwrap(), "ended", "mpv did not go on");
}

// ---- Props ----

/// The props become mpv's properties.
#[test]
fn props_set_mpvs_properties() {
    let _serial = serial();
    let mut p = Player::new();
    let f = |p: &Player, name: &CStr| -> f64 { p.prop(name).unwrap().parse().unwrap() };

    for (level, want) in [(json!(150), 100.0), (json!(-5), 0.0), (json!(42.4), 42.0)] {
        p.set("volume", level);
        p.wait(None, "volume", |p| f(p, c"volume") == want);
    }
    p.inner.set_prop("volume", None);
    p.wait(None, "volume", |p| f(p, c"volume") == 100.0);
    p.set("muted", json!(true));
    p.wait_prop(c"mute", "yes");
    p.set("muted", json!(false));
    p.wait_prop(c"mute", "no");
    p.set("audioDelay", json!(250));
    p.wait(None, "audio-delay", |p| f(p, c"audio-delay") == 0.25);
    p.set("audioDelay", json!(-99_999));
    p.wait(None, "audio-delay", |p| f(p, c"audio-delay") == -10.0);
    p.set("paused", json!(true));
    p.wait_prop(c"pause", "yes");

    p.set("videoTrack", json!("2"));
    p.wait_prop(c"vid", "2");
    assert!(!p.inner.state.borrow().video_auto);
    p.set("videoTrack", json!("auto"));
    p.wait_prop(c"vid", "auto");
    assert!(p.inner.state.borrow().video_auto);
    p.set("audioTrack", json!("3"));
    p.wait_prop(c"aid", "3");
    p.set("textTrack", json!("4"));
    p.wait_prop(c"sid", "4");
    p.set("textTrack", json!("off"));
    p.wait_prop(c"sid", "no");

    p.set("pictureSize", json!("zoom"));
    p.wait(None, "panscan", |p| f(p, c"panscan") == 1.0);
    p.set("pictureSize", json!("full"));
    p.wait_prop(c"keepaspect", "no");
    p.set("pictureSize", json!("4:3"));
    p.wait(None, "aspect", |p| {
        (f(p, c"video-aspect-override") - 4.0 / 3.0).abs() < 1e-3
    });
    p.inner.set_prop("pictureSize", None);
    p.wait_prop(c"keepaspect", "yes");
    // What mpv makes of `no` (its number differs between versions).
    let no = {
        let m = super::handle::Mpv::new().unwrap();
        m.set("video-aspect-override", "no");
        m.get(c"video-aspect-override")
    };
    p.wait(None, "no aspect", |p| {
        p.prop(c"video-aspect-override") == no
    });

    // The video's shape only matters to `cinema`.
    p.note(Note::Aspect(2.0));
    assert_eq!(p.inner.state.borrow().aspect, Some(2.0));
    assert_eq!(f(&p, c"video-zoom"), 0.0);
    p.set("pictureSize", json!("cinema"));
    let zoom = |a: f64| (2.35 / a).log2();
    p.wait(None, "zoom", |p| {
        (f(p, c"video-zoom") - zoom(2.0)).abs() < 1e-4
    });
    p.note(Note::Aspect(4.0 / 3.0));
    p.wait(None, "zoom", |p| {
        (f(p, c"video-zoom") - zoom(4.0 / 3.0)).abs() < 1e-4
    });
    p.note(Note::Aspect(4.0 / 3.0)); // unchanged

    // The user agent is the source's.
    p.set(
        "source",
        json!({
            "url": "file:///synthetic/clip.ts",
            "headers": { "User-Agent": "SourceAgent/1.0", "X-Synthetic": "two", "Bad": 3 },
        }),
    );
    p.wait_prop(c"user-agent", "SourceAgent/1.0");
    p.wait_prop(c"http-header-fields", "");
    // mpv may not open files: refused before it is asked.
    assert_eq!(
        p.events("error"),
        [json!({ "code": map::UNSUPPORTED_SCHEME })]
    );
    assert!(p.states().is_empty(), "it was idle");
    // A source that names none, or a blank one, goes out as the player's own.
    p.set(
        "source",
        json!({
            "url": "file:///synthetic/clip.ts",
            "token": 1,
            "headers": { "User-Agent": "  " },
        }),
    );
    p.wait_prop(c"user-agent", DEFAULT_UA);

    // Names outside the contract are ignored, the `userAgent` prop the
    // players once had among them. The control thread runs its commands in
    // order: once the pause shows, anything sent before it has run too.
    p.set("userAgent", json!("SyntheticAgent/2.0"));
    p.set("someOtherProp", json!(1));
    p.set("paused", json!(false));
    p.wait_prop(c"pause", "no");
    assert_eq!(p.prop(c"user-agent").as_deref(), Some(DEFAULT_UA));
}

/// Every prop of the contract's shared fixture is one the player takes:
/// each sets what it names, none is skipped as unknown.
#[test]
fn the_contracts_fixture_props_are_all_taken() {
    let _serial = serial();
    let fixture: Value =
        serde_json::from_str(include_str!("../../fixtures/player-props.json")).unwrap();
    let mut p = Player::new();
    for (name, value) in fixture.as_object().unwrap() {
        assert!(PlayerProp::from_wire(name).is_some(), "unknown prop {name}");
        p.set(name, value.clone());
    }
    {
        let st = p.inner.state.borrow();
        assert_eq!(
            st.url.as_deref(),
            Some("http://127.0.0.1:18080/synthetic/index.m3u8")
        );
        assert_eq!((st.token, st.live, st.public_only), (37, true, false));
        assert_eq!(
            st.headers,
            [("X-Synthetic".to_owned(), "fixture".to_owned())]
        );
        assert!(st.paused);
        assert_eq!(st.picture, "zoom");
        assert!(!st.video_auto, "a video track was chosen");
    }
    let f = |p: &Player, name: &CStr| -> f64 { p.prop(name).unwrap().parse().unwrap() };
    p.wait_prop(c"pause", "yes");
    p.wait_prop(c"mute", "yes");
    p.wait_prop(c"vid", "1");
    p.wait_prop(c"aid", "2");
    p.wait_prop(c"sid", "3");
    p.wait(None, "volume", |p| f(p, c"volume") == 80.0);
    p.wait(None, "audio-delay", |p| f(p, c"audio-delay") == 0.125);
    assert_eq!(pip::wanted(), Some(p.inner.id), "the window shows it");
}

/// The window's picture-in-picture view shows the player whose `pip` says
/// so, until it says otherwise or goes.
#[test]
fn the_pip_window_follows_its_player() {
    let _serial = serial();
    host::pip_syncs();
    let p1 = Player::new();
    let p2 = Player::new();
    let (id1, id2) = (p1.inner.id, p2.inner.id);
    p1.set("pip", json!({ "window": true }));
    assert_eq!(host::pip_syncs(), [Some(id1)]);
    p1.set("pip", json!({ "window": true }));
    assert!(host::pip_syncs().is_empty(), "unchanged");
    p2.set("pip", json!({ "window": false }));
    assert!(host::pip_syncs().is_empty(), "another player's off");
    p2.set("pip", json!({ "window": true }));
    p1.set("pip", Value::Null);
    assert_eq!(host::pip_syncs(), [Some(id2)], "p1 was no longer shown");
    drop(p1);
    assert!(host::pip_syncs().is_empty());
    assert!(find(id1).is_none());
    drop(p2);
    assert_eq!(host::pip_syncs(), [None], "the shown player went");
    assert!(find(id2).is_none());
}

// ---- What mpv reports, one note at a time ----

const URL: &str = "http://stream.example.test/one.ts";

/// The current file's notes become events; another file's are ignored.
#[test]
fn notes_of_the_current_file_become_events() {
    let _serial = serial();
    let p = Player::new();

    // Not current yet: ignored.
    assert!(p.note(Note::Tracks("[]".into())).is_empty());
    assert!(p.note(Note::Restarted { audio_out: None }).is_empty());
    assert!(p.note(Note::Fps(25.0)).is_empty());
    assert!(p.note(Note::Channels(2)).is_empty());
    assert!(p.note(Note::Position(3.0)).is_empty());

    p.current(URL, 7);
    let out = p.note(Note::Restarted {
        audio_out: Some("null".into()),
    });
    // Format seen before it started is sent now.
    assert_eq!(
        out,
        [
            ("state", json!({ "state": "ready" })),
            ("format", json!({ "fps": 25.0, "channels": 2 })),
        ]
    );
    assert!(!p.inner.state.borrow().shutter);
    assert!(p.note(Note::Fps(25.0)).is_empty(), "unchanged");
    assert!(p.note(Note::Fps(f64::NAN)).is_empty());
    assert!(p.note(Note::Fps(0.0)).is_empty());
    assert_eq!(
        p.note(Note::Fps(50.0)),
        [("format", json!({ "fps": 50.0 }))]
    );
    assert!(p.note(Note::Channels(0)).is_empty());
    assert_eq!(
        p.note(Note::Channels(6)),
        [("format", json!({ "channels": 6 }))]
    );

    assert_eq!(
        p.note(Note::Buffering(true)),
        [("state", json!({ "state": "buffering" }))]
    );
    assert!(
        p.note(Note::Position(3.0)).is_empty(),
        "no position while buffering"
    );
    assert_eq!(
        p.note(Note::Buffering(false)),
        [("state", json!({ "state": "ready" }))]
    );
    assert_eq!(
        p.note(Note::Position(-1.0)),
        [("position", json!({ "ms": 0 }))]
    );
    assert_eq!(
        p.note(Note::Position(2.0004)),
        [("position", json!({ "ms": 2000 }))]
    );

    assert!(p.note(Note::Width(0)).is_empty());
    assert!(p.note(Note::Width(640)).is_empty(), "no height yet");
    assert_eq!(
        p.note(Note::Height(360)),
        [("size", json!({ "width": 640, "height": 360 }))]
    );
    assert!(p.note(Note::Height(360)).is_empty(), "unchanged");
    let wide = (1_i64 << 32) + 1280;
    assert_eq!(
        p.note(Note::Width(wide)),
        [("size", json!({ "width": wide, "height": 360 }))]
    );

    let tracks = r#"[{"id":1,"type":"video","selected":true,"demux-w":640,"demux-h":360}]"#;
    let out = p.note(Note::Tracks(tracks.into()));
    // The typed event's own data: whole numbers, not mpv's floats, and
    // the same tracks as the shell read from mpv's.
    let read = crate::parse_tracks(&out[0].1);
    assert_eq!(out, [crate::PlayerEvent::Tracks(read.clone()).wire()]);
    assert_eq!(out[0].0, "tracks");
    assert_eq!(out[0].1["video"][0]["width"], 640);
    let auto = p.inner.state.borrow().video_auto;
    let mpvs = map::tracks(tracks, auto).unwrap();
    assert_eq!(read, crate::parse_tracks(&mpvs));
    assert_eq!(mpvs["video"][0]["width"], 640.0);
    assert!(p.note(Note::Tracks("not json".into())).is_empty());

    // Output formats are logged; warnings are read for clues.
    assert!(
        p.note(Note::Log {
            warn: false,
            text: "AO: [null] 48000Hz stereo 2ch\n".into()
        })
        .is_empty()
    );
    assert!(
        p.note(Note::Log {
            warn: true,
            text: "VO: synthetic\n".into()
        })
        .is_empty()
    );
    assert_eq!(p.inner.state.borrow().clues, map::Clues::default());

    // Another file's start and end: ignored.
    assert!(p.note(Note::Started(9)).is_empty());
    assert!(
        p.note(Note::Ended {
            entry: 8,
            reason: ffi::MPV_END_FILE_REASON_EOF,
            error: 0
        })
        .is_empty()
    );
    p.note(Note::Started(7));
    // A stop (mpv's reason 2) is not an end the app sees.
    assert!(
        p.note(Note::Ended {
            entry: 7,
            reason: 2,
            error: 0
        })
        .is_empty()
    );
    assert!(!p.inner.playing());
}

/// Loads answer in order; only the last one's entry counts, and a refused
/// load fails.
#[test]
fn only_the_newest_load_counts() {
    let _serial = serial();
    let p = Player::new();
    {
        let mut st = p.inner.state.borrow_mut();
        st.url = Some(URL.into());
        st.loads = 2;
    }
    assert!(p.note(Note::Loading(Some(3))).is_empty());
    assert_eq!(
        p.inner.state.borrow().expected,
        None,
        "a newer load is on its way"
    );
    assert!(p.note(Note::Loading(Some(4))).is_empty());
    assert_eq!(p.inner.state.borrow().expected, Some(4));

    // A live stream bridged to its spare: the spare is the current file.
    p.note(Note::Started(4));
    p.note(Note::Bridged(5));
    assert_eq!(p.inner.state.borrow().expected, Some(5));
    p.note(Note::Bridged(6)); // not playing the expected entry: ignored
    assert_eq!(p.inner.state.borrow().expected, Some(5));

    // mpv refused the load.
    p.inner.state.borrow_mut().loads = 1;
    assert_eq!(
        p.note(Note::Loading(None)),
        [("error", json!({ "code": map::IO_UNSPECIFIED }))],
        "already idle"
    );
    // Nothing to load: no error.
    p.inner.state.borrow_mut().url = None;
    p.inner.state.borrow_mut().loads = 1;
    assert!(p.note(Note::Loading(None)).is_empty());
}

/// A failed file is reported once mpv's log was read: with the server's
/// status a warning gave. A live stream keeps its last frame.
#[test]
fn an_error_end_reports_what_the_log_said() {
    let _serial = serial();
    let p = Player::new();
    p.current(URL, 7);
    {
        let mut st = p.inner.state.borrow_mut();
        st.live = true;
        st.shutter = false;
        st.playing = true;
        st.last_state = PlayerState::Ready;
    }
    p.note(Note::Log {
        warn: true,
        text: "HTTP error 403 Forbidden\n".into(),
    });
    let out = p.note(Note::Ended {
        entry: 7,
        reason: ffi::MPV_END_FILE_REASON_ERROR,
        error: -13,
    });
    assert!(out.is_empty(), "reported after the queue is read");
    p.inner.drain();
    assert_eq!(
        p.events("error"),
        [json!({ "code": map::HTTP_STATUS, "http": 403 })]
    );
    assert_eq!(p.states(), ["idle"]);
    assert!(
        !p.inner.state.borrow().shutter,
        "a live stream keeps its frame"
    );
    // Reported once.
    p.inner.drain();
    assert_eq!(p.events("error").len(), 1);
}

/// Without a current GL context mpv makes no render context: the player
/// draws nothing and its source keeps waiting.
#[test]
fn no_render_context_without_gl() {
    let _serial = serial();
    let p = Player::new();
    p.set("source", json!({ "url": URL }));
    assert!(!p.inner.create_render());
    assert!(p.inner.render.get().is_null());
    p.inner.draw(W, H);
    assert!(p.inner.state.borrow().deferred, "still waiting");
    assert!(p.states().is_empty());
}

/// In the background the stream stops; in the foreground it opens again
/// (keeping its last frame). A source set meanwhile waits for it.
#[test]
fn suspend_stops_the_stream_until_resume() {
    let _serial = serial();
    let mut p = Player::new();
    p.inner.suspend(); // nothing playing
    assert!(!p.inner.state.borrow().suspended);
    p.current(URL, 7);
    p.inner.state.borrow_mut().shutter = false;
    p.inner.suspend();
    assert!(p.inner.state.borrow().suspended);
    assert_eq!(p.inner.state.borrow().expected, None);
    p.inner.suspend(); // already
    p.set(
        "source",
        json!({ "url": "http://stream.example.test/two.ts" }),
    );
    assert!(
        !p.inner.state.borrow().deferred,
        "not opened in the background"
    );
    p.inner.resume();
    let st = p.inner.state.borrow();
    assert!(!st.suspended);
    assert!(st.deferred, "opens once there is a render context");
    assert!(!st.shutter, "the last frame stays");
    drop(st);
    p.inner.resume(); // not suspended: nothing
    p.idle(None, 50);
    assert!(p.states().is_empty());
}

/// Real libmpv follows a redirected HLS manifest and plays its remote-origin
/// segment, while only the first origin sees generated identity headers.
#[test]
fn adaptive_playback_never_sends_identity_to_another_origin() {
    let _serial = serial();
    let Some((dir, gpu)) = rig() else { return };
    let Some(red_ts) = clip(dir.path(), "red", 4.0) else {
        return;
    };
    let remote = Server::start(vec![("/red.ts", Answer::Body(red_ts))]);
    let manifest = format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4,\n{}\n#EXT-X-ENDLIST\n",
        remote.url("/red.ts")
    );
    let origin = Server::start(vec![
        ("/start.m3u8", Answer::Redirect("/main.m3u8".into())),
        ("/main.m3u8", Answer::Body(manifest.into_bytes())),
    ]);
    let mut player = Player::new();
    assert!(player.inner.create_render());
    player.set("source", json!({"url": origin.url("/start.m3u8"), "headers": {
        "Authorization": "synthetic-identity", "Cookie": "synthetic-identity", "SN": "synthetic-identity", "X-User-Agent": "synthetic-identity"
    }}));
    player.wait_state(Some(&gpu), "ready");
    player.wait_picture(&gpu, "red", red);
    assert!(!remote.heads("/red.ts").is_empty());
    for path in ["/start.m3u8", "/main.m3u8"] {
        let heads = origin.heads(path);
        assert!(!heads.is_empty());
        for head in heads {
            let head = head.to_ascii_lowercase();
            for name in ["authorization", "cookie", "sn", "x-user-agent"] {
                assert!(head.contains(&format!("{name}: synthetic-identity")));
            }
        }
    }
    for head in remote.heads("/red.ts") {
        assert!(!head.contains("synthetic-identity"));
    }
}
