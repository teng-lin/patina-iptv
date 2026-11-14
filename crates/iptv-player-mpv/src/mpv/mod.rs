//! `tv.player` on the desktops: one persistent libmpv player per component
//! drawing through mpv's render API (OpenGL) into the native view NSH hosts
//! in the video layer's view (`video.rs`): a `GtkGLArea` on Linux
//! ([`gtk`]), an `NSView` backed by a `CAOpenGLLayer` on macOS ([`mac`]),
//! a `UIView` whose layer is a `CAEAGLLayer` on iOS and iPadOS ([`ios`],
//! OpenGL ES 3, MPVKit's libmpv). Windows uses the software render API
//! and composes its frames with Direct2D ([`win`]).
//! In-process; the app's UI draws above it as above any view. Layout
//! changes resize the view; the mpv instance lives as long as the
//! component. Nothing is process-wide but the registry mpv's threads find
//! a player in by id, so several players (multiview: up to four, all but one `muted`) work side by side.
//!
//! Same contract as the Android host's `PlayerComponent.kt` (summed up in
//! `video.rs`):
//! - props `source` `{url, headers, token, live, reconnect}` or null (a new
//!   url or token reopens; `live` streams are bridged across connections
//!   the server closes, see [`worker`]), `paused`, `pictureSize` (see
//!   [`map::picture`]);
//!   `videoTrack` / `audioTrack` (`auto` or a track id: mpv's `vid` /
//!   `aid`), `textTrack` (`off` or an id: `sid`), each starting again from
//!   the default with a new source;
//!   `volume` (0–100), `muted`, `audioDelay` (ms: mpv's `audio-delay`);
//! - prop `pip` `{window}`: the desktops' picture-in-picture window shows
//!   this player too ([`pip`]: its picture, no second stream);
//! - events `state` {state: idle|buffering|ready|ended}, `size` {width,
//!   height}, `error` {code, http?} with Media3's code names
//!   ([`map::Clues::error`]), never addresses or messages, `position`
//!   {ms} once a second while playing (from where the source opened),
//!   `tracks` (from mpv's `track-list`, [`map::tracks`]) and `format`
//!   {fps?, channels?}.
//!
//! The props' names come from the contract's one declaration
//! ([`crate::PlayerProp`], [`crate::SourceProps`], [`crate::PipProps`]) and
//! the events are its [`crate::PlayerEvent`]. A stream whose source names
//! no `User-Agent` goes out as [`DEFAULT_UA`].
//!
//! The picture is black from a new source until the stream plays; a
//! reopen of the same stream (`reconnect`, or a new token for the same
//! url: [`map::keeps_frame`]) keeps the last frame, and so does a live
//! stream's error (the app reconnects it; other errors go black): the
//! player holds it itself until the new first frame ([`held`]), as mpv's
//! render API draws black meanwhile. Each
//! load sets its cache options first ([`map::load_options`]). mpv
//! loads through the origin-scoped playback transport ([`map::http`]).
//! `IPTV_RS_AUDIO` names mpv's audio output (`null` mutes, for tests);
//! `IPTV_RS_HWDEC` overrides `hwdec`; `IPTV_RS_LIVE_BRIDGE=0` turns
//! bridging off.
//!
//! The platform's main thread renders; every other mpv call runs on a
//! control thread ([`worker`]), as mpv's render API requires. The platform
//! modules only make the view and its GL context, draw when asked and run
//! closures on the main thread ([`platform::on_main`]); this module is the
//! player.

mod ffi;
#[cfg(not(windows))]
mod gl;
mod handle;
#[cfg(not(windows))]
mod held;
mod map;
mod pip;
#[cfg(windows)]
mod win;
mod worker;
#[cfg(windows)]
use win as platform;

#[cfg(all(target_os = "linux", not(test)))]
mod gtk;
#[cfg(all(target_os = "linux", not(test)))]
mod gtk_pip;
#[cfg(all(target_os = "linux", not(test)))]
mod x11;
#[cfg(all(target_os = "linux", not(test)))]
use gtk as platform;
// The unit tests run the player without a display (`test_host`).
#[cfg(all(target_os = "linux", test))]
mod test_host;
#[cfg(all(target_os = "linux", test))]
use test_host as platform;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod mac_pip;
#[cfg(all(target_os = "linux", test))]
mod tests;
#[cfg(target_os = "macos")]
use mac as platform;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
use ios as platform;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::TryRecvError;

use iptv_core::playback::PlayerState;
use serde_json::Value;

use crate::{PlayerEvent, PlayerProp, SourceProps};
use handle::Mpv;
/// Supplies transport settings for each new load, including current trust roots.
pub type Policy = fn(bool) -> iptv_net::playback::PlaybackPolicy;

thread_local! {
    static POLICY: Cell<Policy> = const { Cell::new(default_policy) };
}

fn default_policy(public_only: bool) -> iptv_net::playback::PlaybackPolicy {
    iptv_net::playback::PlaybackPolicy {
        public_only,
        #[cfg(debug_assertions)]
        allow_private_fixture: cfg!(test),
        ..Default::default()
    }
}

#[cfg(target_os = "ios")]
use nsh_platform_ios::Components;
#[cfg(target_os = "linux")]
use nsh_platform_linux::Components;
#[cfg(target_os = "macos")]
use nsh_platform_macos::Components;
#[cfg(windows)]
use nsh_platform_windows::Components;

/// Registers persistent player and PiP surfaces with the shell's transport policy.
pub fn register(components: &Components, policy: Policy) {
    POLICY.set(policy);
    platform::register(components);
}
use platform::{ComponentContext, Video};
use worker::{Cmd, Note, Worker};

#[cfg_attr(test, allow(dead_code))] // GTK's (`test_host` registers nothing)
pub const NAME: &str = crate::PLAYER_VIEW;
/// The `User-Agent` of a stream whose source names none.
pub(crate) const DEFAULT_UA: &str = "iptv-rs/0.1 (mpv)";

thread_local! {
    /// The players by id, for mpv's threads (which reach the main loop with
    /// only an id).
    static PLAYERS: RefCell<HashMap<u64, Weak<Inner>>> = RefCell::default();
}

fn find(id: u64) -> Option<Rc<Inner>> {
    PLAYERS.with(|p| p.borrow().get(&id).and_then(Weak::upgrade))
}

/// What mpv's callbacks hold: the player's id and whether a main-loop
/// call is already queued.
struct Waker {
    id: u64,
    events: AtomicBool,
    frame: AtomicBool,
}

/// Runs `f` on the main loop once per burst of `flag` wake-ups (never
/// synchronously: mpv may call back from inside its API on this thread).
fn wake(flag: &AtomicBool, id: u64, f: fn(&Inner)) {
    if !flag.swap(true, Ordering::AcqRel) {
        platform::on_main(move || {
            if let Some(p) = find(id) {
                f(&p);
            }
        });
    }
}

unsafe extern "C" fn on_frame(data: *mut c_void) {
    // SAFETY: `data` is the player's `Waker`, alive until the render context
    // is freed.
    let w = unsafe { &*(data as *const Waker) };
    wake(&w.frame, w.id, |p| {
        p.waker.frame.store(false, Ordering::Release);
        p.video.redraw();
    });
}

/// What the player has been told and has seen.
struct State {
    url: Option<String>,
    token: i64,
    /// The source is a live channel (bridged, see [`worker`]).
    live: bool,
    headers: Vec<(String, String)>,
    public_only: bool,
    picture: String,
    aspect: Option<f64>,
    width: i64,
    height: i64,
    /// The size last reported.
    reported: (i64, i64),
    /// Black over the picture until the stream plays.
    shutter: bool,
    /// The playlist entry of the source last loaded (`None` while a load is
    /// on its way): other files' events (one the viewer left, or a load mpv
    /// dropped for a newer one) are ignored.
    expected: Option<i64>,
    /// Loads queued and not yet answered ([`Note::Loading`]).
    loads: u32,
    /// The playlist entry mpv started last.
    entry: Option<i64>,
    /// The current file started playing (later buffering is a rebuffer).
    playing: bool,
    buffering: bool,
    last_state: PlayerState,
    clues: map::Clues,
    /// A source that waits for the GL context.
    deferred: bool,
    audio_out: Option<String>,
    /// The `paused` prop.
    paused: bool,
    /// mpv's error for the current file, reported after the log is read.
    failed: Option<i32>,
    /// The viewer left the video track to mpv (`videoTrack` `auto`).
    video_auto: bool,
    /// The current file's frame rate and audio channels (0: unknown).
    fps: f64,
    channels: i64,
    /// The frame rate and channels last sent in `format`.
    format_sent: (f64, i64),
    /// The app is in the background (iOS): the stream stopped and opens
    /// again in the foreground ([`Inner::suspend`]).
    suspended: bool,
}

/// One player: its mpv instance, control thread, render context and the
/// platform's view it draws in.
struct Inner {
    id: u64,
    video: Video,
    ctx: ComponentContext,
    mpv: Option<Arc<Mpv>>,
    /// The control thread; `None` after it ended ([`Inner::drain`]) until
    /// the next open starts another.
    worker: RefCell<Option<Worker>>,
    /// A control thread that ended, kept until its replacement starts.
    dead: RefCell<Option<Worker>>,
    render: Cell<*mut ffi::mpv_render_context>,
    #[cfg(not(windows))]
    gl: Cell<Option<gl::Gl>>,
    /// The last picture, shown while mpv has none of the stream.
    #[cfg(not(windows))]
    held: RefCell<held::Held>,
    state: RefCell<State>,
    waker: Arc<Waker>,
}

/// Events to send once the state is no longer borrowed (an app handler
/// may set props right away).
type Out = Vec<(&'static str, Value)>;

impl State {
    /// `format` with what changed since it was last sent.
    fn format(&mut self, out: &mut Out) {
        let fps = (self.fps > 0.0 && self.fps != self.format_sent.0).then_some(self.fps);
        let channels = (self.channels > 0 && self.channels != self.format_sent.1)
            .then(|| u32::try_from(self.channels).unwrap_or(0));
        self.format_sent = (self.fps, self.channels);
        if fps.is_some() || channels.is_some() {
            out.push(PlayerEvent::Format { fps, channels }.wire());
        }
    }

    fn state(&mut self, out: &mut Out, state: PlayerState) {
        if self.last_state != state {
            self.last_state = state;
            out.push(PlayerEvent::State(state).wire());
        }
    }
}

impl Inner {
    /// A player with a new mpv instance and control thread, drawing in the
    /// view `video` makes for its id. It reports `position` from then on.
    fn new(ctx: ComponentContext, video: impl FnOnce(u64) -> Video) -> Rc<Inner> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let video = video(id);
        let mpv = Mpv::new().map(Arc::new);
        if mpv.is_none() {
            log::error!("iptv mpv: no player (is libmpv installed?)");
        }
        let waker = Arc::new(Waker {
            id,
            events: AtomicBool::new(false),
            frame: AtomicBool::new(false),
        });
        let worker = mpv
            .as_ref()
            .map(|mpv| Worker::spawn(mpv.clone(), Self::notify(&waker)));
        let inner = Rc::new(Inner {
            id,
            video,
            ctx,
            mpv,
            worker: RefCell::new(worker),
            dead: RefCell::default(),
            render: Cell::new(std::ptr::null_mut()),
            #[cfg(not(windows))]
            gl: Cell::new(None),
            #[cfg(not(windows))]
            held: RefCell::default(),
            state: RefCell::new(State {
                url: None,
                token: 0,
                live: false,
                headers: Vec::new(),
                public_only: true,
                picture: "normal".to_owned(),
                aspect: None,
                width: 0,
                height: 0,
                reported: (0, 0),
                shutter: true,
                expected: None,
                loads: 0,
                entry: None,
                playing: false,
                buffering: false,
                last_state: PlayerState::Idle,
                clues: map::Clues::default(),
                deferred: false,
                audio_out: None,
                paused: false,
                failed: None,
                video_auto: true,
                fps: 0.0,
                channels: 0,
                format_sent: (0.0, 0),
                suspended: false,
            }),
            waker,
        });
        PLAYERS.with(|p| p.borrow_mut().insert(id, Rc::downgrade(&inner)));
        // `position` once a second while playing (catch-up seeking).
        platform::every_second(id, Inner::tick);
        inner
    }

    /// The app went to the background, where iOS suspends it: the stream
    /// stops (no connection held for nothing); [`Inner::resume`] opens it
    /// again. Android's host does the same (`PlayerComponent.background`);
    /// the app counts no stall meanwhile (`player.rs`).
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    fn suspend(&self) {
        {
            let mut st = self.state.borrow_mut();
            if st.url.is_none() || st.suspended {
                return;
            }
            st.suspended = true;
            st.expected = None;
        }
        self.send(Cmd::Stop);
    }

    /// Back in the foreground: the stream opens again, its last frame
    /// kept until it plays.
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    fn resume(&self) {
        let was = std::mem::take(&mut self.state.borrow_mut().suspended);
        if was {
            self.open(true);
        }
    }

    /// Prop `name` of the contract changed; any other name is ignored.
    fn set_prop(&self, name: &str, value: Option<&Value>) {
        let Some(prop) = PlayerProp::from_wire(name) else {
            return;
        };
        match prop {
            PlayerProp::Source => self.set_source(value),
            PlayerProp::Paused => {
                let paused = value.and_then(Value::as_bool).unwrap_or(false);
                self.state.borrow_mut().paused = paused;
                self.send(Cmd::Set("pause", if paused { "yes" } else { "no" }.into()));
            }
            PlayerProp::PictureSize => {
                let mode = value.and_then(Value::as_str).unwrap_or("normal");
                self.state.borrow_mut().picture = mode.to_owned();
                self.apply_picture();
            }
            PlayerProp::VideoTrack => {
                let vid = map::track_choice(value.and_then(Value::as_str));
                self.state.borrow_mut().video_auto = vid == "auto";
                self.send(Cmd::Set("vid", vid));
            }
            PlayerProp::AudioTrack => {
                let aid = map::track_choice(value.and_then(Value::as_str));
                self.send(Cmd::Set("aid", aid));
            }
            PlayerProp::TextTrack => {
                let sid = map::text_choice(value.and_then(Value::as_str));
                self.send(Cmd::Set("sid", sid));
            }
            PlayerProp::Volume => {
                let level = value.and_then(Value::as_f64).unwrap_or(100.0);
                self.send(Cmd::Set(
                    "volume",
                    format!("{:.0}", level.clamp(0.0, 100.0)),
                ));
            }
            PlayerProp::Muted => {
                let muted = value.and_then(Value::as_bool).unwrap_or(false);
                log::info!("iptv mpv {}: muted {muted}", self.id);
                self.send(Cmd::Set("mute", if muted { "yes" } else { "no" }.into()));
            }
            PlayerProp::AudioDelay => {
                let ms = value.and_then(Value::as_f64).unwrap_or(0.0);
                let secs = ms.clamp(-10_000.0, 10_000.0) / 1000.0;
                self.send(Cmd::Set("audio-delay", format!("{secs:.3}")));
            }
            // The desktops' picture-in-picture window shows this player.
            PlayerProp::Pip => pip::set_prop(self.id, value),
        }
    }

    /// What the control thread runs after it sent notes, and when it ends:
    /// [`Inner::drain`] on the main loop.
    fn notify(waker: &Arc<Waker>) -> impl Fn() + Send + 'static {
        let waker = waker.clone();
        move || wake(&waker.events, waker.id, Inner::drain)
    }

    /// Queues `cmd` for the control thread. Without one (it ended), the
    /// command is dropped: the next open starts a thread and sends what a
    /// new stream needs.
    fn send(&self, cmd: Cmd) {
        if let Some(w) = self.worker.borrow().as_ref() {
            w.send(cmd);
        }
    }

    /// Starts a control thread for the handle when the last one ended.
    fn revive(&self) {
        let replacement = {
            let worker = self.worker.borrow();
            match (worker.as_ref(), &self.mpv) {
                (Some(_), _) | (None, None) => return,
                (None, Some(mpv)) => self.dead.take().map_or_else(
                    || Worker::spawn(mpv.clone(), Self::notify(&self.waker)),
                    |dead| dead.replacement(Self::notify(&self.waker)),
                ),
            }
        };
        log::info!("iptv mpv {}: started a new control thread", self.id);
        *self.worker.borrow_mut() = Some(replacement);
        // The new thread has the handle's properties, not the old one's
        // HTTP options (they belong to its relay).
        self.apply_http();
    }

    fn emit(&self, out: Out) {
        for (name, data) in out {
            self.ctx.emit(name, data);
        }
    }

    fn set_source(&self, value: Option<&Value>) {
        let source = value.filter(|v| v.is_object());
        let url = source
            .and_then(|s| s[SourceProps::URL].as_str())
            .map(str::to_owned)
            .filter(|u| !u.is_empty());
        let public_only = source
            .and_then(|s| s[SourceProps::PUBLIC_ONLY].as_bool())
            .unwrap_or(true);
        let token = source
            .and_then(|s| s[SourceProps::TOKEN].as_i64())
            .unwrap_or(0);
        let live = source
            .and_then(|s| s[SourceProps::LIVE].as_bool())
            .unwrap_or(false);
        let reconnect = source
            .and_then(|s| s[SourceProps::RECONNECT].as_bool())
            .unwrap_or(false);
        let headers = source
            .and_then(|s| s[SourceProps::HEADERS].as_object())
            .map(|h| {
                h.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let (changed, keep) = {
            let mut st = self.state.borrow_mut();
            let changed = url != st.url || token != st.token || public_only != st.public_only;
            let keep = map::keeps_frame(st.url.as_deref(), url.as_deref(), reconnect);
            st.url = url;
            st.token = token;
            st.live = live;
            st.headers = headers;
            st.public_only = public_only;
            (changed, keep)
        };
        self.apply_http();
        if changed && !self.state.borrow().suspended {
            self.open(keep);
        }
    }

    /// Opens the current source (or stops), behind the black shutter
    /// unless `keep_frame` (the same stream again: its last frame stays
    /// until the new one plays).
    fn open(&self, keep_frame: bool) {
        self.revive();
        let mut out = Out::new();
        let url = {
            let mut st = self.state.borrow_mut();
            if !keep_frame {
                st.shutter = true;
            }
            st.playing = false;
            st.buffering = false;
            st.clues = map::Clues::default();
            st.failed = None;
            st.expected = None;
            st.deferred = false;
            // The load starts from mpv's default tracks.
            st.video_auto = true;
            st.fps = 0.0;
            st.channels = 0;
            st.format_sent = (0.0, 0);
            st.url.clone()
        };
        self.video.redraw();
        match (url, &self.mpv) {
            (None, Some(_)) => {
                self.send(Cmd::Stop);
                self.state.borrow_mut().state(&mut out, PlayerState::Idle);
            }
            (None, None) => {}
            (Some(_), None) => fail(&self.state, &mut out, map::DECODER_INIT_FAILED, None),
            (Some(url), Some(_)) if !map::playable(&url) => {
                self.send(Cmd::Stop);
                fail(&self.state, &mut out, map::UNSUPPORTED_SCHEME, None);
            }
            // mpv's video output needs the render context: load once there.
            (Some(_), Some(_)) if self.render.get().is_null() => {
                self.state.borrow_mut().deferred = true;
            }
            (Some(url), Some(_)) => {
                let mut st = self.state.borrow_mut();
                st.loads += 1;
                st.state(&mut out, PlayerState::Buffering);
                let live = st.live;
                let public_only = st.public_only;
                drop(st);
                let options = map::load_options(live, keep_frame);
                self.send(Cmd::Load {
                    url,
                    live,
                    options,
                    public_only,
                });
            }
        }
        self.emit(out);
    }

    fn apply_http(&self) {
        let (source, http) = {
            let st = self.state.borrow();
            (st.url.clone(), map::http(&st.headers))
        };
        self.send(Cmd::Http {
            source,
            user_agent: http.user_agent,
            fields: http.fields,
        });
    }

    fn apply_picture(&self) {
        let p = {
            let st = self.state.borrow();
            map::picture(&st.picture, st.aspect)
        };
        let keep = if p.keepaspect { "yes" } else { "no" };
        self.send(Cmd::Set("keepaspect", keep.into()));
        self.send(Cmd::Set("panscan", format!("{:.4}", p.panscan)));
        self.send(Cmd::Set("video-aspect-override", p.aspect.into()));
        self.send(Cmd::Set("video-zoom", format!("{:.6}", p.zoom)));
    }

    /// Makes mpv's render context in the platform's GL context, which is
    /// current; then opens a source that waited for it. Whether it is there.
    #[cfg(not(windows))]
    fn create_render(&self) -> bool {
        let Some(mpv) = &self.mpv else { return false };
        let Some(gl) = gl::Gl::load() else {
            log::error!("iptv mpv: GL functions not found");
            return false;
        };
        self.gl.set(Some(gl));
        let mut init = ffi::mpv_opengl_init_params {
            get_proc_address: Some(gl::get_proc_address),
            get_proc_address_ctx: std::ptr::null_mut(),
        };
        let mut params = [
            param(
                ffi::MPV_RENDER_PARAM_API_TYPE,
                c"opengl".as_ptr().cast_mut(),
            ),
            param(ffi::MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &mut init),
            param(
                ffi::MPV_RENDER_PARAM_INVALID,
                std::ptr::null_mut::<c_void>(),
            ),
        ];
        let mut render = std::ptr::null_mut();
        // SAFETY: the platform's context is current; params end with INVALID.
        let r = unsafe { ffi::mpv_render_context_create(&mut render, mpv.0, params.as_mut_ptr()) };
        if r < 0 || render.is_null() {
            log::error!("iptv mpv: no render context ({r})");
            return false;
        }
        let data = Arc::as_ptr(&self.waker) as *mut c_void;
        // SAFETY: the waker outlives the render context.
        unsafe { ffi::mpv_render_context_set_update_callback(render, Some(on_frame), data) };
        self.render.set(render);
        if self.state.borrow().deferred {
            self.open(false);
        }
        true
    }

    /// Frees the render context (in the platform's GL context); a source
    /// that plays waits for the next one.
    fn free_render(&self) {
        let render = self.render.replace(std::ptr::null_mut());
        if render.is_null() {
            return;
        }
        self.video.make_current();
        // SAFETY: ours, freed once, with its GL context current.
        unsafe {
            ffi::mpv_render_context_set_update_callback(render, None, std::ptr::null_mut());
            ffi::mpv_render_context_free(render);
        }
        #[cfg(not(windows))]
        {
            self.held.borrow_mut().clear();
            self.gl.set(None);
        }
        let mut st = self.state.borrow_mut();
        st.deferred = st.url.is_some();
    }

    /// Draws into the bound framebuffer, `width`×`height` pixels: the
    /// stream's picture; black behind the shutter; while mpv has no
    /// picture of the stream (a live stream dropped, a reopen of the same
    /// stream waiting for its first frame, the end), the last one
    /// ([`held`]). The platform's GL context is current.
    #[cfg(not(windows))]
    fn draw(&self, width: i32, height: i32) {
        let Some(gl) = self.gl.get() else { return };
        let render = self.render.get();
        let (shutter, playing) = {
            let st = self.state.borrow();
            (st.shutter, st.playing)
        };
        if render.is_null() || shutter {
            gl.black();
            return;
        }
        let view = gl.framebuffer();
        let mut held = self.held.borrow_mut();
        if !playing {
            if !held.show(view, width, height) {
                gl.black();
            }
            return;
        }
        let target = held.target(view, width, height);
        Self::render_into(render, target.unwrap_or(view), width, height);
        if target.is_some() {
            held.show(view, width, height);
        }
    }

    /// mpv draws its current frame into framebuffer `fbo`.
    #[cfg(not(windows))]
    fn render_into(render: *mut ffi::mpv_render_context, fbo: i32, width: i32, height: i32) {
        let mut fbo = ffi::mpv_opengl_fbo {
            fbo,
            w: width,
            h: height,
            internal_format: 0,
        };
        let (mut flip, mut block): (c_int, c_int) = (1, 0);
        let mut params = [
            param(ffi::MPV_RENDER_PARAM_OPENGL_FBO, &mut fbo),
            param(ffi::MPV_RENDER_PARAM_FLIP_Y, &mut flip),
            param(ffi::MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, &mut block),
            param(
                ffi::MPV_RENDER_PARAM_INVALID,
                std::ptr::null_mut::<c_void>(),
            ),
        ];
        // SAFETY: the platform made its context current to draw.
        unsafe { ffi::mpv_render_context_render(render, params.as_mut_ptr()) };
    }

    /// What the control thread reported (on the main loop).
    fn drain(&self) {
        self.waker.events.store(false, Ordering::Release);
        let mut notes = Vec::new();
        let ended = {
            let worker = self.worker.borrow();
            let Some(worker) = worker.as_ref() else {
                return;
            };
            loop {
                match worker.notes.try_recv() {
                    Ok(note) => notes.push(note),
                    Err(TryRecvError::Empty) => break false,
                    Err(TryRecvError::Disconnected) => break true,
                }
            }
        };
        let mut out = Out::new();
        // Handling a note may send commands: the worker is not borrowed.
        for note in notes {
            self.note(note, &mut out);
        }
        if ended {
            self.control_thread_ended(&mut out);
        }
        let failed = self.state.borrow_mut().failed.take();
        if let Some(error) = failed {
            let (code, http) = self.state.borrow().clues.error(error);
            fail(&self.state, &mut out, code, http);
            self.video.redraw();
        }
        self.emit(out);
    }

    /// The control thread is gone (a panic in it, or mpv shut down):
    /// nothing reaches mpv any more. What plays fails as a player error,
    /// which the app retries like any other; that open starts a new thread
    /// ([`Inner::revive`]).
    fn control_thread_ended(&self, out: &mut Out) {
        log::error!("iptv mpv {}: the control thread stopped", self.id);
        // Its replacement inherits the handle's hook state from it.
        if let Some(dead) = self.worker.borrow_mut().take() {
            *self.dead.borrow_mut() = Some(dead);
        }
        let playing = {
            let mut st = self.state.borrow_mut();
            st.expected = None;
            st.loads = 0;
            st.url.is_some()
        };
        if playing {
            fail(&self.state, out, map::UNSPECIFIED, None);
            self.video.redraw();
        }
    }

    fn note(&self, note: Note, out: &mut Out) {
        match note {
            Note::TransportError(code) => self.state.borrow_mut().clues.transport_error(code),
            Note::Loading(id) => {
                let mut st = self.state.borrow_mut();
                st.loads = st.loads.saturating_sub(1);
                if st.loads > 0 {
                    return; // a newer load is on its way
                }
                log::debug!("iptv mpv: loading entry {id:?}");
                st.expected = id;
                if id.is_none() && st.url.is_some() {
                    drop(st);
                    fail(&self.state, out, map::IO_UNSPECIFIED, None);
                }
            }
            Note::Started(entry) => {
                log::debug!("iptv mpv: started entry {entry}");
                self.state.borrow_mut().entry = Some(entry);
            }
            Note::Ended {
                entry,
                reason,
                error,
            } => self.end_file(entry, reason, error, out),
            Note::Bridged(entry) => {
                let mut st = self.state.borrow_mut();
                if st.expected.is_some() && st.expected == st.entry {
                    log::info!(
                        "iptv mpv {}: the live connection closed; playing on",
                        self.id
                    );
                    st.expected = Some(entry);
                    st.clues.handed_over();
                }
            }
            Note::Restarted { audio_out } => self.restarted(audio_out, out),
            Note::Buffering(on) => {
                let mut st = self.state.borrow_mut();
                st.buffering = on;
                if st.playing && self.current(&st) {
                    st.state(
                        out,
                        if on {
                            PlayerState::Buffering
                        } else {
                            PlayerState::Ready
                        },
                    );
                }
            }
            Note::Width(v) | Note::Height(v) => {
                let mut st = self.state.borrow_mut();
                if matches!(note, Note::Width(_)) {
                    st.width = v;
                } else {
                    st.height = v;
                }
                let size = (st.width, st.height);
                if size.0 > 0 && size.1 > 0 && size != st.reported {
                    st.reported = size;
                    out.push(
                        PlayerEvent::Size {
                            width: size.0 as u64,
                            height: size.1 as u64,
                        }
                        .wire(),
                    );
                }
            }
            Note::Aspect(a) => {
                let mut st = self.state.borrow_mut();
                if st.aspect != Some(a) {
                    st.aspect = Some(a);
                    let cinema = st.picture == "cinema";
                    drop(st);
                    if cinema {
                        self.apply_picture();
                    }
                }
            }
            Note::Log { warn, text } => {
                // Output formats carry no addresses; everything else may.
                if text.starts_with("AO:") || text.starts_with("VO:") {
                    log::info!("iptv mpv: {}", text.trim_end());
                }
                if warn {
                    self.state.borrow_mut().clues.hear(&text);
                }
            }
            Note::Tracks(json) => {
                let st = self.state.borrow();
                if !self.current(&st) {
                    return; // a file the viewer left
                }
                if let Some(tracks) = map::tracks(&json, st.video_auto) {
                    // The typed event of what the shell read from that
                    // JSON all along, so it reads the same tracks from it.
                    out.push(PlayerEvent::Tracks(crate::parse_tracks(&tracks)).wire());
                }
            }
            Note::Fps(fps) => {
                let mut st = self.state.borrow_mut();
                if fps.is_finite() && fps > 0.0 {
                    st.fps = fps;
                    if self.current(&st) {
                        st.format(out);
                    }
                }
            }
            Note::Channels(channels) => {
                let mut st = self.state.borrow_mut();
                if channels > 0 {
                    st.channels = channels;
                    if self.current(&st) {
                        st.format(out);
                    }
                }
            }
            Note::Position(seconds) => {
                if self.playing() {
                    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
                    log::debug!("iptv mpv: position {ms} ms");
                    out.push(PlayerEvent::Position { ms }.wire());
                }
            }
        }
    }

    fn playing(&self) -> bool {
        let st = self.state.borrow();
        st.playing && !st.paused && !st.buffering && self.current(&st)
    }

    /// Asks for the position (ms from where the source opened: mpv rebases
    /// a stream's first timestamp to 0) while the current file plays.
    fn tick(&self) {
        if self.playing() {
            self.send(Cmd::Position);
        }
    }

    fn current(&self, st: &State) -> bool {
        st.url.is_some() && st.expected.is_some() && st.entry == st.expected
    }

    fn end_file(&self, entry: i64, reason: c_int, error: c_int, out: &mut Out) {
        log::debug!("iptv mpv: ended entry {entry} (reason {reason}, error {error})");
        let mut st = self.state.borrow_mut();
        if !self.current(&st) || st.entry != Some(entry) {
            return; // a file the viewer already left
        }
        st.playing = false;
        match reason {
            ffi::MPV_END_FILE_REASON_EOF => st.state(out, PlayerState::Ended),
            // Reported once the queue is empty: mpv's log messages come
            // apart from its events and may follow this one.
            ffi::MPV_END_FILE_REASON_ERROR => st.failed = Some(error),
            _ => {}
        }
    }

    fn restarted(&self, ao: Option<String>, out: &mut Out) {
        let mut st = self.state.borrow_mut();
        if !self.current(&st) {
            return;
        }
        st.playing = true;
        st.shutter = false;
        if !st.buffering {
            st.state(out, PlayerState::Ready);
        }
        // What the file plays: seen before it started (kept, not sent), or
        // unchanged from the last file (not observed again): ask.
        st.format(out);
        self.send(Cmd::Format);
        if ao.is_some() && ao != st.audio_out {
            log::info!("iptv mpv: audio output {}", ao.as_deref().unwrap_or("?"));
            st.audio_out = ao;
        }
        drop(st);
        self.video.redraw();
    }
}

/// The current file failed: `error`, then `idle`. A live stream keeps
/// its last frame (the app reconnects it); anything else goes black.
fn fail(state: &RefCell<State>, out: &mut Out, code: &'static str, http: Option<u16>) {
    let mut st = state.borrow_mut();
    if !map::keeps_frame_on_error(st.live) {
        st.shutter = true;
    }
    st.playing = false;
    let code = code.to_owned();
    out.push(PlayerEvent::Error { code, http }.wire());
    st.state(out, PlayerState::Idle);
}

fn param<T>(type_: c_int, data: *mut T) -> ffi::mpv_render_param {
    ffi::mpv_render_param {
        type_,
        data: data.cast(),
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        PLAYERS.with(|p| p.borrow_mut().remove(&self.id));
        pip::forget(self.id);
        // The control thread stops first, then the render context goes, then
        // the handle (its last owner); the view and its GL context last.
        self.worker.get_mut().take();
        self.dead.get_mut().take();
        self.free_render();
        self.mpv.take();
    }
}
