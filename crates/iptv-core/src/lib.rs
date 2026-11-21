#![forbid(unsafe_code)]
//! The UI-independent core of iptv-rs. Everything here is plain Rust: no NSH,
//! no platform APIs, no blocking I/O, and it must build for
//! `wasm32-unknown-unknown`.
//!
//! Today: the guide's synthetic schedule, the remote/touch focus model and
//! its columns (rail, groups, lists), favorites, menus, panels, playback.

pub mod browse;
pub mod browse_playback;
pub mod capabilities;
pub mod channel_list;
pub mod channel_picker;
pub mod clock;
pub mod editor;
pub mod favorites;
pub mod guide;
pub mod i18n;
pub mod import_limits;
pub mod interruption;
pub mod layout;
pub mod lists;
pub mod live;
pub mod membership;
pub mod menu;
pub mod multiview;
pub mod panels;
pub mod parental;
pub mod pip;
pub mod pip_window;
pub mod play_bar;
pub mod playback;
pub mod playback_windows;
pub mod player_info;
pub mod player_options;
pub mod recording;
pub mod reorder;
pub mod seek_hold;
pub mod sound;
pub mod timeline;
pub mod tracks;
pub mod wizard;

pub mod on_demand;
pub mod organization;

pub mod vod;

pub mod channel_number;
