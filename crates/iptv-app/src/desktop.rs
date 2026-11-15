//! The desktop's window and mouse around the player: window full screen
//! (F11 on Linux and Ctrl+Cmd+F on macOS are the platform's own
//! shortcuts; the panels' button, a double click on the video and Escape
//! are ours) and mouse movement over full-screen video, which shows the
//! player panels. Off desktops the window has no state, so these read "not
//! full screen" and do nothing.

use crate::i18n::tr;

use std::cell::{Cell, RefCell};

use nsh_core::desktop::{MenuRole, WindowMode};
use nsh_core::element::desktop::{MenuItem, WindowRef, current_window, set_application_menu};

use crate::Ui;

pub mod pip_window;
pub mod player_windows;
/// The desktop backends, where the app owns the window and the sound:
/// Linux, macOS and Windows.
pub const DESKTOP: bool = cfg!(any(target_os = "linux", target_os = "macos", windows));

thread_local! {
    static WINDOW: RefCell<Option<WindowRef>> = const { RefCell::new(None) };
    /// Where the mouse last was (window points).
    static POINTER: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
}

/// Keeps the app's window; call while the app renders.
pub fn bind() {
    WINDOW.with(|w| *w.borrow_mut() = Some(current_window()));
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
    let role = MenuItem::role;
    set_application_menu(vec![
        // The app menu (AppKit titles it with the app's name).
        MenuItem::submenu(
            "Patina",
            [
                role(MenuRole::About),
                MenuItem::separator(),
                MenuItem::new("Settings…")
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
        MenuItem::submenu("View", [role(MenuRole::ToggleFullScreen)]),
        MenuItem::submenu(
            "Window",
            [
                role(MenuRole::Minimize),
                role(MenuRole::Zoom),
                MenuItem::separator(),
                role(MenuRole::Close),
            ],
        ),
    ]);
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
    set_fullscreen(!nsh_core::prelude::untracked(fullscreen));
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

/// How often mouse movement restarts the panels' timer at most.
const POINTER_EVERY_MS: u64 = 250;

thread_local! {
    static LAST_MOVE: Cell<Option<u64>> = const { Cell::new(None) };
}
