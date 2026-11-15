//! iOS and iPadOS's side of `tv.player`: the player draws through mpv's
//! render API (OpenGL ES 3) into a `UIView` whose layer is a
//! `CAEAGLLayer`. OpenGL ES is deprecated on iOS but present on every
//! device and simulator, and it is the render API MPVKit's libmpv offers
//! (`tools/fetch-mpvkit.sh`; `build.rs` links it). The view owns the
//! context, a framebuffer and a renderbuffer whose storage is the layer's
//! (made again when the view's size or scale changes). A frame mpv offers
//! asks for a draw ([`Video::redraw`]), coalesced on the main queue: the
//! context current, the framebuffer bound, the player draws its frame or
//! black, the renderbuffer is presented. Touches pass through to NSH's
//! view (`hitTest:` answers nil). GCD's main queue runs what mpv's threads
//! ask for, as on macOS.

// Apple marks OpenGL ES deprecated; mpv's render API needs it.
#![allow(deprecated)]

use std::cell::Cell;
use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

use dispatch2::{DispatchQueue, DispatchTime};
pub use nsh_platform_ios::ComponentContext;
use nsh_platform_ios::{Components, NativeComponent};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_ui_kit::{UIEvent, UIResponder, UIView};
use serde_json::Value;

use super::{Inner, NAME, find};

/// Registers `tv.player`.
pub fn register(components: &Components) {
    audio_session();
    components.register(
        NAME,
        std::rc::Rc::new(|ctx| Box::new(Player::new(ctx)) as Box<dyn NativeComponent>),
    );
}

/// Runs `f` on the main thread (GCD's main queue, which UIKit's run loop
/// drains).
pub(super) fn on_main(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

/// The desktops' picture-in-picture window (`desktop/pip_window.rs`)
/// has no iPad counterpart: no player moves anywhere ([`super::pip`]).
pub(super) fn pip_sync() {}

/// Calls `f` once a second while player `id` lives.
pub(super) fn every_second(id: u64, f: fn(&Inner)) {
    let when = DispatchTime::NOW.time(1_000_000_000);
    let scheduled = DispatchQueue::main().after(when, move || {
        if let Some(p) = find(id) {
            f(&p);
            every_second(id, f);
        }
    });
    if let Err(e) = scheduled {
        log::error!("iptv mpv: no position timer ({e:?})");
    }
}

/// Sound plays as a video app's does: through the silent switch, and
/// alone (other apps' audio stops while a channel plays).
fn audio_session() {
    let Some(class) = AnyClass::get(c"AVAudioSession") else {
        return;
    };
    // AVAudioSessionCategoryPlayback and AVAudioSessionModeMoviePlayback.
    let category = objc2_foundation::NSString::from_str("AVAudioSessionCategoryPlayback");
    let mode = objc2_foundation::NSString::from_str("AVAudioSessionModeMoviePlayback");
    // SAFETY: AVAudioSession's documented class method and setters; an
    // error out-param of null is allowed.
    unsafe {
        let session: *mut AnyObject = msg_send![class, sharedInstance];
        if session.is_null() {
            return;
        }
        let ok: Bool = msg_send![
            session,
            setCategory: &*category,
            mode: &*mode,
            options: 0_usize,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];
        if !ok.as_bool() {
            log::warn!("iptv mpv: the audio session's category was refused");
        }
    }
}

// OpenGL ES's constants and the calls the view makes itself.
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_RENDERBUFFER: u32 = 0x8D41;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_RENDERBUFFER_WIDTH: u32 = 0x8D42;
const GL_RENDERBUFFER_HEIGHT: u32 = 0x8D43;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
/// `kEAGLRenderingAPIOpenGLES3`.
const EAGL_ES3: usize = 3;

/// OpenGL ES's entry points (the framework has one implementation, as on
/// macOS).
pub(super) fn address(name: &CStr) -> *mut c_void {
    static FRAMEWORK: OnceLock<usize> = OnceLock::new();
    let handle = *FRAMEWORK.get_or_init(|| {
        // SAFETY: dlopen with a NUL-terminated path; never closed.
        unsafe {
            libc::dlopen(
                c"/System/Library/Frameworks/OpenGLES.framework/OpenGLES".as_ptr(),
                libc::RTLD_LAZY | libc::RTLD_LOCAL,
            ) as usize
        }
    });
    if handle == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: a live handle and a NUL-terminated name.
    unsafe { libc::dlsym(handle as *mut c_void, name.as_ptr()) }
}

/// The few GL calls of the view's own.
#[derive(Clone, Copy)]
struct Es {
    gen_framebuffers: unsafe extern "C" fn(i32, *mut u32),
    gen_renderbuffers: unsafe extern "C" fn(i32, *mut u32),
    bind_framebuffer: unsafe extern "C" fn(u32, u32),
    bind_renderbuffer: unsafe extern "C" fn(u32, u32),
    framebuffer_renderbuffer: unsafe extern "C" fn(u32, u32, u32, u32),
    get_renderbuffer_parameteriv: unsafe extern "C" fn(u32, u32, *mut i32),
    check_framebuffer_status: unsafe extern "C" fn(u32) -> u32,
    delete_framebuffers: unsafe extern "C" fn(i32, *const u32),
    delete_renderbuffers: unsafe extern "C" fn(i32, *const u32),
    viewport: unsafe extern "C" fn(i32, i32, i32, i32),
}

impl Es {
    fn load() -> Option<Es> {
        macro_rules! f {
            ($name:literal, $ty:ty) => {{
                let p = address($name);
                if p.is_null() {
                    return None;
                }
                // SAFETY: OpenGL ES's documented signature for this name.
                unsafe { std::mem::transmute::<*mut c_void, $ty>(p) }
            }};
        }
        type GenFn = unsafe extern "C" fn(i32, *mut u32);
        type BindFn = unsafe extern "C" fn(u32, u32);
        type DeleteFn = unsafe extern "C" fn(i32, *const u32);
        Some(Es {
            gen_framebuffers: f!(c"glGenFramebuffers", GenFn),
            gen_renderbuffers: f!(c"glGenRenderbuffers", GenFn),
            bind_framebuffer: f!(c"glBindFramebuffer", BindFn),
            bind_renderbuffer: f!(c"glBindRenderbuffer", BindFn),
            framebuffer_renderbuffer: f!(
                c"glFramebufferRenderbuffer",
                unsafe extern "C" fn(u32, u32, u32, u32)
            ),
            get_renderbuffer_parameteriv: f!(
                c"glGetRenderbufferParameteriv",
                unsafe extern "C" fn(u32, u32, *mut i32)
            ),
            check_framebuffer_status: f!(
                c"glCheckFramebufferStatus",
                unsafe extern "C" fn(u32) -> u32
            ),
            delete_framebuffers: f!(c"glDeleteFramebuffers", DeleteFn),
            delete_renderbuffers: f!(c"glDeleteRenderbuffers", DeleteFn),
            viewport: f!(c"glViewport", unsafe extern "C" fn(i32, i32, i32, i32)),
        })
    }
}

/// The view's GL state: an EAGL context, the layer's framebuffer and its
/// size in pixels (0 until the layer has storage).
struct Target {
    context: Retained<AnyObject>,
    es: Es,
    framebuffer: Cell<u32>,
    renderbuffer: Cell<u32>,
    size: Cell<(i32, i32)>,
}

impl Target {
    fn new() -> Option<Target> {
        let es = Es::load()?;
        let class = AnyClass::get(c"EAGLContext")?;
        // SAFETY: EAGLContext's designated initializer; nil if ES 3 is
        // missing.
        let context: Option<Retained<AnyObject>> =
            unsafe { msg_send![msg_send![class, alloc], initWithAPI: EAGL_ES3] };
        Some(Target {
            context: context?,
            es,
            framebuffer: Cell::new(0),
            renderbuffer: Cell::new(0),
            size: Cell::new((0, 0)),
        })
    }

    fn make_current(&self) -> bool {
        let Some(class) = AnyClass::get(c"EAGLContext") else {
            return false;
        };
        // SAFETY: EAGLContext's class method with a live context.
        let ok: Bool = unsafe { msg_send![class, setCurrentContext: &*self.context] };
        ok.as_bool()
    }

    /// (Re)makes the renderbuffer's storage from `layer` (its bounds times
    /// its contents scale) and the framebuffer around it; the context is
    /// current.
    fn storage(&self, layer: &AnyObject) {
        let es = self.es;
        // SAFETY: GL calls with the context current; ids made here, deleted
        // once; `renderbufferStorage:fromDrawable:` with a CAEAGLLayer.
        unsafe {
            if self.framebuffer.get() == 0 {
                let (mut fb, mut rb) = (0, 0);
                (es.gen_framebuffers)(1, &mut fb);
                (es.gen_renderbuffers)(1, &mut rb);
                self.framebuffer.set(fb);
                self.renderbuffer.set(rb);
            }
            (es.bind_renderbuffer)(GL_RENDERBUFFER, self.renderbuffer.get());
            let ok: Bool = msg_send![
                &*self.context,
                renderbufferStorage: GL_RENDERBUFFER as usize,
                fromDrawable: layer
            ];
            (es.bind_framebuffer)(GL_FRAMEBUFFER, self.framebuffer.get());
            (es.framebuffer_renderbuffer)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_RENDERBUFFER,
                self.renderbuffer.get(),
            );
            let (mut w, mut h) = (0, 0);
            (es.get_renderbuffer_parameteriv)(GL_RENDERBUFFER, GL_RENDERBUFFER_WIDTH, &mut w);
            (es.get_renderbuffer_parameteriv)(GL_RENDERBUFFER, GL_RENDERBUFFER_HEIGHT, &mut h);
            let complete = (es.check_framebuffer_status)(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE;
            if !ok.as_bool() || !complete {
                log::warn!("iptv mpv: no drawable storage for {w}x{h}");
                self.size.set((0, 0));
                return;
            }
            self.size.set((w, h));
        }
    }

    /// Presents what was drawn into the renderbuffer.
    fn present(&self) {
        // SAFETY: GL and EAGL calls with the context current.
        unsafe {
            (self.es.bind_renderbuffer)(GL_RENDERBUFFER, self.renderbuffer.get());
            let _: Bool = msg_send![&*self.context, presentRenderbuffer: GL_RENDERBUFFER as usize];
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if !self.make_current() {
            return;
        }
        let (fb, rb) = (self.framebuffer.get(), self.renderbuffer.get());
        // SAFETY: ours, deleted once, with the context current.
        unsafe {
            if fb != 0 {
                (self.es.delete_framebuffers)(1, &fb);
                (self.es.delete_renderbuffers)(1, &rb);
            }
        }
        if let Some(class) = AnyClass::get(c"EAGLContext") {
            // SAFETY: no context is left current once it is gone.
            let _: Bool =
                unsafe { msg_send![class, setCurrentContext: std::ptr::null::<AnyObject>()] };
        }
    }
}

/// What the view holds: its player's id, its GL target (`None` if no
/// context could be made: then black), and whether a draw is queued.
struct ViewIvars {
    player: u64,
    target: Option<Target>,
    queued: Cell<bool>,
}

define_class!(
    /// The view NSH hosts, its layer a `CAEAGLLayer`; touches pass
    /// through to the node's view.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IptvMpvView"]
    #[ivars = ViewIvars]
    struct VideoView;

    // SAFETY: this NSObject subclass inherits the NSObject protocol implementation.
    unsafe impl NSObjectProtocol for VideoView {}

    impl VideoView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            AnyClass::get(c"CAEAGLLayer").expect("QuartzCore's CAEAGLLayer")
        }

        #[unsafe(method_id(hitTest:withEvent:))]
        fn hit_test(&self, _point: CGPoint, _event: Option<&UIEvent>) -> Option<Retained<UIView>> {
            None
        }

        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            // SAFETY: UIView's own.
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            self.resized();
        }

        #[unsafe(method(didMoveToWindow))]
        fn moved_to_window(&self) {
            self.resized();
        }
    }
);

impl VideoView {
    fn new(mtm: MainThreadMarker, player: u64) -> Retained<VideoView> {
        let target = Target::new();
        if target.is_none() {
            log::error!("iptv mpv: no OpenGL ES context");
        }
        let this = VideoView::alloc(mtm).set_ivars(ViewIvars {
            player,
            target,
            queued: Cell::new(false),
        });
        // SAFETY: UIView's designated initializer.
        let view: Retained<VideoView> =
            unsafe { msg_send![super(this), initWithFrame: CGRect::ZERO] };
        view.setOpaque(true);
        view.setUserInteractionEnabled(false);
        // SAFETY: the layer is a CAEAGLLayer (`layerClass`); opaque layers
        // composite cheaper.
        unsafe {
            let layer: *mut AnyObject = msg_send![&view, layer];
            let _: () = msg_send![layer, setOpaque: true];
        }
        view
    }

    /// The layer's pixels follow the view's size and the screen's scale:
    /// new storage, then a frame at once.
    fn resized(&self) {
        let Some(target) = &self.ivars().target else {
            return;
        };
        let scale = self
            .window()
            .map(|w| w.screen().scale())
            .unwrap_or_else(|| self.contentScaleFactor());
        if self.contentScaleFactor() != scale {
            self.setContentScaleFactor(scale);
        }
        let bounds = self.bounds();
        if bounds.size.width < 1.0 || bounds.size.height < 1.0 || !target.make_current() {
            return;
        }
        // SAFETY: the view's layer is the CAEAGLLayer it was made with.
        let layer: Retained<AnyObject> = unsafe { msg_send![self, layer] };
        target.storage(&layer);
        self.draw();
    }

    /// Draws a frame now (the player's, or black), on the main thread.
    fn draw(&self) {
        self.ivars().queued.set(false);
        let Some(target) = &self.ivars().target else {
            return;
        };
        let (w, h) = target.size.get();
        if w == 0 || h == 0 || !target.make_current() {
            return;
        }
        // SAFETY: the context is current; the framebuffer is ours.
        unsafe {
            (target.es.bind_framebuffer)(GL_FRAMEBUFFER, target.framebuffer.get());
            (target.es.viewport)(0, 0, w, h);
        }
        if let Some(p) = find(self.ivars().player)
            && p.gl.get().is_some()
        {
            p.draw(w, h);
        } else if let Some(gl) = super::gl::Gl::load() {
            gl.black();
        }
        target.present();
    }
}

/// The view the player draws in.
pub(super) struct Video {
    view: Retained<VideoView>,
}

impl Video {
    fn new(mtm: MainThreadMarker, player: u64) -> Video {
        Video {
            view: VideoView::new(mtm, player),
        }
    }

    /// Draws again soon: once per turn of the main queue, however many
    /// frames asked.
    pub(super) fn redraw(&self) {
        let ivars = self.view.ivars();
        if ivars.queued.replace(true) {
            return;
        }
        let view = self.view.clone();
        // The view stays on the main thread: only its pointer crosses, and
        // the closure runs on the main queue.
        let ptr = Retained::into_raw(view) as usize;
        on_main(move || {
            // SAFETY: the pointer `into_raw` gave above, taken back once, on
            // the main thread.
            let view = unsafe { Retained::from_raw(ptr as *mut VideoView) }
                .expect("the view retained above");
            view.draw();
        });
    }

    /// Makes the view's GL context current, if it has one.
    pub(super) fn make_current(&self) {
        if let Some(t) = &self.view.ivars().target {
            t.make_current();
        }
    }
}

define_class!(
    /// Hears the app go to the background and come back: its player's
    /// stream stops there and opens again here (iOS suspends a background
    /// app; Android's host does the same).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IptvMpvLifecycle"]
    #[ivars = u64]
    struct Lifecycle;

    // SAFETY: this NSObject subclass inherits the NSObject protocol implementation.
    unsafe impl NSObjectProtocol for Lifecycle {}

    impl Lifecycle {
        #[unsafe(method(background:))]
        fn background(&self, _note: &AnyObject) {
            if let Some(p) = find(*self.ivars()) {
                log::info!("iptv mpv {}: background, the stream stops", self.ivars());
                p.suspend();
            }
        }

        #[unsafe(method(foreground:))]
        fn foreground(&self, _note: &AnyObject) {
            if let Some(p) = find(*self.ivars()) {
                log::info!("iptv mpv {}: foreground, requesting playback authorization", self.ivars());
                p.resume();
            }
        }
    }
);

/// UIKit's notifications (their names are their constants' values).
const BACKGROUND: &str = "UIApplicationDidEnterBackgroundNotification";
const FOREGROUND: &str = "UIApplicationWillEnterForegroundNotification";

fn notification_center() -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"NSNotificationCenter")?;
    // SAFETY: Foundation's class method.
    unsafe { msg_send![class, defaultCenter] }
}

impl Lifecycle {
    fn new(mtm: MainThreadMarker, player: u64) -> Retained<Lifecycle> {
        let this = Lifecycle::alloc(mtm).set_ivars(player);
        // SAFETY: NSObject's initializer.
        let observer: Retained<Lifecycle> = unsafe { msg_send![super(this), init] };
        if let Some(center) = notification_center() {
            for (selector, name) in [
                (sel!(background:), BACKGROUND),
                (sel!(foreground:), FOREGROUND),
            ] {
                let name = objc2_foundation::NSString::from_str(name);
                // SAFETY: the observer lives until it is removed (Drop of
                // the player); the selector takes one NSNotification.
                unsafe {
                    let _: () = msg_send![
                        &*center,
                        addObserver: &*observer,
                        selector: selector,
                        name: &*name,
                        object: std::ptr::null::<AnyObject>()
                    ];
                }
            }
        }
        observer
    }

    fn remove(&self) {
        if let Some(center) = notification_center() {
            // SAFETY: removes every registration of this observer.
            unsafe {
                let _: () = msg_send![&*center, removeObserver: self];
            }
        }
    }
}

/// The component NSH holds.
struct Player {
    inner: std::rc::Rc<Inner>,
    lifecycle: Retained<Lifecycle>,
}

impl Player {
    fn new(ctx: ComponentContext) -> Self {
        let mtm = MainThreadMarker::new().expect("NSH makes components on the main thread");
        let inner = Inner::new(ctx, |id| Video::new(mtm, id));
        if inner.video.view.ivars().target.is_some() {
            inner.video.make_current();
            if inner.create_render() {
                log::info!("iptv mpv: rendering through a CAEAGLLayer (OpenGL ES 3)");
            }
        }
        inner.video.redraw();
        let lifecycle = Lifecycle::new(mtm, inner.id);
        Player { inner, lifecycle }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.lifecycle.remove();
    }
}

impl NativeComponent for Player {
    fn view(&self) -> &UIView {
        &self.inner.video.view
    }

    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        self.inner.set_prop(name, value);
    }
}
