//! The desktops' picture-in-picture window, as browsers have it: a
//! separate small window of the app's own showing the full-screen player,
//! while the main window stays usable (it shows a placeholder where the
//! video was). The player is the same one: the window shows its picture,
//! no stream opens for it.
//!
//! The window is titled exactly [`TITLE`], the title browsers give theirs,
//! so the rules desktops keep for browsers' picture-in-picture windows
//! (floating, pinned on every workspace, above the others) apply to it
//! too. Its size follows the stream's shape ([`size_for`]).
//!
//! The pointer over it shows its controls for a while ([`Hover`]); a
//! double click, Escape or its "back" control bring the video back to the
//! main window ([`Exit::BackToApp`]); its close control, or the system
//! closing it, closes it and stops playback ([`Exit::Close`]). Dragging
//! it moves it: a press on the video is a window drag, except right after
//! a click, so that the second click of a double click reaches the app.
//! Times are milliseconds on the caller's monotonic clock.

/// The window's title: browsers' picture-in-picture title.
pub const TITLE: &str = crate::i18n::key("Picture-in-Picture");

/// The window's width when it opens (points).
pub const WIDTH: f32 = 600.0;

/// How long the controls stay after the pointer last moved over the window.
pub const HIDE_AFTER_MS: u64 = 2_500;

/// Two clicks this close together are a double click.
pub const DOUBLE_CLICK_MS: u64 = 500;

/// The window's size when it opens (points): [`WIDTH`] wide, the stream's
/// shape (16:9 until it is known, kept between 4:3 and 21:9), rounded to
/// whole points.
pub fn size_for(video: Option<(u32, u32)>) -> (f32, f32) {
    let aspect = video
        .filter(|(w, h)| *w > 0 && *h > 0)
        .map_or(16.0 / 9.0, |(w, h)| w as f32 / h as f32)
        .clamp(4.0 / 3.0, 21.0 / 9.0);
    (WIDTH, (WIDTH / aspect).round())
}

/// Initial slots for up to four compact players. Native adapters place only
/// newly compact windows; existing user placements remain unchanged.
/// Slots reserve the supported tallest shape (4:3); adapters fit each player's
/// actual shape inside its slot without stretching it.
pub fn popup_rects(area: (f32, f32, f32, f32), count: usize) -> Vec<(f32, f32, f32, f32)> {
    let (x, y, width, height) = area;
    let count = count.min(crate::multiview::MAX_SURFACES);
    if count == 0
        || width <= 0.0
        || height <= 0.0
        || ![x, y, width, height].into_iter().all(f32::is_finite)
    {
        return Vec::new();
    }
    let columns = count.min(2);
    let rows = count.div_ceil(columns);
    let gap = 16.0_f32.min(width / 8.0).min(height / 8.0);
    let w = WIDTH
        .min((width - gap * (columns + 1) as f32) / columns as f32)
        .min((height - gap * (rows + 1) as f32) / rows as f32 * 4.0 / 3.0);
    let h = w * 3.0 / 4.0;
    (0..count)
        .map(|i| {
            (
                x + width - gap - w - (i % columns) as f32 * (w + gap),
                y + height - gap - h - (i / columns) as f32 * (h + gap),
                w,
                h,
            )
        })
        .collect()
}

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
    fn compact_slots_are_separate_and_within_small_and_offset_displays() {
        for area in [(220.0, -482.0, 2560.0, 1440.0), (0.0, 0.0, 800.0, 600.0)] {
            for count in 1..=4 {
                let rects = popup_rects(area, count);
                assert_eq!(rects.len(), count);
                for (i, &(x, y, w, h)) in rects.iter().enumerate() {
                    assert!(x >= area.0 && y >= area.1);
                    assert!(x + w <= area.0 + area.2 + 0.01 && y + h <= area.1 + area.3 + 0.01);
                    for &(a, b, c, d) in &rects[..i] {
                        assert!(x + w <= a || a + c <= x || y + h <= b || b + d <= y);
                    }
                }
            }
        }
        assert!(popup_rects((0.0, 0.0, 0.0, 600.0), 4).is_empty());
        assert!(popup_rects((0.0, 0.0, f32::NAN, 600.0), 4).is_empty());
    }

    #[test]
    fn the_title_is_the_one_browsers_give_their_window() {
        assert_eq!(TITLE, "Picture-in-Picture");
    }

    #[test]
    fn it_opens_600_wide_in_the_shape_of_the_stream() {
        assert_eq!(size_for(None), (600.0, 338.0), "16:9 until known");
        assert_eq!(size_for(Some((1920, 1080))), (600.0, 338.0));
        assert_eq!(
            size_for(Some((720, 576))),
            (600.0, 450.0),
            "5:4 → 4:3 at most"
        );
        assert_eq!(size_for(Some((640, 480))), (600.0, 450.0));
        assert_eq!(
            size_for(Some((3840, 1080))),
            (600.0, 257.0),
            "21:9 at least"
        );
        assert_eq!(size_for(Some((0, 0))), (600.0, 338.0), "no size yet");
    }

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
