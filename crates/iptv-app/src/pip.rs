//! Picture in picture: the playing channel in a small window over other
//! apps, video only. The state is `iptv_core::pip`.
//!
//! **Android**: a system window. It opens from the control strip's
//! button, or by itself when the viewer leaves the app (Home) while a
//! channel plays full screen and `player.pip_on_leave` allows it (on
//! phones and tablets by default, not on a TV). The window's buttons
//! play/pause and change channel; a tap on it brings the app back full
//! screen; closing it stops playback (the host stops its players when the
//! activity stops, as for Home without it). The host's side is surface
//! 0's `tv.player` (`video.rs`):
//! - prop `pip` `{auto, enter}`: `auto`, whether leaving the app opens the
//!   window now; a new `enter` (a count) opens it now;
//! - event `pip` `{supported, tv, active}`: once mounted, and whenever the
//!   window opens or closes;
//! - event `pipAction` `{action: play|pause|next|previous}`: its buttons.
//!
//! In Android's window only the video shows (`lib.rs` hides every other
//! layer, [`ui_shown`]).
//!
//! **The desktops** (Linux, macOS) open a window of the app's own, as
//! browsers do (`desktop/pip_window.rs`, `iptv_core::pip_window`), from
//! the same buttons (leaving the app opens nothing there): the full-screen
//! player shows in it, the same player and stream (surface 0's `pip` prop
//! says `{window: true}`, `mpv/pip.rs`), while the main window stays
//! usable with a placeholder where the video was. Changing the channel
//! there changes the window's. Back to the app (the window's control, a
//! double click on it, Escape in it, the placeholder's button) closes the
//! window and the video plays full screen in the main window again;
//! closing it (its ✕, the system's close) stops playback. Multiview, or
//! nothing playing any more, closes it too.
//!
//! As the window opens, the panels, the play bar, the channel list and
//! any chooser or menu close, and multiview leaves to its active tile,
//! which plays on in the window (its other tiles stop).

use iptv_core::pip::{Change, OnLeave};
use iptv_core::pip_window::Exit;
use iptv_player_mpv::PipAction;
use iptv_settings::keys::PIP_ON_LEAVE;
use nsh_core::prelude::*;

use crate::{Ui, ViewMode};

/// A channel plays full screen: what the window would show.
fn playing(ui: Ui) -> bool {
    ui.mode.get() == ViewMode::Fullscreen && ui.playing.get().is_some()
}

/// Whether the player shows in the desktops' window of its own now
/// (reading subscribes).
pub fn in_window(ui: Ui) -> bool {
    ui.pip.with(|p| p.in_window())
}

/// Whether Android's window is the app's own: it then shows only the
/// video, filling it.
pub fn fills_app(ui: Ui) -> bool {
    ui.pip.with(|p| p.active && !p.separate)
}

/// Whether the app's layers over the video show: not in Android's window.
pub fn ui_shown(ui: Ui) -> bool {
    ui.pip.with(|p| p.ui_shown())
}

/// Whether this device has the window at all (reading subscribes): the
/// play bar shows its button only then.
pub fn supported(ui: Ui) -> bool {
    ui.pip.with(|p| p.supported)
}

/// Whether the control strip's button can open the window now.
pub fn can_enter(ui: Ui) -> bool {
    let playing = playing(ui);
    ui.pip.with(|p| p.can_enter(playing))
}

/// The desktops: their window can (Android's host reports its own), and
/// closes when nothing plays any more or multiview opens.
pub fn bind(ui: Ui) {
    if !crate::desktop::DESKTOP {
        return;
    }
    ui.pip.update(|p| {
        p.separate = true;
        p.report(true, false, false);
    });
    // On the change only: the window may open from multiview, which then
    // leaves to its tile.
    let was = std::cell::Cell::new(false);
    effect(move || {
        let gone = ui.playing.get().is_none() || crate::multiview::is_open(ui);
        let became = !was.replace(gone) && gone;
        if became && ui.pip.with_untracked(|p| p.in_window()) {
            log::info!("iptv pip: nothing for the window to show");
            untracked(|| report(ui, false));
        }
    });
}

/// The control strip's button: asks the host to open the window (on the
/// desktops, opens the app's own).
pub fn enter(ui: Ui) {
    let playing = untracked(|| playing(ui));
    let mut asked = false;
    ui.pip.update(|p| asked = p.request(playing));
    if !asked {
        return;
    }
    log::info!("iptv pip: asked to enter");
    if crate::desktop::DESKTOP {
        report(ui, true);
    }
}

/// The desktops' window closes: back to the app (the video full screen in
/// the main window) or closed (playback stops). False when none is open.
pub fn exit(ui: Ui, how: Exit) -> bool {
    if !ui.pip.with_untracked(|p| p.in_window()) {
        return false;
    }
    log::info!("iptv pip: {how:?}");
    report(ui, false);
    match how {
        Exit::BackToApp => {
            ui.mode.set(ViewMode::Fullscreen);
            crate::desktop::focus_main();
        }
        Exit::Close => crate::player_keys::stop_playback(ui),
    }
    true
}

/// The desktop window opened or closed.
fn report(ui: Ui, active: bool) {
    on_state(ui, true, false, active);
}

/// Surface `surface`'s `pip` prop: `{auto, enter, window}` for surface 0
/// (`window`: the desktops' window shows it), null for the others (the
/// window shows surface 0).
pub fn prop(ui: Ui, surface: usize) -> Option<iptv_player_mpv::PipProps> {
    if surface != 0 {
        return None;
    }
    ui.settings.get();
    let on_leave = OnLeave::from_key(crate::settings::choice(PIP_ON_LEAVE));
    let playing = playing(ui);
    let (auto, enter, window) = ui
        .pip
        .with(|p| (p.auto_enter(on_leave, playing), p.requests, p.in_window()));
    Some(iptv_player_mpv::PipProps {
        auto,
        enter,
        window,
    })
}

/// The host's `pip` event.
pub fn on_state(ui: Ui, supported: bool, tv: bool, active: bool) {
    let mut next = ui.pip.get_untracked();
    let change = next.report(supported, tv, active);
    if next == ui.pip.get_untracked() {
        return;
    }
    ui.pip.set(next);
    match change {
        Change::Entered => entered(ui),
        Change::Left => log::info!("iptv pip: left"),
        Change::None => log::info!("iptv pip: supported {supported}, tv {tv}"),
    }
}

/// The window opened: one stream, the full-screen player's.
fn entered(ui: Ui) {
    log::info!("iptv pip: entered");
    ui.panels.update(|p| p.hide());
    ui.play_bar.update(iptv_core::play_bar::PlayBar::hide);
    crate::chooser::close(ui);
    crate::menu_view::close(ui);
    if ui.channels.with_untracked(Option::is_some) {
        crate::channels_view::close(ui);
    }
    // The grid leaves to its active tile; a tile watched full screen ends
    // the session (its hidden tiles stop).
    if untracked(|| crate::multiview::is_open(ui)) {
        crate::multiview::exit(ui);
    } else {
        crate::multiview::close(ui);
    }
}

/// The host's `pipAction` event: one of the window's buttons.
pub fn on_action(ui: Ui, action: PipAction) {
    log::info!("iptv pip: {}", action.wire());
    match action {
        PipAction::Play => ui.paused.set(false),
        PipAction::Pause => ui.paused.set(true),
        PipAction::Next => crate::player_keys::zap(ui, 1),
        PipAction::Previous => crate::player_keys::zap(ui, -1),
    }
}
