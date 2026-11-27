//! The host's audio interruptions (#88): Android's `audio` player event
//! (the audio focus lost or back, the headphones' route gone) applied by
//! the core's policy (`iptv_core::interruption`) to the player that is
//! heard: the full-screen one, or multiview's heard tile (the others are
//! muted; focus never moves the sound, #135). Desktop player windows get
//! no such event. It only pauses and resumes: a stream the viewer stopped stays
//! stopped, and resuming opens no stream.

use iptv_player_mpv::AudioChange;
use iptv_presentation::interruption::Response;
use nsh_core::prelude::*;

use crate::Ui;

/// The heard player: its surface, and multiview's tile on it while the
/// grid shows (its pause is the tile's own; otherwise `ui.playback.paused`).
#[derive(Clone, Copy, PartialEq)]
struct Heard {
    surface: usize,
    tile: Option<usize>,
}

fn heard(ui: Ui) -> Heard {
    let surface = (0..crate::playback::player::SURFACES)
        .find(|&s| crate::playback::multiview::audible(ui, s))
        .unwrap_or(0);
    let tile = if crate::playback::multiview::is_open(ui) {
        ui.playback
            .multiview
            .with(|m| m.as_ref().and_then(|m| m.tile_on(surface)))
    } else {
        None
    };
    Heard { surface, tile }
}

fn paused(ui: Ui, heard: Heard) -> bool {
    match heard.tile {
        Some(_) => crate::playback::multiview::paused(ui, heard.surface),
        None => ui.playback.paused.get(),
    }
}

fn set_paused(ui: Ui, heard: Heard, paused: bool) {
    match heard.tile {
        Some(index) => ui.playback.multiview.update(|m| {
            if let Some(m) = m.as_mut() {
                m.set_paused(index, paused);
            }
        }),
        None => ui.playback.paused.set(paused),
    }
}

/// The policy's identity for what `heard` plays: its surface and its
/// stream's token (0: nothing open).
fn stream(ui: Ui, heard: Heard) -> u64 {
    let s = ui.playback.player.stream(heard.surface);
    if s.source.with(Option::is_none) {
        return 0;
    }
    ((heard.surface as u64) << 56) | (s.token.get() & ((1 << 56) - 1))
}

/// The host reported `change` on `surface`'s player. Every mounted player
/// reports the one host-wide change: surface 0's report is applied.
pub fn on_change(ui: Ui, surface: usize, change: AudioChange) {
    if surface != 0 {
        return;
    }
    untracked(|| {
        let heard = heard(ui);
        let id = stream(ui, heard);
        let playing = id != 0 && !paused(ui, heard);
        let mut response = None;
        ui.playback
            .interruptions
            .update(|p| response = p.report(change.interruption(), id, playing));
        log::info!(
            "iptv audio: {} on surface {} -> {response:?}",
            change.wire(),
            heard.surface
        );
        match response {
            Some(Response::Pause) => set_paused(ui, heard, true),
            Some(Response::Resume) => set_paused(ui, heard, false),
            None => {}
        }
    });
}

/// The viewer acted on playback (played, paused, sought, zapped, chose
/// something, stopped): a call's pause is theirs now, and the audio focus
/// coming back leaves playback as it is. Called as each explicit action is
/// accepted, before any delayed work it starts.
pub fn viewer_acted(ui: Ui) {
    ui.playback.interruptions.update(|p| p.viewer_acted());
}

/// Changes nobody reported (a tile's own pause, another stream, the heard
/// tile changing) hand playback back to the viewer too.
pub fn bind(ui: Ui) {
    // Only a change of what is heard or its pause: not every grid move.
    let now = computed(move || {
        let heard = heard(ui);
        (paused(ui, heard), stream(ui, heard))
    });
    effect(move || {
        let (paused, id) = now.get();
        ui.playback.interruptions.update(|p| p.observed(paused, id));
    });
}
