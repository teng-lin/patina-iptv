//! The host's audio interruptions (#88): Android's `audio` player event
//! (the audio focus lost or back, the headphones' route gone) applied to
//! the main player by the core's policy (`iptv_core::interruption`). It
//! only pauses and resumes: a stream the viewer stopped stays stopped, and
//! resuming opens no stream.

use iptv_core::interruption::Response;
use iptv_player_mpv::AudioChange;
use nsh_core::prelude::*;

use crate::Ui;

/// The host reported `change` on `surface`'s player. Only the main
/// player's (surface 0) is applied: every mounted player reports the one
/// host-wide change.
pub fn on_change(ui: Ui, surface: usize, change: AudioChange) {
    if surface != 0 {
        return;
    }
    let main = ui.player.main();
    let open = main.source.with_untracked(Option::is_some);
    let token = if open { main.token.get_untracked() } else { 0 };
    let playing = open && !ui.paused.get_untracked();
    let mut response = None;
    ui.interruptions
        .update(|p| response = p.report(change.interruption(), token, playing));
    log::info!("iptv audio: {} -> {response:?}", change.wire());
    match response {
        Some(Response::Pause) => ui.paused.set(true),
        Some(Response::Resume) => ui.paused.set(false),
        None => {}
    }
}

/// A pause or play the viewer made, or another stream, hands playback
/// back to them: the audio focus returning leaves it as it is.
pub fn bind(ui: Ui) {
    effect(move || {
        let main = ui.player.main();
        let token = if main.source.with(Option::is_some) {
            main.token.get()
        } else {
            0
        };
        let paused = ui.paused.get();
        ui.interruptions.update(|p| p.observed(paused, token));
    });
}
