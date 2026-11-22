//! Guide focus, the selected listing and viewport movement.

mod floor;
pub mod moves;
pub use floor::view_range;

use iptv_core::guide::{Airing, Program, Schedule, VISIBLE};

/// Minutes before now in the default guide window.
pub const PAST: i32 = 30;

/// Minutes per step in the guide time header.
pub const STEP: i32 = 30;
/// The row reached by a signed page step, clamped to the list.
pub fn page_row(row: usize, rows: usize, step: isize) -> usize {
    row.saturating_add_signed(step).min(rows.saturating_sub(1))
}

/// The result of selecting a listing in the guide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramSelection {
    Live,
    AlreadyLive,
    Replay,
    Upcoming,
    Unavailable,
}

impl ProgramSelection {
    /// The playback or information action for this listing. The selected and
    /// playing channels are catalog indices on the same schedule.
    pub fn for_program(
        program: &Program,
        now: i32,
        archive_days: u8,
        channel: usize,
        live_channel: Option<usize>,
    ) -> Self {
        match program.airing(now) {
            Airing::Live if live_channel == Some(channel) => Self::AlreadyLive,
            Airing::Live => Self::Live,
            Airing::Ended if program.can_replay(now, archive_days) => Self::Replay,
            Airing::Ended => Self::Unavailable,
            Airing::Upcoming => Self::Upcoming,
        }
    }
}

/// Which surface owns the D-pad: the navigation rail, the groups column
/// (both shown beside the guide while either has the focus) or the guide
/// grid. The columns' rules are in [`crate::browse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Rail,
    Groups,
    Guide,
    /// The existing playing video above the guide and navigation columns.
    Preview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
}

impl Move {
    pub(crate) fn wrap_vertical(self, current: usize, count: usize) -> usize {
        let last = count.saturating_sub(1);
        let current = current.min(last);
        match self {
            Self::Up => current.checked_sub(1).unwrap_or(last),
            Self::Down if current < last => current + 1,
            Self::Down => 0,
            Self::Left | Self::Right => current,
        }
    }
}

/// The remote's focus: the pane, the selected list and the focused row in
/// it, and the time cursor that Up/Down keep, so moving between rows stays
/// in the same time column (TiviMate-style guide navigation).
///
/// `channel` is a row of the list the guide shows, and the schedule the
/// focus methods take is that list ([`crate::lists::ListView`]); the
/// catalog channel behind a row comes from the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub pane: Pane,
    /// The selected entry of the groups column ([`iptv_core::lists`]): the
    /// list the guide shows.
    pub group: usize,
    /// The focused row of that list.
    pub channel: usize,
    /// The focused item of the navigation rail ([`crate::browse::RailItem`]).
    pub rail: crate::browse::RailItem,
    /// The hint card shown after a group was chosen; Back dismisses it
    /// first.
    pub hint: bool,
    pub cursor: i32,
    /// The first minute shown across the guide.
    pub window: i32,
    /// Minutes visible across the grid, independent of navigation step size.
    pub visible: i32,
    /// The guide minute it is now; [`Focus::tick`] advances it.
    pub now: i32,
}

impl Default for Focus {
    /// The guide at minute 0 (a synthetic schedule just shown).
    fn default() -> Self {
        Focus::at(0)
    }
}

/// The window that shows `now` with between half an hour and an hour of
/// the past before it, aligned to the time bar's steps.
pub fn home_window(now: i32) -> i32 {
    (now - PAST).div_euclid(STEP) * STEP
}

impl Focus {
    /// The guide at the first channel of "All channels" at guide minute
    /// `now`, with the past half hour shown.
    pub fn at(now: i32) -> Self {
        Focus {
            pane: Pane::Guide,
            group: crate::lists::ALL,
            channel: 0,
            rail: crate::browse::RailItem::Tv,
            hint: false,
            cursor: now,
            window: home_window(now),
            visible: VISIBLE,
            now,
        }
    }

    /// The earliest and latest minute the view shows on `s`
    /// ([`view_range`]); the cursor stays within them and at or after the
    /// focused row's [`Focus::floor`].
    pub(crate) fn bounds(&self, s: &dyn Schedule) -> (i32, i32) {
        let (first, end) = view_range(s, self.now);
        (first, (end - 1).max(first + self.visible - 1))
    }

    /// Resize the time window while retaining focused content.
    pub fn set_visible(&mut self, minutes: i32, s: &dyn Schedule) {
        self.visible = minutes.clamp(60, 360);
        self.keep_cursor_visible(s);
    }

    /// Now moved to `now`. A focus on the live program follows the
    /// clock (onto the next program when it ends) and a window at its
    /// default position moves with now; a focus browsing the past or the
    /// future stays where it is. Returns false when `now` is unchanged.
    pub fn tick(&mut self, now: i32, s: &dyn Schedule) -> bool {
        if now == self.now {
            return false;
        }
        let following =
            self.pane == Pane::Guide && self.program(s).airing(self.now) == Airing::Live;
        let home = self.window == home_window(self.now);
        let old = self.now;
        self.now = now;
        if following || self.cursor == old {
            self.cursor = now;
        }
        if home {
            self.window = home_window(now);
        }
        let (lo, hi) = self.bounds(s);
        self.cursor = self.cursor.clamp(lo, hi).max(self.floor(s));
        if following {
            self.keep_cursor_visible(s);
        } else {
            self.window = self.window.clamp(lo, (hi + 1 - self.visible).max(lo));
        }
        true
    }

    /// Keeps the focused row inside the list `s` (after it changed, or on
    /// an empty list: 0).
    pub fn clamp_to(&mut self, s: &dyn Schedule) {
        self.channel = self.channel.min(s.channel_count().saturating_sub(1));
    }

    /// A direction in the guide grid (`s`: the list it shows). The rail and
    /// the groups column move through [`Focus::navigate`].
    pub fn apply(&mut self, m: Move, s: &dyn Schedule) {
        match (self.pane, m) {
            (Pane::Rail | Pane::Groups | Pane::Preview, _) => return,
            // Rows keep the time column within retained guide coverage.
            (Pane::Guide, Move::Up | Move::Down) => {
                self.channel = m.wrap_vertical(self.channel, s.channel_count());
                self.lift_to_floor(s);
            }
            // Left shows the groups after the earliest retained listing.
            (Pane::Guide, Move::Left) => match self.previous(s) {
                Some(p) => self.cursor = p.start.max(self.floor(s)),
                None => self.pane = Pane::Groups,
            },
            (Pane::Guide, Move::Right) => {
                let current = s.program_at(self.channel, self.cursor);
                if current.end <= self.bounds(s).1 {
                    self.cursor = current.end;
                }
            }
        }
        self.keep_cursor_visible(s);
    }

    /// One program back, down to the row's floor, never to the groups
    /// (`Action::PastPrograms`).
    pub fn enter_past(&mut self, s: &dyn Schedule) {
        if self.pane == Pane::Guide {
            self.step_back(s);
            self.keep_cursor_visible(s);
        }
    }

    /// Back on a past or future program: the channel's live program and
    /// the default window. Returns false when already there.
    pub fn return_to_now(&mut self, s: &dyn Schedule) -> bool {
        if self.pane != Pane::Guide || self.program(s).airing(self.now) == Airing::Live {
            return false;
        }
        self.cursor = self.now;
        self.window = home_window(self.now);
        true
    }

    pub(crate) fn step_back(&mut self, s: &dyn Schedule) {
        if let Some(p) = self.previous(s) {
            self.cursor = p.start.max(self.floor(s));
        }
    }

    /// The list's first row at now (a list just chosen or previewed).
    pub(crate) fn go_to_top(&mut self) {
        self.channel = 0;
        self.cursor = self.now;
        self.window = home_window(self.now);
    }

    /// Shows what plays on row `channel` of the list `s` (a zap, a resumed
    /// channel): its live program and the default window.
    pub fn show_channel(&mut self, channel: usize, s: &dyn Schedule) {
        self.pane = Pane::Guide;
        self.channel = channel.min(s.channel_count().saturating_sub(1));
        self.cursor = self.now;
        self.window = home_window(self.now);
    }

    /// A tap on the program of `channel` airing at `minute`: it gets the
    /// focus where it is (the window stays on the visible cell).
    pub fn tap(&mut self, channel: usize, minute: i32, s: &dyn Schedule) {
        self.pane = Pane::Guide;
        self.channel = channel.min(s.channel_count().saturating_sub(1));
        let (lo, hi) = self.bounds(s);
        self.cursor = minute.clamp(lo, hi).max(self.floor(s));
    }

    /// The time window moved by touch: the cursor follows into it, but not
    /// before the focused row's floor (the next key brings it on screen).
    pub fn shift_window(&mut self, minutes: i32, s: &dyn Schedule) {
        let (lo, hi) = self.bounds(s);
        self.window = (self.window + minutes).clamp(lo, hi + 1 - self.visible);
        let lo = self.window;
        self.cursor = self
            .cursor
            .clamp(lo, (self.window + self.visible - 1).max(lo));
        self.cursor = self.cursor.max(self.floor(s));
    }

    /// After a touch scroll, the first key focuses a row on screen instead
    /// of jumping back to a row the user scrolled away from.
    pub fn adopt_rows(&mut self, first: usize, last: usize) {
        if self.channel < first || self.channel > last {
            self.channel = if self.channel < first { first } else { last };
        }
    }

    /// The focused program (in the guide pane).
    pub fn program(&self, s: &dyn Schedule) -> Program {
        s.program_at(self.channel, self.cursor)
    }

    pub(crate) fn keep_cursor_visible(&mut self, s: &dyn Schedule) {
        // The focused program's start (now, for a live one) stays on
        // screen; the window moves in steps.
        let p = self.program(s);
        let anchor = if p.airing(self.now) == Airing::Live {
            self.now
        } else {
            p.start
        };
        // In (or before) the first column: step back so a column of context
        // shows before it (half an hour of the past when it is live).
        if anchor < self.window + STEP {
            self.window = (anchor - STEP).div_euclid(STEP) * STEP;
        }
        while anchor >= self.window + self.visible - STEP {
            self.window += STEP;
        }
        let (lo, hi) = self.bounds(s);
        self.window = self.window.clamp(lo, hi + 1 - self.visible);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::guide::{
        CHANNELS, GroupName, HISTORY, HORIZON, Listing, SyntheticSchedule, group_start,
    };

    const S: SyntheticSchedule = SyntheticSchedule;

    #[test]
    fn show_channel_focuses_its_live_program() {
        // A real guide shown 53 minutes after its epoch, browsing the past.
        let mut f = Focus::at(53);
        f.apply(Move::Left, &S);
        f.apply(Move::Left, &S);
        let channel = group_start(3) + 2;
        f.show_channel(channel, &S);
        assert_eq!(
            (f.pane, f.channel, f.group),
            (Pane::Guide, channel, crate::lists::ALL),
            "the list stays"
        );
        assert_eq!(f.cursor, 53, "the cursor is at now, not at the epoch");
        assert_eq!(f.window, home_window(53));
        assert_eq!(f.program(&S).airing(f.now), Airing::Live);
        f.show_channel(usize::MAX, &S);
        assert_eq!(f.channel, CHANNELS - 1);
    }

    #[test]
    fn up_down_keep_the_time_column() {
        let mut f = Focus::default();
        for _ in 0..4 {
            f.apply(Move::Right, &S);
        }
        let cursor = f.cursor;
        assert!(cursor > 0);
        for _ in 0..50 {
            f.apply(Move::Down, &S);
            assert_eq!(f.cursor, cursor, "Down must not drift the time cursor");
            let p = f.program(&S);
            assert!(p.start <= cursor && cursor < p.end);
        }
        assert_eq!(f.channel, 50);
    }

    #[test]
    fn the_window_follows_the_cursor_within_the_horizon() {
        let mut f = Focus::default();
        for _ in 0..500 {
            f.apply(Move::Right, &S);
            let start = f.program(&S).start.max(0);
            assert!(f.window <= start && start < f.window + VISIBLE);
        }
        assert!(f.window <= HORIZON - VISIBLE);
    }

    #[test]
    fn the_guide_opens_with_the_past_half_hour_shown() {
        let f = Focus::default();
        assert_eq!((f.window, f.cursor), (-PAST, 0));
        assert_eq!(f.program(&S).airing(0), Airing::Live);
    }

    #[test]
    fn choosing_a_program_uses_the_airing_and_archive_boundary() {
        let program = Program {
            start: 120,
            end: 180,
        };
        let select =
            |now, days, playing| ProgramSelection::for_program(&program, now, days, 2, playing);
        assert_eq!(select(119, 1, Some(2)), ProgramSelection::Upcoming);
        assert_eq!(select(120, 1, Some(1)), ProgramSelection::Live);
        assert_eq!(select(179, 1, Some(2)), ProgramSelection::AlreadyLive);
        assert_eq!(select(180, 0, Some(2)), ProgramSelection::Unavailable);
        assert_eq!(select(180, 1, Some(2)), ProgramSelection::Replay);
        assert_eq!(select(120 + 1440, 1, None), ProgramSelection::Replay);
        assert_eq!(select(121 + 1440, 1, None), ProgramSelection::Unavailable);
    }

    #[test]
    fn a_tap_focuses_in_place_without_moving_the_window() {
        let mut f = Focus {
            pane: Pane::Groups,
            ..Focus::default()
        };
        f.tap(321, 50, &S);
        assert_eq!((f.pane, f.channel, f.window), (Pane::Guide, 321, -PAST));
        let p = f.program(&S);
        assert!(p.start <= 50 && 50 < p.end);
    }

    #[test]
    fn shifting_the_window_by_touch_pulls_the_cursor_along() {
        let mut f = Focus::default();
        f.shift_window(3 * STEP, &S);
        assert_eq!(f.window, 60);
        assert!(f.cursor >= 60 && f.cursor < 60 + VISIBLE);
        // Touch can browse back into history too (Long Left's range).
        f.shift_window(-10 * STEP, &S);
        assert_eq!(f.window, 60 - 10 * STEP);
        assert!(f.cursor >= f.window && f.cursor < f.window + VISIBLE);
        // Both directions stop at the synthetic guide's bounds.
        f.shift_window(-HISTORY * 2, &S);
        assert_eq!(f.window, -HISTORY);
        f.shift_window(HISTORY + HORIZON, &S);
        assert_eq!(f.window, HORIZON - VISIBLE);
    }

    #[test]
    fn keys_after_a_touch_scroll_start_from_a_visible_row() {
        let mut f = Focus {
            channel: 10,
            ..Focus::default()
        };
        f.adopt_rows(1740, 1760);
        assert_eq!(f.channel, 1740);
        f.channel = 4000;
        f.adopt_rows(1740, 1760);
        assert_eq!(f.channel, 1760);
        f.adopt_rows(1700, 1800);
        assert_eq!(f.channel, 1760, "a visible row keeps the focus");
    }

    #[test]
    fn stepping_left_back_to_now_shows_the_past_again() {
        let mut f = Focus::default();
        for _ in 0..20 {
            f.apply(Move::Right, &S);
        }
        assert!(f.window > 0);
        while f.program(&S).start > 0 {
            f.apply(Move::Left, &S);
        }
        assert_eq!(f.window, -PAST);
    }

    #[test]
    fn right_and_left_back_to_live_keeps_the_past_shown() {
        for channel in [15, 1, 3, 77] {
            let mut f = Focus {
                channel,
                ..Focus::default()
            };
            for _ in 0..3 {
                f.apply(Move::Right, &S);
            }
            for _ in 0..3 {
                f.apply(Move::Left, &S);
            }
            assert_eq!(f.window, -PAST, "channel {channel}: {f:?}");
        }
    }

    #[test]
    fn a_replay_row_enters_the_past_and_left_right_step_programs() {
        let mut f = Focus {
            channel: 4,
            ..Focus::default()
        };
        let live = f.program(&S);
        f.enter_past(&S);
        let first = f.program(&S);
        assert_eq!(first.end, live.start, "the program before the live one");
        assert_eq!(f.pane, Pane::Guide);
        f.apply(Move::Left, &S);
        assert_eq!(
            f.program(&S).end,
            first.start,
            "short Left steps back in the past"
        );
        f.apply(Move::Right, &S);
        assert_eq!(f.program(&S), first);
        let start = f.program(&S).start;
        assert!(
            f.window <= start && start < f.window + VISIBLE,
            "the grid follows into the past"
        );
    }

    #[test]
    fn up_down_keep_the_past_time_column_and_back_returns_to_now() {
        let mut f = Focus {
            channel: 10,
            ..Focus::default()
        };
        for _ in 0..6 {
            f.enter_past(&S);
        }
        let cursor = f.cursor;
        assert!(cursor < 0);
        // Channel 12 replays too: the same time column.
        f.channel = 13;
        f.apply(Move::Up, &S);
        assert_eq!((f.channel, f.cursor), (12, cursor));
        assert_eq!(f.program(&S).airing(0), Airing::Ended);
        assert!(f.return_to_now(&S));
        assert_eq!(
            (f.cursor, f.window, f.program(&S).airing(0)),
            (0, -PAST, Airing::Live)
        );
        assert!(!f.return_to_now(&S), "already at now");
    }

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
    fn the_guide_opens_at_now_with_an_aligned_window() {
        for now in [0, 29, 47, 600, -31] {
            let f = Focus::at(now);
            assert_eq!((f.cursor, f.now), (now, now));
            assert_eq!(f.window.rem_euclid(STEP), 0, "now {now}");
            assert!(f.window <= now - PAST && now - f.window < PAST + STEP);
            assert_eq!(f.program(&S).airing(now), Airing::Live);
        }
    }

    #[test]
    fn a_tick_moves_a_focus_on_now_onto_the_next_live_program() {
        let mut f = Focus::at(0);
        let live = f.program(&S);
        assert!(!f.tick(0, &S), "same minute");
        assert!(f.tick(live.end, &S));
        assert_eq!(f.cursor, live.end);
        assert_eq!(f.program(&S).start, live.end, "the next program");
        assert_eq!(f.program(&S).airing(f.now), Airing::Live);
        assert_eq!(f.window, home_window(live.end), "the default window moves");
        // Back and the groups return to the new now.
        f.enter_past(&S);
        assert!(f.return_to_now(&S));
        assert_eq!((f.cursor, f.window), (f.now, home_window(f.now)));
    }

    #[test]
    fn a_tick_leaves_a_focus_browsing_the_past_or_future() {
        let mut f = Focus {
            channel: 4,
            ..Focus::at(0)
        };
        for _ in 0..3 {
            f.enter_past(&S);
        }
        let (cursor, window) = (f.cursor, f.window);
        assert!(f.tick(5, &S));
        assert_eq!((f.cursor, f.window, f.now), (cursor, window, 5));
        let mut f = Focus::at(0);
        for _ in 0..6 {
            f.apply(Move::Right, &S);
        }
        let (cursor, window) = (f.cursor, f.window);
        f.tick(3, &S);
        assert_eq!((f.cursor, f.window), (cursor, window));
    }

    #[test]
    fn bounds_move_with_now() {
        let mut f = Focus::at(10 * 60);
        f.shift_window(-HISTORY * 2, &S);
        assert_eq!(f.window, 10 * 60 - HISTORY);
        f.shift_window(HISTORY * 3, &S);
        assert_eq!(f.window, 10 * 60 + HORIZON - VISIBLE);
        f.tap(0, i32::MAX, &S);
        assert_eq!(f.cursor, 10 * 60 + HORIZON - 1);
    }

    #[test]
    fn a_shorter_list_clamps_the_focus() {
        let mut f = Focus {
            channel: CHANNELS - 1,
            ..Focus::at(0)
        };
        let small = Small {
            channels: 3,
            groups: 2,
        };
        f.clamp_to(&small);
        assert_eq!(f.channel, 2);
        let empty = Small {
            channels: 0,
            groups: 0,
        };
        f.clamp_to(&empty);
        assert_eq!(f.channel, 0);
        f.apply(Move::Down, &empty);
        f.apply(Move::Up, &empty);
        assert_eq!(f.channel, 0);
    }

    #[test]
    fn rows_wrap_without_losing_the_time_column() {
        let mut f = Focus::at(40);
        f.apply(Move::Right, &S);
        let cursor = f.cursor;
        f.apply(Move::Up, &S);
        assert_eq!((f.channel, f.cursor), (CHANNELS - 1, cursor));
        f.apply(Move::Down, &S);
        assert_eq!((f.channel, f.cursor), (0, cursor));

        let single = Small {
            channels: 1,
            groups: 1,
        };
        f.apply(Move::Up, &single);
        f.apply(Move::Down, &single);
        assert_eq!(f.channel, 0);
    }
}
