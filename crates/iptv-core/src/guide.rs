//! The guide's data and its remote-control focus model, free of UI so the
//! navigation rules are unit-tested on the host.
//!
//! Focus is owned by the app: NSH's Android backend gives ordinary views no
//! D-pad focus, so the keymap feeds [`Focus::apply`] and the UI draws the
//! focused cell from it. Focus and playback are separate states.
//!
//! Time: guide minutes count from the schedule's epoch
//! ([`Schedule::epoch_unix`], minute 0), and "now" is a guide minute that
//! advances ([`Focus::now`], set by [`Focus::tick`] from [`now_minute`]).
//! Every "live", "past" and window rule is relative to that now.

pub mod floor;
pub mod moves;

pub use floor::{floor_of, view_range};

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
/// The time bar's step when the window has to move.
pub const STEP: i32 = 30;
/// How far before "now" the guide can show (the window's lower bound).
pub const PAST: i32 = 30;
/// How far back the synthetic guide has data (and a schedule's default
/// [`Schedule::browsable`] reaches). The view and the keys go back only as
/// far as catch-up does ([`floor`]).
pub const HISTORY: i32 = 7 * 24 * 60;

/// Where a programme sits relative to now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Airing {
    Ended,
    Live,
    Upcoming,
}

impl Program {
    /// Where the programme sits at guide minute `now`.
    pub fn airing(&self, now: i32) -> Airing {
        if self.end <= now {
            Airing::Ended
        } else if self.start <= now {
            Airing::Live
        } else {
            Airing::Upcoming
        }
    }

    /// Played fraction of a live programme at `now` (0 otherwise).
    pub fn progress(&self, now: i32) -> f32 {
        match self.airing(now) {
            Airing::Live => (now - self.start) as f32 / (self.end - self.start) as f32,
            _ => 0.0,
        }
    }
}

/// A programme: `[start, end)` in guide minutes from the schedule's epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program {
    pub start: i32,
    pub end: i32,
}

/// Guide data the focus model and the UI read: channels, groups and
/// programmes by guide minute (minute 0 is [`Schedule::epoch_unix`]). Implemented by
/// [`SyntheticSchedule`] (tests, demos) and by the store + EPG backed
/// schedule in the app.
pub trait Schedule {
    /// Unix time of guide minute 0. Fixed for the schedule's lifetime, so
    /// programmes keep their minutes while now advances.
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
        self.group_name(group)
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
    /// The programme airing at `minute`; gaps without guide data are filled
    /// with placeholder programmes so navigation always has a cell.
    fn program_at(&self, channel: usize, minute: i32) -> Program;
    /// The programmes overlapping `[from, from + minutes)`, in order.
    fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program>;
    /// The channel's archive window in days (0: no catch-up).
    fn catchup_days(&self, channel: usize) -> u8;
    /// The longest archive window of any channel ([`floor::view_range`]).
    fn max_catchup_days(&self) -> u8 {
        (0..self.channel_count())
            .map(|c| self.catchup_days(c))
            .max()
            .unwrap_or(0)
    }
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
    fn group_name(&self, group: usize) -> String;

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
    fn all_channels_name(&self) -> String {
        "All channels".into()
    }

    fn title(&self, channel: usize, program: Program) -> String;
    /// Translate generated gap/loading labels only; provider titles stay opaque.
    fn title_in(
        &self,
        channel: usize,
        program: Program,
        _language: &crate::i18n::Language,
    ) -> String {
        self.title(channel, program)
    }
    /// Generated group labels may be translated by adapters that know their origin.
    fn group_name_in(&self, group: usize, _language: &crate::i18n::Language) -> String {
        self.group_name(group)
    }
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
    /// the oldest day its store keeps to a little past its last programme.
    /// At least [`VISIBLE`] minutes around now.
    fn browsable(&self, now: i32) -> (i32, i32) {
        (now - HISTORY, now + HORIZON)
    }
    /// The concrete schedule, for features that need more than this trait
    /// (programme search reads the EPG store; playback resolves streams).
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
/// groups, programmes of 15–120 minutes from a week back to the horizon,
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
    fn max_catchup_days(&self) -> u8 {
        catchup_days(0)
    }
    fn channel_name(&self, channel: usize) -> String {
        format!("Channel {}", channel + 1)
    }
    fn group_name(&self, group: usize) -> String {
        format!("Group {:02}", group + 1)
    }
    fn title(&self, _channel: usize, program: Program) -> String {
        format!("Show {}", program.start.rem_euclid(97))
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

/// The `n`th programme of `channel`, deterministic and cheap to recompute.
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

/// The programme of `channel` airing at `minute` (clamped to the guide's data).
pub fn program_at(channel: usize, minute: i32) -> Program {
    let minute = minute.clamp(-HISTORY, SYNTHETIC_AHEAD - 1);
    program_seq(channel)
        .find(|p| p.end > minute)
        .expect("endless")
}

/// The programmes of `channel` overlapping `[from, from + VISIBLE)`.
pub fn visible_programs(channel: usize, from: i32) -> Vec<Program> {
    let to = from + VISIBLE;
    program_seq(channel)
        .skip_while(|p| p.end <= from)
        .take_while(|p| p.start < to)
        .collect()
}

/// Which surface owns the D-pad: the navigation rail, the groups column
/// (both shown beside the guide while either has the focus) or the guide
/// grid. The columns' rules are in [`crate::browse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Rail,
    Groups,
    Guide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
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
    /// The selected entry of the groups column ([`crate::lists`]): the
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

/// The first channel of synthetic `group`.
pub fn group_start(group: usize) -> usize {
    group * CHANNELS.div_ceil(GROUPS)
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

    /// Now moved to `now`. A focus on the live programme follows the
    /// clock (onto the next programme when it ends) and a window at its
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
            (Pane::Rail | Pane::Groups, _) => return,
            // Rows keep the time column, but not before a row's floor: on a
            // row without catch-up the focus is on now's programme at least.
            (Pane::Guide, Move::Up) => {
                self.channel = self.channel.saturating_sub(1);
                self.lift_to_floor(s);
            }
            (Pane::Guide, Move::Down) => {
                self.channel = (self.channel + 1).min(s.channel_count().saturating_sub(1));
                self.lift_to_floor(s);
            }
            // Left steps to the previous programme down to the row's floor
            // (the oldest replay, or now's programme without catch-up);
            // there it shows the groups.
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

    /// One programme back, down to the row's floor, never to the groups
    /// (`Action::PastPrograms`).
    pub fn enter_past(&mut self, s: &dyn Schedule) {
        if self.pane == Pane::Guide {
            self.step_back(s);
            self.keep_cursor_visible(s);
        }
    }

    /// Back on a past or future programme: the channel's live programme and
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
    /// channel): its live programme and the default window.
    pub fn show_channel(&mut self, channel: usize, s: &dyn Schedule) {
        self.pane = Pane::Guide;
        self.channel = channel.min(s.channel_count().saturating_sub(1));
        self.cursor = self.now;
        self.window = home_window(self.now);
    }

    /// A tap on the programme of `channel` airing at `minute`: it gets the
    /// focus where it is (the window does not move, the cell is on screen);
    /// a past one without a replay leaves it on the programme on air.
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

    /// The focused programme (in the guide pane).
    pub fn program(&self, s: &dyn Schedule) -> Program {
        s.program_at(self.channel, self.cursor)
    }

    pub(crate) fn keep_cursor_visible(&mut self, s: &dyn Schedule) {
        // The focused programme's start (now, for a live one) stays on
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

    const S: SyntheticSchedule = SyntheticSchedule;

    #[test]
    fn schedules_are_contiguous_and_cover_now() {
        for ch in [0, 1, 77, CHANNELS - 1] {
            let ps = visible_programs(ch, 0);
            assert!(ps[0].start <= 0 && ps[0].end > 0);
            for w in ps.windows(2) {
                assert_eq!(w[0].end, w[1].start);
            }
            assert!(ps.last().unwrap().end >= VISIBLE);
        }
    }

    #[test]
    fn show_channel_focuses_its_live_programme() {
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
    fn left_from_the_current_programme_moves_to_groups_and_back() {
        let mut f = Focus {
            channel: 9,
            ..Focus::default()
        };
        f.apply(Move::Right, &S);
        f.apply(Move::Left, &S);
        assert_eq!(f.pane, Pane::Guide);
        f.apply(Move::Left, &S);
        assert_eq!(f.pane, Pane::Groups);
        // The grid ignores directions while the columns have the focus.
        f.apply(Move::Down, &S);
        assert_eq!((f.pane, f.channel), (Pane::Groups, 9));
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
        // The archive of the synthetic catch-up channels: three days.
        f.shift_window(-HISTORY * 2, &S);
        assert_eq!(f.window, -3 * 24 * 60);
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
    fn right_then_left_into_the_groups_keeps_the_past_shown() {
        // The device sequence: Channel 15, Right x3, Left x4 (the last opens the groups).
        for channel in [15, 1, 3, 77] {
            let mut f = Focus {
                channel,
                ..Focus::default()
            };
            for _ in 0..3 {
                f.apply(Move::Right, &S);
            }
            for _ in 0..4 {
                f.apply(Move::Left, &S);
            }
            assert_eq!(f.window, -PAST, "channel {channel}: {f:?}");
        }
    }

    #[test]
    fn a_replay_row_enters_the_past_and_left_right_step_programmes() {
        let mut f = Focus {
            channel: 4,
            ..Focus::default()
        };
        let live = f.program(&S);
        f.enter_past(&S);
        let first = f.program(&S);
        assert_eq!(first.end, live.start, "the programme before the live one");
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

    #[test]
    fn the_past_reaches_the_archive_and_never_without_catchup() {
        // Odd: no catch-up. Earlier programmes stop at now's.
        let mut f = Focus {
            channel: 1,
            ..Focus::default()
        };
        assert_eq!(catchup_days(1), 0);
        let live = f.program(&S);
        for _ in 0..20 {
            f.enter_past(&S);
        }
        assert_eq!(f.program(&S), live);
        // Even: three days back, no further.
        f.channel = 2;
        for _ in 0..2000 {
            f.enter_past(&S);
        }
        let start = f.program(&S).start;
        assert!(
            (-3 * 24 * 60..-3 * 24 * 60 + 120).contains(&start),
            "three days back: {:?}",
            f.program(&S)
        );
        assert!(f.window >= -3 * 24 * 60);
    }

    /// A small schedule with real-data time: the synthetic programmes, a
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
        fn group_name(&self, group: usize) -> String {
            format!("Small group {group}")
        }
        fn title(&self, _channel: usize, _program: Program) -> String {
            String::new()
        }
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
    fn a_tick_moves_a_focus_on_now_onto_the_next_live_programme() {
        let mut f = Focus::at(0);
        let live = f.program(&S);
        assert!(!f.tick(0, &S), "same minute");
        assert!(f.tick(live.end, &S));
        assert_eq!(f.cursor, live.end);
        assert_eq!(f.program(&S).start, live.end, "the next programme");
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
        assert_eq!(f.window, 10 * 60 - 3 * 24 * 60);
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
        f.apply(Move::Down, &small);
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
    fn edges_clamp() {
        let mut f = Focus::default();
        f.apply(Move::Up, &S);
        assert_eq!(f.channel, 0);
        f.channel = CHANNELS - 1;
        f.apply(Move::Down, &S);
        assert_eq!(f.channel, CHANNELS - 1);
    }
}
