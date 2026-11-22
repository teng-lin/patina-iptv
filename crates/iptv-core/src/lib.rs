#![forbid(unsafe_code)]
//! The UI-independent core of iptv-rs. Everything here is plain Rust: no NSH,
//! no platform APIs, no blocking I/O, and it must build for
//! `wasm32-unknown-unknown`.
//!
//! Today: the guide's synthetic schedule, the remote/touch focus model and
//! its columns (rail, groups, lists), favorites, menus, panels, playback.

pub mod add_playlist;
pub mod capabilities;
pub mod favorites;
pub mod guide;
pub mod import_limits;
pub mod interruption;
pub mod lists;
pub mod live;
pub mod membership;
pub mod message;
pub mod multiview;
pub mod parental;
pub mod playback;
pub mod recording;
pub mod reorder;
pub mod tracks;

pub mod on_demand;
pub mod organization;

pub mod vod;
