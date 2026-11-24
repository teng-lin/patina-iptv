//! Multiview's session against the rest of the app: the data changing
//! under its tiles, full screen being left, a detour going elsewhere, the
//! window no longer holding the tiles ([`collapse`]), and the key hint's
//! time.

use std::time::Duration;

use nsh_core::prelude::*;

use super::{HINT_MS, State, end, leave, session};
use crate::{Playback, Ui, ViewMode};

/// The data changed: each tile finds its channel again by key; a tile
/// whose channel went is removed, and with none left nothing plays.
/// `false` without a session.
pub fn follow(ui: Ui) -> bool {
    if !session(ui) {
        return false;
    }
    let schedule = crate::schedule();
    let mut moved = Vec::new();
    let mut gone = Vec::new();
    let mut any = true;
    ui.multiview.update(|m| {
        if let Some(m) = m.as_mut() {
            any = m.retain(|surface, channel| {
                let key = ui.player.stream(surface).key.get_untracked();
                match key.and_then(|k| schedule.find_channel(&k)) {
                    Some(row)
                        if schedule.channel_block(row) == iptv_core::organization::Block::None =>
                    {
                        if row != *channel {
                            *channel = row;
                            moved.push((surface, row));
                        }
                        true
                    }
                    _ => {
                        gone.push(surface);
                        false
                    }
                }
            });
        }
    });
    for (surface, row) in moved {
        if surface == 0 {
            ui.playing.set(Some(row));
        } else {
            ui.player.stream(surface).channel.set(Some(row));
        }
    }
    for surface in gone {
        log::info!("iptv multiview: surface {surface}'s channel left the guide");
        crate::player::stop_surface(ui, surface);
    }
    if !any {
        ui.multiview.set(None);
        ui.mode.set(ViewMode::Guide);
    }
    true
}

/// Multiview belongs to full-screen playback: leaving it (the guide's
/// keys) from the grid keeps the active tile playing; on a detour, leaving
/// full screen or another channel (or catch-up) on surface 0 ends the
/// session with that channel playing on. The key hint goes after a while.
pub fn bind(ui: Ui) {
    effect(move || {
        let mode = ui.mode.get();
        let playing = ui.playing.get();
        let live = ui.playback.get() == Playback::Live;
        untracked(|| {
            let state = ui.multiview.with(|m| {
                m.as_ref().map(|m| {
                    let watched = m.detour().and_then(|d| m.tiles().get(d)).map(|t| t.channel);
                    (m.detour().is_some(), watched, m.active().clone())
                })
            });
            match state {
                Some((true, watched, _)) => {
                    if mode != ViewMode::Fullscreen || !live || playing != watched {
                        log::info!("iptv multiview: the detour went elsewhere; session ends");
                        end(ui);
                    }
                }
                Some((false, _, active)) if mode != ViewMode::Fullscreen => {
                    leave(ui, active.channel, active.surface);
                }
                _ => {}
            }
        });
    });
    // A tablet's window going to split screen, a phone turned upright: the
    // device no longer offers multiview here (#279), so the session
    // collapses to the tile being heard.
    let omitted = computed(move || !crate::capabilities::multiview_offer(ui).shown());
    effect(move || {
        if omitted.get() {
            untracked(|| collapse(ui));
        }
    });
    let hint = computed(move || {
        ui.keys_shown()
            && ui.multiview.with(|m| {
                m.as_ref().is_some_and(|m| {
                    m.hint() && m.detour().is_none() && m.dialog().is_none() && m.pick().is_none()
                })
            })
    });
    effect(move || {
        if hint.get() {
            let _task = set_timeout(Duration::from_millis(HINT_MS), move || {
                if ui
                    .multiview
                    .with_untracked(|m| m.as_ref().is_some_and(State::hint))
                {
                    ui.multiview
                        .update(|m| m.iter_mut().for_each(State::dismiss_hint));
                }
            });
        }
    });
}

/// Multiview is no longer offered while a session is open: the session
/// ends, and the tile being heard plays on full screen. On a detour that
/// is the tile already full screen (surface 0), which plays on untouched.
/// From the grid, the heard tile's stream is kept when it plays on surface
/// 0, the full-screen player (the channel multiview opened with, unless
/// Listen to this moved the sound); a tile heard on another surface plays
/// on in surface 0 instead, as Exit multiview does, since only surface 0
/// is shown full screen and no player view moves. The other surfaces stop.
pub fn collapse(ui: Ui) {
    let heard = ui.multiview.with_untracked(|m| {
        m.as_ref().map(|m| {
            let tile = &m.tiles()[m.heard_index()];
            (m.detour().is_some(), tile.channel, tile.surface)
        })
    });
    match heard {
        None => {}
        Some((true, _, _)) => {
            log::info!("iptv multiview: no longer offered here; the detour plays on");
            end(ui);
        }
        Some((false, channel, surface)) => {
            log::info!("iptv multiview: no longer offered here; surface {surface} heard plays on");
            leave(ui, channel, surface);
        }
    }
}
