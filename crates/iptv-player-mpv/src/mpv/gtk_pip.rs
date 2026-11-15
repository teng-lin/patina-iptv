//! Linux: the picture-in-picture window's `tv.pip` (`pip`). Its widget is
//! a box; while a player is wanted there it holds a `GtkGLArea` of its own
//! whose GL context is the player's area's (`create-context` hands it
//! over), so mpv's render context, made in that context, draws in it as
//! in the player's area: the same frames, nothing reopened. The player's
//! area stays in the main window (shrunk to nothing by the layout), which
//! keeps the context alive; GTK's GL contexts all share the display's, so
//! the window's renderer takes the area's frames like any other's.
//!
//! Moving the player's own area into the window instead would unrealize
//! it, and its GL context with mpv's render context would go: mpv drops
//! the video output then, and the stream would have to open again.
//!
//! On X11 the window asks to stay above the others and on every
//! workspace (`desktop/x11.rs`); on Wayland a client cannot, and the
//! compositor's rules for windows titled as browsers' picture in picture
//! do it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use nsh_platform_linux::gtk4::{self as gtk, glib, prelude::*};
use nsh_platform_linux::{Components, NativeComponent};
use serde_json::Value;

use super::find;

thread_local! {
    static HOSTS: RefCell<Vec<Weak<RefCell<Host>>>> = const { RefCell::new(Vec::new()) };
    static SURFACES: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
}

struct Host {
    frame: gtk::Box,
    /// None is the existing single-player PiP window.
    surface: Option<usize>,
    area: Option<(u64, gtk::GLArea)>,
}

pub(super) fn register_surface(surface: usize, id: u64) {
    SURFACES.with(|s| s.borrow_mut().insert(surface, id));
    sync();
}

pub(super) fn register(components: &Components) {
    components.register(
        super::pip::VIEW,
        Rc::new(|_| Box::new(PipView::new()) as Box<dyn NativeComponent>),
    );
}

struct PipView(Rc<RefCell<Host>>);

impl PipView {
    fn new() -> PipView {
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.set_hexpand(true);
        frame.set_vexpand(true);
        frame.set_can_target(false);
        frame.set_focusable(false);
        frame.connect_map(|frame| {
            let frame = frame.clone();
            glib::idle_add_local_once(move || {
                if let Some(window) = frame.root().and_downcast::<gtk::Window>() {
                    super::x11::keep_above(&window);
                }
            });
        });
        let host = Rc::new(RefCell::new(Host {
            frame,
            surface: None,
            area: None,
        }));
        HOSTS.with(|h| h.borrow_mut().push(Rc::downgrade(&host)));
        sync();
        PipView(host)
    }
}

impl Drop for PipView {
    fn drop(&mut self) {
        let mut host = self.0.borrow_mut();
        if let Some((_, area)) = host.area.take() {
            host.frame.remove(&area);
        }
    }
}

impl NativeComponent for PipView {
    fn widget(&self) -> gtk::Widget {
        self.0.borrow().frame.clone().upcast()
    }

    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        if name == "surface" {
            self.0.borrow_mut().surface = value.and_then(Value::as_u64).map(|s| s as usize);
            sync();
        }
    }
}

/// Each host draws only its assigned player's existing render context.
pub(super) fn sync() {
    HOSTS.with(|hosts| {
        hosts.borrow_mut().retain(|host| host.strong_count() > 0);
        for host in hosts.borrow().iter().filter_map(Weak::upgrade) {
            let mut host = host.borrow_mut();
            let wanted = match host.surface {
                Some(surface) => SURFACES.with(|s| s.borrow().get(&surface).copied()),
                None => super::pip::wanted(),
            }
            .filter(|id| find(*id).is_some_and(|p| p.video.context().is_some()));
            if host.area.as_ref().map(|(id, _)| *id) == wanted {
                continue;
            }
            if let Some((_, area)) = host.area.take() {
                host.frame.remove(&area);
            }
            if let Some(id) = wanted {
                let area = area_for(id);
                host.frame.append(&area);
                host.area = Some((id, area));
                log::info!("iptv mpv {id}: drawing in a picture-in-picture window");
            }
        }
    });
}

pub(super) fn redraw(id: u64) {
    HOSTS.with(|hosts| {
        for host in hosts.borrow().iter().filter_map(Weak::upgrade) {
            if let Some((shown, area)) = &host.borrow().area
                && *shown == id
            {
                area.queue_render();
            }
        }
    });
}

/// An area drawing player `id`'s frames with its area's GL context.
fn area_for(id: u64) -> gtk::GLArea {
    let area = gtk::GLArea::new();
    area.set_auto_render(false);
    area.set_has_depth_buffer(false);
    area.set_has_stencil_buffer(false);
    area.set_can_target(false);
    area.set_focusable(false);
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.connect_create_context(move |_| {
        let context = find(id).and_then(|p| p.video.context());
        if context.is_none() {
            log::warn!("iptv mpv {id}: no GL context to share with the window");
        }
        context
    });
    area.connect_render(move |area, _| {
        if let Some(p) = find(id) {
            let scale = area.scale_factor();
            p.draw(area.width() * scale, area.height() * scale);
        }
        glib::Propagation::Stop
    });
    area.connect_realize(|area| area.queue_render());
    area
}
