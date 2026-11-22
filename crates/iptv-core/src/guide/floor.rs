//! The guide can browse retained listings independently of replay availability.

use super::Schedule;

/// The first minute the focused row can inspect, up to the present.
pub fn floor_of(s: &dyn Schedule, channel: usize, now: i32) -> i32 {
    if channel >= s.channel_count() {
        return now;
    }
    s.listing_start(channel)
        .map_or(now, |start| start.max(s.browsable(now).0).min(now))
}

pub use crate::guide_state::view_range;
