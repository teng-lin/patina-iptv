//! The process clock and profiling hooks of the native hosts.
use std::time::Duration;

/// With `adb shell setprop debug.iptvrs.profile 1`: logs where the app's
/// library is loaded, for symbolizing a simpleperf profile of a cold
/// start (simpleperf can miss the mapping of a library loaded from the
/// APK before it attached).
pub(crate) fn log_library_base() {
    #[cfg(target_os = "android")]
    {
        let mut value = [0 as libc::c_char; 92];
        // SAFETY: a NUL-terminated name and a buffer of PROP_VALUE_MAX bytes.
        let n = unsafe {
            libc::__system_property_get(c"debug.iptvrs.profile".as_ptr(), value.as_mut_ptr())
        };
        if n <= 0 || value[0] != b'1' as libc::c_char {
            return;
        }
        // SAFETY: an all-zero Dl_info is valid; dladdr fills it.
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        // SAFETY: the address of a function of this library.
        if unsafe { libc::dladdr(log_library_base as *const libc::c_void, &mut info) } != 0 {
            log::info!("iptv startup: library at {:p}", info.dli_fbase);
        }
    }
}

/// The process's age: now on the boot clock less its start time
/// (`/proc/self/stat` field 22, in clock ticks since boot).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn process_age() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let ticks = start_ticks(&stat)?;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = u64::try_from(hz).ok().filter(|&h| h > 0)?;
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid timespec to write.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) } != 0 {
        return None;
    }
    let now = Duration::new(u64::try_from(now.tv_sec).ok()?, now.tv_nsec as u32);
    now.checked_sub(Duration::from_millis(ticks * 1000 / hz))
}

/// Field 22 of a `/proc/<pid>/stat` line (after the command in
/// parentheses, which may hold spaces and parentheses itself).
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn start_ticks(stat: &str) -> Option<u64> {
    let after = &stat[stat.rfind(')')? + 1..];
    // Fields 3.. follow; the start time is the 20th of them.
    after.split_whitespace().nth(19)?.parse().ok()
}

/// The process's start is not known here: startup marks count from the
/// first one.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(crate) fn process_age() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_time_is_field_22() {
        let stat = "1234 (a (b) c) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4242 99";
        assert_eq!(start_ticks(stat), Some(4242));
        assert_eq!(start_ticks("1 (x) S 1"), None);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn the_process_is_younger_than_a_day() {
        let age = process_age().expect("this process's age");
        assert!(age < Duration::from_secs(86_400), "{age:?}");
    }
}
