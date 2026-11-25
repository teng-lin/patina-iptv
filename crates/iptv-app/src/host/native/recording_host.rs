//! OS execution hooks. Provider identities never cross this boundary.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub(crate) use android::{notify_failed, schedule, set_language, standby_blocks_recording};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::activate_existing;
/// Whether a capture is running or due (the runtime's schedule
/// publication). A desktop stays awake while a capture runs (#275).
#[cfg(not(target_os = "android"))]
pub(crate) fn schedule(active: bool) {
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    super::keep_awake::set(active);
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let _ = active;
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod startup;

/// Scheduled recording is retired. On the first launch of this version, and
/// on every launch by an earlier version's login item or task
/// (`background`), removes the login and wake registrations earlier
/// versions made. A failure is retried at the next launch.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(crate) fn retire_registrations(directory: &std::path::Path, background: bool) {
    if !background && directory.join(startup::RETIRED).exists() {
        return;
    }
    match startup::retire(directory, &startup::Registrations::current()) {
        Ok(()) => log::info!("iptv recordings: login and wake registrations removed"),
        Err(error) => log::warn!(
            "iptv recordings: removing login and wake registrations failed ({:?}); retrying at the next launch",
            error.kind()
        ),
    }
}
