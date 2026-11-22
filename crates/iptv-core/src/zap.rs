//! Group and list navigation for live playback.

use crate::guide::Schedule;
use crate::lists::ChannelList;
use std::ops::Range;

/// The channels of the group containing `channel` (channels are stored
/// grouped; the last group runs to the end).
pub fn group_range(schedule: &dyn Schedule, channel: usize) -> Range<usize> {
    let count = schedule.channel_count();
    let members = schedule.channel_group_channels(channel);
    members.iter().min().copied().unwrap_or(0)..members.iter().max().map_or(count, |c| c + 1)
}

/// Channel up/down from `channel` by `step`, wrapping within its group.
pub fn zap_target(schedule: &dyn Schedule, channel: usize, step: isize) -> usize {
    let mut channels = schedule.channel_group_channels(channel);
    channels.retain(|&c| {
        !schedule.channel_hidden(c) && schedule.channel_block(c) == crate::organization::Block::None
    });
    if channels.is_empty() {
        return channel;
    }
    let at = channels.iter().position(|&c| c == channel);
    let to = match at {
        Some(at) => (at as isize + step).rem_euclid(channels.len() as isize) as usize,
        None if step < 0 => channels.len() - 1,
        None => 0,
    };
    channels[to]
}

/// The starting point for a channel change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZapOrigin {
    Row(usize),
    OtherGroup(usize),
}

impl ZapOrigin {
    /// Finds the playing channel in the displayed list, or falls back to
    /// the focused row when nothing is playing.
    pub fn from_channel(
        channel: Option<usize>,
        row_of: impl FnOnce(usize) -> Option<usize>,
        focused_row: impl FnOnce() -> usize,
    ) -> Self {
        match channel {
            Some(channel) => row_of(channel).map_or(Self::OtherGroup(channel), Self::Row),
            None => Self::Row(focused_row()),
        }
    }
}

/// Zaps within the displayed list or within the playing channel's group.
pub fn zap_from_list(
    schedule: &dyn Schedule,
    list: &ChannelList,
    from: ZapOrigin,
    step: isize,
    allowed: impl Fn(usize) -> bool,
) -> Option<usize> {
    match from {
        ZapOrigin::Row(row) => (1..=list.len())
            .filter_map(|offset| list.zap(row, (offset as isize).wrapping_mul(step)))
            .filter_map(|row| list.channel(row))
            .find(|&channel| allowed(channel)),
        ZapOrigin::OtherGroup(channel) => {
            Some(zap_target(schedule, channel, step)).filter(|&c| allowed(c))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::{GROUPS, SyntheticSchedule, group_start};

    #[test]
    fn zapping_wraps_within_the_group() {
        let s = SyntheticSchedule;
        let (g0, g1) = (group_start(0), group_start(1));
        assert_eq!(group_range(&s, g0), g0..g1);
        assert_eq!(group_range(&s, g1 - 1), g0..g1);
        assert_eq!(
            zap_target(&s, g1 - 1, 1),
            g0,
            "down from the last wraps to the first"
        );
        assert_eq!(
            zap_target(&s, g0, -1),
            g1 - 1,
            "up from the first wraps to the last"
        );
        assert_eq!(zap_target(&s, g0, 1), g0 + 1);
        let last = group_start(GROUPS - 1);
        let end = crate::guide::CHANNELS;
        assert_eq!(group_range(&s, end - 1), last..end);
        assert_eq!(zap_target(&s, end - 1, 1), last);
    }

    /// A schedule with uneven groups, an empty group and one channel groups.
    struct Groups(Vec<usize>, usize);

    impl Schedule for Groups {
        fn channel_count(&self) -> usize {
            self.1
        }
        fn group_count(&self) -> usize {
            self.0.len()
        }
        fn group_start(&self, group: usize) -> usize {
            self.0[group]
        }
        fn program_at(&self, _: usize, minute: i32) -> crate::guide::Program {
            crate::guide::Program {
                start: minute,
                end: minute + 1,
            }
        }
        fn programs_between(&self, _: usize, _: i32, _: i32) -> Vec<crate::guide::Program> {
            Vec::new()
        }
        fn catchup_days(&self, _: usize) -> u8 {
            0
        }
        fn channel_name(&self, _: usize) -> String {
            String::new()
        }
        fn group_name(&self, _: usize) -> String {
            String::new()
        }
        fn title(&self, _: usize, _: crate::guide::Program) -> String {
            String::new()
        }
    }

    #[test]
    fn zapping_handles_uneven_and_single_channel_groups() {
        // Groups: [0,3) [3,4) [4,8)
        let s = Groups(vec![0, 3, 4], 8);
        assert_eq!(group_range(&s, 3), 3..4);
        assert_eq!(zap_target(&s, 3, 1), 3, "a one-channel group stays");
        assert_eq!(zap_target(&s, 7, 1), 4);
        assert_eq!(zap_target(&s, 2, 1), 0);
        assert_eq!(zap_target(&s, 4, -3), 5);
        let none = Groups(vec![], 5);
        assert_eq!(zap_target(&none, 4, 1), 0, "no groups: the whole list");
        let empty = Groups(vec![], 0);
        assert_eq!(zap_target(&empty, 0, 1), 0);
    }
}
