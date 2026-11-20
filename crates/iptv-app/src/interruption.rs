//! The host's audio interruptions (#88): Android's `audio` player event
//! (the audio focus lost or back, the headphones' route gone) applied to
//! the main player by the core's policy (`iptv_core::interruption`). It
//! only pauses and resumes: a stream the viewer stopped stays stopped, and
//! resuming opens no stream.

use std::cell::{Cell, RefCell};

use iptv_core::interruption::{Interruptions, Response};
use iptv_player_mpv::AudioChange;
use nsh_core::prelude::*;

use crate::Ui;

thread_local! {
    static POLICY: RefCell<Interruptions> = RefCell::default();
    /// The pause state and stream this policy last set: seen again, the
    /// change is ours, not the viewer's.
    static APPLIED: Cell<Option<(bool, u64)>> = const { Cell::new(None) };
}

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
    let response = POLICY.with(|p| p.borrow_mut().report(change.interruption(), token, playing));
    log::info!("iptv audio: {} -> {response:?}", change.wire());
    let paused = match response {
        Some(Response::Pause) => true,
        Some(Response::Resume) => false,
        None => return,
    };
    APPLIED.with(|a| a.set(Some((paused, token))));
    ui.paused.set(paused);
}

/// A pause or play the viewer made, or another stream, hands playback
/// back to them: the audio focus returning leaves it as it is.
pub fn bind(ui: Ui) {
    effect(move || {
        let now = (ui.paused.get(), ui.player.main().token.get());
        if APPLIED.with(|a| a.take()) != Some(now) {
            POLICY.with(|p| p.borrow_mut().viewer_acted());
        }
    });
}
