//! Seeking while a program plays from the archive (catch-up).
//!
//! Archive addresses are time-based, so a seek opens the archive again from
//! the new time (`Playback::CatchUp::offset`); that works the same for every
//! provider and container. Presses add up to one target, shown on the info
//! card, and the archive reopens once the keys pause.
//!
//! Steps are the `catchup.*_step` settings (by default fast-forward 30 s,
//! rewind 10 s, one time-bar step 5 min); `catchup.seek_keys` chooses
//! which keys step along the time bar (`player_keys.rs`). A held key steps
//! a few times a second, faster the longer it is held
//! (`iptv_presentation::seek_hold`); OK opens the archive at the target at once.

use std::cell::Cell;
use std::time::Duration;

use iptv_presentation::seek_hold::SeekHold;
use iptv_settings::{Settings, Target, keys};
use nsh_core::prelude::*;

use crate::{Playback, Ui};

/// Seek steps in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Steps {
    pub forward: i32,
    pub rewind: i32,
    pub timebar: i32,
}

/// The steps chosen in `catchup.*_step`.
pub fn steps_of(s: &Settings) -> Steps {
    let secs = |key| i32::try_from(s.duration_ms(key, &Target::Global) / 1000).unwrap_or(i32::MAX);
    Steps {
        forward: secs(keys::FORWARD_STEP),
        rewind: secs(keys::REWIND_STEP),
        timebar: secs(keys::TIMEBAR_STEP),
    }
}

/// The current steps.
pub fn steps() -> Steps {
    steps_of(&crate::settings::get())
}
/// The archive reopens this long after the last seek key.
const COMMIT_AFTER: Duration = Duration::from_millis(900);
/// A seek stops this far before the archive's end (the program's end,
/// or now for a program still airing).
const END_MARGIN: i32 = 10;

thread_local! {
    /// The latest seek key; an older commit timer does nothing.
    static PRESSES: Cell<u64> = const { Cell::new(0) };
    /// The seek key being held.
    static HOLD: Cell<SeekHold> = Cell::new(SeekHold::default());
}

/// The archived program playing: its start and length in seconds and how
/// far into it the archive can go now.
fn program(ui: Ui) -> Option<(i32, i32, i32)> {
    let channel = ui.playing.get_untracked()?;
    let Playback::CatchUp { start, offset } = ui.playback.get_untracked() else {
        return None;
    };
    let schedule = crate::schedule();
    let p = schedule.program_at(channel, start);
    let length = (p.end - p.start) * 60;
    let aired = crate::clock::wall_unix() - crate::guide::unix_of(&*schedule, p.start);
    let limit = length.min(i32::try_from(aired).unwrap_or(i32::MAX)) - END_MARGIN;
    Some((offset, length, limit.max(0)))
}

/// Seconds into the archived program: the seek being chosen, else where
/// the stream is. `None` unless a program plays from the archive.
pub fn elapsed(ui: Ui) -> Option<(i32, i32)> {
    if crate::on_demand::active(ui) {
        let t = ui.player.main().timeline.get();
        return Some((
            i32::try_from(t.position_ms / 1000).unwrap_or(i32::MAX),
            i32::try_from(t.duration_ms.unwrap_or(0) / 1000).unwrap_or(i32::MAX),
        ));
    }
    let Playback::CatchUp { offset, start } = ui.playback.get() else {
        return None;
    };
    let channel = ui.playing.get()?;
    let p = crate::schedule().program_at(channel, start);
    let length = (p.end - p.start) * 60;
    let at = ui.seek_target.get().unwrap_or_else(|| {
        let played = i32::try_from(ui.player.main().position_ms.get() / 1000).unwrap_or(i32::MAX);
        offset.saturating_add(played)
    });
    Some((at.clamp(0, length), length))
}

/// Moves the seek target by `delta` seconds; the archive reopens there
/// once the keys pause. False when nothing plays from the archive.
pub fn step(ui: Ui, delta: i32) -> bool {
    if crate::on_demand::active(ui) {
        let moved = crate::on_demand::step(ui, delta);
        if moved {
            show_info(ui);
        }
        return moved;
    }
    let Some((offset, _, limit)) = program(ui) else {
        return false;
    };
    let from = ui.seek_target.get_untracked().unwrap_or_else(|| {
        let played =
            i32::try_from(ui.player.main().position_ms.get_untracked() / 1000).unwrap_or(i32::MAX);
        offset.saturating_add(played)
    });
    crate::interruption::viewer_acted(ui);
    ui.seek_target.set(Some((from + delta).clamp(0, limit)));
    show_info(ui);
    commit_later(ui, COMMIT_AFTER);
    true
}

/// A seek key: one `step` (seconds, negative back), or a multiple of it
/// while the key is held (`repeat`: an auto-repeat); some repeats take
/// no step. False when nothing plays from the archive.
pub fn key(ui: Ui, step: i32, repeat: bool) -> bool {
    let factor = HOLD.with(|h| {
        let mut hold = h.get();
        let factor = hold.step(step, repeat, ui.now_ms());
        h.set(hold);
        factor
    });
    match factor {
        Some(f) => self::step(ui, step.saturating_mul(f)),
        None => program(ui).is_some() || crate::on_demand::seekable(ui),
    }
}

/// A seek is being chosen (the archive has not reopened there yet).
pub fn pending(ui: Ui) -> bool {
    ui.seek_target.get_untracked().is_some()
}

/// Retires the old archive's draft whenever playback changes or stops.
pub(crate) fn cancel(ui: Ui) {
    if pending(ui) {
        PRESSES.with(|p| p.set(p.get().wrapping_add(1)));
        ui.seek_target.set(None);
    }
}

/// OK while a seek is being chosen: the archive opens there now.
pub fn commit_now(ui: Ui) {
    PRESSES.with(|p| p.set(p.get() + 1));
    commit(ui);
}

/// The program playing from the archive: its length in seconds and how
/// far into it a seek may go now. `None` unless one plays.
pub fn span(ui: Ui) -> Option<(i32, i32)> {
    if crate::on_demand::active(ui) {
        if !crate::on_demand::seekable(ui) {
            return None;
        }
        let (_, length) = elapsed(ui)?;
        return (length > 0).then_some((length, length));
    }
    program(ui).map(|(_, length, limit)| (length, limit))
}

/// A seek to `target` seconds into the archived program being chosen
/// (the play bar's progress bar under a finger or the mouse, within
/// [`span`]'s limit): shown at once, opened once `commit` says so or the
/// pointer pauses.
pub fn to(ui: Ui, target: i32, commit_now: bool) {
    if crate::on_demand::active(ui) {
        crate::on_demand::seek_to(ui, target.max(0) as u64 * 1000);
        return;
    }
    crate::interruption::viewer_acted(ui);
    ui.seek_target.set(Some(target.max(0)));
    commit_later(
        ui,
        if commit_now {
            Duration::ZERO
        } else {
            COMMIT_AFTER
        },
    );
}

/// Opens the archive at the target after `wait`, unless another seek came.
fn commit_later(ui: Ui, wait: Duration) {
    let press = PRESSES.with(|p| {
        p.set(p.get() + 1);
        p.get()
    });
    ui.owner.run(|| {
        let _task = set_timeout(wait, move || {
            if PRESSES.with(Cell::get) == press {
                commit(ui);
            }
        });
    });
}

/// The info card shows where a remote's seek goes; a pointer's shows on
/// the play bar.
fn show_info(ui: Ui) {
    if ui.keys_shown() && !crate::play_bar::captures(ui) {
        ui.update_panels(|p, now| p.show_info(now));
    }
}

/// Opens the archive at the chosen target.
fn commit(ui: Ui) {
    let Some(target) = ui.seek_target.get_untracked() else {
        return;
    };
    let (Some(channel), Playback::CatchUp { start, .. }) =
        (ui.playing.get_untracked(), ui.playback.get_untracked())
    else {
        ui.seek_target.set(None);
        return;
    };
    log::info!("iptv seek: catch-up to {target} s into the program");
    ui.player.main().position_ms.set(0);
    ui.play(
        channel,
        Playback::CatchUp {
            start,
            offset: target,
        },
    );
    ui.seek_target.set(None);
    show_info(ui);
}

/// `m:ss` or `h:mm:ss`.
pub fn hms(secs: i32) -> String {
    let s = secs.max(0);
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use iptv_settings::Value;

    use super::*;

    #[test]
    fn steps_follow_the_settings() {
        let mut s = Settings::new();
        let defaults = Steps {
            forward: 30,
            rewind: 10,
            timebar: 300,
        };
        assert_eq!(steps_of(&s), defaults);
        s.set("catchup.forward_step", Value::secs(60)).unwrap();
        s.set("catchup.rewind_step", Value::secs(5)).unwrap();
        s.set("catchup.timebar_step", Value::secs(15 * 60)).unwrap();
        assert_eq!(
            steps_of(&s),
            Steps {
                forward: 60,
                rewind: 5,
                timebar: 900,
            }
        );
    }

    #[test]
    fn times_read_as_minutes_or_hours() {
        assert_eq!(hms(0), "0:00");
        assert_eq!(hms(75), "1:15");
        assert_eq!(hms(3600 + 62), "1:01:02");
        assert_eq!(hms(-5), "0:00");
    }
}
