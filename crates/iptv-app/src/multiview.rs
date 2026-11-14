//! Multiview in the app: the play bar's or the control row's Multiview
//! turns the playing channel into the first tile and opens a searchable
//! group picker for the next one; each tile plays on its own player surface
//! (`player.rs`, `video.rs`; surface 0 is the full-screen player). The
//! state machine is `iptv_core::multiview`; this runs it, starts and stops
//! the surfaces its tiles name, mutes all but the active one, places the
//! tiles ([`tile_rect`]: the core's layout less a gap), and leaves to full
//! screen. `multiview_view.rs` draws it.
//!
//! Keys: the arrows, OK (the tile's menu), Long OK where keys report their
//! release (enlarge/reduce), Back. A pointer (desktop, tablet; never a TV)
//! shows the pointer chrome while it moves or after a tap, on the play
//! bar's timer (`ui.play_bar`, which does not show itself in multiview): a
//! toolbar (Add screen, Exit multiview) and each tile's remove and
//! enlarge buttons. A long press on a tile enlarges or reduces it.
//!
//! The menu's Search and add opens the search screen over the grid to
//! pick (`search_view/pick.rs`), as Add screen opens `multiview_picker`;
//! Pause holds that tile's player alone (`video.rs`'s `paused`, and its
//! stall checks), and a pause made full screen on a detour stays with
//! the tile when the grid comes back.
//!
//! Full screen is a detour: the session waits (its other tiles keep
//! playing, hidden and muted) while that tile plays full screen with the
//! normal player panels and play bar; Back, or the play bar's / control
//! row's Multiview, brings the grid back as it was. The session ends when
//! the full-screen channel changes (a zap, the channel list, the guide,
//! the history), catch-up starts, or full screen is left; then only that
//! channel plays on.
//!
//! Limits: the device's decoders ([`DECODERS`]) and the streams the
//! account allows at once (an Xtream account's `max_connections`, from its
//! last import). Nothing else of the app opens streams (no recordings), so
//! the whole allowance is multiview's: adding the stream that goes past it
//! asks first.

mod lifecycle;
mod pointer;

use iptv_core::multiview::{Key, Limits, MAX_SURFACES, Multiview, Outcome};
use iptv_core::play_bar::PlayBar;
use iptv_nav::Action;
use nsh_core::prelude::*;

use crate::{Playback, Ui, ViewMode};

pub use lifecycle::{bind, follow};
pub use pointer::{
    add, choose, chrome_shows, dismiss, exit, long_press, remove, tap, toggle_enlarge,
};

pub type State = Multiview<usize>;

/// Streams this device decodes at once (setting-ready): cheap TV boxes
/// manage two, desktops four.
pub const DECODERS: usize = if cfg!(target_os = "android") { 2 } else { 4 };

/// Space around each tile, px (twice this between two).
const GAP: f32 = 3.0;

/// How long the key hint stays.
const HINT_MS: u64 = 5_000;

/// The streams the shown source's account allows at once, if known: the
/// failure card names it when the provider refuses a stream.
pub fn account_limit() -> Option<u32> {
    crate::runtime().account_limit()
}

/// The source on screen changed its account's allowance (`data.rs`).
pub fn set_account_streams(allowed: Option<u32>) {
    crate::runtime().update_account_limit(allowed);
}

/// A multiview session exists (its grid shows, or it waits for a detour).
pub fn session(ui: Ui) -> bool {
    ui.multiview.with(Option::is_some)
}

/// The grid shows (a session, not on a detour): it owns the keys and taps.
pub fn is_open(ui: Ui) -> bool {
    ui.multiview
        .with(|m| m.as_ref().is_some_and(|m| m.detour().is_none()))
}

/// A tile plays full screen while the grid waits.
pub fn detour(ui: Ui) -> bool {
    ui.multiview
        .with(|m| m.as_ref().is_some_and(|m| m.detour().is_some()))
}

/// The channel list picks for multiview.
pub fn picking(ui: Ui) -> bool {
    ui.multiview
        .with_untracked(|m| m.as_ref().is_some_and(|m| m.pick().is_some()))
}

/// Where the channel list starts while picking: the active tile's channel.
pub fn pick_from(ui: Ui) -> Option<usize> {
    ui.multiview.with_untracked(|m| {
        m.as_ref()
            .filter(|m| m.pick().is_some())
            .and_then(|m| m.tiles().get(m.active_index()))
            .map(|t| t.channel)
    })
}

/// Whether `surface` is heard: the active tile's (on a detour that is
/// surface 0), or the full-screen one outside multiview.
pub fn audible(ui: Ui, surface: usize) -> bool {
    ui.multiview.with(|m| match m {
        None => surface == 0,
        Some(m) => m.tiles().get(m.active_index()).map(|t| t.surface) == Some(surface),
    })
}

/// Surfaces 1–3 exist while a tile plays on them (on a detour too: hidden).
pub fn mounted(ui: Ui, surface: usize) -> bool {
    surface > 0
        && ui
            .multiview
            .with(|m| m.as_ref().is_some_and(|m| m.tile_on(surface).is_some()))
}

/// Tile `index`'s rectangle (px) in the window.
pub fn tile_rect(ui: Ui, index: usize) -> Option<(f32, f32, f32, f32)> {
    let layout = ui.multiview.with(|m| {
        m.as_ref()
            .filter(|m| index < m.tiles().len())
            .map(State::layout)
    })?;
    let w = window();
    let (width, height) = (w.width.0, w.height.0);
    let r = layout.rect(index, width / height.max(1.0));
    Some((
        r.x * width + GAP,
        r.y * height + GAP,
        (r.w * width - 2.0 * GAP).max(0.0),
        (r.h * height - 2.0 * GAP).max(0.0),
    ))
}

/// Where `surface` shows while the grid is open; `None` without a tile.
pub fn surface_rect(ui: Ui, surface: usize) -> Option<(f32, f32, f32, f32)> {
    let tile = ui
        .multiview
        .with(|m| m.as_ref().and_then(|m| m.tile_on(surface)))?;
    tile_rect(ui, tile)
}

/// The play bar's or the control row's Multiview: the playing channel
/// becomes the first tile (live), and adding the next one starts at once.
/// On a detour it goes back to the grid.
pub fn open(ui: Ui) {
    if untracked(|| detour(ui)) {
        resume(ui);
        return;
    }
    let Some(channel) = ui.playing.get_untracked() else {
        return;
    };
    if untracked(|| session(ui)) {
        return;
    }
    if ui.playback.get_untracked() != Playback::Live {
        ui.play(channel, Playback::Live);
    }
    ui.paused.set(false);
    ui.panels.update(|p| p.hide());
    crate::channels_view::close(ui);
    let limits = Limits {
        decoders: DECODERS,
        connections: crate::runtime().account_limit(),
    };
    log::info!(
        "iptv multiview: open (device {} streams, account {:?})",
        limits.decoders,
        limits.connections
    );
    ui.multiview.set(Some(Multiview::open(channel, limits)));
    apply(ui, |m, _| m.add());
}

/// Ends the session at once (another playlist, stop, sleep): the other
/// surfaces stop. Surface 0 is the caller's.
pub fn close(ui: Ui) {
    if !untracked(|| session(ui)) {
        return;
    }
    log::info!("iptv multiview: closed");
    end(ui);
}

fn end(ui: Ui) {
    for surface in 1..MAX_SURFACES {
        crate::player::stop_surface(ui, surface);
    }
    ui.multiview.set(None);
}

/// The remote while the grid owns it (`input.rs`).
pub fn key(ui: Ui, action: Action) {
    let key = match action {
        Action::MoveUp => Key::Up,
        Action::MoveDown => Key::Down,
        Action::MoveLeft => Key::Left,
        Action::MoveRight => Key::Right,
        Action::Select => Key::Ok,
        Action::ToggleEnlarge => Key::LongOk,
        Action::ShowMenu => Key::Menu,
        Action::Back => Key::Back,
        Action::Stop => return crate::player_keys::stop_playback(ui),
        _ => return,
    };
    apply(ui, |m, failed| m.key(key, failed));
}

/// Back (`lib.rs`): closes a dialog, else leaves to the active tile.
pub fn back(ui: Ui) -> bool {
    apply(ui, |m, failed| m.key(Key::Back, failed));
    true
}

/// Back on a detour: the grid again, as it was; paused full screen, its
/// tile stays paused (the grid's pause is the tile's own).
pub fn resume(ui: Ui) -> bool {
    log::info!("iptv multiview: back to the grid");
    ui.panels.update(|p| p.hide());
    let paused = ui.paused.get_untracked();
    ui.paused.set(false);
    ui.multiview.update(|m| {
        if let Some(m) = m.as_mut() {
            if let Some(d) = m.detour() {
                m.set_paused(d, paused);
            }
            m.resume();
        }
    });
    true
}

/// Whether the tile on `surface` is paused (the menu's Pause: that player
/// alone holds).
pub fn paused(ui: Ui, surface: usize) -> bool {
    ui.multiview.with(|m| {
        m.as_ref()
            .and_then(|m| m.tile_on(surface).map(|t| m.tiles()[t].paused))
            .unwrap_or(false)
    })
}

/// The channel list (or the search screen picking for multiview) chose
/// `channel`; `false` when it was not picking for it.
pub fn picked(ui: Ui, channel: usize) -> bool {
    if !picking(ui) {
        return false;
    }
    log::info!("iptv multiview: picked {channel}");
    let schedule = crate::schedule();
    let all = schedule
        .as_any()
        .and_then(|s| s.downcast_ref::<iptv_guide::combined::CombinedSchedule>());
    apply(ui, |m, _| {
        let exceeded = all.and_then(|all| {
            let (source, _) = all.channel_source(channel)?;
            let allowed = crate::data::combined::connection_limit(
                &crate::runtime(),
                crate::runtime::model::SourceId(source.id),
            )?;
            let replacing = match m.pick() {
                Some(iptv_core::multiview::Pick::Replace(tile)) => Some(tile),
                _ => None,
            };
            let count = m
                .tiles()
                .iter()
                .enumerate()
                .filter(|(at, t)| {
                    Some(*at) != replacing
                        && all
                            .channel_source(t.channel)
                            .is_some_and(|(s, _)| s.id == source.id)
                })
                .count();
            (count as u32 >= allowed).then_some(allowed)
        });
        m.picked_with_warning(channel, exceeded)
    });
    true
}

/// The channel list (or the search screen picking for multiview) closed,
/// with or without a choice.
pub fn list_closed(ui: Ui) {
    if picking(ui) {
        ui.multiview
            .update(|m| m.iter_mut().for_each(State::cancel_pick));
    }
}

/// Runs a step, starts and stops the surfaces whose channel changed, and
/// does what the step asks.
fn apply(ui: Ui, f: impl FnOnce(&mut State, &dyn Fn(usize) -> bool) -> Outcome<usize>) {
    let failed = move |surface| crate::player::surface_failed(ui, surface);
    let mut step = None;
    ui.multiview.update(|m| {
        if let Some(m) = m.as_mut() {
            let before = m.assignments();
            let outcome = f(m, &failed);
            step = Some((before, m.assignments(), outcome));
        }
    });
    let Some((before, after, outcome)) = step else {
        return;
    };
    for surface in 0..MAX_SURFACES {
        if before[surface] == after[surface] {
            continue;
        }
        match after[surface] {
            Some(channel) => {
                log::info!("iptv multiview: surface {surface} plays {channel}");
                crate::player::start_surface(ui, surface, channel);
            }
            None => {
                log::info!("iptv multiview: surface {surface} stops");
                crate::player::stop_surface(ui, surface);
            }
        }
    }
    match outcome {
        Outcome::Stay => {}
        Outcome::PickChannel => {
            ui.play_bar.update(PlayBar::hide);
            crate::multiview_picker::open(ui);
        }
        Outcome::SearchChannel => {
            ui.play_bar.update(PlayBar::hide);
            crate::search_view::open_to_pick(ui);
        }
        Outcome::Retry(surface) => crate::player::retry_surface(ui, surface),
        Outcome::FullScreen => {
            log::info!("iptv multiview: a tile full screen, the grid waits");
            if ui.keys_shown() {
                ui.show_playing();
            }
        }
        Outcome::Leave { channel, surface } => leave(ui, channel, surface),
    }
}

/// Leaves multiview: `channel` (on `surface`) plays full screen on
/// surface 0, the other surfaces stop.
fn leave(ui: Ui, channel: usize, surface: usize) {
    log::info!("iptv multiview: leaving to surface {surface}");
    end(ui);
    if surface != 0 || ui.playing.get_untracked() != Some(channel) {
        crate::lists::show_channel(ui, channel);
        ui.play(channel, Playback::Live);
    }
    if ui.mode.get_untracked() == ViewMode::Fullscreen && ui.keys_shown() {
        ui.show_playing();
    }
}
