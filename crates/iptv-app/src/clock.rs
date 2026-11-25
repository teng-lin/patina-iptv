//! The app's clock: the guide's "now" (from the wall clock for real data,
//! from when the schedule was shown for synthetic data), local-time labels
//! and the next minute boundary.
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
use nsh_core::time::Instant;
use std::cell::Cell;
use std::time::Duration;

use iptv_core::guide;

use crate::runtime_owner::schedule;

thread_local! {
    /// When the current schedule was shown (synthetic now starts there).
    static SHOWN: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Unix seconds now.
pub fn wall_unix() -> i64 {
    crate::host::since_epoch().map_or(0, |d| d.as_secs() as i64)
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
/// read [`iptv_presentation::guide::Focus::now`], which the tick keeps equal to this).
pub fn now_minute() -> i32 {
    now_minute_in(&*schedule())
}

pub(crate) fn now_minute_in(schedule: &dyn guide::Schedule) -> i32 {
    guide::now_minute(schedule, wall_unix(), shown_secs())
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
    iptv_presentation::clock::label(unix, now, offset_minutes, crate::i18n::clock_style())
}

/// A calendar date without a time, for date selectors.
pub fn date_only(unix: i64) -> String {
    iptv_presentation::clock::date(unix, offset_minutes(unix), crate::i18n::clock_style())
}

/// A local clock time without a date, respecting the chosen hour cycle.
pub fn time_only(unix: i64) -> String {
    iptv_presentation::clock::time_at(unix, offset_minutes(unix), crate::i18n::clock_style())
}

/// Time until just after the next guide minute boundary.
pub(crate) fn next_tick() -> Duration {
    let secs = guide::secs_to_next_minute(&*schedule(), wall_unix(), shown_secs());
    Duration::from_millis(secs * 1000 + 20)
}
