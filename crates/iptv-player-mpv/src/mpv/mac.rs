//! The macOS desktop's side of `tv.player`: the player draws into an
//! `NSView` backed by a `CAOpenGLLayer` (mpv's render API has no Metal
//! backend; OpenGL is deprecated on macOS but present on every Mac,
//! Apple silicon included, and it is what mpv's own macOS player and IINA
//! use). The layer draws on the main thread (`asynchronous` off) when the
//! player asks for a frame (`setNeedsDisplay`), with the CGL context the
//! player made at its creation current and the layer's framebuffer bound;
//! the render context is made in that context at once, so a source never
//! waits for the view to reach a window. The view lets the pointer through
//! (`hitTest:` answers nil), so presses on the video reach NSH's view, as
//! the GTK area's `can_target(false)` does on Linux. GCD's main queue runs
//! what mpv's threads ask for.

// Apple marks OpenGL (CGL, CAOpenGLLayer) deprecated; mpv's render API
// needs it.
#![allow(deprecated)]

use std::ptr::NonNull;

use dispatch2::{DispatchQueue, DispatchTime};
pub use nsh_platform_macos::ComponentContext;
use nsh_platform_macos::{Components, NativeComponent};
use objc2::rc::Retained;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_app_kit::{NSResponder, NSView};
use objc2_core_video::CVTimeStamp;
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect};
use objc2_open_gl::{
    CGLChoosePixelFormat, CGLContextObj, CGLCreateContext, CGLError, CGLGetCurrentContext,
    CGLOpenGLProfile, CGLPixelFormatAttribute, CGLPixelFormatObj, CGLReleaseContext,
    CGLReleasePixelFormat, CGLRetainContext, CGLRetainPixelFormat, CGLSetCurrentContext,
};
use objc2_quartz_core::{CALayer, CAOpenGLLayer};
use serde_json::Value;

use super::{Inner, NAME, find};

pub(super) use super::mac_pip::sync as pip_sync;

/// Registers `tv.player` and the picture-in-picture window's `tv.pip`.
pub fn register(components: &Components) {
    components.register(
        NAME,
        std::rc::Rc::new(|ctx| Box::new(Player::new(ctx)) as Box<dyn NativeComponent>),
    );
    super::mac_pip::register(components);
}

/// Runs `f` on the main thread (GCD's main queue, which AppKit's run loop
/// drains).
pub(super) fn on_main(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

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

/// A CGL pixel format and a context made with it, released together.
struct Gl {
    pixel_format: CGLPixelFormatObj,
    context: CGLContextObj,
}

impl Gl {
    /// An accelerated OpenGL 3.2 core context (mpv's renderer needs 2.1 at
    /// least: the legacy profile if there is no core one), on whichever
    /// GPU the system picks.
    fn new() -> Option<Gl> {
        let profiles = [
            CGLOpenGLProfile::CGLOGLPVersion_3_2_Core,
            CGLOpenGLProfile::CGLOGLPVersion_Legacy,
        ];
        profiles.into_iter().find_map(Gl::with_profile)
    }

    fn with_profile(profile: CGLOpenGLProfile) -> Option<Gl> {
        let mut attributes = [
            CGLPixelFormatAttribute::CGLPFAOpenGLProfile,
            CGLPixelFormatAttribute(profile.0),
            CGLPixelFormatAttribute::CGLPFAAccelerated,
            CGLPixelFormatAttribute::CGLPFADoubleBuffer,
            CGLPixelFormatAttribute::CGLPFAAllowOfflineRenderers,
            CGLPixelFormatAttribute::CGLPFASupportsAutomaticGraphicsSwitching,
            CGLPixelFormatAttribute(0),
        ];
        let mut pixel_format: CGLPixelFormatObj = std::ptr::null_mut();
        let mut count = 0;
        // SAFETY: a 0-terminated attribute list and valid out pointers.
        let error = unsafe {
            CGLChoosePixelFormat(
                NonNull::new_unchecked(attributes.as_mut_ptr()),
                NonNull::from(&mut pixel_format),
                NonNull::from(&mut count),
            )
        };
        if error != CGLError::NoError || pixel_format.is_null() {
            return None;
        }
        let mut context: CGLContextObj = std::ptr::null_mut();
        // SAFETY: a pixel format CGL made, no share context, a valid out
        // pointer.
        let error = unsafe {
            CGLCreateContext(
                pixel_format,
                std::ptr::null_mut(),
                NonNull::from(&mut context),
            )
        };
        if error != CGLError::NoError || context.is_null() {
            // SAFETY: ours, released once.
            unsafe { CGLReleasePixelFormat(pixel_format) };
            return None;
        }
        Some(Gl {
            pixel_format,
            context,
        })
    }

    fn make_current(&self) {
        // SAFETY: a live context.
        unsafe { CGLSetCurrentContext(self.context) };
    }
}

impl Drop for Gl {
    fn drop(&mut self) {
        // SAFETY: ours, released once; a context is not left current once
        // it is gone.
        unsafe {
            if CGLGetCurrentContext() == self.context {
                CGLSetCurrentContext(std::ptr::null_mut());
            }
            CGLReleaseContext(self.context);
            CGLReleasePixelFormat(self.pixel_format);
        }
    }
}

/// What the layer holds: its player's id and the GL context it draws in
/// (`None` if none could be made: then CA's own, and black).
struct LayerIvars {
    player: u64,
    gl: Option<Gl>,
}

define_class!(
    /// The player's picture. CA calls `drawInCGLContext:…` on the main
    /// thread after `setNeedsDisplay`, with the context current and the
    /// layer's framebuffer bound; the player draws its frame or black.
    #[unsafe(super(CAOpenGLLayer, CALayer, NSObject))]
    #[name = "IptvMpvLayer"]
    #[ivars = LayerIvars]
    struct VideoLayer;

    // SAFETY: this NSObject subclass inherits the NSObject protocol implementation.
    unsafe impl NSObjectProtocol for VideoLayer {}

    impl VideoLayer {
        #[unsafe(method(copyCGLPixelFormatForDisplayMask:))]
        fn copy_pixel_format(&self, mask: u32) -> CGLPixelFormatObj {
            match &self.ivars().gl {
                // SAFETY: the caller releases what it is given.
                Some(gl) => unsafe { CGLRetainPixelFormat(gl.pixel_format) },
                // SAFETY: forward Core Animation's arguments to the superclass implementation.
                None => unsafe { msg_send![super(self), copyCGLPixelFormatForDisplayMask: mask] },
            }
        }

        #[unsafe(method(copyCGLContextForPixelFormat:))]
        fn copy_context(&self, pixel_format: CGLPixelFormatObj) -> CGLContextObj {
            match &self.ivars().gl {
                // SAFETY: the caller releases what it is given.
                Some(gl) => unsafe { CGLRetainContext(gl.context) },
                // SAFETY: forward the live pixel format to the superclass implementation.
                None => unsafe { msg_send![super(self), copyCGLContextForPixelFormat: pixel_format] },
            }
        }

        #[unsafe(method(canDrawInCGLContext:pixelFormat:forLayerTime:displayTime:))]
        fn can_draw(
            &self,
            _context: CGLContextObj,
            _pixel_format: CGLPixelFormatObj,
            _time: f64,
            _display_time: *const CVTimeStamp,
        ) -> bool {
            true
        }

        #[unsafe(method(drawInCGLContext:pixelFormat:forLayerTime:displayTime:))]
        fn draw_in(
            &self,
            context: CGLContextObj,
            pixel_format: CGLPixelFormatObj,
            time: f64,
            display_time: *const CVTimeStamp,
        ) {
            if let Some(p) = find(self.ivars().player)
                && let Some(gl) = p.gl.get()
            {
                let (width, height) = gl.viewport();
                p.draw(width, height);
                let size = { let st = p.state.borrow(); (st.width, st.height) };
                super::mac_pip::drew(p.id, size.0, size.1);
            } else if let Some(gl) = super::gl::Gl::load() {
                // No player (gone, or without a render context): black,
                // never what the framebuffer held.
                gl.black();
            }
            // CAOpenGLLayer's own flushes the context.
            // SAFETY: the arguments CA passed.
            unsafe {
                let _: () = msg_send![
                    super(self),
                    drawInCGLContext: context,
                    pixelFormat: pixel_format,
                    forLayerTime: time,
                    displayTime: display_time
                ];
            }
        }
    }
);

impl VideoLayer {
    fn new(player: u64, gl: Option<Gl>) -> Retained<VideoLayer> {
        let this = VideoLayer::alloc().set_ivars(LayerIvars { player, gl });
        // SAFETY: CAOpenGLLayer's designated initializer.
        let layer: Retained<VideoLayer> = unsafe { msg_send![super(this), init] };
        layer.setAsynchronous(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        layer.setOpaque(true);
        layer
    }
}

define_class!(
    /// The view NSH hosts: it hosts the player's layer; the pointer
    /// passes through to the node's view.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IptvMpvView"]
    #[ivars = Retained<VideoLayer>]
    struct VideoView;

    // SAFETY: this NSObject subclass inherits the NSObject protocol implementation.
    unsafe impl NSObjectProtocol for VideoView {}

    impl VideoView {
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn moved_to_window(&self) {
            self.match_scale();
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_changed(&self) {
            self.match_scale();
        }
    }
);

impl VideoView {
    fn new(mtm: MainThreadMarker, layer: Retained<VideoLayer>) -> Retained<VideoView> {
        let this = VideoView::alloc(mtm).set_ivars(layer);
        // SAFETY: NSView's designated initializer.
        let view: Retained<VideoView> =
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        // Layer-hosting (the layer first, then `wantsLayer`), as mpv's own
        // macOS player has it: as a backing layer (`makeBackingLayer`) the
        // view would be its delegate and AppKit's `displayLayer:` would
        // stand in for the layer's OpenGL drawing, which then never runs.
        let layer: &CALayer = view.ivars();
        view.setLayer(Some(layer));
        view.setWantsLayer(true);
        view
    }

    /// Draws at the window's pixel density (Retina: 2).
    fn match_scale(&self) {
        if let Some(window) = self.window() {
            let layer = self.ivars();
            let scale = window.backingScaleFactor();
            if layer.contentsScale() != scale {
                layer.setContentsScale(scale);
                layer.setNeedsDisplay();
            }
        }
    }
}

/// The view the player draws in, in the frame NSH hosts (the
/// picture-in-picture window borrows the view: `mac_pip`).
pub(super) struct Video {
    view: Retained<VideoView>,
    frame: Retained<super::mac_pip::Frame>,
}

impl Video {
    fn new(mtm: MainThreadMarker, player: u64) -> Video {
        let gl = Gl::new();
        if gl.is_none() {
            log::error!("iptv mpv: no OpenGL context");
        }
        let layer = VideoLayer::new(player, gl);
        let view = VideoView::new(mtm, layer);
        let frame = super::mac_pip::Frame::new(mtm);
        frame.hold(&view);
        Video { view, frame }
    }

    /// The player's view.
    pub(super) fn view(&self) -> Retained<NSView> {
        Retained::into_super(self.view.clone())
    }

    /// The frame NSH hosts.
    pub(super) fn frame(&self) -> &super::mac_pip::Frame {
        &self.frame
    }

    fn layer(&self) -> &VideoLayer {
        self.view.ivars()
    }

    /// Draws again soon (CA asks the layer at its next commit).
    pub(super) fn redraw(&self) {
        self.layer().setNeedsDisplay();
    }

    /// Makes the layer's GL context current, if it has one.
    pub(super) fn make_current(&self) {
        if let Some(gl) = &self.layer().ivars().gl {
            gl.make_current();
        }
    }
}

/// The component NSH holds.
struct Player(std::rc::Rc<Inner>);

impl Drop for Player {
    fn drop(&mut self) {
        super::mac_pip::forget_player(self.0.id);
    }
}

impl Player {
    fn new(ctx: ComponentContext) -> Self {
        let mtm = MainThreadMarker::new().expect("NSH makes components on the main thread");
        let inner = Inner::new(ctx, |id| Video::new(mtm, id));
        if inner.video.layer().ivars().gl.is_some() {
            // SAFETY: reads the thread's current context, restored below.
            let previous = unsafe { CGLGetCurrentContext() };
            inner.video.make_current();
            if inner.create_render() {
                log::info!("iptv mpv: rendering through a CAOpenGLLayer");
            }
            // SAFETY: the context that was current (or none).
            unsafe { CGLSetCurrentContext(previous) };
        }
        inner.video.redraw();
        Player(inner)
    }
}

impl NativeComponent for Player {
    fn view(&self) -> Retained<NSView> {
        Retained::into_super(self.0.video.frame.clone())
    }

    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        if name == "popupSurface" {
            let surface = value
                .and_then(Value::as_u64)
                .and_then(|surface| usize::try_from(surface).ok());
            super::mac_pip::bind_surface(self.0.id, surface);
        } else {
            self.0.set_prop(name, value);
        }
    }
}
