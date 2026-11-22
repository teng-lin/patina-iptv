//! Holding a seek key in a replay: the steps grow the longer it is held.
//!
//! A remote auto-repeats a held key about 20 times a second; a step per
//! repeat would race along the time bar. While held, a step is taken at
//! most every [`REPEAT_EVERY_MS`], and it doubles after each
//! [`DOUBLE_AFTER_MS`] of holding, up to [`MAX_FACTOR`] times the set step.
//! A press (not a repeat) always steps once, by the set step. Times are
//! milliseconds on the caller's monotonic clock.

/// While held, at most one step this often.
pub const REPEAT_EVERY_MS: u64 = 200;
/// The step doubles after each this long of holding.
pub const DOUBLE_AFTER_MS: u64 = 1_500;
/// The largest multiple of the set step.
pub const MAX_FACTOR: i32 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeekHold {
    /// The held direction (-1, 1), since when, and the last step taken.
    held: Option<(i8, u64, u64)>,
}

impl SeekHold {
    /// A seek key in `dir` (negative: back) at `now_ms`; `repeat` when it
    /// is an auto-repeat of a held key. Returns the multiple of the set
    /// step to take now, or `None` to skip this repeat.
    pub fn step(&mut self, dir: i32, repeat: bool, now_ms: u64) -> Option<i32> {
        let dir = dir.signum() as i8;
        match self.held {
            Some((held, since, last)) if repeat && held == dir => {
                if now_ms.saturating_sub(last) < REPEAT_EVERY_MS {
                    return None;
                }
                self.held = Some((dir, since, now_ms));
                let doublings = now_ms.saturating_sub(since) / DOUBLE_AFTER_MS;
                // 1, 2, 4, 8: three doublings reach MAX_FACTOR.
                Some((1_i32 << doublings.min(3)).min(MAX_FACTOR))
            }
            _ => {
                self.held = Some((dir, now_ms, now_ms));
                Some(1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_step_once_each_however_quick() {
        let mut h = SeekHold::default();
        for at in [0, 50, 100, 150] {
            assert_eq!(h.step(1, false, at), Some(1));
        }
        assert_eq!(h.step(-1, false, 160), Some(1));
    }

    #[test]
    fn a_held_key_steps_at_a_steady_pace_and_faster_the_longer_it_is_held() {
        let mut h = SeekHold::default();
        assert_eq!(h.step(-1, false, 0), Some(1), "the press");
        // Repeats every 50 ms: one step in four.
        assert_eq!(h.step(-1, true, 450), Some(1));
        assert_eq!(h.step(-1, true, 500), None);
        assert_eq!(h.step(-1, true, 600), None);
        assert_eq!(h.step(-1, true, 650), Some(1));
        // Doubling after 1.5 s, 3 s, 4.5 s of holding; then no more.
        assert_eq!(h.step(-1, true, 1_500), Some(2));
        assert_eq!(h.step(-1, true, 3_000), Some(4));
        assert_eq!(h.step(-1, true, 4_500), Some(8));
        assert_eq!(h.step(-1, true, 60_000), Some(MAX_FACTOR));
    }

    #[test]
    fn a_new_press_or_the_other_direction_starts_over() {
        let mut h = SeekHold::default();
        h.step(1, false, 0);
        assert_eq!(h.step(1, true, 3_000), Some(4));
        assert_eq!(
            h.step(1, false, 3_100),
            Some(1),
            "released and pressed again"
        );
        assert_eq!(h.step(1, true, 3_400), Some(1));
        assert_eq!(h.step(-1, true, 3_450), Some(1), "a repeat of another key");
        assert_eq!(h.step(-1, true, 3_500), None);
    }
}
