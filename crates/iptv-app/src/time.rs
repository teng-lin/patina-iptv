//! The app's clock: the guide's "now" (from the wall clock for real data,
//! from when the schedule was shown for synthetic data), local-time labels
//! and the minute tick that advances [`iptv_core::guide::Focus::now`].
//!
//! Local time comes from the host (`crate::host::offset_minutes`); natively
//! from the C library's `localtime_r` (`tm_gmtoff`), per
//! instant so labels across a daylight saving change are right. On Android
//! bionic resolves the device's time zone itself (the `persist.sys.timezone`
//! setting, with the system tzdata) and notices changes; on desktop Linux
//! glibc reads `TZ` or `/etc/localtime`, re-read each minute through
//! `tzset`. Windows uses chrono's per-instant system local-time conversion.
//! Other non-Unix targets fall back to UTC.

pub(crate) use crate::host::offset_minutes;
use crate::host::reload_time_zone;
use std::cell::Cell;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iptv_core::guide;
use nsh_core::prelude::*;

use crate::{Ui, schedule};

thread_local! {
    /// When the current schedule was shown (synthetic now starts there).
    static SHOWN: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Unix seconds now.
pub fn wall_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The current schedule was just shown.
pub(crate) fn mark_shown() {
    SHOWN.with(|s| s.set(Some(Instant::now())));
}

fn shown_secs() -> u64 {
    SHOWN.with(|s| {
        let at = s.get().unwrap_or_else(|| {
            let now = Instant::now();
            s.set(Some(now));
            now
        });
        at.elapsed().as_secs()
    })
}

/// The guide minute it is now on the current schedule (not reactive: views
/// read [`guide::Focus::now`], which the tick keeps equal to this).
pub fn now_minute() -> i32 {
    guide::now_minute(&*schedule(), wall_unix(), shown_secs())
}

/// Guide `minute` as local wall-clock time on the current schedule, with
/// the weekday and date when it is not today.
pub fn clock(minute: i32) -> String {
    let s = schedule();
    let now = guide::unix_of(&*s, now_minute());
    label(guide::unix_of(&*s, minute), now)
}

/// The local time of `unix` in the interface language and on the clock
/// the settings choose, with the weekday and day when it is not the day
/// of `now` (`14:05`, `Tue 10 · 2:05 PM`).
pub fn label(unix: i64, now: i64) -> String {
    iptv_core::clock::label(unix, now, offset_minutes, crate::i18n::clock_style())
}

/// A calendar date without a time, for date selectors.
pub fn date_only(unix: i64) -> String {
    iptv_core::clock::date(unix, offset_minutes(unix), crate::i18n::clock_style())
}

/// A local clock time without a date, respecting the chosen hour cycle.
pub fn time_only(unix: i64) -> String {
    let minute = unix.div_euclid(60) + i64::from(offset_minutes(unix));
    iptv_core::clock::time(minute, crate::i18n::clock_style())
}

/// Moves the guide's now when the minute changed.
pub fn tick(ui: Ui) {
    reload_time_zone();
    let now = now_minute();
    if ui.focus.with(|f| f.now) != now {
        // The focus is on a row of the list on screen.
        let view = crate::lists::view(ui);
        ui.focus.update(|f| {
            f.tick(now, &view);
        });
    }
}

/// Ticks at every minute boundary of the current schedule's clock.
pub fn start(ui: Ui) {
    // Synthetic work-counter journeys exclude unrelated minute-boundary work.
    #[cfg(feature = "test-support")]
    if crate::performance_probe::enabled() {
        return;
    }
    let secs = guide::secs_to_next_minute(&*schedule(), wall_unix(), shown_secs());
    let _task = set_timeout(Duration::from_millis(secs * 1000 + 20), move || {
        tick(ui);
        start(ui);
    });
}
