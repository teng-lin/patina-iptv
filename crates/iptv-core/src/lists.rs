//! Group membership lookups on a schedule.

use crate::guide::Schedule;

/// The catalog group holding `channel` (channels are stored grouped):
/// [`Schedule::channel_group`].
pub fn group_of(catalog: &(impl Schedule + ?Sized), channel: usize) -> Option<usize> {
    if channel >= catalog.channel_count() || catalog.group_count() == 0 {
        return None;
    }
    catalog.channel_group(channel)
}

/// [`group_of`] by asking every group for its channels: the trait's
/// default, and what an index must agree with.
pub fn scan_group_of(catalog: &(impl Schedule + ?Sized), channel: usize) -> Option<usize> {
    // Prefer a category over its playlist parent, which contains the same
    // channel. A collapsed source still has its parent as a useful fallback.
    (0..catalog.group_count())
        .find(|&g| !catalog.group_is_playlist(g) && catalog.group_channels(g).contains(&channel))
        .or_else(|| {
            (0..catalog.group_count()).find(|&g| {
                catalog.group_is_playlist(g) && catalog.group_channels(g).contains(&channel)
            })
        })
}

pub use crate::lists_state::{
    ALL, ChannelList, Entry, FAVORITES, FIRST_GROUP, ListId, ListView, RECENT, entry_count,
    entry_name, resume_entry,
};

#[cfg(test)]
pub(crate) use crate::lists_state::tests;

#[cfg(test)]
mod domain_tests {
    use super::group_of;
    use crate::guide::{Schedule, SyntheticSchedule};

    #[test]
    fn group_of_finds_the_catalog_group() {
        let schedule = SyntheticSchedule;
        for group in 0..schedule.group_count() {
            for channel in schedule.group_channels(group) {
                assert_eq!(group_of(&schedule, channel), Some(group));
            }
        }
        assert_eq!(group_of(&schedule, schedule.channel_count()), None);
    }
}
