//! Recovering a stream: errors and states from the host go through the
//! core's health machine (`iptv_core::playback::Health`), whose retries run
//! here on timers. A live channel retries for as long as it stays selected
//! (fast, then slowly, then slowly behind the failure card); a retry that
//! comes due while the Android app is away waits for its return
//! ([`resume`]); a catch-up programme reopens where it was.

use std::time::Duration;

use iptv_core::playback::{ErrorKind, PlayerState, Retry};
use nsh_core::prelude::*;

use super::{later, open, target};
use crate::{Playback, Ui};

pub(super) fn fail(ui: Ui, surface: usize, session: u64, kind: ErrorKind, detail: String) {
    let health = ui.player.stream(surface).health;
    if health.with_untracked(|h| h.session()) != session {
        return;
    }
    log::warn!("iptv player {surface}: {kind:?} ({detail})");
    let now = ui.now_ms();
    let mut retry = None;
    let carded = health.with_untracked(|h| h.failure().is_some());
    health.update(|h| retry = h.on_error(kind, detail, now));
    if !carded && let Some(f) = health.with_untracked(|h| h.failure().cloned()) {
        log::warn!(
            "iptv player {surface}: failure card ({:?}, retrying: {})",
            f.kind,
            f.retrying
        );
    }
    if let Some(r) = retry {
        schedule_retry(ui, surface, r);
    }
}

fn schedule_retry(ui: Ui, surface: usize, retry: Retry) {
    let wait = retry.at_ms.saturating_sub(ui.now_ms());
    log::info!("iptv player {surface} retry {} in {wait} ms", retry.attempt);
    watch_indicator(ui, surface);
    later(ui, Duration::from_millis(wait), move || {
        run_retry(ui, surface, retry)
    });
}

/// A retry's time came: it reopens the stream, unless the app is away
/// (then it waits for [`resume`]).
fn run_retry(ui: Ui, surface: usize, retry: Retry) {
    let stream = ui.player.stream(surface);
    if super::stopped_in_background(lifecycle()) {
        log::info!(
            "iptv player {surface}: retry {} waits for the app",
            retry.attempt
        );
        stream.parked.set(Some(retry));
        return;
    }
    let now = ui.now_ms();
    let mut due = false;
    stream.health.update(|h| due = h.due(retry, now));
    if due {
        if surface == 0 {
            resume_position(ui);
        }
        open(ui, surface, retry.session);
    }
}

/// The app came back: retries that came due while it was away run now.
pub(super) fn resume(ui: Ui, surface: usize) {
    let stream = ui.player.stream(surface);
    if let Some(retry) = stream.parked.get_untracked() {
        stream.parked.set(None);
        run_retry(ui, surface, retry);
    }
}

/// A catch-up programme that played reopens where it was, not where it
/// was last opened.
fn resume_position(ui: Ui) {
    let Playback::CatchUp { start, offset } = ui.playback.get_untracked() else {
        return;
    };
    let played = ui.player.main().position_ms.get_untracked() / 1000;
    if played > 0 {
        let offset = offset.saturating_add(i32::try_from(played).unwrap_or(i32::MAX));
        ui.playback.set(Playback::CatchUp { start, offset });
    }
}

/// Shows the waiting indicator once a quiet reconnect has taken long
/// enough (`Health::indicator`).
fn watch_indicator(ui: Ui, surface: usize) {
    let health = ui.player.stream(surface).health;
    let Some(at) = health.with_untracked(|h| h.indicator_due()) else {
        return;
    };
    let wait = at.saturating_sub(ui.now_ms());
    later(ui, Duration::from_millis(wait), move || {
        if health.with_untracked(|h| h.indicator_due()) == Some(at) {
            // Nothing changed but the time: the overlays read it again.
            health.update(|_| {});
        }
    });
}

/// Arms one stall check at the current deadline (each wait gets its own; a
/// check whose deadline moved does nothing).
pub(super) fn watch_stall(ui: Ui, surface: usize) {
    let health = ui.player.stream(surface).health;
    let Some(at) = health.with_untracked(|h| h.stall_deadline()) else {
        return;
    };
    let wait = at.saturating_sub(ui.now_ms());
    later(ui, Duration::from_millis(wait), move || {
        let now = ui.now_ms();
        let mut retry = None;
        health.update(|h| retry = h.tick(now));
        if let Some(r) = retry {
            log::warn!("iptv player {surface}: stalled");
            schedule_retry(ui, surface, r);
        }
    });
}

/// The host's `state` event from `surface`.
pub fn on_state(ui: Ui, surface: usize, state: PlayerState) {
    let health = ui.player.stream(surface).health;
    let now = ui.now_ms();
    let before = health.with_untracked(|h| h.stall_deadline());
    let mut retry = None;
    health.update(|h| retry = h.on_state(state, now));
    if let Some(r) = retry {
        schedule_retry(ui, surface, r);
    } else if health.with_untracked(|h| h.stall_deadline()) != before {
        watch_stall(ui, surface);
    }
}

/// The host's `error` event from `surface`: Media3's code name and the
/// HTTP status.
pub fn on_error(ui: Ui, surface: usize, code: &str, http: Option<u16>) {
    let kind = ErrorKind::from_player_code(code, http);
    let session = ui
        .player
        .stream(surface)
        .health
        .with_untracked(|h| h.session());
    let detail = match http {
        Some(status) => format!("{code} {status}"),
        None => code.to_owned(),
    };
    fail(ui, surface, session, kind, detail);
}

/// The full-screen player shows its failure card (retries may go on
/// behind it).
pub fn failed(ui: &Ui) -> bool {
    ui.playing.get_untracked().is_some() && surface_failed(*ui, 0)
}

/// `surface` shows its failure card.
pub fn surface_failed(ui: Ui, surface: usize) -> bool {
    ui.player
        .stream(surface)
        .health
        .with_untracked(|h| h.failure().is_some())
}

/// The viewer's "Try again": at once, with the backoff reset.
pub fn retry(ui: Ui) {
    retry_surface(ui, 0);
}

/// "Try again" on `surface`.
pub fn retry_surface(ui: Ui, surface: usize) {
    if target(ui, surface).is_none() {
        return;
    }
    let now = ui.now_ms();
    let mut session = 0;
    let stream = ui.player.stream(surface);
    stream.parked.set(None);
    stream.health.update(|h| session = h.retry_now(now));
    open(ui, surface, session);
}
