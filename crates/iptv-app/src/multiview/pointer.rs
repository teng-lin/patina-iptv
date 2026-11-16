//! Multiview by pointer (desktop mouse, tablet touch): taps on tiles and
//! on the menu's items, a tap beside the menu, a long press, and the
//! pointer chrome's toolbar and tile buttons (`multiview_view/chrome.rs`).

use iptv_core::multiview::Outcome;

use super::apply;
use crate::{Ui, ViewMode};

/// A tap (or click) on tile `index`; it also shows the pointer chrome.
pub fn tap(ui: Ui, index: usize) {
    ui.touched();
    show_chrome(ui);
    apply(ui, |m, failed| m.tap(index, failed));
}

/// A long press on tile `index`: enlarges or reduces it.
pub fn long_press(ui: Ui, index: usize) {
    ui.touched();
    apply(ui, |m, _| {
        m.toggle_enlarge(index);
        Outcome::Stay
    });
}

/// A tap on item `index` of the open dialog.
pub fn choose(ui: Ui, index: usize) {
    ui.touched();
    apply(ui, |m, _| m.choose(index));
}

/// The toolbar's Add screen.
pub fn add(ui: Ui) {
    ui.touched();
    apply(ui, |m, _| m.add());
}

/// The toolbar's Exit multiview: the active tile full screen.
pub fn exit(ui: Ui) {
    ui.touched();
    apply(ui, |m, _| m.exit());
}

/// A tile's remove button.
pub fn remove(ui: Ui, index: usize) {
    ui.touched();
    used_chrome(ui);
    apply(ui, |m, _| {
        m.remove_tile(index);
        Outcome::Stay
    });
}

/// A tile's enlarge (or reduce) button.
pub fn toggle_enlarge(ui: Ui, index: usize) {
    used_chrome(ui);
    long_press(ui, index);
}

/// Whether the pointer chrome (toolbar, tile buttons) shows: the grid
/// with nothing over it, a pointer that moved or tapped lately.
pub fn chrome_shows(ui: Ui) -> bool {
    ui.mode.get() == ViewMode::Fullscreen
        && !ui.keys_shown()
        && ui.play_bar.with(|b| b.shown)
        && crate::play_bar::pointer_available()
        && ui.multiview.with(|m| {
            m.as_ref()
                .is_some_and(|m| m.detour().is_none() && m.dialog().is_none() && m.pick().is_none())
        })
}

/// A tap on a tile shows the chrome (a tablet has no moving pointer).
fn show_chrome(ui: Ui) {
    if crate::play_bar::pointer_available() {
        crate::play_bar::pointer_moved(ui);
    }
}

/// A button of the chrome acted: it stays a while longer.
fn used_chrome(ui: Ui) {
    crate::play_bar::used(ui);
}

/// A tap beside the tiles while the menu (or the warning) is open: it
/// closes; `false` with nothing open.
pub fn dismiss(ui: Ui) -> bool {
    let open = ui
        .multiview
        .with_untracked(|m| m.as_ref().is_some_and(|m| m.dialog().is_some()));
    if open {
        ui.touched();
        apply(ui, |m, failed| m.tap(usize::MAX, failed));
    }
    open
}
