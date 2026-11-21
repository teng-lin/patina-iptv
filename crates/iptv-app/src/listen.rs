//! Which player is heard (#135): exactly one at a time, chosen with a
//! visible Listen to this and never moved by focus. The state is the
//! core's: multiview's heard tile (`iptv_core::multiview`) and the
//! desktop's player windows (`iptv_core::playback_windows`); this says
//! what a player shows and acts on its Listen to this.
//!
//! While several players play at once ([`choice`]) each shows whether it
//! is heard: multiview's tiles a speaker or a muted badge
//! (`multiview_view.rs`), a player window its title ("· muted") and its
//! play bar, a compact picture-in-picture window (whose title is fixed) a
//! badge in its corner (`desktop/pip_window.rs`). Every one not heard
//! offers Listen to this: a tile's menu and its pointer button, a
//! window's play bar or compact controls, the main player's play bar.

use nsh_core::prelude::*;

use crate::Ui;

/// Several players play at once: which one is heard shows, and the ones
/// not heard offer Listen to this. A detour (one tile full screen) shows
/// one player only.
pub fn choice(ui: Ui) -> bool {
    ui.multiview.with(|m| {
        m.as_ref()
            .is_some_and(|m| m.detour().is_none() && m.tiles().len() > 1)
    }) || ui.player_windows.with(|w| !w.sessions().is_empty())
}

/// Whether `ui`'s own player (the main one, or a window's) is the heard
/// one, its own Mute aside.
pub fn heard(ui: Ui) -> bool {
    crate::multiview::audible(ui, ui.player_surface)
}

/// Whether `ui`'s own player sounds: heard and not muted by its Mute.
pub fn sounding(ui: Ui) -> bool {
    match ui.player_surface {
        0 => heard(ui) && !crate::media::muted(ui),
        surface => ui.player_windows.with(|w| {
            w.on_surface(surface)
                .is_some_and(|s| w.audible() == Some(s.id) && !s.volume.muted)
        }),
    }
}

/// Listen to this on `ui`'s own player (a window's, or the main one): its
/// sound plays, every other player's is muted.
pub fn listen(ui: Ui) {
    let surface = ui.player_surface;
    log::info!("iptv sound: listening to surface {surface}");
    ui.touched();
    if surface == 0 {
        batch(|| {
            ui.player_windows.update(|w| w.listen_to_main());
            if ui.media.volume.with_untracked(|v| v.muted) {
                ui.media.volume.update(|v| v.muted = false);
            }
        });
        return;
    }
    let id = ui
        .player_windows
        .with_untracked(|w| w.on_surface(surface).map(|s| s.id));
    if let Some(id) = id {
        ui.player_windows.update(|w| w.listen(id));
    }
}
