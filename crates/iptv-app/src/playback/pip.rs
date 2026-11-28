//! Picture in picture: the playing channel in a small window over other
//! apps, video only. The state is `iptv_presentation::pip`.
//!
//! **Android phones and tablets**: a system window. It opens from the
//! control strip's button, or by itself when the viewer leaves the app
//! (Home) while a channel plays full screen (not the guide's preview) and
//! `player.pip_on_leave` allows it. The window's buttons
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
//! **Android TV** has none, on every build: no control, no setting, no
//! entry on Home (its host reports it unsupported and refuses to enter,
//! `PipHost.kt`). Nor do the iPad and the browser TVs, which have no
//! window yet: where the host has none the control strip and the play bar
//! leave the control out rather than show it disabled.
//!
//! **The desktops** (Linux, macOS) open a window of the app's own, as
//! browsers do (`desktop/pip_window.rs`, `iptv_presentation::pip_window`), from
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
//! Auxiliary desktop players use the same core PiP state and compact controls,
//! changing their own existing window's presentation. Returning restores that
//! player's window; closing stops only its session, never surface zero.
//!
//! As the window opens, the panels, the play bar, the channel list and
//! any chooser or menu close, and multiview leaves to its active tile,
//! which plays on in the window (its other tiles stop).

use iptv_player_mpv::PipAction;
use iptv_presentation::pip::{Change, OnLeave};
use iptv_presentation::pip_window::Exit;
use iptv_settings::keys::PIP_ON_LEAVE;
use nsh_core::prelude::*;

use crate::{Ui, ViewMode};

/// A channel plays full screen: what the window would show.
pub(crate) fn playing(ui: Ui) -> bool {
    ui.chrome.mode.get() == ViewMode::Fullscreen && ui.playback.playing.get().is_some()
}

/// Whether the player shows in the desktops' window of its own now
/// (reading subscribes).
pub fn in_window(playback: crate::PlaybackUi) -> bool {
    playback.pip.with(|p| p.in_window())
}

/// Whether Android's window is the app's own: it then shows only the
/// video, filling it.
pub fn fills_app(playback: crate::PlaybackUi) -> bool {
    playback.pip.with(|p| p.active && !p.separate)
}

/// Whether the app's layers over the video show: not in Android's window.
pub fn ui_shown(playback: crate::PlaybackUi) -> bool {
    playback.pip.with(|p| p.ui_shown())
}

/// Whether this device has the window at all (reading subscribes): the
/// play bar shows its button only then, and the control strip once the
/// host reported (`player_panels::offered`).
pub fn supported(playback: crate::PlaybackUi) -> bool {
    playback.pip.with(|p| p.supported)
}

/// Whether the control strip's button can open the window now.
pub fn can_enter(ui: Ui) -> bool {
    if ui.cast.remote.get() {
        return false;
    }
    let playing = playing(ui);
    ui.playback.pip.with(|p| p.can_enter(playing))
}

/// The desktops: their window can (Android's host reports its own), and
/// closes when nothing plays any more or multiview opens.
pub fn bind(ui: Ui) {
    if !crate::host::DESKTOP {
        // Android reports through its persistent player. Other hosts have no
        // native PiP implementation to acknowledge availability later.
        if !crate::host::ANDROID {
            ui.playback.pip.update(|p| {
                p.report(false, false, false);
            });
        }
        return;
    }
    ui.playback.pip.update(|p| {
        p.separate = true;
        p.report(true, false, false);
    });
    // On the change only: the window may open from multiview, which then
    // leaves to its tile.
    let was = std::cell::Cell::new(false);
    effect(move || {
        let gone =
            ui.playback.playing.get().is_none() || crate::playback::multiview::is_open(ui.playback);
        let became = !was.replace(gone) && gone;
        if became && ui.playback.pip.with_untracked(|p| p.in_window()) {
            log::info!("iptv pip: nothing for the window to show");
            untracked(|| report(ui, false));
        }
    });
}

/// The control strip's button: asks the host to open the window (on the
/// desktops, opens the app's own).
pub fn enter(ui: Ui) {
    if ui.cast.remote.get_untracked() {
        crate::chrome::toast::notify(crate::chrome::i18n::tr(
            "Stop casting to open picture in picture.",
        ));
        return;
    }
    let playing = untracked(|| playing(ui));
    let mut asked = false;
    ui.playback.pip.update(|p| asked = p.request(playing));
    if !asked {
        if let Some(reason) = ui
            .playback
            .pip
            .with_untracked(|p| iptv_presentation::capabilities::pip(p, playing).reason())
        {
            crate::chrome::toast::notify(crate::wiring::capabilities::reason(reason));
        }
        return;
    }
    log::info!("iptv pip: asked to enter");
    if crate::host::DESKTOP {
        report(ui, true);
    }
}

/// The desktops' window closes: back to the app (the video full screen in
/// the main window) or closed (playback stops). False when none is open.
pub fn exit(ui: Ui, how: Exit) -> bool {
    if !ui.playback.pip.with_untracked(|p| p.in_window()) {
        return false;
    }
    log::info!("iptv pip: {how:?}");
    if ui.playback.player_surface != 0 {
        // Auxiliary PiP is a presentation of its own player window. It
        // never returns to, focuses or stops the main player's surface.
        batch(|| {
            report(ui, false);
            if how == Exit::Close {
                ui.chrome
                    .mode
                    .apply(crate::chrome::mode::Action::ReturnToBrowse);
            }
        });
        return true;
    }
    report(ui, false);
    match how {
        Exit::BackToApp => {
            ui.chrome
                .mode
                .apply(crate::chrome::mode::Action::PipBackToApp);
            crate::wiring::navigation::focus_main();
        }
        Exit::Close => crate::playback::player_keys::stop_playback(ui),
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
    ui.app.settings.get();
    let on_leave = OnLeave::from_key(crate::wiring::settings::choice(PIP_ON_LEAVE));
    let playing = playing(ui);
    let (auto, enter, window) = ui
        .playback
        .pip
        .with(|p| (p.auto_enter(on_leave, playing), p.requests, p.in_window()));
    Some(iptv_player_mpv::PipProps {
        auto,
        enter,
        window,
    })
}

thread_local! {
    /// The host's last report of support for a picture-in-picture window.
    static PIP_SUPPORTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn set_supported(supported: bool) {
    PIP_SUPPORTED.set(supported);
}

/// Picture in picture on leaving is offered on an Android handheld whose
/// host says it can open the window.
pub(crate) fn offers_on_leave(host: iptv_core::capabilities::Host) -> bool {
    host.automatic_pip() && PIP_SUPPORTED.get()
}

/// The host's `pip` event.
pub fn on_state(ui: Ui, supported: bool, tv: bool, active: bool) {
    let mut next = ui.playback.pip.get_untracked();
    let change = next.report(supported, tv, active);
    if next == ui.playback.pip.get_untracked() {
        return;
    }
    // Before the signal changes: settings rows that read it then see the
    // setting `player.pip_on_leave` come or go with the window.
    set_supported(next.supported);
    ui.playback.pip.set(next);
    match change {
        Change::Entered => entered(ui),
        Change::Left => log::info!("iptv pip: left"),
        Change::None => log::info!("iptv pip: supported {supported}, tv {tv}"),
    }
}

/// The window opened: one stream, the full-screen player's.
fn entered(ui: Ui) {
    log::info!("iptv pip: entered");
    ui.playback.panels.update(|p| p.hide());
    ui.playback
        .play_bar
        .update(iptv_presentation::play_bar::PlayBar::hide);
    crate::wiring::navigation::close_chooser(ui);
    crate::wiring::navigation::close_menu(ui);
    if ui.playback.channels.with_untracked(Option::is_some) {
        crate::screens::channels_view::close(ui.playback);
    }
    // The grid leaves to its active tile; a tile watched full screen ends
    // the session (its hidden tiles stop).
    if untracked(|| crate::playback::multiview::is_open(ui.playback)) {
        crate::playback::multiview::exit(ui);
    } else {
        crate::playback::multiview::close(ui);
    }
}

/// The host's `pipAction` event: one of the window's buttons.
pub fn on_action(ui: Ui, action: PipAction) {
    log::info!("iptv pip: {}", action.wire());
    // Each accepted action hands playback to the viewer
    // (`interruption.rs`): `toggle_pause` and `Ui::play` say so themselves.
    match action {
        PipAction::Play => crate::playback::player::set_paused(ui, false),
        PipAction::Pause => crate::playback::player::set_paused(ui, true),
        PipAction::Next => crate::playback::player_keys::zap(ui, 1),
        PipAction::Previous => crate::playback::player_keys::zap(ui, -1),
    }
}
