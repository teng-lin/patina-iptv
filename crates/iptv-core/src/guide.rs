//! Program time, synthetic data and the schedule interface.

pub mod floor;

pub use floor::floor_of;

/// Synthetic channels in the guide.
pub const CHANNELS: usize = 5000;
/// Synthetic groups in the side column.
pub const GROUPS: usize = 24;
/// Unix time of the synthetic schedule's minute 0 (Tuesday 10 November
/// 2026, 18:00 UTC).
pub const SYNTHETIC_EPOCH_UNIX: i64 = 1_794_333_600;
/// Minutes of guide data ahead of now.
pub const HORIZON: i32 = 12 * 60;
/// Minutes visible across the guide at once.
pub const VISIBLE: i32 = 150;
/// How far before "now" the guide can show (the window's lower bound).
pub const PAST: i32 = 30;
/// How far back the synthetic guide has data (and a schedule's default
/// [`Schedule::browsable`] reaches).
pub const HISTORY: i32 = 7 * 24 * 60;

/// Where a program sits relative to now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Airing {
    Ended,
    Live,
    Upcoming,
}

impl Program {
    /// Where the program sits at guide minute `now`.
    pub fn airing(&self, now: i32) -> Airing {
        if self.end <= now {
            Airing::Ended
        } else if self.start <= now {
            Airing::Live
        } else {
            Airing::Upcoming
        }
    }

    /// Whether the channel's archive reaches the start of this program.
    pub fn can_replay(&self, now: i32, archive_days: u8) -> bool {
        let window = i32::from(archive_days) * 24 * 60;
        window > 0 && self.start <= now && self.start >= now.saturating_sub(window)
    }

    /// Played fraction of a live program at `now` (0 otherwise).
    pub fn progress(&self, now: i32) -> f32 {
        match self.airing(now) {
            Airing::Live => (now - self.start) as f32 / (self.end - self.start) as f32,
            _ => 0.0,
        }
    }
}

/// A program: `[start, end)` in guide minutes from the schedule's epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program {
    pub start: i32,
    pub end: i32,
}

/// What the schedule knows about a listing on an existing channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// The provider's title, including a title that is empty.
    Title(String),
    /// No listing covers this slot.
    Gap,
    /// The guide's page is being loaded.
    Loading,
}

/// A schedule group name before wording for a particular language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupName {
    /// An opaque name from a provider, user or playlist.
    Named(String),
    Ungrouped,
    Favorites,
    Recent,
    AllChannels,
    AllPlaylists,
}

/// Guide data the focus model and the UI read: channels, groups and
/// programs by guide minute (minute 0 is [`Schedule::epoch_unix`]). Implemented by
/// [`SyntheticSchedule`] (tests, demos) and by the store + EPG backed
/// schedule in the app.
pub trait Schedule {
    /// Unix time of guide minute 0. Fixed for the schedule's lifetime, so
    /// programs keep their minutes while now advances.
    fn epoch_unix(&self) -> i64 {
        SYNTHETIC_EPOCH_UNIX
    }
    /// Whether now follows the wall clock (real data). When false, now
    /// starts at minute 0 when the schedule is shown and advances from
    /// there (synthetic data, which only covers the days around its epoch).
    fn follows_wall_clock(&self) -> bool {
        false
    }
    fn channel_count(&self) -> usize;
    fn group_count(&self) -> usize;
    /// The first channel of `group`.
    fn group_start(&self, group: usize) -> usize;
    /// Stable group identity, independent of its display name.
    fn group_key(&self, group: usize) -> String {
        match self.group_label(group) {
            GroupName::Named(name) => name,
            _ => group.to_string(),
        }
    }
    /// Explicit membership permits custom order without renumbering playback channels.
    fn group_channels(&self, group: usize) -> Vec<usize> {
        if group >= self.group_count() {
            return Vec::new();
        }
        let start = self.group_start(group).min(self.channel_count());
        let end = if group + 1 < self.group_count() {
            self.group_start(group + 1)
                .clamp(start, self.channel_count())
        } else {
            self.channel_count()
        };
        (start..end).collect()
    }
    /// The owning category's channels in display order, independent of which
    /// category rows the browser currently shows. Channels without a category
    /// use the playlist's order.
    fn channel_group_channels(&self, channel: usize) -> Vec<usize> {
        crate::lists::group_of(self, channel)
            .map_or_else(|| self.playlist_order(), |g| self.group_channels(g))
    }
    /// Hidden channels remain addressable for an already playing stream.
    fn channel_hidden(&self, _channel: usize) -> bool {
        false
    }
    /// The channel's direct or inherited playback block.
    fn channel_block(&self, _channel: usize) -> crate::organization::Block {
        crate::organization::Block::None
    }
    /// Whether this group has a direct guide block.
    fn group_blocked(&self, _group: usize) -> bool {
        false
    }
    /// The program airing at `minute`; gaps without guide data are filled
    /// with placeholder programs so navigation always has a cell.
    fn program_at(&self, channel: usize, minute: i32) -> Program;
    /// The programs overlapping `[from, from + minutes)`, in order.
    fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program>;
    /// The channel's archive window in days (0: no catch-up).
    fn catchup_days(&self, channel: usize) -> u8;
    fn channel_name(&self, channel: usize) -> String;
    /// The provider's channel number when it has one, else the 1-based
    /// position. (The guide numbers rows by their place in the list it
    /// shows: [`crate::lists::ListView`].)
    fn channel_number(&self, channel: usize) -> u32 {
        channel as u32 + 1
    }
    /// The channel's logo address, when the playlist names one.
    fn channel_logo(&self, _channel: usize) -> Option<String> {
        None
    }
    /// Every channel in the playlist's own order (for "All channels"):
    /// schedules store channels grouped. Default: as stored.
    fn playlist_order(&self) -> Vec<usize> {
        (0..self.channel_count()).collect()
    }
    fn group_label(&self, group: usize) -> GroupName;

    /// A playlist parent in a multi-source group browser.
    fn group_is_playlist(&self, _group: usize) -> bool {
        false
    }
    fn group_is_collapsed(&self, _group: usize) -> bool {
        false
    }
    fn group_depth(&self, _group: usize) -> usize {
        0
    }
    /// The all-channels entry can represent a single playlist or all playlists.
    fn all_channels_label(&self) -> GroupName {
        GroupName::AllChannels
    }

    /// The listing at `program`, or `None` for a row with no channel.
    fn listing(&self, channel: usize, program: Program) -> Option<Listing>;

    /// The channel's live stream URL, when the schedule knows it.
    fn stream_url(&self, _channel: usize) -> Option<String> {
        None
    }
    /// The archive URL that plays `program` from its start at guide minute
    /// `now`, when the channel has catch-up and the schedule can build it.
    fn catchup_url(&self, _channel: usize, _program: Program, _now: i32) -> Option<String> {
        None
    }
    /// The channel's content identity (stable across refreshes), used to
    /// remember channels (previous/last channel) without list indexes.
    fn channel_key(&self, _channel: usize) -> Option<String> {
        None
    }
    /// The channel with `key`, if this schedule has it. The default looks
    /// at every channel; a schedule with many keeps an index.
    fn find_channel(&self, key: &str) -> Option<usize> {
        (0..self.channel_count()).find(|&c| self.channel_key(c).as_deref() == Some(key))
    }
    /// The group holding `channel`, a category before a playlist parent
    /// (which holds the same channel); `channel` is in range and there are
    /// groups ([`crate::lists::group_of`] checks). The default asks every
    /// group for its channels ([`crate::lists::scan_group_of`]); a schedule
    /// with many keeps an index.
    fn channel_group(&self, channel: usize) -> Option<usize> {
        crate::lists::scan_group_of(self, channel)
    }
    /// The guide minutes that can be browsed at `now`, `[first, end)`: by
    /// default [`HISTORY`] back and [`HORIZON`] ahead; a stored guide from
    /// the oldest retained data to a little past its last program. Supply
    /// `now` as the first minute when there are no retained past listings.
    fn browsable(&self, now: i32) -> (i32, i32) {
        (now - HISTORY, now + HORIZON)
    }
    /// The first retained guide minute for this channel. A fixture can use
    /// the default schedule extent at its epoch; a sparse guide reports
    /// `None` for a row without listings.
    fn listing_start(&self, _channel: usize) -> Option<i32> {
        Some(self.browsable(0).0)
    }
    /// The concrete schedule, for features that need more than this trait
    /// (program search reads the EPG store; playback resolves streams).
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        None
    }
}

/// Guide minute `wall_unix` falls in on `s`: from the wall clock for real
/// data, or `shown_secs` (time since the schedule was shown) for a schedule
/// that does not follow it.
pub fn now_minute(s: &dyn Schedule, wall_unix: i64, shown_secs: u64) -> i32 {
    if s.follows_wall_clock() {
        (wall_unix - s.epoch_unix()).div_euclid(60) as i32
    } else {
        (shown_secs / 60).min(i32::MAX as u64) as i32
    }
}

/// Unix time of guide `minute` on `s`.
pub fn unix_of(s: &dyn Schedule, minute: i32) -> i64 {
    s.epoch_unix() + i64::from(minute) * 60
}

/// Seconds until `now_minute` next changes (at least one).
pub fn secs_to_next_minute(s: &dyn Schedule, wall_unix: i64, shown_secs: u64) -> u64 {
    let into = if s.follows_wall_clock() {
        (wall_unix - s.epoch_unix()).rem_euclid(60) as u64
    } else {
        shown_secs % 60
    };
    60 - into
}

/// Deterministic synthetic guide data: [`CHANNELS`] channels in [`GROUPS`]
/// groups, programs of 15–120 minutes from a week back to the horizon,
/// and catch-up on even channels.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyntheticSchedule;

impl Schedule for SyntheticSchedule {
    fn channel_count(&self) -> usize {
        CHANNELS
    }
    fn group_count(&self) -> usize {
        GROUPS
    }
    fn group_start(&self, group: usize) -> usize {
        group_start(group)
    }
    fn program_at(&self, channel: usize, minute: i32) -> Program {
        program_at(channel, minute)
    }
    fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program> {
        let to = from + minutes;
        program_seq(channel)
            .skip_while(|p| p.end <= from)
            .take_while(|p| p.start < to)
            .collect()
    }
    fn catchup_days(&self, channel: usize) -> u8 {
        catchup_days(channel)
    }
    fn channel_name(&self, channel: usize) -> String {
        format!("Channel {}", channel + 1)
    }
    fn group_label(&self, group: usize) -> GroupName {
        GroupName::Named(format!("Group {:02}", group + 1))
    }
    fn listing(&self, channel: usize, program: Program) -> Option<Listing> {
        (channel < self.channel_count())
            .then(|| Listing::Title(format!("Show {}", program.start.rem_euclid(97))))
    }
    fn channel_key(&self, channel: usize) -> Option<String> {
        (channel < CHANNELS).then(|| format!("synthetic:{channel}"))
    }
    fn find_channel(&self, key: &str) -> Option<usize> {
        key.strip_prefix("synthetic:")
            .and_then(|n| n.parse().ok())
            .filter(|&c| c < CHANNELS)
    }
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x
}

/// The `n`th program of `channel`, deterministic and cheap to recompute.
fn program_seq(channel: usize) -> impl Iterator<Item = Program> {
    const LENGTHS: [i32; 8] = [15, 30, 30, 45, 60, 60, 90, 120];
    let offset = (mix(channel as u64) % 60) as i32;
    // Starts before the guide's earliest minute (-HISTORY), so no row is blank.
    let mut start = -offset - HISTORY - 60;
    (0u64..).map(move |n| {
        let len = LENGTHS[(mix((channel as u64) << 20 | n) % 8) as usize];
        let p = Program {
            start,
            end: start + len,
        };
        start += len;
        p
    })
}

/// Synthetic catch-up window: even channels keep a 3-day archive, odd
/// channels have none (so both cases are always on screen).
pub fn catchup_days(channel: usize) -> u8 {
    if channel.is_multiple_of(2) { 3 } else { 0 }
}

/// How far past its epoch the synthetic schedule has data (now advances
/// while the app runs, so this exceeds [`HORIZON`]).
const SYNTHETIC_AHEAD: i32 = 2 * 24 * 60;

/// The program of `channel` airing at `minute` (clamped to the guide's data).
pub fn program_at(channel: usize, minute: i32) -> Program {
    let minute = minute.clamp(-HISTORY, SYNTHETIC_AHEAD - 1);
    program_seq(channel)
        .find(|p| p.end > minute)
        .expect("endless")
}

/// The first channel of synthetic `group`.
pub fn group_start(group: usize) -> usize {
    group * CHANNELS.div_ceil(GROUPS)
}

/// Minutes in a day (Ctrl + Left/Right).
pub const DAY: i32 = 24 * 60;
/// Minutes Shift + Left/Right move the time column by.
pub const HOUR: i32 = 60;

pub use crate::guide_state::{
    Focus, Move, Pane, PreviewOrigin, ProgramSelection, STEP, home_window, moves, page_row,
    view_range,
};

#[cfg(test)]
mod tests {
    use super::*;

    const S: SyntheticSchedule = SyntheticSchedule;

    /// A small schedule with real-data time: the synthetic programs, a
    /// wall-clock epoch, and `channels`/`groups` rows.
    struct Small {
        channels: usize,
        groups: usize,
    }

    const WALL_EPOCH: i64 = 1_800_000_000;

    impl Schedule for Small {
        fn epoch_unix(&self) -> i64 {
            WALL_EPOCH
        }
        fn follows_wall_clock(&self) -> bool {
            true
        }
        fn channel_count(&self) -> usize {
            self.channels
        }
        fn group_count(&self) -> usize {
            self.groups
        }
        fn group_start(&self, group: usize) -> usize {
            group
        }
        fn program_at(&self, channel: usize, minute: i32) -> Program {
            S.program_at(channel, minute)
        }
        fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program> {
            S.programs_between(channel, from, minutes)
        }
        fn catchup_days(&self, _channel: usize) -> u8 {
            0
        }
        fn channel_name(&self, channel: usize) -> String {
            format!("Small {channel}")
        }
        fn group_label(&self, group: usize) -> GroupName {
            GroupName::Named(format!("Small group {group}"))
        }
        fn listing(&self, _channel: usize, _program: Program) -> Option<Listing> {
            Some(Listing::Gap)
        }
    }

    #[test]
    fn schedules_are_contiguous_and_cover_now() {
        for ch in [0, 1, 77, CHANNELS - 1] {
            let ps = S.programs_between(ch, 0, VISIBLE);
            assert!(ps[0].start <= 0 && ps[0].end > 0);
            for w in ps.windows(2) {
                assert_eq!(w[0].end, w[1].start);
            }
            assert!(ps.last().unwrap().end >= VISIBLE);
        }
    }

    #[test]
    fn airing_and_progress() {
        let live = Program {
            start: -15,
            end: 45,
        };
        assert_eq!(live.airing(0), Airing::Live);
        assert!((live.progress(0) - 0.25).abs() < 1e-6);
        assert_eq!(Program { start: -60, end: 0 }.airing(0), Airing::Ended);
        assert_eq!(Program { start: 10, end: 40 }.airing(0), Airing::Upcoming);
    }

    #[test]
    fn now_comes_from_the_wall_clock_or_from_when_the_schedule_was_shown() {
        let real = Small {
            channels: 3,
            groups: 1,
        };
        assert_eq!(now_minute(&real, WALL_EPOCH + 25 * 60 + 59, 9_999), 25);
        assert_eq!(now_minute(&real, WALL_EPOCH - 1, 0), -1);
        assert_eq!(secs_to_next_minute(&real, WALL_EPOCH + 25 * 60 + 59, 0), 1);
        assert_eq!(secs_to_next_minute(&real, WALL_EPOCH, 0), 60);
        assert_eq!(unix_of(&real, 25), WALL_EPOCH + 25 * 60);
        // The synthetic schedule keeps its epoch; its now is time since shown.
        assert_eq!(S.epoch_unix(), SYNTHETIC_EPOCH_UNIX);
        assert_eq!(now_minute(&S, WALL_EPOCH, 125), 2);
        assert_eq!(secs_to_next_minute(&S, WALL_EPOCH, 125), 55);
    }

    #[test]
    fn airing_and_progress_follow_now() {
        let p = Program { start: 30, end: 90 };
        assert_eq!(p.airing(0), Airing::Upcoming);
        assert_eq!(p.airing(30), Airing::Live);
        assert!((p.progress(45) - 0.25).abs() < 1e-6);
        assert_eq!(p.airing(90), Airing::Ended);
        assert_eq!(p.progress(90), 0.0);
    }
}
