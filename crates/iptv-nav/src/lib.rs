#![forbid(unsafe_code)]
//! Remote and keyboard input for iptv-rs, independent of any UI toolkit.
//!
//! Pipeline: platform key events → [`RemoteKey`] + [`Phase`] →
//! [`PressDetector`] (short / long; [`HeldPresses`] drops a waiting press
//! whose surface changed) → [`Keymap::resolve`] in the current
//! [`Context`] → [`Action`]. The app applies actions to its state; shells only
//! translate their key events into `RemoteKey`s.

mod back;
mod hold;
mod key;
mod keymap;
mod player;
mod press;

pub use back::BackGuard;
pub use hold::{Decision, HeldPresses};
pub use key::RemoteKey;
pub use keymap::{Action, Binding, Context, Keymap, Profile};
pub use player::{seek_arrows, seek_key, up_down_key};
pub use press::{DEFAULT_LONG_PRESS_MS, Phase, Press, PressDetector};

pub mod remap;
