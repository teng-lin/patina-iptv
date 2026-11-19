//! The guide's moves along time beyond the programme steps: a desktop
//! keyboard's hour and day jumps and its key back to now, and the window a
//! touch or wheel pan settled at ([`crate::timeline`]). None goes before
//! the focused row's floor ([`super::floor`]).

#[cfg(test)]
use super::VISIBLE;
use super::{Focus, Pane, Schedule, home_window};

/// Minutes in a day (Ctrl + Left/Right).
pub const DAY: i32 = 24 * 60;
/// Minutes Shift + Left/Right move the time column by.
pub const HOUR: i32 = 60;

impl Focus {
    /// The time column and the window `minutes` later (earlier when
    /// negative): Shift + Left/Right (an hour) and Ctrl + Left/Right (a
    /// day). The cursor stays between the row's floor and the end of the
    /// guide, as Left and Right do ([`super::floor`]), and lands on the
    /// programme airing there; the window keeps its place relative to it (a
    /// long programme does not pull it back to its start) and comes along
    /// when the floor held the cursor back.
    pub fn step_time(&mut self, minutes: i32, s: &dyn Schedule) {
        if self.pane != Pane::Guide {
            return;
        }
        let (lo, hi) = self.bounds(s);
        self.window = (self.window + minutes).clamp(lo, hi + 1 - self.visible);
        self.cursor = (self.cursor + minutes).clamp(lo, hi);
        self.cursor = self
            .cursor
            .clamp(self.window, self.window + self.visible - 1);
        self.lift_to_floor(s);
    }

    /// Home: the focused channel's live programme in the default window,
    /// also when the focus is already on it but the view was moved away.
    pub fn jump_to_now(&mut self) {
        self.pane = Pane::Guide;
        self.cursor = self.now;
        self.window = home_window(self.now);
    }

    /// A tap beside the groups column and the rail (touch): they close and
    /// the focus is back on the guide where it was, nothing chosen.
    pub fn close_columns(&mut self) {
        self.pane = Pane::Guide;
    }

    /// A pan (touch, mouse wheel) settled with `window` the first minute
    /// shown: the window goes there and the cursor follows into it.
    pub fn pan_to(&mut self, window: i32, s: &dyn Schedule) {
        self.shift_window(window - self.window, s);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Airing, Move, STEP, SyntheticSchedule};
    use super::*;

    const S: SyntheticSchedule = SyntheticSchedule;

    #[test]
    fn closing_the_side_columns_keeps_the_guides_place() {
        // A tap beside the groups (and the rail) on a touch screen only
        // closes them: the focus goes back to the guide where it was.
        let mut f = Focus::at(0);
        f.apply(Move::Down, &S);
        f.step_time(HOUR, &S);
        let (channel, cursor, window) = (f.channel, f.cursor, f.window);
        f.pane = Pane::Groups;
        f.close_columns();
        assert_eq!(f.pane, Pane::Guide);
        assert_eq!((f.channel, f.cursor, f.window), (channel, cursor, window));
        f.pane = Pane::Rail;
        f.close_columns();
        assert_eq!(f.pane, Pane::Guide);
        // On the guide already: nothing moves.
        f.close_columns();
        assert_eq!((f.channel, f.cursor, f.window), (channel, cursor, window));
    }

    #[test]
    fn hour_steps_move_the_time_column_and_the_window_together() {
        let mut f = Focus::at(0);
        let window = f.window;
        f.step_time(HOUR, &S);
        assert_eq!((f.window, f.cursor), (window + HOUR, HOUR));
        let p = f.program(&S);
        assert!(p.start <= HOUR && HOUR < p.end);
        f.step_time(-2 * HOUR, &S);
        assert_eq!((f.window, f.cursor), (window - HOUR, -HOUR));
        assert_eq!(f.program(&S).airing(0), Airing::Ended);
    }

    #[test]
    fn day_steps_stop_at_what_can_be_browsed() {
        let mut f = Focus::at(0);
        let (lo, hi) = f.bounds(&S);
        for _ in 0..20 {
            f.step_time(-DAY, &S);
        }
        assert_eq!(f.window, lo, "the oldest retained day");
        assert!(f.cursor >= lo && f.cursor < lo + VISIBLE);
        for _ in 0..20 {
            f.step_time(DAY, &S);
        }
        assert_eq!(f.window, hi + 1 - VISIBLE, "the end of the data");
        assert!(f.cursor <= hi && f.cursor >= f.window);
    }

    #[test]
    fn time_steps_leave_the_columns_alone() {
        let mut f = Focus {
            pane: Pane::Groups,
            ..Focus::at(0)
        };
        let before = f;
        f.step_time(HOUR, &S);
        assert_eq!(f, before);
    }

    #[test]
    fn home_returns_to_now_also_from_a_live_focus_in_a_moved_window() {
        let mut f = Focus::at(40);
        f.shift_window(4 * STEP, &S);
        f.cursor = 40;
        assert_eq!(f.program(&S).airing(40), Airing::Live);
        assert!(!f.return_to_now(&S), "Back's rule: already live");
        f.jump_to_now();
        assert_eq!((f.cursor, f.window), (40, home_window(40)));
        // From the groups too: the grid takes the focus.
        f.pane = Pane::Groups;
        f.step_time(-DAY, &S);
        f.jump_to_now();
        assert_eq!((f.pane, f.cursor), (Pane::Guide, 40));
    }

    #[test]
    fn a_settled_pan_pulls_the_cursor_into_its_window() {
        let mut f = Focus::at(0);
        f.pan_to(-1000, &S);
        assert_eq!(f.window, -1000);
        assert_eq!(f.cursor, -1000 + VISIBLE - 1);
        f.pan_to(5000, &S);
        let (_, hi) = f.bounds(&S);
        assert_eq!(f.window, hi + 1 - VISIBLE, "clamped to the data");
        assert!(f.cursor >= f.window);
    }

    #[test]
    fn left_walks_back_from_a_later_programme_to_the_live_one() {
        // Right steps later. Left walks through each programme in reverse,
        // including those before now even without catch-up.
        for channel in [1, 3, 15, 77] {
            let mut f = Focus {
                channel,
                ..Focus::at(0)
            };
            let live = f.program(&S);
            let mut seen = vec![live];
            for _ in 0..6 {
                f.apply(Move::Right, &S);
                seen.push(f.program(&S));
            }
            seen.pop();
            while let Some(expected) = seen.pop() {
                f.apply(Move::Left, &S);
                assert_eq!(f.pane, Pane::Guide, "channel {channel}");
                assert_eq!(f.program(&S), expected, "channel {channel}");
            }
            assert_eq!(f.program(&S), live);
            f.apply(Move::Left, &S);
            assert_eq!(f.pane, Pane::Guide);
            assert_eq!(f.program(&S).end, live.start);
        }
    }

    #[test]
    fn replays_continue_into_earlier_days() {
        // Channel 0 replays three days: Left walks back past a day.
        let mut f = Focus::at(0);
        for _ in 0..40 {
            f.enter_past(&S);
            assert_eq!(f.pane, Pane::Guide);
        }
        assert!(f.program(&S).end < -DAY, "a day back: {:?}", f.program(&S));
        let start = f.program(&S).start;
        assert!(f.window <= start && start < f.window + VISIBLE);
    }
}
