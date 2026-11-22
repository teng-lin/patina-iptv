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

/// The host's public entry points, re-exported at the crate root (a
/// platform's native components).
#[cfg(not(target_family = "wasm"))]
pub use native::exports;
#[cfg(target_family = "wasm")]
pub use web::exports;
