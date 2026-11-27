//! Multiview in the app: the play bar's or the control row's Multiview
//! turns the playing channel into the first tile and opens a searchable
//! group picker for the next one; each tile plays on its own player surface
//! (`player.rs`, `video.rs`; surface 0 is the full-screen player). The
//! state machine is `iptv_core::multiview`; this runs it, starts and stops
//! the surfaces its tiles name, mutes all but the heard one (which focus
//! never moves: the tile menu's or the pointer chrome's Listen to this
//! does, `listen.rs`; #135; unless `player.sound_follows_tile` is on,
//! which each step hands the core, #179), places the
//! tiles ([`tile_rect`]: the core's layout less a gap), and leaves to full
//! screen. `multiview_view.rs` draws it.
//!
//! Keys: the arrows, OK (the tile's menu), Long OK where keys report their
//! release (enlarge/reduce), Back. A pointer (desktop, tablet; never a TV)
//! shows the pointer chrome while it moves or after a tap, on the play
//! bar's timer (`ui.playback.play_bar`, which does not show itself in multiview, and
//! which keeps the chrome until the next tap while a screen reader runs): a
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
//! Limits: the tiles the device and its window hold ([`tiles`]) and the
//! streams the account allows at once (an Xtream account's
//! `max_connections`, from its last import). Nothing else of the app opens
//! streams (no recordings), so the whole allowance is multiview's: adding
//! the stream that goes past it asks first. A window or a device that no
//! longer offers multiview collapses the session (`lifecycle::collapse`).

mod lifecycle;
mod pointer;

use iptv_core::multiview::MAX_SURFACES;
use iptv_nav::Action;
use iptv_presentation::capabilities::{Availability, Device, MultiviewOffer};
use iptv_presentation::multiview::{Key, Limits, Multiview, Outcome};
use iptv_presentation::play_bar::PlayBar;
use nsh_core::prelude::*;

use crate::{Playback, Ui, ViewMode};

pub use lifecycle::{bind, follow};
pub use pointer::{
    add, choose, chrome_shows, dismiss, exit, listen, long_press, remove, tap, toggle_enlarge,
};

pub type State = Multiview<usize>;

/// How multiview is offered in `ui`'s window now (reactive): the device's
/// signals, whether the window is wider than tall, and the streams the
/// shown source's account allows (#279).
pub fn offer(ui: Ui) -> MultiviewOffer {
    offer_on(ui, crate::device::current())
}

/// [`offer`] on `device`'s signals.
pub fn offer_on(ui: Ui, device: Device) -> MultiviewOffer {
    let w = nsh_core::prelude::window();
    iptv_presentation::capabilities::multiview_offer(
        device,
        w.width.0 > w.height.0,
        account_streams(ui),
        crate::capture_limits::now().metered,
    )
}

/// The streams the playing channel's account allows at once, when it
/// says: the shown source's, or in All playlists the playing channel's
/// own source's (`data::combined::connection_limit`).
fn account_streams(ui: Ui) -> Option<u32> {
    ui.app.account_streams.get().or_else(|| {
        let channel = ui.playback.playing.get()?;
        let schedule = crate::runtime_owner::schedule();
        let all = schedule
            .as_any()?
            .downcast_ref::<iptv_guide::combined::CombinedSchedule>()?;
        let (source, _) = all.channel_source(channel)?;
        crate::data::combined::connection_limit(
            &crate::runtime_owner::runtime(),
            crate::runtime::model::SourceId(source.id),
        )
    })
}

/// The reason multiview is drawn disabled in `ui`'s window now (a TV at
/// the account's limit), for the entry points' accessibility descriptions.
pub fn disabled_reason(ui: Ui) -> Option<&'static str> {
    match offer(ui) {
        MultiviewOffer::Disabled(why) => Some(crate::capabilities::reason(why)),
        _ => None,
    }
}

/// Whether multiview can open (or, on a detour, its grid come back) now.
/// A session already open is not refused for a reason that disables it
/// (the account's limit): its streams were admitted.
pub fn availability(ui: Ui) -> Availability {
    if ui.cast.remote.get() {
        return Availability::Unavailable(iptv_presentation::capabilities::Reason::Casting);
    }
    let mut offer = offer(ui);
    if let MultiviewOffer::Disabled(_) = offer
        && session(ui)
    {
        offer = MultiviewOffer::Tiles(crate::device::host().decoder_slots());
    }
    iptv_presentation::capabilities::multiview(
        offer,
        ui.playback.player_surface == 0,
        ui.playback.playing.get().is_some(),
        ui.playback
            .player_windows
            .with(|windows| !windows.sessions().is_empty()),
    )
}

/// The tiles multiview opens with now: a phone's, a tablet's and a TV's
/// two, a desktop's four, never more than the device decodes at once
/// (`crate::multiview::offer`). Reactive, as it follows the
/// device's signals: a session collapses when its admitted decoder budget
/// shrinks or the window no longer holds it
/// (`lifecycle::collapse`).
pub fn tiles(ui: Ui) -> usize {
    match crate::multiview::offer(ui) {
        MultiviewOffer::Tiles(tiles) => tiles,
        _ => 1,
    }
}

/// Space around each tile, px (twice this between two).
const GAP: f32 = 3.0;

/// How long the key hint stays.
const HINT_MS: u64 = 5_000;

/// The streams the shown source's account allows at once, if known: the
/// failure card names it when the provider refuses a stream.
pub fn account_limit() -> Option<u32> {
    crate::runtime_owner::runtime().account_limit()
}

/// The source on screen changed its account's allowance (`data.rs`).
pub fn set_account_streams(allowed: Option<u32>) {
    crate::runtime_owner::runtime().update_account_limit(allowed);
}

/// A multiview session exists (its grid shows, or it waits for a detour).
pub fn session(ui: Ui) -> bool {
    ui.playback.multiview.with(Option::is_some)
}

/// The grid shows (a session, not on a detour): it owns the keys and taps.
pub fn is_open(ui: Ui) -> bool {
    ui.playback
        .multiview
        .with(|m| m.as_ref().is_some_and(|m| m.detour().is_none()))
}

/// A tile plays full screen while the grid waits.
pub fn detour(ui: Ui) -> bool {
    ui.playback
        .multiview
        .with(|m| m.as_ref().is_some_and(|m| m.detour().is_some()))
}

/// The channel list picks for multiview.
pub fn picking(ui: Ui) -> bool {
    ui.playback
        .multiview
        .with_untracked(|m| m.as_ref().is_some_and(|m| m.pick().is_some()))
}

/// Where the channel list starts while picking: the active tile's channel.
pub fn pick_from(ui: Ui) -> Option<usize> {
    ui.playback.multiview.with_untracked(|m| {
        m.as_ref()
            .filter(|m| m.pick().is_some())
            .and_then(|m| m.tiles().get(m.active_index()))
            .map(|t| t.channel)
    })
}

/// Whether `surface` is heard: the heard tile's (on a detour surface 0,
/// the one watched full screen), a desktop player window that took the
/// sound, or else the full-screen one.
pub fn audible(ui: Ui, surface: usize) -> bool {
    if let Some(audible) = ui.playback.player_windows.with(|w| w.audible()) {
        return ui
            .playback
            .player_windows
            .with(|w| w.on_surface(surface).is_some_and(|s| s.id == audible));
    }
    ui.playback.multiview.with(|m| match m {
        None => surface == 0,
        Some(m) => m.audible_surface() == surface,
    })
}

/// Surfaces 1–3 exist while a tile plays on them (on a detour too: hidden).
pub fn mounted(ui: Ui, surface: usize) -> bool {
    surface > 0
        && ui
            .playback
            .multiview
            .with(|m| m.as_ref().is_some_and(|m| m.tile_on(surface).is_some()))
}

/// Tile `index`'s rectangle (px) in the window.
pub fn tile_rect(ui: Ui, index: usize) -> Option<(f32, f32, f32, f32)> {
    let layout = ui.playback.multiview.with(|m| {
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
        .playback
        .multiview
        .with(|m| m.as_ref().and_then(|m| m.tile_on(surface)))?;
    tile_rect(ui, tile)
}

/// The play bar's or the control row's Multiview: the playing channel
/// becomes the first tile (live), and adding the next one starts at once.
/// On a detour it goes back to the grid.
pub fn open(ui: Ui) {
    if let Some(reason) = untracked(|| crate::multiview::availability(ui)).reason() {
        crate::settings_view::notify(crate::capabilities::reason(reason));
        return;
    }
    if untracked(|| detour(ui)) {
        resume(ui);
        return;
    }
    let Some(channel) = ui.playback.playing.get_untracked() else {
        return;
    };
    if untracked(|| session(ui)) {
        return;
    }
    if ui.playback.playback.get_untracked() != Playback::Live {
        ui.play(channel, Playback::Live);
    }
    ui.playback.paused.set(false);
    ui.playback.panels.update(|p| p.hide());
    crate::channels_view::close(ui);
    let limits = Limits {
        decoders: untracked(|| tiles(ui)),
        connections: crate::runtime_owner::runtime().account_limit(),
    };
    log::info!(
        "iptv multiview: open (device {} streams, account {:?})",
        limits.decoders,
        limits.connections
    );
    ui.playback
        .multiview
        .set(Some(Multiview::open(channel, limits)));
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
    ui.playback.multiview.set(None);
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
    apply(ui, |m, failed| {
        if key == Key::Ok
            && let Some(iptv_presentation::multiview::Dialog::Warning { focus, .. }) = m.dialog()
            && !can_confirm_pick(m, *focus)
        {
            return m.key(Key::Back, failed);
        }
        m.key(key, failed)
    });
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
    ui.playback.panels.update(|p| p.hide());
    let paused = ui.playback.paused.get_untracked();
    ui.playback.paused.set(false);
    ui.playback.multiview.update(|m| {
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
    if let Some(paused) = ui
        .playback
        .player_windows
        .with(|w| w.on_surface(surface).map(|s| s.paused))
    {
        return paused;
    }
    ui.playback.multiview.with(|m| {
        m.as_ref()
            .and_then(|m| m.tile_on(surface).map(|t| m.tiles()[t].paused))
            .unwrap_or(false)
    })
}

fn can_confirm_pick(state: &State, index: usize) -> bool {
    use iptv_presentation::multiview::{WARNING_CHOICES, WarningChoice};
    WARNING_CHOICES.get(index) != Some(&WarningChoice::AddAnyway)
        || state
            .pending_pick_channel()
            .is_none_or(|&channel| crate::player::allow_guide_channel(channel))
}

/// Whether the picker can close after choosing this channel for multiview.
pub fn picked(ui: Ui, channel: usize) -> bool {
    if !picking(ui) || !crate::player::allow_guide_channel(channel) {
        return false;
    }
    log::info!("iptv multiview: picked {channel}");
    let schedule = crate::runtime_owner::schedule();
    let all = schedule
        .as_any()
        .and_then(|s| s.downcast_ref::<iptv_guide::combined::CombinedSchedule>());
    apply(ui, |m, _| {
        let exceeded = all.and_then(|all| {
            let (source, _) = all.channel_source(channel)?;
            let allowed = crate::data::combined::connection_limit(
                &crate::runtime_owner::runtime(),
                crate::runtime::model::SourceId(source.id),
            )?;
            let replacing = match m.pick() {
                Some(iptv_presentation::multiview::Pick::Replace(tile)) => Some(tile),
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
        ui.playback
            .multiview
            .update(|m| m.iter_mut().for_each(State::cancel_pick));
    }
}

/// Runs a step, starts and stops the surfaces whose channel changed, and
/// does what the step asks.
fn apply(ui: Ui, f: impl FnOnce(&mut State, &dyn Fn(usize) -> bool) -> Outcome<usize>) {
    let failed = move |surface| crate::player::surface_failed(ui, surface);
    // Read at each step, so a change in the settings applies at the next
    // focus move, also in a session already open.
    // Not offered on a phone: there a stored value is kept, unused.
    let follows = crate::settings::flag(iptv_settings::keys::SOUND_FOLLOWS_TILE)
        && iptv_presentation::capabilities::sound_follows_tile(untracked(crate::device::host));
    let mut step = None;
    ui.playback.multiview.update(|m| {
        if let Some(m) = m.as_mut() {
            m.set_sound_follows_focus(follows);
            let before = m.assignments();
            let heard = m.audible_surface();
            let outcome = f(m, &failed);
            if m.audible_surface() != heard {
                log::info!("iptv multiview: surface {} heard", m.audible_surface());
            }
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
            ui.playback.play_bar.update(PlayBar::hide);
            crate::multiview_picker::open(ui);
        }
        Outcome::SearchChannel => {
            ui.playback.play_bar.update(PlayBar::hide);
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
    // The stream surface 0 keeps takes its tile's pause (a call's or
    // unplugged headphones' included: the interruption policy still owns
    // it, as the same stream's pause).
    let kept = surface == 0 && ui.playback.playing.get_untracked() == Some(channel);
    // Set before the session ends, so the pause never lapses between.
    let paused = kept && paused(ui, 0);
    if kept && paused != ui.playback.paused.get_untracked() {
        ui.playback.paused.set(paused);
    }
    end(ui);
    if surface != 0 || ui.playback.playing.get_untracked() != Some(channel) {
        crate::lists::show_channel(ui, channel);
        ui.play(channel, Playback::Live);
    }
    if ui.chrome.mode.get_untracked() == ViewMode::Fullscreen && ui.keys_shown() {
        ui.show_playing();
    }
}
