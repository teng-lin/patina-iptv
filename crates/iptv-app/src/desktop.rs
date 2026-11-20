//! The desktop's window and mouse around the player: window full screen
//! (F11 on Linux and Ctrl+Cmd+F on macOS are the platform's own
//! shortcuts; the panels' button, a double click on the video and Escape
//! are ours) and mouse movement over full-screen video, which shows the
//! player panels. Off desktops the window has no state, so these read "not
//! full screen" and do nothing.

use crate::i18n::tr;
use nsh_core::prelude::{effect, px, untracked};

use std::cell::{Cell, RefCell};

use nsh_core::desktop::{MenuRole, WindowMode};
use nsh_core::element::desktop::{MenuItem, WindowRef, current_window, set_application_menu};

use crate::Ui;

pub mod pip_window;
pub mod player_windows;
/// The desktop backends, where the app owns the window and the sound:
/// Linux, macOS and Windows.
pub const DESKTOP: bool = cfg!(any(target_os = "linux", target_os = "macos", windows));

/// Login/task launches run the recorder without automatically opening a viewer.
/// Explicit foreground playback remains available when the user opens the UI.
pub(crate) fn recorder_only_startup() -> bool {
    DESKTOP && std::env::args().any(|arg| arg == "--background-recordings")
}

thread_local! {
    static WINDOW: RefCell<Option<WindowRef>> = const { RefCell::new(None) };
    /// Where the mouse last was (window points).
    static POINTER: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
}

/// Keeps the app's window; call while the app renders.
pub fn bind(ui: Ui) {
    let window = current_window();
    WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
    if !DESKTOP {
        return;
    }
    let mut attributes = nsh_core::desktop::WindowAttributes::default();
    attributes.close_on_press = Some(false);
    window.set(attributes);
    let close_window = window.clone();
    window.on_close_press(move |reason| {
        if reason != nsh_core::desktop::CloseReason::System
            && crate::runtime().recording_background_needed()
        {
            // One contract with or without recordings (`docs/desktop.md`,
            // "Closing"): closing the main window ends every viewing
            // window's playback, as quitting does. Only the recorder runs on.
            // The viewer's Stop, not only the stream's: nothing counts as
            // playing, so choosing the same channel after reopening plays it.
            crate::parental_view::hidden(ui);
            crate::pip::exit(ui, iptv_core::pip_window::Exit::Close);
            player_windows::close_all(ui);
            crate::player_keys::stop_playback(ui);
            // Nothing autoplays in the hidden window, nor on showing again.
            crate::browse_playback::hold(ui);
            close_window.hide();
        } else {
            quit(ui);
        }
    });
    keep_reachable(window.clone());
    if recorder_only_startup() {
        window.hide();
    }
}

/// Whether the host reports and honours window positions: macOS and
/// Windows. GTK 4 tells no position on Linux (X11 or Wayland), where the
/// window manager or compositor keeps windows on the displays there are.
const POSITIONS: bool = cfg!(any(target_os = "macos", windows));

/// Keeps `window` reachable when displays go away or are arranged again,
/// when it first reports its state (a saved place restored onto a display
/// no longer there), and when it comes back from minimized or full screen:
/// it moves onto the nearest remaining display by
/// [`iptv_core::playback_windows::reachable`]. The viewer's own moves and
/// resizes do not run it, and a window the viewer can still grab is never
/// moved. Presentation only: nothing plays or stops.
pub(crate) fn keep_reachable(window: WindowRef) {
    if !POSITIONS {
        return;
    }
    // Changes when the window first reports its state and when it enters
    // or leaves a plain on-screen window; not when it is dragged.
    let settled = {
        let window = window.clone();
        nsh_core::prelude::computed(move || window.state().map(|s| on_screen(&s)))
    };
    effect(move || {
        let displays = nsh_core::element::desktop::displays();
        if settled.get() != Some(true) {
            return;
        }
        untracked(|| {
            let Some(frame) = window.state().and_then(|s| recovery(&s, &displays)) else {
                return;
            };
            log::info!(
                "iptv window {}: back onto a display at {:?},{:?}",
                window.id(),
                frame.x,
                frame.y
            );
            window.set_bounds(frame);
        });
    });
}

/// A plain window on the screen: the only kind Patina moves.
fn on_screen(state: &nsh_core::desktop::WindowState) -> bool {
    state.mode == WindowMode::Normal && !state.minimized
}

/// Where a window in `state` must go to be reachable on `displays`, if
/// anywhere ([`keep_reachable`]).
fn recovery(
    state: &nsh_core::desktop::WindowState,
    displays: &[nsh_core::desktop::Display],
) -> Option<nsh_core::desktop::WindowFrame> {
    if !on_screen(state) {
        return None;
    }
    let rect = |b: nsh_core::Bounds<nsh_core::Points>| {
        (b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0)
    };
    let areas: Vec<_> = displays.iter().map(|d| rect(d.visible_bounds)).collect();
    let (x, y, w, h) = iptv_core::playback_windows::reachable(rect(state.bounds), &areas)?;
    Some(nsh_core::desktop::WindowFrame {
        x: Some(px(x)),
        y: Some(px(y)),
        width: Some(px(w)),
        height: Some(px(h)),
        ..Default::default()
    })
}

/// Runs `f` on the app's window; `None` off desktops.
fn with_window<T>(f: impl FnOnce(&WindowRef) -> T) -> Option<T> {
    WINDOW.with(|w| w.borrow().as_ref().map(f))
}

/// Brings the app's window to the front, focused.
pub fn focus_main() {
    with_window(WindowRef::focus);
}

/// Whether the window is in full screen (reading subscribes).
pub fn fullscreen() -> bool {
    WINDOW.with(|w| {
        w.borrow()
            .as_ref()
            .and_then(WindowRef::state)
            .is_some_and(|s| s.mode == WindowMode::FullScreen)
    })
}

/// macOS's menu bar: the app menu (About, Settings…, Hide, Quit), Edit
/// (text fields' cut, copy and paste go through it), View (window full
/// screen) and Window. Elsewhere none: GTK would draw a menu bar in the
/// window. Call while the app renders.
pub fn menus(ui: Ui) {
    if !cfg!(target_os = "macos") {
        return;
    }
    effect(move || {
        let role = MenuItem::role;
        set_application_menu(vec![
            // The app menu (AppKit titles it with the app's name).
            MenuItem::submenu(
                "Patina",
                [
                    role(MenuRole::About),
                    MenuItem::separator(),
                    MenuItem::new(tr("Settings…"))
                        .shortcut("mod+,")
                        .on_select(move || crate::settings_view::open(ui)),
                    MenuItem::separator(),
                    role(MenuRole::Hide),
                    role(MenuRole::HideOthers),
                    role(MenuRole::ShowAll),
                    MenuItem::separator(),
                    role(MenuRole::Quit),
                ],
            ),
            MenuItem::submenu(
                tr("Edit"),
                [
                    role(MenuRole::Undo),
                    role(MenuRole::Redo),
                    MenuItem::separator(),
                    role(MenuRole::Cut),
                    role(MenuRole::Copy),
                    role(MenuRole::Paste),
                    role(MenuRole::SelectAll),
                ],
            ),
            // Ctrl+Cmd+F, as every macOS app.
            MenuItem::submenu(tr("View"), [role(MenuRole::ToggleFullScreen)]),
            MenuItem::submenu(
                tr("Window"),
                [
                    role(MenuRole::Minimize),
                    role(MenuRole::Zoom),
                    MenuItem::separator(),
                    role(MenuRole::Close),
                ],
            ),
        ]);
    });
}

pub fn set_fullscreen(on: bool) {
    WINDOW.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            log::info!("iptv window: full screen {on}");
            w.set_mode(if on {
                WindowMode::FullScreen
            } else {
                WindowMode::Normal
            });
        }
    });
}

pub fn toggle_fullscreen() {
    set_fullscreen(!untracked(fullscreen));
}

/// The mouse is at (`x`, `y`) at `now_ms`: whether it moved, at most every
/// [`POINTER_EVERY_MS`] (moves come once per frame; a redraw under a
/// resting mouse reports the same place again and must not keep the panels
/// up).
pub fn pointer_moved(x: f32, y: f32, now_ms: u64) -> bool {
    let moved = POINTER.with(|p| {
        let moved = p
            .get()
            .is_none_or(|(px, py)| (px - x).abs() + (py - y).abs() >= 2.0);
        if moved {
            p.set(Some((x, y)));
        }
        moved
    });
    moved
        && LAST_MOVE.with(|last| {
            let due = last
                .get()
                .is_none_or(|at| now_ms.saturating_sub(at) >= POINTER_EVERY_MS);
            if due {
                last.set(Some(now_ms));
            }
            due
        })
}

/// The mouse is at (`x`, `y`) in window `window` (its player surface: 0 is
/// the main window): whether it travelled at least [`TRAVEL`] points from
/// where it rested. Movement accumulates from that anchor, so a slow mouse
/// counts too; the anchor moves only once the threshold is crossed. The
/// first report in a window only records the place, so a resting mouse that
/// a window reports when it opens or redraws never counts. Each window keeps
/// its own anchor: their coordinates are their own.
pub fn pointer_travelled(window: usize, x: f32, y: f32) -> bool {
    TRAVELLED.with(|anchors| {
        let mut anchors = anchors.borrow_mut();
        if anchors.len() <= window {
            anchors.resize(window + 1, None);
        }
        let travelled =
            anchors[window].is_some_and(|(ax, ay)| (ax - x).abs() + (ay - y).abs() >= TRAVEL);
        if travelled || anchors[window].is_none() {
            anchors[window] = Some((x, y));
        }
        travelled
    })
}

/// The mouse entered window `window` at (`x`, `y`): where it rests there
/// now, so entering is no travel.
pub fn pointer_entered(window: usize, x: f32, y: f32) {
    TRAVELLED.with(|anchors| {
        let mut anchors = anchors.borrow_mut();
        if anchors.len() <= window {
            anchors.resize(window + 1, None);
        }
        anchors[window] = Some((x, y));
    });
}

/// Window `window` closed: a later window on its surface starts afresh.
pub fn pointer_forget(window: usize) {
    TRAVELLED.with(|anchors| {
        if let Some(anchor) = anchors.borrow_mut().get_mut(window) {
            *anchor = None;
        }
    });
}

/// How far the mouse must move to count as the viewer using it.
const TRAVEL: f32 = 4.0;

thread_local! {
    static TRAVELLED: RefCell<Vec<Option<(f32, f32)>>> = const { RefCell::new(Vec::new()) };
}

/// How often mouse movement restarts the panels' timer at most.
const POINTER_EVERY_MS: u64 = 250;

thread_local! {
    static LAST_MOVE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Explicit quit preserves future recording intent but stops the native host.
pub(crate) fn quit(ui: Ui) {
    if crate::runtime().shutdown_recording_host() {
        crate::player::watch_time::flush(ui);
        with_window(WindowRef::close);
    } else {
        crate::settings_view::notify(tr(
            "The recording host is still closing; try quitting again",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::recovery;
    use nsh_core::desktop::{Display, DisplayId, WindowMode, WindowPlacement, WindowState};
    use nsh_core::geometry::{Bounds, point, px, size};

    fn state(x: f32, y: f32, mode: WindowMode, minimized: bool) -> WindowState {
        let bounds = Bounds::new(point(px(x), px(y)), size(px(960.0), px(540.0)));
        let mut state = WindowState::new(bounds, WindowPlacement::new(mode, bounds));
        state.mode = mode;
        state.minimized = minimized;
        state
    }

    fn display(x: f32, y: f32, w: f32, h: f32) -> Display {
        let area = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        Display::new(DisplayId(1), area, area, true)
    }

    #[test]
    fn slow_movement_accumulates_per_window_and_entering_is_no_travel() {
        use super::{pointer_entered, pointer_forget, pointer_travelled};
        // The first report only rests the anchor; the same place is no move.
        assert!(!pointer_travelled(0, 100.0, 100.0));
        assert!(!pointer_travelled(0, 100.0, 100.0));
        // One point at a time still adds up to the threshold.
        assert!(!pointer_travelled(0, 101.0, 100.0));
        assert!(!pointer_travelled(0, 102.0, 100.0));
        assert!(!pointer_travelled(0, 103.0, 100.0));
        assert!(pointer_travelled(0, 104.0, 100.0));
        // The anchor moved there.
        assert!(!pointer_travelled(0, 105.0, 100.0));
        // Another window's coordinates are its own: no jump between them.
        assert!(!pointer_travelled(1, 900.0, 600.0));
        assert!(!pointer_travelled(0, 103.0, 101.0));
        // Entering a window rests the anchor where the mouse came in.
        pointer_entered(1, 10.0, 10.0);
        assert!(!pointer_travelled(1, 11.0, 10.0));
        // A new window on a closed one's surface starts afresh.
        pointer_forget(1);
        assert!(!pointer_travelled(1, 500.0, 500.0));
        assert!(pointer_travelled(1, 510.0, 500.0));
    }

    #[test]
    fn only_a_plain_window_off_every_display_moves() {
        let primary = [display(0.0, 0.0, 1920.0, 1040.0)];
        // Left on a display that went away: back onto the primary.
        let frame = recovery(&state(-1400.0, -150.0, WindowMode::Normal, false), &primary)
            .expect("moved back");
        assert_eq!((frame.x, frame.y), (Some(px(0.0)), Some(px(0.0))));
        assert_eq!(
            (frame.width, frame.height),
            (Some(px(960.0)), Some(px(540.0)))
        );
        // Still grabbable: left where the viewer put it.
        assert!(recovery(&state(100.0, 80.0, WindowMode::Normal, false), &primary).is_none());
        // Minimized or full screen: the host places it; checked again when
        // it comes back (`keep_reachable` tracks that change).
        assert!(recovery(&state(-1400.0, 0.0, WindowMode::Normal, true), &primary).is_none());
        assert!(
            recovery(
                &state(-1400.0, 0.0, WindowMode::FullScreen, false),
                &primary
            )
            .is_none()
        );
        // No display reported: never moved.
        assert!(recovery(&state(-1400.0, 0.0, WindowMode::Normal, false), &[]).is_none());
    }
}
