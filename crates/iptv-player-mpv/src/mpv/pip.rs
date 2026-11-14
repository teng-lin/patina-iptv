//! The desktops' picture-in-picture window (`desktop/pip_window.rs`)
//! shows a player that stays where it is, in the main window: the one
//! whose `pip` prop says `{window: true}` (surface 0's) also draws in the
//! window's [`VIEW`] component, from the same mpv instance and render
//! context, so no second player and no second stream open for it. How it
//! gets there is the platform's (`gtk_pip`: a second `GtkGLArea` drawing
//! with the player's own GL context; `mac_pip`: the player's layer-hosting
//! view moves into the window's view and back).
//!
//! The window's view and the player may come in either order (they mount
//! in the same commit); each side calls the platform's `sync`, which
//! attaches the wanted player to the view once both are there and detaches
//! it when either goes.

use std::cell::Cell;

use serde_json::Value;

/// The window's video view: what the app mounts in it (no props).
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[cfg_attr(test, allow(unused_imports))] // GTK's (`test_host` registers nothing)
pub use crate::PIP_VIEW as VIEW;

thread_local! {
    /// The player the window shows, by id.
    static WANTED: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Player `id`'s `pip` prop: `{window}` says whether the window shows it.
pub(super) fn set_prop(id: u64, value: Option<&Value>) {
    let on = value
        .and_then(|v| v[crate::PipProps::WINDOW].as_bool())
        .unwrap_or(false);
    let changed = WANTED.with(|w| {
        let next = match (on, w.get()) {
            (true, _) => Some(id),
            (false, Some(current)) if current == id => None,
            (false, current) => current,
        };
        w.replace(next) != next
    });
    if changed {
        log::info!("iptv mpv {id}: picture-in-picture window {on}");
        super::platform::pip_sync();
    }
}

/// Player `id` goes: the window shows it no more.
pub(super) fn forget(id: u64) {
    if WANTED.with(|w| w.get() == Some(id)) {
        WANTED.with(|w| w.set(None));
        super::platform::pip_sync();
    }
}

/// The player the window shows, if any (the desktops'; iOS has no
/// window to show it in).
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(super) fn wanted() -> Option<u64> {
    WANTED.with(Cell::get)
}
