//! Host glue: what differs by platform, behind one facade the screens call.
//! These modules are the only ones with platform `cfg`.
//!
//! Each host builds the runtime's [`Ports`](crate::runtime::Ports) for its
//! platform (private directory, key broker, network, execution, entropy,
//! backup exclusion, recording schedule) and answers the platform questions
//! the screens ask: where storage lives, how background work runs, the
//! local time zone, the process clock.
//!
//! - `native`: the desktops, the iPad and Android, over `iptv-net`,
//!   threads, the OS key stores and the OS schedulers.
mod native;
pub(crate) use native::*;

use iptv_core::capabilities::Platform;

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
pub mod exports {
    pub use super::native::exports::*;
}
