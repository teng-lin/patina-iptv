//! Multiview's session against the rest of the app: the data changing
//! under its tiles, full screen being left, a detour going elsewhere, and
//! the key hint's time.

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
