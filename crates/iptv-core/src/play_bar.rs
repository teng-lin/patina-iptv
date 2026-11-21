//! The pointer's play bar over full-screen video (desktop mouse, tablet
//! touch and TV remote): when it shows and when it hides on its own,
//! and where a press on its progress bar seeks to.
//!
//! The mouse moving over the video shows it; a tap (or a click) on the
//! video shows it, and another hides it. It hides once nothing happened
//! for [`HIDE_AFTER_MS`], but not while the mouse rests on it nor while a
//! popup it opened (a chooser, the program details) is open. Using one
//! of its controls counts as something happening. Times are milliseconds
//! on the caller's monotonic clock (no clock access here).

/// How long the bar stays after the last pointer input.
pub const HIDE_AFTER_MS: u64 = 3_000;

/// Stable identities shared by pointer buttons and remote focus. Hosts supply
/// the currently enabled controls; focus never depends on a rendered index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Mute,
    Rewind,
    PlayPause,
    Forward,
    Record,
    Favorite,
    Tracks,
    Details,
    Picture,
    Channels,
    Multiview,
    Pip,
    Guide,
    Fullscreen,
    Timeline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayBar {
    pub shown: bool,
    pub focus: Option<Control>,
    /// The mouse is over the bar: it stays.
    pub hovered: bool,
    /// A popup it opened is open: it stays.
    pub held: bool,
    hide_at_ms: Option<u64>,
    /// A held remote key decides tap or hold at this time: apart from the
    /// idle deadline, so the bar's use meanwhile does not pull it in.
    hold_until_ms: Option<u64>,
}

impl PlayBar {
    /// The mouse moved over the video.
    pub fn pointer_moved(&mut self, now_ms: u64) {
        self.shown = true;
        self.focus = None;
        self.restart(now_ms);
    }

    /// A tap or a click on the video (not on the bar): shows the bar, or
    /// hides it when it shows.
    pub fn tapped(&mut self, now_ms: u64) {
        if self.shown {
            self.hide();
        } else {
            self.shown = true;
            self.focus = None;
            self.restart(now_ms);
        }
    }

    /// Explicit remote activation opens the same bar with transport focus.
    pub fn remote(&mut self, now_ms: u64) {
        self.shown = true;
        self.hovered = false;
        self.focus = Some(Control::PlayPause);
        self.restart(now_ms);
    }

    pub fn focus_control(&mut self, control: Control, now_ms: u64) {
        self.focus = Some(control);
        self.used(now_ms);
    }

    /// Traverse available controls by identity, including after capabilities
    /// change while the bar is visible.
    pub fn move_focus(&mut self, step: isize, enabled: &[Control], now_ms: u64) {
        if enabled.is_empty() {
            return;
        }
        let current = self
            .focus
            .and_then(|c| enabled.iter().position(|v| *v == c))
            .or_else(|| enabled.iter().position(|c| *c == Control::PlayPause))
            .unwrap_or(0);
        let next = (current as isize + step).clamp(0, enabled.len() as isize - 1) as usize;
        self.focus_control(enabled[next], now_ms);
    }

    /// One of the bar's controls was used: it stays a while longer.
    pub fn used(&mut self, now_ms: u64) {
        if self.shown {
            self.restart(now_ms);
        }
    }

    /// The mouse came over the bar (`true`) or left it.
    pub fn hover(&mut self, over: bool, now_ms: u64) {
        self.hovered = over;
        if self.shown {
            self.restart(now_ms);
        }
    }

    /// A popup opened (`true`) or closed over the video: the bar stays
    /// while one is open, and the delay starts again once it closes.
    pub fn hold(&mut self, open: bool, now_ms: u64) {
        self.held = open;
        if self.shown {
            self.restart(now_ms);
        }
    }

    pub fn hide(&mut self) {
        self.shown = false;
        self.focus = None;
        self.hovered = false;
        self.hide_at_ms = None;
        self.hold_until_ms = None;
    }

    /// When it hides unless something happens (for scheduling); `None`
    /// while hidden, while the mouse rests on it or a popup is open. A
    /// held key's deadline counts when later than its own.
    pub fn hide_at_ms(&self) -> Option<u64> {
        if self.hovered || self.held {
            None
        } else {
            match (self.hide_at_ms, self.hold_until_ms) {
                (Some(idle), Some(hold)) => Some(idle.max(hold)),
                (idle, hold) => idle.or(hold.filter(|_| self.shown)),
            }
        }
    }

    /// Hides once the deadline passed. Returns whether it hid.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        match self.hide_at_ms() {
            Some(at) if self.shown && now_ms >= at => {
                self.hide();
                true
            }
            _ => false,
        }
    }

    /// A remote key is held and decides tap or hold at `at_ms`: the bar
    /// stays at least until then, so the press finds it on release.
    pub fn keep_until(&mut self, at_ms: u64) {
        if self.shown {
            self.hold_until_ms = Some(at_ms);
        }
    }

    /// The held key's press was decided or dropped: only the bar's own
    /// deadline counts again.
    pub fn release_hold(&mut self) {
        self.hold_until_ms = None;
    }

    /// Whether a held key keeps the bar ([`Self::keep_until`]).
    pub fn holding(&self) -> bool {
        self.hold_until_ms.is_some()
    }

    fn restart(&mut self, now_ms: u64) {
        self.hide_at_ms = Some(now_ms + HIDE_AFTER_MS);
    }
}

/// Where a press `x` points into a progress bar `width` points wide seeks
/// to, in seconds into a program `length` seconds long, no further than
/// `limit` (what the archive holds so far).
pub fn seek_target(x: f32, width: f32, length: i32, limit: i32) -> i32 {
    // A NaN width seeks nowhere too.
    if width.is_nan() || width <= 0.0 || length <= 0 {
        return 0;
    }
    let fraction = (x / width).clamp(0.0, 1.0);
    ((fraction * length as f32).round() as i32).clamp(0, limit.clamp(0, length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_key_keeps_the_bar_until_its_press_is_decided() {
        let mut bar = PlayBar::default();
        bar.keep_until(10_000);
        assert_eq!(bar.hide_at_ms(), None, "a hidden bar stays hidden");
        bar.remote(0);
        bar.keep_until(HIDE_AFTER_MS + 500);
        assert!(!bar.tick(HIDE_AFTER_MS));
        assert!(bar.tick(HIDE_AFTER_MS + 500));
        // Use meanwhile restarts its own timer, not the held key's.
        bar.remote(0);
        bar.keep_until(5_000);
        bar.used(100);
        assert_eq!(bar.hide_at_ms(), Some(5_000));
        bar.release_hold();
        assert!(!bar.holding());
        assert_eq!(bar.hide_at_ms(), Some(100 + HIDE_AFTER_MS));
    }

    #[test]
    fn the_mouse_shows_it_until_it_rests() {
        let mut bar = PlayBar::default();
        assert!(!bar.shown);
        bar.pointer_moved(1_000);
        assert!(bar.shown);
        assert_eq!(bar.hide_at_ms(), Some(1_000 + HIDE_AFTER_MS));
        // Moving again moves the deadline.
        bar.pointer_moved(2_500);
        assert!(!bar.tick(4_000));
        assert!(bar.shown);
        assert!(bar.tick(2_500 + HIDE_AFTER_MS));
        assert!(!bar.shown);
        assert_eq!(bar.hide_at_ms(), None);
    }

    #[test]
    fn it_stays_while_the_mouse_rests_on_it() {
        let mut bar = PlayBar::default();
        bar.pointer_moved(0);
        bar.hover(true, 1_000);
        assert_eq!(bar.hide_at_ms(), None);
        assert!(!bar.tick(60_000));
        assert!(bar.shown);
        // Leaving it starts the delay again.
        bar.hover(false, 60_000);
        assert!(!bar.tick(61_000));
        assert!(bar.tick(60_000 + HIDE_AFTER_MS));
    }

    #[test]
    fn it_stays_while_a_popup_it_opened_is_open() {
        let mut bar = PlayBar::default();
        bar.tapped(0);
        bar.hold(true, 1_000);
        assert_eq!(bar.hide_at_ms(), None);
        assert!(!bar.tick(60_000));
        assert!(bar.shown);
        // The popup closed: hidden after the delay, not at once.
        bar.hold(false, 60_000);
        assert!(!bar.tick(61_000));
        assert!(bar.tick(60_000 + HIDE_AFTER_MS));
        // A popup open while it hides (the remote's) leaves it hidden.
        bar.hold(true, 70_000);
        assert!(!bar.shown);
        // Hiding keeps the hold: it still knows the popup is open.
        bar.tapped(71_000);
        assert_eq!(bar.hide_at_ms(), None);
    }

    #[test]
    fn a_tap_toggles_it_and_using_it_keeps_it() {
        let mut bar = PlayBar::default();
        bar.tapped(0);
        assert!(bar.shown);
        bar.used(2_000);
        assert!(!bar.tick(HIDE_AFTER_MS));
        assert!(bar.shown);
        bar.tapped(2_500);
        assert!(!bar.shown);
        // Using a hidden bar does not show it.
        bar.used(3_000);
        assert!(!bar.shown);
        assert_eq!(bar.hide_at_ms(), None);
    }

    #[test]
    fn remote_focus_skips_unavailable_controls_and_recovers_by_identity() {
        use Control::*;
        let mut bar = PlayBar::default();
        bar.remote(0);
        assert_eq!(bar.focus, Some(PlayPause));
        bar.move_focus(1, &[Mute, PlayPause, Favorite, Guide], 100);
        assert_eq!(bar.focus, Some(Favorite));
        // Favorite disappears: recover from transport, not its old index.
        bar.move_focus(1, &[Mute, PlayPause, Picture, Guide], 200);
        assert_eq!(bar.focus, Some(Picture));
        bar.move_focus(-99, &[Mute, PlayPause, Picture, Guide], 300);
        assert_eq!(bar.focus, Some(Mute));
        assert!(!bar.tick(3_299));
        assert!(bar.tick(3_300));
        assert_eq!(bar.focus, None);
    }

    #[test]
    fn a_press_on_the_progress_bar_seeks_within_the_archive() {
        // A 60-minute program, 600 px wide.
        assert_eq!(seek_target(300.0, 600.0, 3_600, 3_600), 1_800);
        assert_eq!(seek_target(-20.0, 600.0, 3_600, 3_600), 0);
        assert_eq!(seek_target(900.0, 600.0, 3_600, 3_600), 3_600);
        // Still airing: no further than what aired.
        assert_eq!(seek_target(540.0, 600.0, 3_600, 1_200), 1_200);
        // Nothing laid out yet, or no length.
        assert_eq!(seek_target(10.0, 0.0, 3_600, 3_600), 0);
        assert_eq!(seek_target(10.0, 600.0, 0, 0), 0);
    }
}
