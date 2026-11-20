//! The guide's header: the preview at the top left, the focused programme
//! beside it, and the shortcuts (the list on screen, Movies, Shows, Add
//! playlist) at the right or, where the window has no room for three
//! columns, on a row of their own under the other two.
//!
//! The preview keeps its place at the top left in every arrangement: the
//! player under it is only resized, never hidden or moved elsewhere, and it
//! never shrinks below [`PREVIEW_MIN_H`] (the header grows instead).

use super::Class;

/// The header's share of the window's height.
pub const HEADER_SHARE: f32 = 0.33;
/// The preview's largest share of the window's width.
pub const PREVIEW_SHARE: f32 = 0.32;
/// The preview's margin inside the header (points).
pub const PREVIEW_MARGIN: f32 = 12.0;
/// The preview's smallest height (points): the corner ‹ (16 points from the
/// top, a 48-point touch target) stays inside it, above the shortcuts' row.
pub const PREVIEW_MIN_H: f32 = 64.0;
/// The most of the window's height a short window's header grows to.
pub const HEADER_MAX_SHARE: f32 = 0.55;
/// The gap between the header's columns (points at scale 1).
pub const COLUMN_GAP: f32 = 24.0;
/// The shortcuts' row under the preview: its height and the gap above it
/// (points at scale 1; the row also grows with larger text).
pub const SHORTCUTS_ROW: f32 = 48.0;
pub const SHORTCUTS_GAP: f32 = 8.0;
/// The room the shortcuts' column and the focused programme need beside
/// the preview (points at a text factor of 1): Movies and Shows side by
/// side, and a programme's title and times.
pub const SHORTCUTS_W: f32 = 260.0;
pub const DETAILS_MIN_W: f32 = 240.0;

/// Where the guide's header puts its shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcuts {
    /// A column at the right, beside the preview and the focused programme.
    Beside,
    /// A row of their own under the preview and the focused programme, so
    /// the programme keeps the width beside the preview.
    Below,
}

/// The guide header's geometry (logical points, whole).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideHeader {
    /// The header's height under the status bar.
    pub height: f32,
    /// The preview's width and height.
    pub preview: (f32, f32),
    pub shortcuts: Shortcuts,
    /// The shortcuts' row height and the gap above it (0 beside).
    pub row: f32,
    pub row_gap: f32,
}

impl GuideHeader {
    /// The header of a window `width` × `height` whose guide is drawn at
    /// `scale` with the viewer's text size `font` (1 by default).
    pub fn of(width: f32, height: f32, scale: f32, font: f32) -> Self {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let font = if font > 0.0 { font } else { 1.0 };
        let text = scale * font;
        // The programme's width if the shortcuts stayed beside it, with the
        // preview as that arrangement draws it.
        let (_, beside) = fit(width, height, 0.0);
        let room = width - 2.0 * PREVIEW_MARGIN - beside - 2.0 * COLUMN_GAP * scale;
        let shortcuts = if Class::of(width, text) == Class::Compact
            || room < (SHORTCUTS_W + DETAILS_MIN_W) * text
        {
            Shortcuts::Below
        } else {
            Shortcuts::Beside
        };
        let (row, row_gap) = match shortcuts {
            Shortcuts::Beside => (0.0, 0.0),
            Shortcuts::Below => (
                (SHORTCUTS_ROW * scale * font.max(1.0)).round(),
                (SHORTCUTS_GAP * scale).round(),
            ),
        };
        let (height, w) = fit(width, height, row + row_gap);
        Self {
            height,
            preview: (w, (w * 9.0 / 16.0).round()),
            shortcuts,
            row,
            row_gap,
        }
    }
}

/// The header's height and the preview's width in a window `width` ×
/// `height` with `below` points under the preview. A short window grows the
/// header rather than lose the preview, up to [`HEADER_MAX_SHARE`] of its
/// height (the rail and the guide keep the rest); a window too short even
/// for that squeezes the preview instead.
fn fit(width: f32, height: f32, below: f32) -> (f32, f32) {
    let share = (height * HEADER_SHARE).round();
    let wanted = 2.0 * PREVIEW_MARGIN + PREVIEW_MIN_H + below;
    let header = share.max(wanted.min((height * HEADER_MAX_SHARE).floor()));
    let tall = header - 2.0 * PREVIEW_MARGIN - below;
    let mut w = preview_width(width, tall);
    if tall >= PREVIEW_MIN_H {
        w = w.max((PREVIEW_MIN_H * 16.0 / 9.0).ceil());
    }
    (header, w.min((width - 2.0 * PREVIEW_MARGIN).max(0.0)))
}

/// A 16:9 preview as tall as `tall`, narrowed to its share of `width`.
fn preview_width(width: f32, tall: f32) -> f32 {
    (tall.max(0.0) * 16.0 / 9.0)
        .min(width * PREVIEW_SHARE)
        .round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roomy_windows_keep_the_three_columns_and_their_shares() {
        // Tablet (1920 × 1200 at 1.5 px/dp), desktop windows, TV (1080p at 2).
        for (w, h, scale, want) in [
            (1280.0, 800.0, 1.0, (264.0, 410.0, 231.0)),
            (1280.0, 720.0, 0.875, (238.0, 380.0, 214.0)),
            (960.0, 540.0, 0.8, (178.0, 274.0, 154.0)),
            (1920.0, 1080.0, 1.0, (356.0, 590.0, 332.0)),
        ] {
            let g = GuideHeader::of(w, h, scale, 1.0);
            assert_eq!(g.shortcuts, Shortcuts::Beside, "{w}x{h}");
            assert_eq!((g.height, g.preview.0, g.preview.1), want, "{w}x{h}");
            assert_eq!((g.row, g.row_gap), (0.0, 0.0));
        }
    }

    #[test]
    fn the_shortcuts_go_below_where_the_programme_has_no_room() {
        // Compact windows, then medium ones too narrow for three columns:
        // a 560-point desktop window and a portrait tablet.
        for (w, h, scale) in [
            (400.0, 600.0, 0.8),
            (480.0, 800.0, 0.8),
            (360.0, 800.0, 1.0),
            (560.0, 800.0, 0.8),
            (620.0, 800.0, 0.8),
            (800.0, 1280.0, 1.0),
        ] {
            let g = GuideHeader::of(w, h, scale, 1.0);
            assert_eq!(g.shortcuts, Shortcuts::Below, "{w}x{h}");
        }
        // Wide enough beside: a 760-point desktop window, and the same
        // window with extra-large text, which needs more room.
        assert_eq!(
            GuideHeader::of(760.0, 480.0, 0.8, 1.0).shortcuts,
            Shortcuts::Beside
        );
        assert_eq!(
            GuideHeader::of(760.0, 480.0, 0.8, 1.3).shortcuts,
            Shortcuts::Below
        );
    }

    #[test]
    fn the_preview_never_falls_below_its_smallest_height() {
        for (w, h, scale, font) in [
            (400.0, 600.0, 0.8, 1.0),
            (480.0, 240.0, 0.8, 1.0),
            (480.0, 300.0, 0.8, 1.3),
            (480.0, 320.0, 0.8, 1.0),
            (320.0, 540.0, 0.8, 1.3),
            (360.0, 800.0, 1.0, 1.3),
            (960.0, 200.0, 0.8, 1.0),
        ] {
            let g = GuideHeader::of(w, h, scale, font);
            let (pw, ph) = g.preview;
            assert!(ph >= PREVIEW_MIN_H, "{w}x{h} at {font}: {g:?}");
            assert!(pw + 2.0 * PREVIEW_MARGIN <= w, "{w}x{h}: {g:?}");
            // The preview, the row and the margins fit inside the header,
            // so the ‹ (16 + 48 points) ends above the row.
            assert!(
                PREVIEW_MARGIN + ph + g.row_gap + g.row + PREVIEW_MARGIN <= g.height + 0.5,
                "{w}x{h} at {font}: {g:?}"
            );
            assert!(16.0 + 48.0 <= PREVIEW_MARGIN + ph, "{w}x{h}: {g:?}");
            assert!(g.height >= (h * HEADER_SHARE).round());
            if g.shortcuts == Shortcuts::Below {
                assert!(g.row >= (SHORTCUTS_ROW * scale * font).round() - 0.5);
            }
        }
        // A roomy window keeps its share: the minimum only grows short ones.
        assert_eq!(GuideHeader::of(400.0, 600.0, 0.8, 1.0).height, 198.0);
    }

    #[test]
    fn a_very_short_window_squeezes_the_preview_rather_than_the_rail() {
        // Extra-large text in a 180-point window: the minimum would take 144
        // points; the header stops at its largest share and the preview gives.
        for (w, h, font) in [
            (480.0, 180.0, 1.3),
            (480.0, 120.0, 1.0),
            (960.0, 100.0, 1.0),
        ] {
            let g = GuideHeader::of(w, h, 0.8, font);
            assert!(
                g.height
                    <= (h * HEADER_MAX_SHARE)
                        .floor()
                        .max((h * HEADER_SHARE).round())
            );
            assert!(g.height >= (h * HEADER_SHARE).round(), "{w}x{h}: {g:?}");
            let (pw, ph) = g.preview;
            assert!(ph + 2.0 * PREVIEW_MARGIN + g.row + g.row_gap <= g.height + 0.5 || ph == 0.0);
            assert!(pw + 2.0 * PREVIEW_MARGIN <= w, "{w}x{h}: {g:?}");
        }
        assert_eq!(GuideHeader::of(480.0, 180.0, 0.8, 1.3).height, 99.0);
    }

    #[test]
    fn the_arrangement_budgets_the_preview_it_draws() {
        // At 570 × 240 the minimum widens the preview to 114 points; beside
        // it the columns would get 394 of the 400 they need.
        let g = GuideHeader::of(570.0, 240.0, 0.8, 1.0);
        assert_eq!(g.shortcuts, Shortcuts::Below, "{g:?}");
        let beside = GuideHeader::of(1280.0, 720.0, 0.875, 1.0);
        assert_eq!(beside.shortcuts, Shortcuts::Beside);
    }

    #[test]
    fn no_scale_or_text_size_counts_as_one() {
        assert_eq!(
            GuideHeader::of(1280.0, 800.0, 0.0, f32::NAN),
            GuideHeader::of(1280.0, 800.0, 1.0, 1.0)
        );
    }
}
