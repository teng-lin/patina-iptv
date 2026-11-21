use super::*;
use crate::guide::{HISTORY, HORIZON};

/// 8 dp per minute: a 1200 dp program area shows [`VISIBLE`] minutes.
const PPM: f32 = 8.0;
const FRAME: f32 = 1.0 / 60.0;

fn at_now() -> Timeline {
    Timeline::new(home_window(0), (-HISTORY, HORIZON))
}

#[test]
fn the_threshold_is_crossed_once_in_any_direction() {
    let mut t = Threshold::default();
    assert!(!t.moved(1, 40.0, 0.0), "no finger down");
    t.down(1, 100.0, 100.0);
    assert!(!t.moved(1, 104.0, 103.0), "a tap's wobble");
    // Down at a list's top: the list cannot scroll, the finger dragged.
    assert!(t.moved(1, 101.0, 112.0));
    assert!(!t.moved(1, 101.0, 160.0), "reported once");
    t.up(1);
    assert!(!t.moved(1, 200.0, 200.0));
    t.down(1, 0.0, 0.0);
    assert!(t.moved(1, -8.0, -8.0), "diagonal");
}

#[test]
fn the_threshold_follows_one_finger_and_a_still_cancel_is_no_drag() {
    let mut t = Threshold::default();
    t.down(1, 100.0, 100.0);
    // A second finger far away neither moves the start nor crosses.
    t.down(2, 400.0, 400.0);
    assert!(!t.moved(2, 400.0, 460.0));
    assert!(!t.moved(1, 103.0, 100.0));
    // Its lift leaves the first finger followed.
    t.up(2);
    assert!(t.moved(1, 112.0, 100.0));
    t.up(1);
    // Canceled still: no drag; canceled after moving: a drag.
    t.down(1, 0.0, 0.0);
    assert!(!t.canceled(1, 0.0, 1.0));
    t.down(1, 0.0, 0.0);
    assert!(!t.moved(1, 0.0, 6.0));
    assert!(t.canceled(1, 0.0, 6.0), "a scroll view took it after 6 dp");
    // A fast drag canceled before any move arrived that far: the
    // cancel's own position counts.
    t.down(1, 0.0, 0.0);
    assert!(!t.moved(1, 0.0, 2.0));
    assert!(t.canceled(1, 0.0, 9.0));
    // Another finger's cancel is not the followed one's.
    t.down(1, 0.0, 0.0);
    assert!(!t.canceled(2, 0.0, 40.0));
    t.up(1);
    // Already reported crossing: the cancel is not a second drag.
    t.down(1, 0.0, 0.0);
    assert!(t.moved(1, 0.0, 12.0));
    assert!(!t.canceled(1, 0.0, 12.0));
}

#[test]
fn a_drag_picks_its_axis_once_past_the_slop() {
    assert_eq!(Lock::Undecided.decide(6.0, 5.0), Lock::Undecided);
    assert_eq!(Lock::Undecided.decide(11.0, 4.0), Lock::Horizontal);
    assert_eq!(Lock::Undecided.decide(3.0, -12.0), Lock::Vertical);
    // Decided once: a horizontal drag that turns vertical still pans.
    assert_eq!(Lock::Horizontal.decide(0.0, 200.0), Lock::Horizontal);
    assert_eq!(Lock::Vertical.decide(200.0, 0.0), Lock::Vertical);
}

#[test]
fn a_tap_wobble_moves_nothing() {
    let mut t = at_now();
    t.press();
    assert!(!t.drag(4.0, 3.0, PPM));
    assert!(!t.drag(-6.0, 2.0, PPM));
    assert!(!t.release(900.0, PPM), "no fling from a tap");
    assert_eq!(t.view(), home_window(0) as f32);
    assert!(!t.moving());
}

#[test]
fn a_horizontal_drag_follows_the_finger() {
    let mut t = at_now();
    let from = t.view();
    t.press();
    // Right shows earlier programs, by the finger's distance.
    assert!(t.drag(80.0, 6.0, PPM));
    assert_eq!(t.view(), from - 10.0);
    assert!(t.drag(-40.0, 30.0, PPM), "the lock holds when it turns");
    assert_eq!(t.view(), from + 5.0);
    assert!(!t.release(0.0, PPM));
    assert_eq!(t.window(), home_window(0) + 5);
}

#[test]
fn a_vertical_drag_is_the_channel_lists() {
    let mut t = at_now();
    t.press();
    assert!(!t.drag(3.0, 40.0, PPM));
    assert!(!t.drag(300.0, 40.0, PPM), "locked vertical");
    assert!(!t.release(-5000.0, PPM), "no fling either");
    assert_eq!(t.view(), home_window(0) as f32);
}

#[test]
fn the_view_stays_within_what_can_be_browsed() {
    let (first, end) = (-3 * 24 * 60, 600);
    let mut t = Timeline::new(home_window(0), (first, end));
    t.press();
    t.drag(1e7, 0.0, PPM);
    assert_eq!(t.view(), first as f32, "the oldest day kept");
    t.drag(-1e7, 0.0, PPM);
    assert_eq!(t.view(), (end - VISIBLE) as f32, "the end of the data");
    t.release(0.0, PPM);
    // Narrower bounds clamp the view; bounds shorter than a screen pin it.
    t.set_bounds((first, 300));
    assert_eq!(t.view(), (300 - VISIBLE) as f32);
    t.set_bounds((0, 50));
    assert_eq!(t.view(), 0.0);
    assert!(!t.wheel(500.0, PPM));
}

#[test]
fn a_fling_slows_down_and_stops() {
    let mut t = at_now();
    t.press();
    t.drag(-50.0, 0.0, PPM);
    assert!(t.release(-2000.0, PPM), "fast enough to fling");
    let start = t.view();
    let rest = t.rest();
    assert!(rest > start, "a leftward fling goes later");
    let mut last_step = f32::MAX;
    let mut frames = 0;
    while t.step(FRAME, PPM) {
        let step = t.view() - start;
        assert!(step > 0.0);
        frames += 1;
        assert!(frames < 600, "stops within ten seconds");
        last_step = last_step.min(step);
    }
    assert!(!t.moving());
    // The discrete steps land close to the predicted rest.
    assert!((t.view() - rest).abs() < 2.0, "{} vs {rest}", t.view());
    // Each frame moves less than the one before (deceleration).
    let mut t = at_now();
    t.press();
    t.drag(-20.0, 0.0, PPM);
    t.release(-3000.0, PPM);
    let mut prev = t.view();
    let mut prev_delta = f32::MAX;
    while t.step(FRAME, PPM) {
        let delta = t.view() - prev;
        assert!(delta <= prev_delta + 1e-4);
        prev_delta = delta;
        prev = t.view();
    }
}

#[test]
fn a_fling_stops_at_the_bounds_and_a_finger_catches_it() {
    let mut t = Timeline::new(home_window(0), (-120, HORIZON));
    t.press();
    t.drag(20.0, 0.0, PPM);
    t.release(20_000.0, PPM);
    assert_eq!(t.rest(), -120.0, "its rest is clamped too");
    while t.step(FRAME, PPM) {}
    assert_eq!(t.view(), -120.0);
    let mut t = at_now();
    t.press();
    t.drag(20.0, 0.0, PPM);
    t.release(3000.0, PPM);
    t.step(FRAME, PPM);
    t.press();
    assert!(!t.step(FRAME, PPM), "caught");
    assert!(t.moving(), "the finger is still down");
}

#[test]
fn keys_move_the_view_only_while_nothing_pans() {
    let mut t = at_now();
    assert!(t.follow(300));
    assert_eq!(t.view(), 300.0);
    assert!(!t.follow(300));
    t.press();
    t.drag(-80.0, 0.0, PPM);
    assert!(!t.follow(0), "a drag is under way");
    t.release(0.0, PPM);
    assert!(t.follow(0));
    t.jump(-60);
    assert_eq!(t.view(), -60.0);
    t.jump(-HISTORY * 2);
    assert_eq!(t.view(), -HISTORY as f32);
}

#[test]
fn wheels_pan_at_once_without_a_fling() {
    let mut t = at_now();
    let from = t.view();
    assert!(t.wheel(40.0, PPM));
    assert_eq!(t.view(), from + 5.0);
    assert!(t.wheel(-80.0, PPM));
    assert_eq!(t.view(), from - 5.0);
    assert!(!t.moving());
}

#[test]
fn a_fling_into_the_past_wants_the_day_it_lands_in() {
    let mut t = at_now();
    assert_eq!(
        t.wanted(),
        (home_window(0) - MARGIN, home_window(0) + VISIBLE + MARGIN)
    );
    t.press();
    t.drag(40.0, 0.0, PPM);
    t.release(30_000.0, PPM);
    let (from, to) = t.wanted();
    let rest = t.rest();
    assert!(rest < -12.0 * 60.0, "a long fling: {rest}");
    assert!(from <= rest as i32 - MARGIN, "its landing is asked for");
    assert!(to >= t.view() as i32 + VISIBLE, "and what shows meanwhile");
}

#[test]
fn the_now_button_shows_once_the_view_left_now() {
    let mut t = at_now();
    assert!(!t.away_from_now(0));
    assert!(!t.away_from_now(10), "a few minutes later, same window");
    t.wheel(10.0 * PPM, PPM);
    assert!(!t.away_from_now(0), "a nudge");
    t.wheel(10.0 * PPM, PPM);
    assert!(t.away_from_now(0));
    t.jump(home_window(0));
    assert!(!t.away_from_now(0));
    t.jump(home_window(0) - 24 * 60);
    assert!(t.away_from_now(0));
}

#[test]
fn the_band_moves_rarely_and_always_covers_the_view() {
    let mut view = 0.0_f32;
    let mut band = Band::around(view);
    assert!(band.covers(view));
    let mut moves = 0;
    // Pan two hours later a dp (1/8 min) at a time, then back.
    for i in 0..(2 * 120 * 8) {
        view += if i < 120 * 8 { 0.125 } else { -0.125 };
        if band.follow(view) {
            moves += 1;
        }
        assert!(band.covers(view), "view {view} band {band:?}");
        let offset = band.offset(view);
        assert!((0.0..=2.0 * MARGIN as f32).contains(&offset));
    }
    // About once per 3/4 of a margin.
    assert!(moves <= 4 * 240 / (3 * MARGIN) + 2, "{moves} rebuilds");
    // A jump moves it at once.
    assert!(band.follow(5000.0));
    assert!(band.covers(5000.0));
}

#[test]
fn a_grabbed_drag_pans_from_its_first_move() {
    let mut t = at_now();
    let from = t.view();
    t.grab();
    assert!(
        t.drag(-2.0, 1.0, PPM),
        "the platform passed the slop already"
    );
    assert_eq!(t.view(), from + 0.25);
    assert!(t.release(-800.0, PPM));
}

#[test]
fn a_fling_travels_like_a_native_lists() {
    // Released at 2000 dp/s: 0.6 × 2000 = 1200 dp (it was 2000 / 3.5 ≈
    // 570 dp), a whole program area of [`VISIBLE`] minutes here.
    let mut t = Timeline::new(0, (-3 * 24 * 60, 3 * 24 * 60));
    t.grab();
    t.drag(-1.0, 0.0, PPM);
    let start = t.view();
    assert!(t.release(-2000.0, PPM));
    let dp = (t.rest() - start) * PPM;
    assert!((dp - 1200.0).abs() < 1.0, "{dp} dp");
    while t.step(FRAME, PPM) {}
    let traveled = (t.view() - start) * PPM;
    assert!((traveled - 1200.0).abs() < 25.0, "{traveled} dp");
    // A drag follows the finger 1:1: 120 dp is 15 minutes at 8 dp/min.
    t.grab();
    let from = t.view();
    t.drag(120.0, 0.0, PPM);
    assert_eq!(t.view(), from - 15.0);
}
