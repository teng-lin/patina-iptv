#![forbid(unsafe_code)]
//! The UI-independent core of iptv-rs. Everything here is plain Rust: no NSH,
//! no platform APIs, no blocking I/O, and it must build for
//! `wasm32-unknown-unknown`.
//!
//! Today: the guide's synthetic schedule, the remote/touch focus model and
//! its columns (rail, groups, lists), favorites, menus, panels, playback.

pub mod add_playlist;
pub mod browse;
pub mod capabilities;
pub mod channel_list;
pub mod channel_picker;
pub mod clock;
pub mod favorites;
pub mod guide;
pub mod heard;
pub mod i18n;
pub mod import_limits;
pub mod interruption;
pub mod listings;
pub mod lists;
pub mod live;
pub mod membership;
pub mod menu;
pub mod message;
pub mod multiview;
pub mod parental;
pub mod pip_window;
pub mod playback;
pub mod recording;
pub mod reorder;
pub mod sound;
pub mod tracks;
pub mod wizard;

pub mod on_demand;
pub mod organization;

pub mod vod;

mod multiview_state;
mod reorder_state;
mod track_labels;

mod guide_state;
mod lists_state;
mod zap;
