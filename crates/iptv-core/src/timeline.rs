//! The guide's timeline under a finger, a mouse and a wheel: where the
//! programme grid is panned to, in fractional guide minutes, free of UI so
//! the rules are unit-tested on the host.
//!
//! A drag decides its axis once it moved [`SLOP`] (a tap's small wobble
//! stays a tap; a vertical drag stays the channel list's), then moves the
//! view with the finger, 1:1 in dp (the platform's pan reports dp, and dp
//! per minute is the programme area's width over [`VISIBLE`]); released
//! fast enough it flings on and slows down ([`FLING_GAIN`], [`FRICTION`]),
//! about as far as a native list's fling. The view stays within what the
//! guide can show ([`crate::guide::view_range`]): back as far as the
//! retained listings go, on to a little past the last programme. Keys move the focus model's window
//! ([`crate::guide::Focus`]); the view follows it while nothing pans, and a
//! settled pan hands its window back ([`Timeline::window`]).
//!
//! The grid draws a [`Band`] of programmes around the view and slides it
//! by the view's offset in it, so a pan rebuilds cells only when the band
//! moves (every [`MARGIN`] or so), not every frame.

use crate::guide::{STEP, VISIBLE, home_window};

/// How far (dp) a finger moves before its drag picks an axis.
pub const SLOP: f32 = 10.0;
/// A fling's speed falls by `e^(-FRICTION · seconds)`, so from `v` it
/// travels `v / FRICTION`: with 2.0 about half a second's worth, as an iOS
/// list's normal deceleration (0.998 per ms, `-ln 0.998 · 1000` ≈ 2.0)
/// and an Android list's fling at everyday speeds. (It was 3.5: a third of
/// a second's worth, so flings felt short and slow.)
pub const FRICTION: f32 = 2.0;
/// A fling starts this much faster than the finger lifted: the platform's
/// velocity is smoothed over the last moves, which slow as the finger
/// lifts, so a native list's fling reads it high too. With [`FRICTION`] a
/// fling travels `0.6 v` (it was `0.29 v`).
pub const FLING_GAIN: f32 = 1.2;
/// Below this speed (dp per second) a release or a fling stops.
pub const STOP_SPEED: f32 = 40.0;
/// Minutes of programmes the grid holds beyond each edge of the view.
pub const MARGIN: i32 = STEP;
/// The minutes a [`Band`] spans.
pub const BAND: i32 = VISIBLE + 2 * MARGIN;

/// A drag's axis, decided once it moved [`SLOP`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    Undecided,
    /// It pans the timeline.
    Horizontal,
    /// It is the channel list's: the timeline ignores it.
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

/// The panned view: its first minute, its drag or fling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    view: f32,
    span: i32,
    /// The view's range: the first browsable minute, the last first one.
    lo: f32,
    hi: f32,
    /// The view where the drag began, and its axis.
    drag: Option<(f32, Lock)>,
    /// A fling's speed in minutes per second (0: at rest).
    speed: f32,
}

impl Timeline {
    /// At `window`, browsing `[first, end)` ([`Self::set_bounds`]).
    pub fn new(window: i32, (first, end): (i32, i32)) -> Self {
        let mut t = Timeline {
            view: window as f32,
            span: VISIBLE,
            lo: 0.0,
            hi: 0.0,
            drag: None,
            speed: 0.0,
        };
        t.set_bounds((first, end));
        t
    }

    /// What can be browsed: `[first, end)`, so the view runs from `first`
    /// to `end - VISIBLE`.
    pub fn set_bounds(&mut self, (first, end): (i32, i32)) {
        self.lo = first as f32;
        self.hi = (end - self.span).max(first) as f32;
        self.view = self.clamp(self.view);
    }

    /// Change visible minutes; callers set bounds again afterwards.
    pub fn set_span(&mut self, minutes: i32) {
        self.span = minutes.clamp(60, 360);
    }

    fn clamp(&self, view: f32) -> f32 {
        view.clamp(self.lo, self.hi)
    }

    /// The first minute shown (fractional).
    pub fn view(&self) -> f32 {
        self.view
    }

    /// The window to hand to the focus model: the view's whole minute.
    pub fn window(&self) -> i32 {
        self.view.round() as i32
    }

    /// A drag or a fling is under way.
    pub fn moving(&self) -> bool {
        self.drag.is_some() || self.speed != 0.0
    }

    /// The finger is down (its drag may not have picked an axis yet).
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The focus model's window moved (keys, a touch step, now): the view
    /// goes there unless a pan is under way. Whether it moved.
    pub fn follow(&mut self, window: i32) -> bool {
        if self.moving() {
            return false;
        }
        let to = self.clamp(window as f32);
        let moved = to != self.view;
        self.view = to;
        moved
    }

    /// Straight to `window` (the Now button), ending any pan.
    pub fn jump(&mut self, window: i32) {
        self.drag = None;
        self.speed = 0.0;
        self.view = self.clamp(window as f32);
    }

    /// A finger went down: it catches a fling where it is.
    pub fn press(&mut self) {
        self.speed = 0.0;
        self.drag = Some((self.view, Lock::Undecided));
    }

    /// A drag the platform already found horizontal (its pan activated
    /// past the slop): it pans from here, with no slop of its own.
    pub fn grab(&mut self) {
        self.speed = 0.0;
        self.drag = Some((self.view, Lock::Horizontal));
    }

    /// The finger is `(dx, dy)` dp from where it went down, at `ppm` dp
    /// per minute. Whether the view moved (the drag is horizontal).
    pub fn drag(&mut self, dx: f32, dy: f32, ppm: f32) -> bool {
        let Some((from, lock)) = self.drag else {
            return false;
        };
        let lock = lock.decide(dx, dy);
        self.drag = Some((from, lock));
        if lock != Lock::Horizontal || ppm <= 0.0 {
            return false;
        }
        let to = self.clamp(from - dx / ppm);
        let moved = to != self.view;
        self.view = to;
        moved
    }

    /// The finger lifted moving `vx` dp per second: a horizontal drag
    /// flings on ([`FLING_GAIN`] faster). Whether a fling started.
    pub fn release(&mut self, vx: f32, ppm: f32) -> bool {
        let horizontal = matches!(self.drag.take(), Some((_, Lock::Horizontal)));
        self.speed = if horizontal && vx.abs() >= STOP_SPEED && ppm > 0.0 {
            -vx * FLING_GAIN / ppm
        } else {
            0.0
        };
        self.speed != 0.0
    }

    /// The system took the touches (or the grid went away): at rest where
    /// it is.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.speed = 0.0;
    }

    /// A horizontal wheel or touchpad scroll by `dx` dp (positive: later).
    pub fn wheel(&mut self, dx: f32, ppm: f32) -> bool {
        if self.dragging() || ppm <= 0.0 {
            return false;
        }
        self.speed = 0.0;
        let to = self.clamp(self.view + dx / ppm);
        let moved = to != self.view;
        self.view = to;
        moved
    }

    /// A display frame `dt` seconds after the last one: a fling moves on
    /// and slows down, and stops at the bounds or once slow. Whether it is
    /// still flinging.
    pub fn step(&mut self, dt: f32, ppm: f32) -> bool {
        if self.speed == 0.0 {
            return false;
        }
        let to = self.view + self.speed * dt;
        self.view = self.clamp(to);
        self.speed *= (-FRICTION * dt).exp();
        if self.view != to || (self.speed * ppm).abs() < STOP_SPEED {
            self.speed = 0.0;
        }
        self.speed != 0.0
    }

    /// Where the view comes to rest: a fling's end (the speed's integral),
    /// within the bounds.
    pub fn rest(&self) -> f32 {
        self.clamp(self.view + self.speed / FRICTION)
    }

    /// The minutes the grid will show until the view rests, with its
    /// margins: what to have read in (older days are paged in by the
    /// first query of a minute in them).
    pub fn wanted(&self) -> (i32, i32) {
        let (a, b) = (self.view.min(self.rest()), self.view.max(self.rest()));
        (
            a.floor() as i32 - MARGIN,
            b.ceil() as i32 + VISIBLE + MARGIN,
        )
    }

    /// Whether the view is away from now's default window, so a Now
    /// button takes it back.
    pub fn away_from_now(&self, now: i32) -> bool {
        (self.view - home_window(now) as f32).abs() >= (STEP / 2) as f32
    }
}

/// The minutes the grid has cells for: [`BAND`] from `base`, around the
/// view with [`MARGIN`] on either side; the cells slide by the view's
/// offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub base: i32,
}

impl Band {
    /// The band centred on `view`.
    pub fn around(view: f32) -> Band {
        Band {
            base: view.floor() as i32 - MARGIN,
        }
    }

    /// Moves along with `view` once it nears either margin's end. Whether
    /// it moved (the cells are rebuilt then).
    pub fn follow(&mut self, view: f32) -> bool {
        let offset = view - self.base as f32;
        let quarter = MARGIN as f32 / 4.0;
        if offset < quarter || offset > 2.0 * MARGIN as f32 - quarter {
            *self = Band::around(view);
            return true;
        }
        false
    }

    /// The view's offset into the band, in minutes.
    pub fn offset(&self, view: f32) -> f32 {
        view - self.base as f32
    }

    /// Whether the band holds every minute shown from `view` on.
    pub fn covers(&self, view: f32) -> bool {
        view >= self.base as f32 && view + VISIBLE as f32 <= (self.base + BAND) as f32
    }
}

#[cfg(test)]
mod tests;
