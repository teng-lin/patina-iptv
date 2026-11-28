//! Host glue: what differs by platform, behind one facade the screens call.
//! Platform `cfg` belongs here; the few files outside that still have one
//! are listed, to shrink, in `tests/app/architecture.rs`.
//!
//! Each host builds the runtime's [`Ports`](crate::runtime::Ports) for its
//! platform (private directory, key broker, network, execution, entropy,
//! backup exclusion, recording schedule) and answers the platform questions
//! the screens ask: where storage lives, how background work runs, the
//! local time zone, the process clock.
//!
//! - `native`: the desktops, the iPad and Android, over `iptv-net`,
//!   threads, the OS key stores and the OS schedulers.
//! - `web`: a browser TV, over `iptv-web`'s adapters and the cooperative
//!   executor.
pub(crate) mod backup_documents;
#[cfg(not(target_family = "wasm"))]
mod native;
pub(crate) mod updates;
#[cfg(not(target_family = "wasm"))]
pub(crate) use native::*;

#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
pub(crate) use web::*;

use iptv_core::capabilities::{Host, Platform};

/// The firewall CLI detected on Linux desktops for AirPlay setup hints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinuxFirewall {
    Ufw,
    Firewalld,
}

/// The host as its build describes it, before the device reports its form
/// factor and input: for the facilities that follow the operating system
/// alone (background recording, login registration, the alarm
/// permission), and for work off the UI thread or before the root mounts.
pub(crate) const BUILT: Host = Host::of(PLATFORM);

/// What an image's header says about its size, before anything decodes it.
pub(crate) enum Extent {
    /// A raster of this many pixels across and down.
    Pixels(u32, u32),
    /// A vector image (SVG), drawn by NSH's bounded rasterizer.
    Vector,
    /// Not an image this host decodes.
    Unknown,
}

const fn on(platform: Platform) -> bool {
    PLATFORM as u8 == platform as u8
}

/// Android (a TV box or a tablet).
pub(crate) const ANDROID: bool = on(Platform::Android);
/// An iPad.
pub(crate) const IPAD: bool = on(Platform::Ipad);
/// The Linux desktop.
pub(crate) const LINUX: bool = on(Platform::Linux);
/// The macOS desktop.
pub(crate) const MACOS: bool = on(Platform::Macos);
/// The Windows desktop.
pub(crate) const WINDOWS: bool = on(Platform::Windows);

/// The video plane is under the whole page (a browser TV package: Tizen's
/// AVPlay), not a view among the app's layers: everything over a playing
/// video, the backdrops below the video layer included, must stay
/// transparent for the picture to show.
pub(crate) fn video_under_page() -> bool {
    #[cfg(all(not(target_family = "wasm"), feature = "test-support"))]
    {
        // Let an isolated native test render the browser host's backdrop.
        std::env::var_os("IPTV_TEST_VIDEO_UNDER_PAGE").as_deref() == Some("1".as_ref())
    }
    #[cfg(not(all(not(target_family = "wasm"), feature = "test-support")))]
    {
        cfg!(all(target_family = "wasm", feature = "browser-package"))
    }
}

/// Keys come from a TV remote that reports each key's press and release
/// (Android's, and a browser TV's through its NSH platform), not from a
/// keyboard whose bindings NSH's keymap fires.
pub(crate) const REMOTE_KEYS: bool = ANDROID || REMOTE_ONLY;

/// Ask for Android's optional notification permission after accepting a capture.
pub(crate) async fn request_recording_notification_permission() {
    #[cfg(target_os = "android")]
    let _ = nsh_core::prelude::request_notification_permission().await;
}

/// The font files the app registers itself, WOFF2. A browser package's
/// page loads them instead (feature `browser-package`, `hosts/tizen-nsh`),
/// which keeps them out of the module.
#[cfg(not(all(target_family = "wasm", feature = "browser-package")))]
pub(crate) const FONT_FILES: &[(&str, &[u8])] = &[
    (
        "Roboto",
        include_bytes!("../fonts/Roboto-Regular.woff2").as_slice(),
    ),
    (
        "Roboto",
        include_bytes!("../fonts/Roboto-Medium.woff2").as_slice(),
    ),
    (
        "Roboto Condensed",
        include_bytes!("../fonts/RobotoCondensed-Regular.woff2").as_slice(),
    ),
];
/// The font files the app registers itself: none, the page loads them.
#[cfg(all(target_family = "wasm", feature = "browser-package"))]
pub(crate) const FONT_FILES: &[(&str, &[u8])] = &[];

/// The startup splash's artwork: Patina's own, embedded so startup needs
/// no file or network access; a browser package's own file
/// (`assets/banner.png`), which keeps it out of the module.
pub(crate) fn splash_artwork() -> String {
    #[cfg(not(all(target_family = "wasm", feature = "browser-package")))]
    {
        use base64::Engine;
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD
                .encode(include_bytes!("../../../art/icon/banner.png")),
        )
    }
    #[cfg(all(target_family = "wasm", feature = "browser-package"))]
    {
        "asset:/banner.png".to_owned()
    }
}

/// The host's public entry points, re-exported at the crate root (a
/// platform's native components).
#[cfg(not(target_family = "wasm"))]
pub use native::exports;
#[cfg(target_family = "wasm")]
pub use web::exports;

static TUI_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Enables TUI-host layout adjustments (such as bypassing desktop horizontal
/// `StickyCells` wrappers that `nsh-platform-tui` does not implement).
pub fn set_tui_mode(enabled: bool) {
    TUI_MODE.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn is_tui_mode() -> bool {
    TUI_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// The local mock server (`tools/mock-server.py`) as the player reaches it:
/// through the Android emulator's host alias, or on this machine. Debug
/// builds only: the synthetic guide's channels have no streams in a release.
#[cfg(all(debug_assertions, target_os = "android"))]
pub(crate) const MOCK_SERVER: &str = "http://10.0.2.2:8089";
#[cfg(all(debug_assertions, not(target_os = "android")))]
pub(crate) const MOCK_SERVER: &str = "http://127.0.0.1:8089";

/// Native players on these desktops can be routed to auxiliary windows.
pub(crate) const PLAYER_POPUPS: bool = LINUX || MACOS;
