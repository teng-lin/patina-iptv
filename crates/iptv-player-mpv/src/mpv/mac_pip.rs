//! macOS: the picture-in-picture window's `tv.pip` (`pip`). The player's
//! view (layer-hosting, its `CAOpenGLLayer` drawing with the CGL context
//! the player owns, `mac`) sits in a frame view of its own, the one NSH
//! hosts and lays out. While the window wants the player, that view moves
//! from its frame into the window's (`tv.pip`'s) frame, and back after:
//! AppKit moves views between windows as they are, the layer asks the
//! player for its context again if it asks at all (`copyCGLContextFor…`
//! hands out the same one), so mpv's render context, made in it, goes on
//! drawing; nothing reopens. Both frames let the pointer through.
//!
//! Each auxiliary window selects an explicit surface; the ordinary PiP
//! window follows `pip::wanted`. Only PiP keeps the stream's aspect ratio.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use nsh_platform_macos::{Components, NativeComponent};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSResponder, NSView};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize};
use serde_json::Value;

use super::find;

define_class!(
    /// A view that holds the player's view: the pointer passes through
    /// to NSH's view under it, as through the player's own.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IptvMpvFrame"]
    pub(super) struct Frame;

    unsafe impl NSObjectProtocol for Frame {}

    impl Frame {
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }
    }
);

impl Frame {
    pub(super) fn new(mtm: MainThreadMarker) -> Retained<Frame> {
        let this = Frame::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// Puts `view` in this frame, filling it as it resizes.
    pub(super) fn hold(&self, view: &NSView) {
        view.removeFromSuperview();
        view.setFrame(self.bounds());
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        self.addSubview(view);
    }
}

thread_local! {
    // Weak hosts cannot keep a closed window or its view alive.
    static HOSTS: RefCell<Vec<Weak<RefCell<Host>>>> = const { RefCell::new(Vec::new()) };
    static ROUTES: RefCell<Routes> = RefCell::default();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    // A component is created before its props arrive. It must not borrow
    // the ordinary PiP player's view during that interval.
    Pending,
    Unbound,
    Pip,
    Surface(usize),
}

#[derive(Default)]
struct Routes(HashMap<usize, u64>);

impl Routes {
    fn bind(&mut self, id: u64, surface: Option<usize>) {
        self.forget(id);
        if let Some(surface) = surface {
            self.0.insert(surface, id);
        }
    }

    fn forget(&mut self, id: u64) {
        // A late drop of an old player must not erase its replacement.
        self.0.retain(|_, player| *player != id);
    }

    fn wanted(&self, target: Target, pip: Option<u64>) -> Option<u64> {
        match target {
            Target::Pending | Target::Unbound => None,
            Target::Pip => pip,
            Target::Surface(surface) => self.0.get(&surface).copied(),
        }
    }
}

struct Host {
    frame: Retained<Frame>,
    target: Target,
    /// The player shown, and its view.
    shown: Option<(u64, Retained<NSView>)>,
    /// The window's aspect is set.
    shaped: bool,
}

pub(super) fn bind_surface(id: u64, surface: Option<usize>) {
    ROUTES.with(|routes| routes.borrow_mut().bind(id, surface));
    sync();
}

/// Runs before the player's Inner is dropped, while its original frame
/// and CGL context still live. Either host/player destruction order is safe.
pub(super) fn forget_player(id: u64) {
    ROUTES.with(|routes| routes.borrow_mut().forget(id));
    super::pip::forget(id);
    sync();
}

/// Registers `tv.pip`.
pub(super) fn register(components: &Components) {
    components.register(
        super::pip::VIEW,
        Rc::new(|_| Box::new(PipView::new()) as Box<dyn NativeComponent>),
    );
}

/// The component NSH holds.
struct PipView(Rc<RefCell<Host>>);

impl PipView {
    fn new() -> PipView {
        let mtm = MainThreadMarker::new().expect("NSH makes components on the main thread");
        let frame = Frame::new(mtm);
        let host = Rc::new(RefCell::new(Host {
            frame,
            target: Target::Pending,
            shown: None,
            shaped: false,
        }));
        HOSTS.with(|hosts| hosts.borrow_mut().push(Rc::downgrade(&host)));
        // Existing PiP has no surface prop. Wait until this mount's props
        // have arrived before choosing that default; an auxiliary host
        // must never temporarily steal PiP's view.
        super::mac::on_main(|| {
            for host in live_hosts() {
                if host.borrow().target == Target::Pending {
                    host.borrow_mut().target = Target::Pip;
                }
            }
            sync();
        });
        PipView(host)
    }
}

impl Drop for PipView {
    fn drop(&mut self) {
        let (frame, shown) = {
            let mut host = self.0.borrow_mut();
            host.target = Target::Unbound;
            (host.frame.clone(), host.shown.take())
        };
        if let Some((id, view)) = shown {
            give_back(id, &view, &frame);
        }
        // If a replacement mount temporarily superseded another host,
        // let that remaining host reclaim its player after this one goes.
        sync();
    }
}

impl NativeComponent for PipView {
    fn view(&self) -> Retained<NSView> {
        Retained::into_super(self.0.borrow().frame.clone())
    }

    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        if name == "surface" {
            self.0.borrow_mut().target = match value {
                None | Some(Value::Null) => Target::Pip,
                Some(value) => value
                    .as_u64()
                    .and_then(|surface| usize::try_from(surface).ok())
                    .map_or(Target::Unbound, Target::Surface),
            };
            sync();
        }
    }
}

/// Player `id`'s view goes back into its own frame (or out of the window
/// when the player is gone).
fn give_back(id: u64, view: &NSView, frame: &Frame) {
    // SAFETY: both views are live and accessed only on AppKit's main thread.
    if !unsafe { view.superview() }.is_some_and(|parent| std::ptr::eq::<NSView>(&*parent, &**frame))
    {
        return;
    }
    match find(id) {
        Some(p) => p.video.frame().hold(view),
        None => view.removeFromSuperview(),
    }
    view.setNeedsDisplay(true);
}

/// Moves the wanted player's view into the window's frame, or back.
pub(super) fn sync() {
    let hosts = live_hosts();
    let mut claimed = HashSet::new();
    let mut changes = Vec::new();
    // A replacement host wins over an older mount for the same surface.
    for host in hosts.into_iter().rev() {
        let target = host.borrow().target;
        let wanted = ROUTES
            .with(|routes| routes.borrow().wanted(target, super::pip::wanted()))
            .filter(|id| claimed.insert(*id))
            .and_then(|id| find(id).map(|p| (id, p)));
        let mut state = host.borrow_mut();
        if state.shown.as_ref().map(|(id, _)| *id) == wanted.as_ref().map(|(id, _)| *id) {
            continue;
        }
        let frame = state.frame.clone();
        let shown = state.shown.take();
        state.shaped = false;
        drop(state);
        changes.push((host, frame, shown, wanted));
    }
    // Detach all obsolete bindings before attaching replacements. No
    // RefCell borrow spans an AppKit call, which may call back into drew.
    for (_, frame, shown, _) in &changes {
        if let Some((id, view)) = shown {
            give_back(*id, view, frame);
        }
    }
    for (host, frame, _, wanted) in changes {
        if let Some((id, p)) = wanted {
            let view = p.video.view();
            frame.hold(&view);
            host.borrow_mut().shown = Some((id, view.clone()));
            view.setNeedsDisplay(true);
            p.video.redraw();
            log::info!("iptv mpv {id}: drawing in a player window");
        }
    }
}

fn live_hosts() -> Vec<Rc<RefCell<Host>>> {
    HOSTS.with(|hosts| {
        let mut hosts = hosts.borrow_mut();
        hosts.retain(|host| host.strong_count() > 0);
        hosts.iter().filter_map(Weak::upgrade).collect()
    })
}

/// Player `id` drew a frame: the window takes the stream's shape once it
/// is on screen and the shape is known.
pub(super) fn drew(id: u64, width: i64, height: i64) {
    for host in live_hosts() {
        let frame = {
            let host = host.borrow();
            if host.target != Target::Pip
                || host.shaped
                || host.shown.as_ref().is_none_or(|(shown, _)| *shown != id)
            {
                continue;
            }
            host.frame.clone()
        };
        let Some(window) = frame.window() else {
            continue;
        };
        let known = width > 0 && height > 0;
        let shape = if known {
            NSSize::new(width as f64, height as f64)
        } else {
            NSSize::new(16.0, 9.0)
        };
        window.setContentAspectRatio(shape);
        host.borrow_mut().shaped = known;
    }
}

#[cfg(test)]
mod tests {
    use super::{Routes, Target};

    #[test]
    fn auxiliary_hosts_wait_for_their_surface_without_borrowing_pip() {
        let mut routes = Routes::default();
        assert_eq!(routes.wanted(Target::Pending, Some(10)), None);
        assert_eq!(routes.wanted(Target::Unbound, Some(10)), None);
        assert_eq!(routes.wanted(Target::Surface(1), Some(10)), None);
        routes.bind(20, Some(1));
        routes.bind(30, Some(2));
        assert_eq!(routes.wanted(Target::Pip, Some(10)), Some(10));
        assert_eq!(routes.wanted(Target::Surface(1), Some(10)), Some(20));
        assert_eq!(routes.wanted(Target::Surface(2), Some(10)), Some(30));
        assert_eq!(routes.wanted(Target::Surface(3), Some(10)), None);
        // Returning the main PiP leaves auxiliary routing intact.
        assert_eq!(routes.wanted(Target::Pip, None), None);
        assert_eq!(routes.wanted(Target::Surface(1), None), Some(20));
    }

    #[test]
    fn retiring_an_old_player_cannot_remove_its_replacement_or_other_windows() {
        let mut routes = Routes::default();
        routes.bind(20, Some(1));
        routes.bind(30, Some(2));
        routes.bind(40, Some(1));
        routes.forget(20);
        assert_eq!(routes.wanted(Target::Surface(1), None), Some(40));
        assert_eq!(routes.wanted(Target::Surface(2), None), Some(30));
        routes.forget(40);
        assert_eq!(routes.wanted(Target::Surface(1), None), None);
        assert_eq!(routes.wanted(Target::Surface(2), None), Some(30));
        routes.bind(50, Some(1));
        routes.forget(40); // A late second teardown is harmless.
        assert_eq!(routes.wanted(Target::Surface(1), None), Some(50));
    }

    #[test]
    fn rebinding_or_removing_metadata_leaves_no_stale_surface_alias() {
        let mut routes = Routes::default();
        routes.bind(20, Some(1));
        routes.bind(20, Some(3));
        assert_eq!(routes.wanted(Target::Surface(1), None), None);
        assert_eq!(routes.wanted(Target::Surface(3), None), Some(20));
        routes.bind(20, None);
        assert_eq!(routes.wanted(Target::Surface(3), None), None);
        assert_eq!(routes.wanted(Target::Pip, Some(10)), Some(10));
    }
}
