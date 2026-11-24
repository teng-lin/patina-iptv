//! The native hosts: the desktops, the iPad and Android. Storage is a
//! private directory, the database key comes from the OS key store, the
//! network is `iptv-net`, work runs on threads, and recordings register
//! with the OS scheduler.
#[cfg(target_os = "android")]
mod android_transport;
mod clock;
mod external_player;
pub(crate) use external_player::*;
pub(crate) mod credential_key;
mod keep_awake;
pub(crate) mod platform_data;
mod process;
pub(crate) mod recording_host;
#[cfg(all(target_os = "ios", target_abi = "sim"))]
mod simulator_entitlements;
pub(crate) mod transport;

use std::path::PathBuf;

use iptv_core::capabilities::Platform;

/// The platform this build runs on.
pub(crate) const PLATFORM: Platform = if cfg!(target_os = "android") {
    Platform::Android
} else if cfg!(target_os = "linux") {
    Platform::Linux
} else if cfg!(target_os = "macos") {
    Platform::Macos
} else if cfg!(windows) {
    Platform::Windows
} else if cfg!(target_os = "ios") {
    Platform::Ipad
} else {
    Platform::Unknown
};

pub(crate) use clock::{offset_minutes, reload_time_zone};
#[cfg(feature = "test-support")]
pub(crate) use platform_data::host_handle;
pub(crate) use platform_data::{flush_on_host_stop, initialize, resolve_data_dir};
pub(crate) use process::{log_library_base, process_age};

/// Whether a capture runs while the desktop could not be kept awake
/// (`keep_awake.rs`).
pub(crate) fn keep_awake_failed() -> bool {
    keep_awake::failed()
}

/// The execution this host supplies has blocking workers
/// (`Runtime::has_workers`).
pub(crate) const HAS_WORKERS: bool = true;

/// Native hosts with a remote say so by platform (`host::ANDROID`).
pub(crate) const REMOTE_ONLY: bool = false;

/// Imports and guide downloads run on the refresh pool's workers.
pub(crate) const IMPORTS: bool = true;

/// The runtime's ports on this host.
pub(crate) fn ports(directory: PathBuf) -> crate::runtime::Ports {
    let mut ports = crate::runtime::Ports::new(directory, credential_key::database_key);
    ports.background_recordings = crate::capabilities::BUILT.background_recordings();
    ports.recording_schedule = recording_host::schedule;
    ports.exclude_from_backup = platform_data::exclude_from_backup;
    ports.startup_mark = |phase, detail| crate::startup::mark_with(phase, detail);
    ports
}

/// Time since the Unix epoch on the system clock; `None` before it.
pub(crate) fn since_epoch() -> Option<std::time::Duration> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
}

/// An image's size from its header (NSH's decoders' own probe).
pub(crate) fn image_extent(bytes: &[u8]) -> super::Extent {
    use nsh_core::image::Natural;
    match Natural::of(bytes) {
        Some(Natural::Pixels(width, height)) => super::Extent::Pixels(width, height),
        Some(Natural::Vector(_)) => super::Extent::Vector,
        None => super::Extent::Unknown,
    }
}

/// Runs `work` off the UI thread, on a thread of its own named `name`.
pub(crate) fn background(name: &str, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .map(drop)
}

/// The host's recording notifications follow the interface language
/// ([`language_changed`]).
pub(crate) const NOTIFICATIONS_FOLLOW_LANGUAGE: bool = cfg!(target_os = "android");

/// The interface language is now `tag`.
pub(crate) fn language_changed(tag: &str) {
    #[cfg(target_os = "android")]
    recording_host::set_language(tag);
    #[cfg(not(target_os = "android"))]
    let _ = tag;
}

/// Public entry points of the native hosts.
pub mod exports {
    /// Explicit synthetic-key injection for the headless test binary only.
    #[cfg(feature = "test-support")]
    pub use super::credential_key::install_test_key;
    pub use super::platform_data::exit_background_recorder_launch;
    /// Use a fresh explicit synthetic profile without changing the process home.
    #[cfg(feature = "test-support")]
    pub use super::platform_data::install_test_data_dir;
    // Android's host registers its components in Kotlin.
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
    use iptv_player_mpv as mpv;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
    use nsh_core::prelude::*;

    #[cfg(windows)]
    #[native_components(windows)]
    pub fn components(components: &nsh_platform_windows::Components) {
        mpv::register(components, super::transport::policy);
    }

    /// The Linux desktop's native components: `tv.player` on libmpv (the
    /// Android host registers its own in Kotlin).
    #[cfg(target_os = "linux")]
    #[native_components(linux)]
    pub fn components(components: &nsh_platform_linux::Components) {
        mpv::register(components, super::transport::policy);
    }

    /// The macOS desktop's native components: `tv.player` on libmpv, as on
    /// Linux.
    #[cfg(target_os = "macos")]
    #[native_components(macos)]
    pub fn components(components: &nsh_platform_macos::Components) {
        mpv::register(components, super::transport::policy);
    }

    /// iOS and iPadOS's native components: `tv.player` on libmpv (MPVKit's,
    /// OpenGL ES), as on the desktops.
    #[cfg(target_os = "ios")]
    #[native_components(ios)]
    pub fn components(components: &nsh_platform_ios::Components) {
        mpv::register(components, super::transport::policy);
    }
}
