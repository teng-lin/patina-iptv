//! The device's local time zone on the native hosts.

/// The local UTC offset at `unix`, in minutes.
#[cfg(unix)]
pub(crate) fn offset_minutes(unix: i64) -> i32 {
    let t = unix as libc::time_t;
    // SAFETY: an all-zero `tm` is valid; localtime_r only writes into it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call; localtime_r is reentrant.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok { (tm.tm_gmtoff / 60) as i32 } else { 0 }
}

#[cfg(windows)]
pub(crate) fn offset_minutes(unix: i64) -> i32 {
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
pub(crate) fn offset_minutes(_unix: i64) -> i32 {
    0
}

#[cfg(unix)]
pub(crate) fn reload_time_zone() {
    unsafe extern "C" {
        fn tzset();
    }
    // SAFETY: tzset takes no arguments; it re-reads the zone for later
    // localtime_r calls (glibc caches it; bionic checks on its own).
    unsafe { tzset() }
}

#[cfg(not(unix))]
pub(crate) fn reload_time_zone() {}
