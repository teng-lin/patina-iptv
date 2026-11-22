//! The guide's scale on desktop: its type and geometry (`fonts.rs`, the
//! grid, the ruler, the channel column and the header's text) are the
//! full-screen sizes, shrunk a little in a smaller window. An Android
//! tablet and an iPad keep the full sizes in every window, an iPad's Split
//! View and Slide Over ones included (touch needs them); a TV shows them
//! at [`TV`], as dense as the reference's TV guide (8 rows of ≈ 39 dp
//! under its header on a 960 × 540 dp screen).
//!
//! The window's ratio to a full-screen desktop is the smaller of its height
//! over [`H_REF`] and its width over [`W_REF`]; the scale follows it
//! [`DAMPING`] of the way down (type that shrank with the window would be
//! hard to read in a small one), in steps of [`STEP`] (a live resize lays
//! the text out again only when a step is crossed), within [`MIN`] and
//! [`MAX`]: 1920 × 1080 and 2560 × 1440 → 1.0, 1440 × 900 → 0.95,
//! 1280 × 720 → 0.875, 960 × 540 → 0.8.

use nsh_core::platform::{Idiom, Os};
use nsh_core::prelude::*;

/// The window (logical px) at and above which the full sizes show: a
/// full-screen 1080p desktop is taller and wider, even under a panel.
pub const H_REF: f32 = 1000.0;
pub const W_REF: f32 = 1600.0;
/// How much of the window's shrinking the scale follows.
pub const DAMPING: f32 = 0.45;
/// The smallest and largest scale.
pub const MIN: f32 = 0.8;
pub const MAX: f32 = 1.0;
/// The scale moves in these steps.
pub const STEP: f32 = 0.025;
/// Whether this build scales the guide with its window (desktops).
const SCALES: bool = !(crate::host::ANDROID || crate::host::IPAD);

/// Whether the guide scales with its window: on desktops, not on a tablet
/// or a phone, whose windows (an Android tablet's or phone's screen, an
/// iPad's full screen, Split View, Slide Over or Stage Manager window) are
/// touched and keep the full sizes. The reported idiom and system decide
/// too, so a headless tablet, or a headless Android phone, lays out as the
/// device does. (NSH's default headless environment is an iOS phone; it
/// keeps the desktop's scaling, which the scenarios built on it measure.)
fn scales() -> bool {
    SCALES && idiom() != Idiom::Pad && !phone()
}

/// A phone: an Android or iOS build reporting the phone idiom, or a
/// headless Android phone. (NSH's default headless environment, an iOS
/// phone, is not one: the scenarios built on it measure the desktop.)
///
/// Reactive through the window's size: NSH reads the idiom untracked, and
/// Android derives it from the window's smallest width (a foldable phone
/// unfolded becomes a tablet), so a change of idiom comes with a resize.
pub(crate) fn phone() -> bool {
    let _ = window();
    idiom() == Idiom::Phone && (!SCALES || os() == Os::Android)
}

/// The scale on a TV (rows 50 → 40 dp, type 20 → 16).
pub const TV: f32 = 0.8;
/// The scale on a phone (rows 50 → 35 dp, type 20 → 14): the tablet's full
/// sizes are far too large on a screen about 360 to 412 points across, and
/// the system's font scale still applies on top. Measured on a 360-point
/// phone at font scale 1.15, the guide's names then draw at about the size
/// of a phone IPTV app's secondary lines (14 sp), under its list names
/// (about 17 sp).
pub const PHONE: f32 = 0.7;

/// The scale for a `width` × `height` window (logical px); 1.0 where the
/// platform does not scale.
pub fn scale_for(width: f32, height: f32, scales: bool) -> f32 {
    if !scales || !(width > 0.0 && height > 0.0) {
        return MAX;
    }
    let ratio = (height / H_REF).min(width / W_REF);
    let raw = 1.0 - DAMPING * (1.0 - ratio);
    ((raw / STEP).round() * STEP).clamp(MIN, MAX)
}

/// The scale of the window now (reactive: the window's size, which a change
/// of idiom comes with: read before the TV's and the phone's fixed scales,
/// so a foldable phone unfolded takes the tablet's).
pub fn now() -> f32 {
    let w = window();
    if idiom() == Idiom::Tv {
        return TV;
    }
    if phone() {
        return PHONE;
    }
    scale_for(w.width.0, w.height.0, scales())
}

/// The scale as a memo: views read it in their bindings, which run again
/// only when the scale changes (not on every resize).
#[derive(Clone, Copy)]
pub struct Scale(Memo<f32>);

impl Scale {
    pub fn new() -> Self {
        Self(computed(now))
    }

    /// The scale now (reactive).
    pub fn get(self) -> f32 {
        self.0.get()
    }

    /// `size` (dp or sp at full size) at the scale now.
    pub fn of(self, size: f32) -> f32 {
        size * self.get()
    }

    /// Text size, independent of geometry and responsive window scaling.
    pub fn text_px(self, size: f32) -> Points {
        px(self.of(size) * crate::settings::font_factor())
    }

    /// [`Self::of`] in whole px, for row pitches and boxes (rows the same
    /// height each, not alternating around a fraction).
    pub fn whole(self, size: f32) -> f32 {
        self.of(size).round()
    }

    /// [`Self::of`] as a length.
    pub fn px(self, size: f32) -> Points {
        px(self.of(size))
    }
}

impl Default for Scale {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn full_screen_desktops_show_the_full_sizes() {
        assert_eq!(scale_for(1920.0, 1080.0, true), 1.0);
        assert_eq!(scale_for(2560.0, 1440.0, true), 1.0);
        assert_eq!(scale_for(3840.0, 2160.0, true), 1.0, "never above 1");
        // Full screen under a 30 px panel.
        assert_eq!(scale_for(1920.0, 1050.0, true), 1.0);
        assert_eq!(scale_for(1600.0, 1000.0, true), 1.0, "the reference");
    }

    #[test]
    fn smaller_windows_shrink_in_steps() {
        assert!(close(scale_for(1280.0, 720.0, true), 0.875));
        assert!(close(scale_for(1440.0, 900.0, true), 0.95));
        assert!(
            close(scale_for(1280.0, 1000.0, true), 0.9),
            "the width decides"
        );
        assert!(
            close(scale_for(1920.0, 720.0, true), 0.875),
            "the height decides"
        );
        assert!(close(scale_for(1366.0, 768.0, true), 0.9));
        // Within a step: the same scale (no re-layout while resizing).
        assert_eq!(
            scale_for(1280.0, 720.0, true),
            scale_for(1290.0, 726.0, true)
        );
    }

    #[test]
    fn small_windows_stop_at_the_minimum() {
        assert!(close(scale_for(960.0, 540.0, true), MIN));
        assert!(close(scale_for(640.0, 360.0, true), MIN));
        assert!(close(scale_for(400.0, 300.0, true), MIN));
        assert_eq!(scale_for(0.0, 0.0, true), MAX, "no window yet");
    }

    #[test]
    fn android_keeps_the_full_sizes() {
        // A tablet (1280 × 800 dp) and a TV (960 × 540 dp).
        assert_eq!(scale_for(1280.0, 800.0, false), 1.0);
        assert_eq!(scale_for(960.0, 540.0, false), 1.0);
        assert_eq!(SCALES, !(crate::host::ANDROID || crate::host::IPAD));
    }
}
