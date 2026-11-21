//! The player's channel list at the left of full-screen video
//! (`channels_view.rs`): the groups column, when it shows, and the list,
//! kept inside the window, and the video's rectangle beside them in the
//! preview layout. A window too narrow for the video beside the columns (a
//! phone held upright) keeps the video full screen under the list, as the
//! overlay layout does, rather than lay it out past the window's edge,
//! and the columns leave a touch target's width of it beside them.

/// The gap around the video beside the columns (points).
pub const LIST_PREVIEW_GAP: f32 = 40.0;
/// The narrowest video beside the columns (points).
pub const LIST_PREVIEW_MIN_W: f32 = 160.0;
/// The room the columns always leave at the right (points): one touch
/// target, where a tap closes the list over the video, and a strip of the
/// video itself.
pub const LIST_BESIDE_MIN: f32 = 48.0;

/// The channel list's columns in a window (logical points, whole).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListPanel {
    /// The groups column's width (0 while it is hidden).
    pub groups: f32,
    /// The list's width.
    pub list: f32,
}

impl ListPanel {
    /// The columns of a window `width` points wide whose list would be
    /// `list` and groups column `groups` points wide (0 while hidden):
    /// the groups column first, then the list in what is left, both
    /// leaving [`LIST_BESIDE_MIN`] at the right.
    pub fn of(width: f32, list: f32, groups: f32) -> Self {
        let room = (width - LIST_BESIDE_MIN).max(0.0);
        let groups = groups.clamp(0.0, room);
        let list = list.clamp(0.0, room - groups);
        Self { groups, list }
    }

    /// Both columns together.
    pub fn width(&self) -> f32 {
        self.groups + self.list
    }

    /// The video's rectangle (x, y, width, height) beside the columns in a
    /// window `width` points wide: right of them, 16:9, at the top. `None`
    /// where it would be narrower than [`LIST_PREVIEW_MIN_W`]: the video
    /// then stays full screen under the list.
    pub fn preview(&self, width: f32) -> Option<(f32, f32, f32, f32)> {
        let x = self.width() + LIST_PREVIEW_GAP;
        let w = width - x - LIST_PREVIEW_GAP;
        (w >= LIST_PREVIEW_MIN_W).then(|| (x, LIST_PREVIEW_GAP, w, w * 9.0 / 16.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roomy_windows_keep_both_columns_and_the_video_beside() {
        // A TV (scale 0.8) and a tablet, with and without the groups.
        let tv = ListPanel::of(960.0, 368.0, 208.0);
        assert_eq!((tv.groups, tv.list), (208.0, 368.0));
        assert_eq!(tv.preview(960.0), Some((616.0, 40.0, 304.0, 171.0)));
        let tablet = ListPanel::of(1280.0, 460.0, 0.0);
        assert_eq!(tablet.preview(1280.0), Some((500.0, 40.0, 740.0, 416.25)));
    }

    #[test]
    fn a_narrow_window_keeps_the_columns_inside_it() {
        // A phone held upright: the list fits the window, leaving a strip
        // to tap the list closed; the groups column takes its width first;
        // the video stays full screen.
        let list = ListPanel::of(412.0, 460.0, 0.0);
        assert_eq!((list.groups, list.list), (0.0, 364.0));
        assert_eq!(list.preview(412.0), None);
        let groups = ListPanel::of(412.0, 460.0, 260.0);
        assert_eq!((groups.groups, groups.list), (260.0, 104.0));
        assert_eq!(groups.width(), 412.0 - LIST_BESIDE_MIN);
        assert_eq!(groups.preview(412.0), None);
        // A window narrower than the groups column.
        let tiny = ListPanel::of(200.0, 460.0, 260.0);
        assert_eq!((tiny.groups, tiny.list), (152.0, 0.0));
    }

    #[test]
    fn the_video_goes_beside_only_where_it_has_its_width() {
        // A phone on its side: 915 - 460 - 80 leaves 375 points.
        let side = ListPanel::of(915.0, 460.0, 0.0);
        assert_eq!(side.preview(915.0), Some((500.0, 40.0, 375.0, 210.9375)));
        // With the groups column too, 915 - 720 - 80 leaves 115: too narrow.
        let both = ListPanel::of(915.0, 460.0, 260.0);
        assert_eq!(both.preview(915.0), None);
        // Exactly the narrowest video.
        let edge = ListPanel::of(660.0, 420.0, 0.0);
        assert_eq!(edge.preview(660.0).map(|r| r.2), Some(LIST_PREVIEW_MIN_W));
        assert_eq!(ListPanel::of(659.0, 420.0, 0.0).preview(659.0), None);
    }

    #[test]
    fn no_window_has_no_columns() {
        let p = ListPanel::of(-10.0, 460.0, 260.0);
        assert_eq!((p.groups, p.list), (0.0, 0.0));
    }
}
