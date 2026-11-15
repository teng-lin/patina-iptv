//! Cold-start phase marks: one `iptv startup: <phase> +<ms> ms` line per
//! phase in the log, in milliseconds since the process started (on Linux
//! and Android from `/proc/self/stat`; elsewhere since the first mark),
//! with what it took or counted. Each phase is logged once per process:
//! later imports, guide checks and zaps stay quiet. `tools/perf-android.sh`
//! reads them for its phase table.
//!
//! The phases, in the order a stored playlist's cold start reaches them:
//! `rust root` (the app's root component runs), `settings loaded`,
//! `fonts registered` (all three registered), `store opened` (the
//! startup source read), `last channel resolved` (its stream known from
//! the stored catalog), `channels read` and `guide window read` (on the
//! import thread), `guide shown` (the schedule swapped in on the UI
//! thread), `guide frame` (the next display frame after it), `player
//! opened` (the stream handed to the player) and `video frame` (the
//! player's first rendered frame, its `size` event).

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The phases marked so far.
static MARKED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Where [`since_start`] counts from where the process start is unknown.
static FALLBACK: Mutex<Option<Instant>> = Mutex::new(None);

/// Logs `phase` (the first time only).
pub fn mark(phase: &'static str) {
    mark_with(phase, "");
}

/// Logs `phase` with `detail` (the first time only).
pub fn mark_with(phase: &'static str, detail: impl std::fmt::Display) {
    if !first(phase) {
        return;
    }
    let detail = detail.to_string();
    let sep = if detail.is_empty() { "" } else { ", " };
    log::info!(
        "iptv startup: {phase} +{} ms{sep}{detail}",
        since_start().as_millis()
    );
}

/// `data.rs` put a schedule on screen; `steps` say how long after it
/// started each step had ended. Marks `guide shown`, and `guide frame` at
/// the next display frame (the first one showing it).
pub fn shown(steps: &[(&str, Duration)]) {
    if marked("guide shown") {
        return;
    }
    let steps: Vec<String> = steps
        .iter()
        .map(|(step, at)| format!("{step} {} ms", at.as_millis()))
        .collect();
    mark_with("guide shown", steps.join(", "));
    drop(nsh_core::prelude::request_animation_frame(|_| {
        mark("guide frame")
    }));
}

/// With `adb shell setprop debug.iptvrs.profile 1`: logs where the app's
/// library is loaded, for symbolizing a simpleperf profile of a cold
/// start (simpleperf can miss the mapping of a library loaded from the
/// APK before it attached).
pub fn log_library_base() {
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

/// Whether `phase` was marked already (without marking it).
pub fn marked(phase: &'static str) -> bool {
    MARKED.lock().is_ok_and(|m| m.contains(&phase))
}

/// Records `phase`; false when it was recorded before.
fn first(phase: &'static str) -> bool {
    let Ok(mut marked) = MARKED.lock() else {
        return false;
    };
    if marked.contains(&phase) {
        return false;
    }
    marked.push(phase);
    true
}

/// Time since the process started.
pub fn since_start() -> Duration {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(d) = process_age() {
        return d;
    }
    let mut origin = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    origin.get_or_insert_with(Instant::now).elapsed()
}

/// The process's age: now on the boot clock less its start time
/// (`/proc/self/stat` field 22, in clock ticks since boot).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn process_age() -> Option<Duration> {
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

/// Original Patina artwork, embedded so startup needs no file or network access.
pub(crate) fn splash() -> impl nsh_core::prelude::IntoElement {
    use base64::Engine;
    use nsh_core::prelude::*;
    let artwork = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("../../../art/icon/banner.png")),
    );
    view()
        .test_id("startup-splash")
        .absolute()
        .inset_0()
        .bg(crate::tint(
            crate::theme::Theme::default().background.window,
            0xff,
        ))
        .child(
            image(artwork)
                .accessible(true)
                .label("Patina")
                .w_full()
                .h_full()
                .object_fit(ResizeMode::Contain),
        )
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

    #[test]
    fn a_phase_is_marked_once() {
        assert!(!marked("test phase"));
        assert!(first("test phase"));
        assert!(!first("test phase"));
        assert!(marked("test phase"));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn the_process_is_younger_than_a_day() {
        let age = process_age().expect("this process's age");
        assert!(age < Duration::from_secs(86_400), "{age:?}");
    }
}
