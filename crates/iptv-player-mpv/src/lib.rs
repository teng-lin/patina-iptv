//! Native libmpv adapter and the player wire contract shared with Android.
mod wire;

mod events;
pub use events::{AudioChange, PipAction, PlayerEvent, PlayerEventError, PlayerEventKind};
mod props;
pub use props::{
    PipProps, PlayerBindings, PlayerProp, PlayerProps, SeekProps, SourceProps, WireValue,
};
mod tracks;
pub use tracks::parse_tracks;

/// The player component, registered by each host.
pub const PLAYER_VIEW: &str = "tv.player";

pub mod pip_window;
pub use pip_window::PIP_VIEW;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
mod mpv;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
pub use mpv::register;
