#![forbid(unsafe_code)]
//! Toolkit-free feature state, display data and localization for Patina's
//! front ends. Feature holders act on content identities and return effects
//! for their caller to perform.

pub mod browse_playback;
pub mod capabilities;
pub mod cast;
pub mod channel_number;
pub mod editor;
pub mod external_player;
pub mod favorites;
pub mod input_mode;
pub mod interruption;
pub mod layout;
pub mod library_recovery;
pub mod membership;
pub mod on_demand;
pub mod organization;
pub mod panels;
pub mod parental;
pub mod pip;
pub mod play_bar;
pub mod playback;
pub mod playback_health;
pub mod playback_windows;
pub mod player_info;
pub mod player_options;
pub mod recording;
pub mod seek_hold;
pub mod vod;

pub mod library;
pub mod search;
pub mod settings;

pub mod heard;
pub mod menu;
pub mod multiview;
pub mod pip_window;
pub mod reorder;
pub mod sound;
pub mod tracks;
pub mod wizard;

pub mod browse;
pub mod channel_list;
pub mod channel_picker;
pub mod drag;
pub mod guide;
pub mod listings;
pub mod lists;
pub mod touch_zap;
pub mod zap;

pub mod clock;
pub mod i18n;
