use super::super::moves::{DAY, HOUR};
use super::super::{Move, Pane, home_window};
use super::*;
use crate::guide::{HORIZON, SyntheticSchedule};
use crate::timeline::Timeline;

const S: SyntheticSchedule = SyntheticSchedule;

/// The synthetic programs with chosen archive windows (`days[c]` for
/// channel `c`) and stored guide (`first` minutes back from now 0).
struct Archive {
    days: Vec<u8>,
    first: i32,
}

impl Schedule for Archive {
    fn browsable(&self, now: i32) -> (i32, i32) {
        (now - self.first, now + HORIZON)
    }
    fn channel_count(&self) -> usize {
        self.days.len()
    }
    fn group_count(&self) -> usize {
        1
    }
    fn group_start(&self, _group: usize) -> usize {
        0
    }
    fn program_at(&self, channel: usize, minute: i32) -> Program {
        S.program_at(channel, minute)
    }
    fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program> {
        S.programs_between(channel, from, minutes)
    }
    fn catchup_days(&self, channel: usize) -> u8 {
        self.days[channel]
    }
    fn channel_name(&self, channel: usize) -> String {
        format!("Archive {channel}")
    }
    fn group_name(&self, _group: usize) -> String {
        "All".to_owned()
    }
    fn title(&self, _channel: usize, _program: Program) -> String {
        String::new()
    }
}

/// Rows 0 and 1 replay a day, row 2 has no catch-up; two days are stored.
fn mixed() -> Archive {
    Archive {
        days: vec![1, 1, 0],
        first: 2 * DAY,
    }
}

fn on(channel: usize) -> Focus {
    Focus {
        channel,
        ..Focus::at(0)
    }
}

#[test]
fn left_inspects_retained_listings_with_or_without_catch_up() {
    let s = mixed();
    for channel in [0, 2] {
        let mut f = on(channel);
        let live = f.program(&s);
        f.apply(Move::Left, &s);
        assert_eq!(f.pane, Pane::Guide);
        assert_eq!(f.program(&s).end, live.start);
        assert_eq!(
            f.program(&s).can_replay(f.now, s.catchup_days(channel)),
            channel == 0
        );

        let floor = f.floor(&s);
        assert_eq!(floor, -2 * DAY);
        let mut steps = 0;
        while f.pane == Pane::Guide {
            let before = f.program(&s);
            f.apply(Move::Left, &s);
            if f.pane == Pane::Guide {
                assert_eq!(f.program(&s).end, before.start);
                assert!(f.window <= f.cursor && f.cursor < f.window + VISIBLE);
            }
            steps += 1;
            assert!(steps < 200);
        }
        let oldest = f.program(&s);
        assert_eq!(f.cursor, floor);
        assert!(oldest.start <= floor && floor < oldest.end);
        assert!(!oldest.can_replay(f.now, s.catchup_days(channel)));
    }
}

#[test]
fn no_retained_past_leaves_left_available_to_open_groups() {
    let s = Archive {
        days: vec![0],
        first: 0,
    };
    let mut f = on(0);
    let live = f.program(&s);
    f.enter_past(&s);
    assert_eq!((f.pane, f.program(&s)), (Pane::Guide, live));
    f.apply(Move::Left, &s);
    assert_eq!(f.pane, Pane::Groups);
    assert_eq!(view_range(&s, 0).0, home_window(0));
}

#[test]
fn the_retained_floor_keeps_an_overlapping_listing_reachable() {
    let listing = S.program_at(0, -DAY);
    let floor = listing.start + 1;
    let s = Archive {
        days: vec![0],
        first: -floor,
    };
    let mut f = on(0);
    while f.cursor > floor {
        f.enter_past(&s);
    }
    assert_eq!((f.cursor, f.program(&s)), (floor, listing));
    assert_eq!(f.window, floor);
    f.apply(Move::Left, &s);
    assert_eq!(f.pane, Pane::Groups);
}

#[test]
fn rows_keep_the_time_column_across_archive_capabilities() {
    let s = mixed();
    let mut f = on(0);
    for _ in 0..5 {
        f.apply(Move::Left, &s);
    }
    let cursor = f.cursor;
    assert!(cursor < 0);
    f.apply(Move::Down, &s);
    assert_eq!((f.channel, f.cursor), (1, cursor));
    f.apply(Move::Down, &s);
    assert_eq!((f.channel, f.cursor), (2, cursor));
    assert_eq!(f.program(&s).airing(0), super::super::Airing::Ended);
}

#[test]
fn hour_and_day_steps_stop_at_the_retained_floor() {
    let s = mixed();
    let mut f = on(2);
    f.step_time(-HOUR, &s);
    assert_eq!(f.cursor, -HOUR);
    for _ in 0..5 {
        f.step_time(-DAY, &s);
    }
    assert_eq!((f.cursor, f.window), (-2 * DAY, -2 * DAY));
    for _ in 0..5 {
        f.step_time(DAY, &s);
    }
    let (_, end) = view_range(&s, 0);
    assert_eq!(f.window, end - VISIBLE);
}

#[test]
fn a_tap_focuses_a_retained_listing_without_replay() {
    let s = mixed();
    let mut f = on(0);
    f.tap(2, -90, &s);
    assert_eq!((f.channel, f.cursor), (2, -90));
    assert!(!f.program(&s).can_replay(f.now, s.catchup_days(2)));
}

#[test]
fn panning_reaches_retained_listings_on_lists_without_catch_up() {
    let s = Archive {
        days: vec![0, 0],
        first: 2 * DAY,
    };
    let (first, end) = view_range(&s, 0);
    assert_eq!((first, end), (-2 * DAY, HORIZON));
    let mut timeline = Timeline::new(home_window(0), (first, end));
    timeline.press();
    timeline.drag(1.0e6, 0.0, 8.0);
    assert_eq!(timeline.view(), first as f32);
    let mut f = on(0);
    f.pan_to(-10 * DAY, &s);
    assert_eq!(f.window, first);
    assert!(f.cursor >= first && f.cursor < first + VISIBLE);
}
