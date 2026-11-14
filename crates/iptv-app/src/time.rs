//! The app's clock: the guide's "now" (from the wall clock for real data,
//! from when the schedule was shown for synthetic data), local-time labels
//! and the minute tick that advances [`iptv_core::guide::Focus::now`].
//!
//! Local time comes from the C library's `localtime_r` (`tm_gmtoff`), per
//! instant so labels across a daylight saving change are right. On Android
//! bionic resolves the device's time zone itself (the `persist.sys.timezone`
//! setting, with the system tzdata) and notices changes; on desktop Linux
//! glibc reads `TZ` or `/etc/localtime`, re-read each minute through
//! `tzset`. Windows uses chrono's per-instant system local-time conversion.
//! Other non-Unix targets fall back to UTC.

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

/// The local UTC offset at `unix`, in minutes.
#[cfg(unix)]
pub fn offset_minutes(unix: i64) -> i32 {
    let t = unix as libc::time_t;
    // SAFETY: an all-zero `tm` is valid; localtime_r only writes into it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call; localtime_r is reentrant.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok { (tm.tm_gmtoff / 60) as i32 } else { 0 }
}

#[cfg(windows)]
pub fn offset_minutes(unix: i64) -> i32 {
    // Windows system time does not follow POSIX TZ. Isolated scenarios supply
    // an explicit fixed offset; release builds never read this test override.
    #[cfg(feature = "test-support")]
    if let Some(offset) = std::env::var("IPTV_TEST_UTC_OFFSET_MINUTES")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|n| (-720..=720).contains(n))
    {
        return offset;
    }
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(unix, 0)
        .single()
        .map_or(0, |time| time.offset().local_minus_utc() / 60)
}

#[cfg(not(any(unix, windows)))]
pub fn offset_minutes(_unix: i64) -> i32 {
    0
}

#[cfg(unix)]
fn reload_time_zone() {
    unsafe extern "C" {
        fn tzset();
    }
    // SAFETY: tzset takes no arguments; it re-reads the zone for later
    // localtime_r calls (glibc caches it; bionic checks on its own).
    unsafe { tzset() }
}

#[cfg(not(unix))]
fn reload_time_zone() {}

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
    let secs = guide::secs_to_next_minute(&*schedule(), wall_unix(), shown_secs());
    let _task = set_timeout(Duration::from_millis(secs * 1000 + 20), move || {
        tick(ui);
        start(ui);
    });
}
