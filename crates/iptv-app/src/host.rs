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
#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub(crate) use native::*;

#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
pub(crate) use web::*;

use iptv_core::capabilities::Platform;

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
pub(crate) fn request_recording_notification_permission() {
    // The system prompt must not delay the viewer's Start or Stop action.
    #[cfg(target_os = "android")]
    crate::data::spawn_in_app(async {
        let _ = nsh_core::prelude::request_notification_permission().await;
    });
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
