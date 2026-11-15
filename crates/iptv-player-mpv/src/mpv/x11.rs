//! X11 (Linux): asks the window manager to keep a window above the others
//! and on every workspace (EWMH `_NET_WM_STATE_ABOVE` and `_STICKY`), what
//! GTK 4 has no API for. The picture-in-picture window's (`mpv/gtk_pip.rs`).
//!
//! libX11 is loaded when first needed (`dlopen`) and GDK's X11 functions
//! looked up in the GTK already loaded, as NSH's test agent does, so
//! nothing links X11: on Wayland, or a GTK without its X11 backend, this
//! does nothing (there a client cannot place itself; the compositor's
//! rules for picture-in-picture windows do it).

use std::cell::OnceCell;
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};

use nsh_platform_linux::gtk4::{self as gtk, prelude::*};

type Display = c_void;
type XWindow = c_ulong;
type Atom = c_ulong;

const RTLD_NOW: c_int = 2;
const CLIENT_MESSAGE: c_int = 33;
/// `SubstructureNotifyMask | SubstructureRedirectMask`.
const ROOT_MASK: c_long = (1 << 19) | (1 << 20);
/// `_NET_WM_STATE_ADD`.
const ADD: c_long = 1;
/// The request comes from a normal application.
const FROM_APP: c_long = 1;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// Xlib's `XClientMessageEvent`, padded to an `XEvent`'s size.
#[repr(C)]
struct ClientMessage {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    window: XWindow,
    message_type: Atom,
    format: c_int,
    data: [c_long; 5],
    _pad: [c_long; 12],
}

struct X {
    intern_atom: unsafe extern "C" fn(*mut Display, *const c_char, c_int) -> Atom,
    send_event: unsafe extern "C" fn(*mut Display, XWindow, c_int, c_long, *mut c_void) -> c_int,
    flush: unsafe extern "C" fn(*mut Display) -> c_int,
    root: unsafe extern "C" fn(*mut Display) -> XWindow,
    xdisplay: unsafe extern "C" fn(*mut c_void) -> *mut Display,
    xid: unsafe extern "C" fn(*mut c_void) -> XWindow,
}

thread_local! {
    static X11: OnceCell<Option<X>> = const { OnceCell::new() };
}

/// A symbol from `lib` (the process' own with a null `lib`).
unsafe fn sym<T: Copy>(lib: *mut c_void, name: &CStr) -> Option<T> {
    // SAFETY: the caller names a function of type `T`.
    unsafe {
        let p = dlsym(lib, name.as_ptr());
        (!p.is_null()).then(|| std::mem::transmute_copy::<*mut c_void, T>(&p))
    }
}

impl X {
    fn load() -> Option<X> {
        // SAFETY: a plain dlopen (the library stays loaded); the
        // signatures are Xlib's and GDK's X11 ones.
        unsafe {
            let x11 = dlopen(c"libX11.so.6".as_ptr(), RTLD_NOW);
            if x11.is_null() {
                return None;
            }
            Some(X {
                intern_atom: sym(x11, c"XInternAtom")?,
                send_event: sym(x11, c"XSendEvent")?,
                flush: sym(x11, c"XFlush")?,
                root: sym(x11, c"XDefaultRootWindow")?,
                xdisplay: sym(std::ptr::null_mut(), c"gdk_x11_display_get_xdisplay")?,
                xid: sym(std::ptr::null_mut(), c"gdk_x11_surface_get_xid")?,
            })
        }
    }
}

/// Keeps `window` above the others, on every workspace, where the display
/// is X11 (and its window manager follows EWMH).
/// Changes the PiP hints when an existing player window enters/leaves
/// compact mode. Wayland window levels remain compositor policy.
pub fn set_keep_above(window: &gtk::Window, above: bool) {
    let Some(surface) = window.surface() else {
        return;
    };
    if surface.display().type_().name() != "GdkX11Display" {
        return;
    }
    X11.with(|x| {
        let Some(x) = x.get_or_init(X::load) else {
            return;
        };
        // SAFETY: Xlib calls on GDK's own display, on its thread; the
        // event is a complete `XEvent`.
        unsafe {
            let display = (x.xdisplay)(surface.display().as_ptr().cast());
            if display.is_null() {
                return;
            }
            let atom = |name: &CStr| (x.intern_atom)(display, name.as_ptr(), 0);
            let mut event = ClientMessage {
                kind: CLIENT_MESSAGE,
                serial: 0,
                send_event: 1,
                display,
                window: (x.xid)(surface.as_ptr().cast()),
                message_type: atom(c"_NET_WM_STATE"),
                format: 32,
                data: [
                    if above { ADD } else { 0 }, // EWMH REMOVE is zero.
                    atom(c"_NET_WM_STATE_ABOVE") as c_long,
                    atom(c"_NET_WM_STATE_STICKY") as c_long,
                    FROM_APP,
                    0,
                ],
                _pad: [0; 12],
            };
            let root = (x.root)(display);
            (x.send_event)(display, root, 0, ROOT_MASK, (&raw mut event).cast());
            (x.flush)(display);
        }
        log::info!("iptv window: X11 picture-in-picture hints enabled: {above}");
    });
}
