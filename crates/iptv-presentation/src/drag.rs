//! A finger's drag threshold and axis, shared by the guide's timeline
//! (`iptv-app`'s `timeline.rs`) and the swipe that changes channel over
//! full-screen video ([`crate::touch_zap`]).

/// How far (dp) a finger moves before its drag picks an axis.
pub const SLOP: f32 = 10.0;

/// A drag's axis, decided once it moved [`SLOP`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    Undecided,
    Horizontal,
    Vertical,
}

impl Lock {
    /// The axis after the finger moved `(dx, dy)` dp from where it went
    /// down: decided once, by the larger component.
    pub fn decide(self, dx: f32, dy: f32) -> Lock {
        match self {
            Lock::Undecided if dx.hypot(dy) >= SLOP => {
                if dx.abs() > dy.abs() {
                    Lock::Horizontal
                } else {
                    Lock::Vertical
                }
            }
            decided => decided,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_is_decided_once_past_the_slop() {
        assert_eq!(Lock::Undecided.decide(6.0, 5.0), Lock::Undecided);
        assert_eq!(Lock::Undecided.decide(11.0, 4.0), Lock::Horizontal);
        assert_eq!(Lock::Undecided.decide(3.0, -12.0), Lock::Vertical);
        assert_eq!(Lock::Vertical.decide(100.0, 0.0), Lock::Vertical);
    }
}
