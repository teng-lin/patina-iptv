//! Native libmpv adapter and the player wire contract shared with Android.
mod wire;

mod events;
pub use events::{PipAction, PlayerEvent, PlayerEventError, PlayerEventKind};
mod props;
pub use props::{PipProps, PlayerBindings, PlayerProp, PlayerProps, SourceProps, WireValue};
mod tracks;
pub use tracks::parse_tracks;

/// The player component, registered by each host.
pub const PLAYER_VIEW: &str = "tv.player";

/// The PiP component shares surface zero's player and stream.
pub const PIP_VIEW: &str = "tv.pip";

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
mod mpv;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
pub use mpv::register;
