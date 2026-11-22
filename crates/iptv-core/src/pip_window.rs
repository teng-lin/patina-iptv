//! The desktops' picture-in-picture window, as browsers have it: a
//! separate small window of the app's own showing the full-screen player,
//! while the main window stays usable (it shows a placeholder where the
//! video was). The player is the same one: the window shows its picture,
//! no stream opens for it.
//!
//! The pointer over it shows its controls for a while ([`Hover`]); a
//! double click, Escape or its "back" control bring the video back to the
//! main window ([`Exit::BackToApp`]); its close control, or the system
//! closing it, closes it and stops playback ([`Exit::Close`]). Dragging
//! it moves it: a press on the video is a window drag, except right after
//! a click, so that the second click of a double click reaches the app.
//! Times are milliseconds on the caller's monotonic clock.

/// How long the controls stay after the pointer last moved over the window.
pub const HIDE_AFTER_MS: u64 = 2_500;

/// Two clicks this close together are a double click.
pub const DOUBLE_CLICK_MS: u64 = 500;

/// How the window closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The video comes back to the main window, full screen, playing on.
    BackToApp,
    /// The window closes and playback stops (its close control, the
    /// system's close: the user dismissed it).
    Close,
}

/// The pointer over the window: when its controls show, and when a press
/// drags the window rather than counting as a click.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hover {
    /// The pointer is over the window.
    pub inside: bool,
    /// When it last moved over it.
    moved_ms: Option<u64>,
    /// When the last single click was.
    click_ms: Option<u64>,
}

impl Hover {
    /// The pointer moved over the window at `now_ms`: the controls show.
    pub fn moved(&mut self, now_ms: u64) {
        self.inside = true;
        self.moved_ms = Some(now_ms);
    }

    /// The pointer left the window: the controls go.
    pub fn left(&mut self) {
        self.inside = false;
    }

    /// Whether the controls show at `now_ms`.
    pub fn shows(&self, now_ms: u64) -> bool {
        self.inside
            && self
                .moved_ms
                .is_some_and(|at| now_ms.saturating_sub(at) < HIDE_AFTER_MS)
    }

    /// When the controls hide unless the pointer moves again.
    pub fn hide_at_ms(&self) -> Option<u64> {
        self.moved_ms
            .filter(|_| self.inside)
            .map(|at| at + HIDE_AFTER_MS)
    }

    /// A click on the video at `now_ms`: whether it completes a double
    /// click (the window then goes back to the app).
    pub fn click(&mut self, now_ms: u64) -> bool {
        let double = self
            .click_ms
            .is_some_and(|at| now_ms.saturating_sub(at) <= DOUBLE_CLICK_MS);
        self.click_ms = if double { None } else { Some(now_ms) };
        double
    }

    /// Whether a press on the video at `now_ms` drags the window: not
    /// while a click may still become a double click.
    pub fn drags(&self, now_ms: u64) -> bool {
        self.click_ms
            .is_none_or(|at| now_ms.saturating_sub(at) > DOUBLE_CLICK_MS)
    }

    /// When presses drag the window again after a click.
    pub fn drags_again_at_ms(&self) -> Option<u64> {
        self.click_ms.map(|at| at + DOUBLE_CLICK_MS + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controls_show_while_the_pointer_moves_over_it() {
        let mut h = Hover::default();
        assert!(!h.shows(0));
        assert_eq!(h.hide_at_ms(), None);
        h.moved(1_000);
        assert!(h.shows(1_000));
        assert!(h.shows(1_000 + HIDE_AFTER_MS - 1));
        assert!(!h.shows(1_000 + HIDE_AFTER_MS), "resting");
        assert_eq!(h.hide_at_ms(), Some(1_000 + HIDE_AFTER_MS));
        h.moved(2_000);
        assert!(h.shows(1_000 + HIDE_AFTER_MS), "moved again");
        h.left();
        assert!(!h.shows(2_000), "the pointer left");
        assert_eq!(h.hide_at_ms(), None);
    }

    #[test]
    fn two_quick_clicks_are_a_double_click_and_suspend_dragging_between() {
        let mut h = Hover::default();
        assert!(h.drags(0));
        assert!(!h.click(1_000), "one click");
        assert!(!h.drags(1_100), "the second press is a click, not a drag");
        assert_eq!(h.drags_again_at_ms(), Some(1_000 + DOUBLE_CLICK_MS + 1));
        assert!(h.click(1_300), "double");
        assert!(h.drags(1_301), "dragging again");
        assert!(!h.click(1_400), "a new first click");
        assert!(
            !h.click(1_400 + DOUBLE_CLICK_MS + 1),
            "too slow: first again"
        );
        assert!(h.drags(1_400 + 2 * DOUBLE_CLICK_MS + 2));
    }
}
