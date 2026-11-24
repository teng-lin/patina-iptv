//! Changing channel by a vertical swipe over full-screen live video
//! (#285). The app feeds one finger's touches in dp and milliseconds; this
//! decides whether they are a swipe, which way it goes and whether its
//! release changes channel. Which channel that is, and whether full-screen
//! live video takes swipes at all, are the app's.
//!
//! - A swipe starts away from the screen's edges ([`EDGE`]): the systems
//!   keep those for their own gestures (the notification shade, Home and
//!   Back on Android; Control Center and Home on iPadOS).
//! - It is vertical once it moved [`SLOP`] (as the guide's drags decide
//!   their axis); a horizontal drag is left alone.
//! - Up is [`Direction::Next`], as Ch+ is; down is [`Direction::Previous`].
//! - Released past [`COMMIT`] of the screen's height, or flung faster than
//!   [`FLING_SPEED`] the way it went, it changes channel; otherwise, a
//!   swipe brought back included, it is cancelled.
//! - A second finger cancels it: a pinch is no swipe.

use crate::drag::Lock;
pub use crate::drag::SLOP;

/// How far from each edge of the screen (dp) a swipe must start.
pub const EDGE: f32 = 40.0;
/// The part of the screen's height a released swipe must have travelled.
pub const COMMIT: f32 = 0.25;
/// The speed (dp per second) at which a shorter swipe still changes
/// channel, released moving the way it went.
pub const FLING_SPEED: f32 = 1_000.0;
/// How far a fling must have travelled, so a tap's flick does nothing.
pub const FLING_DISTANCE: f32 = 3.0 * SLOP;
/// The release's velocity is measured over this much of the finger's
/// latest movement (ms).
const VELOCITY_WINDOW: f64 = 100.0;
/// At most this many recent positions are kept for the velocity.
const SAMPLES: usize = 16;

/// Which way a swipe changes channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// A swipe up: the channel Ch+ tunes.
    Next,
    /// A swipe down: the channel Ch− tunes.
    Previous,
}

impl Direction {
    /// The direction of a swipe `dy` dp from where it started (negative:
    /// up).
    pub fn of(dy: f32) -> Direction {
        if dy < 0.0 {
            Direction::Next
        } else {
            Direction::Previous
        }
    }

    /// The step along the channel list Ch± takes for this direction: Ch+
    /// (`Next`) goes to the row above, as the remote's Up does.
    pub fn step(self) -> isize {
        match self {
            Direction::Next => -1,
            Direction::Previous => 1,
        }
    }
}

/// What the app does after a touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// Nothing changes.
    None,
    /// The swipe moved: the target's banner follows the finger, `offset`
    /// dp from where it went down (negative: above). `committing`: it is
    /// far enough to change channel if released now.
    Follow {
        direction: Direction,
        offset: f32,
        committing: bool,
    },
    /// Released: change channel this way.
    Commit(Direction),
    /// The swipe ended without changing channel: the banner goes.
    Cancel,
}

/// The finger a swipe follows.
#[derive(Clone, Debug, PartialEq)]
struct Finger {
    id: u32,
    from: (f32, f32),
    height: f32,
    lock: Lock,
    /// Not a swipe: it started at an edge, went sideways, or another
    /// finger came down. It stays ignored until it lifts.
    ignored: bool,
    /// Recent `(ms, y)` positions, oldest first.
    samples: Vec<(f64, f32)>,
}

impl Finger {
    fn swiping(&self) -> bool {
        !self.ignored && self.lock == Lock::Vertical
    }

    fn offset(&self) -> f32 {
        self.samples.last().map_or(0.0, |&(_, y)| y - self.from.1)
    }

    fn record(&mut self, t: f64, y: f32) {
        if self.samples.len() == SAMPLES {
            self.samples.remove(0);
        }
        self.samples.push((t, y));
    }

    /// The finger's vertical speed over its latest movement, dp per
    /// second (negative: up).
    fn velocity(&self) -> f32 {
        let Some(&(t1, y1)) = self.samples.last() else {
            return 0.0;
        };
        let Some(&(t0, y0)) = self
            .samples
            .iter()
            .find(|&&(t, _)| t1 - t <= VELOCITY_WINDOW)
        else {
            return 0.0;
        };
        let dt = t1 - t0;
        if dt <= 0.0 {
            return 0.0;
        }
        ((y1 - y0) as f64 / dt * 1_000.0) as f32
    }

    fn committing(&self, offset: f32) -> bool {
        offset.abs() >= COMMIT * self.height
    }
}

/// One swipe's state, from a finger down to its lift.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Swipe {
    finger: Option<Finger>,
}

impl Swipe {
    /// Whether a swipe is under way (past the slop, vertical).
    pub fn swiping(&self) -> bool {
        self.finger.as_ref().is_some_and(Finger::swiping)
    }

    /// Finger `id` went down at `(x, y)` at `t` ms, on a screen `size`
    /// dp across. A second finger cancels a swipe under way.
    pub fn down(&mut self, id: u32, (x, y): (f32, f32), t: f64, size: (f32, f32)) -> Outcome {
        if let Some(finger) = &mut self.finger {
            if finger.id == id {
                // A finger the platform reports down again: start over.
                self.finger = None;
            } else {
                let was = finger.swiping();
                finger.ignored = true;
                return if was { Outcome::Cancel } else { Outcome::None };
            }
        }
        let (width, height) = size;
        let edge = x < EDGE || y < EDGE || x > width - EDGE || y > height - EDGE;
        self.finger = Some(Finger {
            id,
            from: (x, y),
            height,
            lock: Lock::Undecided,
            ignored: edge,
            samples: vec![(t, y)],
        });
        Outcome::None
    }

    /// Finger `id` moved to `(x, y)` at `t` ms.
    pub fn moved(&mut self, id: u32, (x, y): (f32, f32), t: f64) -> Outcome {
        let Some(finger) = self.finger.as_mut().filter(|f| f.id == id) else {
            return Outcome::None;
        };
        if finger.ignored {
            return Outcome::None;
        }
        finger.record(t, y);
        let (dx, dy) = (x - finger.from.0, y - finger.from.1);
        finger.lock = finger.lock.decide(dx, dy);
        match finger.lock {
            Lock::Undecided => Outcome::None,
            Lock::Horizontal => {
                finger.ignored = true;
                Outcome::None
            }
            Lock::Vertical => Outcome::Follow {
                direction: Direction::of(dy),
                offset: dy,
                committing: finger.committing(dy),
            },
        }
    }

    /// Finger `id` lifted at `(x, y)` at `t` ms: the swipe commits or is
    /// cancelled.
    pub fn up(&mut self, id: u32, at: (f32, f32), t: f64) -> Outcome {
        if self.finger.as_ref().is_none_or(|f| f.id != id) {
            return Outcome::None;
        }
        // Where it lifted counts as its last move.
        let moved = self.moved(id, at, t);
        let Some(finger) = self.finger.take() else {
            return Outcome::None;
        };
        if !finger.swiping() {
            return match moved {
                Outcome::Follow { .. } => Outcome::Cancel,
                _ => Outcome::None,
            };
        }
        let offset = finger.offset();
        let velocity = finger.velocity();
        let flung = offset.abs() >= FLING_DISTANCE
            && velocity.abs() >= FLING_SPEED
            && velocity.signum() == offset.signum();
        if finger.committing(offset) || flung {
            Outcome::Commit(Direction::of(offset))
        } else {
            Outcome::Cancel
        }
    }

    /// The system took finger `id`'s touches (a dialog, the notification
    /// shade): a swipe under way is cancelled.
    pub fn cancel(&mut self, id: u32) -> Outcome {
        if self.finger.as_ref().is_none_or(|f| f.id != id) {
            return Outcome::None;
        }
        let was = self.swiping();
        self.finger = None;
        if was { Outcome::Cancel } else { Outcome::None }
    }

    /// Video stopped taking swipes (it left full screen, a replay began):
    /// whatever was under way ends without a change.
    pub fn reset(&mut self) -> Outcome {
        let was = self.swiping();
        self.finger = None;
        if was { Outcome::Cancel } else { Outcome::None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f32, f32) = (1280.0, 800.0);
    const FRAME: f64 = 16.0;

    /// A finger dragged from `from` by `by`, 10 dp a frame, then lifted:
    /// every outcome, the release's last.
    fn swipe(s: &mut Swipe, from: (f32, f32), by: (f32, f32)) -> Vec<Outcome> {
        let mut out = vec![s.down(1, from, 0.0, SCREEN)];
        let steps = (by.0.abs().max(by.1.abs()) / 10.0).ceil().max(1.0) as u32;
        let mut t = 100.0;
        for i in 1..=steps {
            let f = i as f32 / steps as f32;
            t += FRAME;
            out.push(s.moved(1, (from.0 + by.0 * f, from.1 + by.1 * f), t));
        }
        out.push(s.up(1, (from.0 + by.0, from.1 + by.1), t));
        out
    }

    #[test]
    fn a_long_swipe_up_is_next_and_down_is_previous() {
        let mut s = Swipe::default();
        let out = swipe(&mut s, (640.0, 600.0), (0.0, -300.0));
        assert_eq!(out.last(), Some(&Outcome::Commit(Direction::Next)));
        assert_eq!(Direction::Next.step(), -1, "as Ch+");
        let out = swipe(&mut s, (640.0, 200.0), (4.0, 300.0));
        assert_eq!(out.last(), Some(&Outcome::Commit(Direction::Previous)));
        assert_eq!(Direction::Previous.step(), 1, "as Ch-");
        assert!(!s.swiping());
    }

    #[test]
    fn the_banner_follows_past_the_slop_and_says_when_it_would_commit() {
        let mut s = Swipe::default();
        assert_eq!(s.down(1, (640.0, 600.0), 0.0, SCREEN), Outcome::None);
        assert_eq!(s.moved(1, (640.0, 595.0), 16.0), Outcome::None, "a wobble");
        assert_eq!(
            s.moved(1, (642.0, 580.0), 32.0),
            Outcome::Follow {
                direction: Direction::Next,
                offset: -20.0,
                committing: false
            }
        );
        assert!(s.swiping());
        assert_eq!(
            s.moved(1, (642.0, 400.0), 48.0),
            Outcome::Follow {
                direction: Direction::Next,
                offset: -200.0,
                committing: true
            },
            "a quarter of 800 dp"
        );
        // Past where it went down, the other way: the other channel.
        assert_eq!(
            s.moved(1, (642.0, 630.0), 300.0),
            Outcome::Follow {
                direction: Direction::Previous,
                offset: 30.0,
                committing: false
            }
        );
    }

    #[test]
    fn a_slow_short_swipe_and_one_brought_back_cancel() {
        let mut s = Swipe::default();
        // 100 dp, slowly: an eighth of the height, no fling.
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        let mut t = 0.0;
        for i in 1..=20 {
            t += 50.0;
            s.moved(1, (640.0, 500.0 - 5.0 * i as f32), t);
        }
        assert_eq!(s.up(1, (640.0, 400.0), t + 50.0), Outcome::Cancel);
        // Far, then back to where it began.
        let mut s = Swipe::default();
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        s.moved(1, (640.0, 200.0), 200.0);
        let mut t = 200.0;
        for y in [250.0, 350.0, 450.0, 495.0] {
            t += 100.0;
            s.moved(1, (640.0, y), t);
        }
        t += 200.0;
        assert_eq!(
            s.moved(1, (640.0, 496.0), t),
            Outcome::Follow {
                direction: Direction::Next,
                offset: -4.0,
                committing: false
            }
        );
        assert_eq!(s.up(1, (640.0, 496.0), t + 50.0), Outcome::Cancel);
    }

    #[test]
    fn a_fast_fling_commits_a_short_swipe() {
        let mut s = Swipe::default();
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        // 80 dp in 50 ms: 1 600 dp/s.
        let mut t = 100.0;
        for i in 1..=5 {
            t += 10.0;
            s.moved(1, (640.0, 500.0 + 16.0 * i as f32), t);
        }
        assert_eq!(
            s.up(1, (640.0, 580.0), t),
            Outcome::Commit(Direction::Previous)
        );
        // A flick of a tap is no fling.
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        s.moved(1, (640.0, 488.0), 5.0);
        assert_eq!(s.up(1, (640.0, 480.0), 10.0), Outcome::Cancel);
        // Flung back the other way: no change.
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        s.moved(1, (640.0, 400.0), 300.0);
        s.moved(1, (640.0, 420.0), 310.0);
        s.moved(1, (640.0, 440.0), 320.0);
        assert_eq!(s.up(1, (640.0, 460.0), 330.0), Outcome::Cancel);
    }

    #[test]
    fn a_swipe_from_an_edge_or_sideways_is_none() {
        for from in [(640.0, 10.0), (640.0, 790.0), (5.0, 400.0), (1275.0, 400.0)] {
            let mut s = Swipe::default();
            let by = if from.1 < 400.0 { 400.0 } else { -400.0 };
            let out = swipe(&mut s, from, (0.0, by));
            assert!(out.iter().all(|o| *o == Outcome::None), "{from:?}: {out:?}");
        }
        let mut s = Swipe::default();
        let out = swipe(&mut s, (640.0, 400.0), (-400.0, 150.0));
        assert!(out.iter().all(|o| *o == Outcome::None), "sideways: {out:?}");
    }

    #[test]
    fn a_second_finger_or_the_system_cancels() {
        let mut s = Swipe::default();
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        s.moved(1, (640.0, 300.0), 50.0);
        assert_eq!(s.down(2, (300.0, 300.0), 60.0, SCREEN), Outcome::Cancel);
        assert_eq!(s.moved(1, (640.0, 100.0), 70.0), Outcome::None);
        assert_eq!(s.up(2, (300.0, 300.0), 80.0), Outcome::None);
        assert_eq!(s.up(1, (640.0, 100.0), 90.0), Outcome::None);
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        s.moved(1, (640.0, 300.0), 50.0);
        assert_eq!(s.cancel(1), Outcome::Cancel);
        assert_eq!(s.up(1, (640.0, 300.0), 60.0), Outcome::None);
        s.down(1, (640.0, 500.0), 0.0, SCREEN);
        s.moved(1, (640.0, 300.0), 50.0);
        assert_eq!(s.reset(), Outcome::Cancel);
        assert_eq!(s.reset(), Outcome::None);
    }
}
