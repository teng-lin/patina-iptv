#![forbid(unsafe_code)]
//! Portable Domain values and rules for Patina IPTV. The core defines the
//! guide schedule contract, playlist requests, user data and the rules
//! for playback, recording and parental access. It builds for
//! `wasm32-unknown-unknown`.

pub mod add_playlist;
pub mod capabilities;
pub mod cast;
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
