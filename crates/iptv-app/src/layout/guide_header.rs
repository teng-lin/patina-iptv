//! The guide's header: the preview at the top left, the focused program
//! beside it, and the shortcuts (the list on screen, Movies, Shows, Add
//! playlist) at the right or, where the window has no room for three
//! columns, on a row of their own under the other two. A compact portrait
//! window (a phone held upright) stacks them instead: the preview across
//! the window's width, the program under it, then the shortcuts' row,
//! where beside the program the preview would be a thumbnail.
//!
//! The preview keeps its place at the top left in every arrangement: the
//! player under it is only resized, never hidden or moved elsewhere, and it
//! never shrinks below [`PREVIEW_MIN_H`] (the header grows instead).

use super::Class;
use crate::fonts::{HEADER_TIMES, HEADER_TITLE};

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
/// At the accessibility text sizes (a text factor from
/// [`LARGE_TEXT`]), the most the header grows to keep the focused
/// program's title and times whole: the guide's rows, drawn as large,
/// show fewer of themselves either way.
pub const LARGE_TEXT: f32 = 1.5;
pub const LARGE_TEXT_MAX_SHARE: f32 = 0.7;
/// The gap between the header's columns (points at scale 1).
pub const COLUMN_GAP: f32 = 24.0;
/// The shortcuts' row under the preview: its height and the gap above it
/// (points at scale 1; the row also grows with larger text).
pub const SHORTCUTS_ROW: f32 = 48.0;
pub const SHORTCUTS_GAP: f32 = 8.0;
/// The room the shortcuts' column and the focused program need beside
/// the preview (points at a text factor of 1): Movies and Shows side by
/// side, and a program's title and times.
pub const SHORTCUTS_W: f32 = 260.0;
pub const DETAILS_MIN_W: f32 = 240.0;
/// The focused program's lines beside the preview, as the app draws
/// them (points at a text factor of 1): the title's and the times' type,
/// their line height, and the widths of the times row's parts (the span
/// "12:40 PM – 1:00 PM", the progress bar, "15 min left") and the gaps.
/// [`details_height`] estimates from them the room the title and the
/// times take, before the description, which gives way.
pub const LINE_HEIGHT: f32 = 1.2;
pub const TIMES_SPAN_W: f32 = 187.0;
pub const TIMES_PROGRESS_W: f32 = 64.0;
pub const TIMES_LEFT_W: f32 = 100.0;
pub const DETAILS_GAP: f32 = 6.0;
pub const TIMES_GAP: f32 = 12.0;
/// The most lines each part of the times row takes: the app ends a longer
/// part with an ellipsis (`header.rs`, `number_of_lines(2)`).
pub const MAX_TIMES_LINES: u32 = 2;
/// A compact window at least this many times as tall as it is wide stacks
/// the header ([`GuideHeader::stacked`]): a phone held upright (about 2.2)
/// and a portrait tablet's narrow Split View window, not a small desktop
/// window (400 × 600 is 1.5), which keeps its rows for the guide.
pub const STACK_RATIO: f32 = 1.6;
/// The gap between the stacked preview and the program under it (points
/// at scale 1).
pub const STACK_GAP: f32 = 12.0;

/// Where the guide's header puts its shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcuts {
    /// A column at the right, beside the preview and the focused program.
    Beside,
    /// A row of their own under the preview and the focused program, so
    /// the program keeps the width beside the preview.
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
    /// The gap between the preview and the focused program under it, in
    /// a stacked header ([`GuideHeader::stacked`]); 0 when the program is
    /// beside the preview.
    pub stack_gap: f32,
}

impl GuideHeader {
    /// The header of a window `width` × `height` whose guide is drawn at
    /// `scale` with the viewer's text size `font` (1 by default).
    pub fn of(width: f32, height: f32, scale: f32, font: f32) -> Self {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let font = if font > 0.0 { font } else { 1.0 };
        let text = scale * font;
        if Class::of(width, text) == Class::Compact
            && height >= STACK_RATIO * width
            && let Some(stacked) = stack(width, height, scale, font)
        {
            return stacked;
        }
        // The program's width if the shortcuts stayed beside it, with the
        // preview as that arrangement draws it.
        let (_, beside) = fit(width, height, 0.0, 0.0, HEADER_MAX_SHARE);
        let needed = (SHORTCUTS_W + DETAILS_MIN_W) * text;
        let room = |preview: f32| width - 2.0 * PREVIEW_MARGIN - preview - 2.0 * COLUMN_GAP * scale;
        let mut shortcuts = if Class::of(width, text) == Class::Compact || room(beside) < needed {
            Shortcuts::Below
        } else {
            Shortcuts::Beside
        };
        let mut arranged = arrange(width, height, scale, font, shortcuts);
        // A header grown for the program widens its preview: beside, the
        // columns must still have their room next to the preview it draws.
        if shortcuts == Shortcuts::Beside && room((arranged.0).1) < needed {
            shortcuts = Shortcuts::Below;
            arranged = arrange(width, height, scale, font, shortcuts);
        }
        let ((height, w), (row, row_gap)) = arranged;
        Self {
            height,
            preview: (w, (w * 9.0 / 16.0).round()),
            shortcuts,
            row,
            row_gap,
            stack_gap: 0.0,
        }
    }

    /// Whether the focused program sits under the preview, which spans
    /// the window's width, rather than beside it.
    pub fn stacked(&self) -> bool {
        self.stack_gap > 0.0
    }
}

/// The stacked header of a compact portrait window: the preview as wide as
/// the window inside its margins, the program's title and times under it,
/// then the shortcuts' row. `None` where that header would pass the
/// header's largest share of the height (very large text, or a window
/// barely taller than the ratio): the window keeps the other arrangement.
/// A header shorter than its usual share grows to it, which gives the
/// program's description the room.
fn stack(width: f32, height: f32, scale: f32, font: f32) -> Option<GuideHeader> {
    let text = scale * font;
    let w = (width - 2.0 * PREVIEW_MARGIN).floor();
    let h = (w * 9.0 / 16.0).round();
    if h < PREVIEW_MIN_H {
        return None;
    }
    let row = (SHORTCUTS_ROW * scale * font.max(1.0)).round();
    let row_gap = (SHORTCUTS_GAP * scale).round();
    let stack_gap = (STACK_GAP * scale).round().max(1.0);
    let details = details_height(w, scale, text).ceil();
    let wanted = 2.0 * PREVIEW_MARGIN + h + stack_gap + details + row_gap + row;
    let most = if text >= LARGE_TEXT {
        LARGE_TEXT_MAX_SHARE
    } else {
        HEADER_MAX_SHARE
    };
    (wanted <= (height * most).floor()).then(|| GuideHeader {
        height: wanted.max((height * HEADER_SHARE).round()),
        preview: (w, h),
        shortcuts: Shortcuts::Below,
        row,
        row_gap,
        stack_gap,
    })
}

/// The header's height and preview width, and the shortcuts' row and gap,
/// for the arrangement `shortcuts`.
fn arrange(
    width: f32,
    height: f32,
    scale: f32,
    font: f32,
    shortcuts: Shortcuts,
) -> ((f32, f32), (f32, f32)) {
    let text = scale * font;
    let (row, row_gap) = match shortcuts {
        Shortcuts::Beside => (0.0, 0.0),
        Shortcuts::Below => (
            (SHORTCUTS_ROW * scale * font.max(1.0)).round(),
            (SHORTCUTS_GAP * scale).round(),
        ),
    };
    // The program's column, with the preview at its widest share (so
    // never narrower than drawn), and the room its title and times take.
    let side = match shortcuts {
        Shortcuts::Beside => SHORTCUTS_W * text + COLUMN_GAP * scale,
        Shortcuts::Below => 0.0,
    };
    let column =
        width - 2.0 * PREVIEW_MARGIN - (width * PREVIEW_SHARE).round() - COLUMN_GAP * scale - side;
    let details = details_height(column, scale, text);
    let most = if text >= LARGE_TEXT {
        LARGE_TEXT_MAX_SHARE
    } else {
        HEADER_MAX_SHARE
    };
    (
        fit(width, height, row + row_gap, details, most),
        (row, row_gap),
    )
}

/// The height (points) the focused program's title and times take in a
/// column `column` points wide at `scale` and text factor `text`: the title
/// on one line, then the times row, whose parts wrap onto further lines
/// where the column is narrower than they are (the span over at most two
/// lines of its own).
pub fn details_height(column: f32, scale: f32, text: f32) -> f32 {
    let column = column.max(1.0);
    let line = |pt: f32| (pt * text * LINE_HEIGHT).ceil();
    let times_line = line(HEADER_TIMES);
    let gap = TIMES_GAP * scale;
    // Lay the parts out greedily, as the flex row wraps them.
    let parts = [
        (TIMES_SPAN_W * text, times_line),
        (TIMES_PROGRESS_W * scale, times_line),
        (TIMES_LEFT_W * text, times_line),
    ];
    let mut rows: Vec<f32> = Vec::new();
    let mut used = f32::INFINITY;
    for (w, h) in parts {
        // The app draws each part on at most two lines (`number_of_lines`),
        // ending a longer one with an ellipsis: two is the most it takes.
        let lines = (w / column).ceil().clamp(1.0, MAX_TIMES_LINES as f32);
        let w = w.min(column);
        let h = h * lines;
        // A part that does not fit the row's rest wraps, even by a hair.
        if used + gap + w <= column {
            used += gap + w;
            let last = rows.last_mut().expect("a row is open");
            *last = last.max(h);
        } else {
            used = w;
            rows.push(h);
        }
    }
    let times: f32 = rows.iter().sum::<f32>() + gap * (rows.len().saturating_sub(1)) as f32;
    line(HEADER_TITLE) + DETAILS_GAP * scale + times
}

/// The header's height and the preview's width in a window `width` ×
/// `height` with `below` points under the preview and `details` points
/// wanted by the program beside it. A short window grows the header
/// rather than lose the preview or clip the program, up to `most` of its
/// height ([`HEADER_MAX_SHARE`], more at the largest text: the rail and
/// the guide keep the rest); a window too short even for that squeezes
/// them instead.
fn fit(width: f32, height: f32, below: f32, details: f32, most: f32) -> (f32, f32) {
    let share = (height * HEADER_SHARE).round();
    let wanted = 2.0 * PREVIEW_MARGIN + PREVIEW_MIN_H.max(details.ceil()) + below;
    let header = share.max(wanted.min((height * most).floor()));
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
    fn the_shortcuts_go_below_where_the_program_has_no_room() {
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
    fn the_header_keeps_the_programs_title_and_times_whole() {
        // An iPad's Split View and Stage Manager windows at the largest
        // standard text and at Accessibility Extra Large (AX3).
        for (w, h, font) in [
            (678.0, 820.0, 40.0 / 17.0),
            (700.0, 560.0, 40.0 / 17.0),
            (375.0, 820.0, 23.0 / 17.0),
            (1180.0, 820.0, 40.0 / 17.0),
        ] {
            let g = GuideHeader::of(w, h, 1.0, font);
            let side = if g.shortcuts == Shortcuts::Beside {
                SHORTCUTS_W * font + COLUMN_GAP
            } else {
                0.0
            };
            let (column, room) = if g.stacked() {
                (
                    g.preview.0,
                    g.height - 2.0 * PREVIEW_MARGIN - g.preview.1 - g.stack_gap - g.row - g.row_gap,
                )
            } else {
                (
                    w - 2.0 * PREVIEW_MARGIN - g.preview.0 - COLUMN_GAP - side,
                    g.height - 2.0 * PREVIEW_MARGIN - g.row - g.row_gap,
                )
            };
            assert!(
                room + 0.5 >= details_height(column, 1.0, font),
                "{w}x{h} at {font}: {g:?}, room {room}"
            );
        }
        // At the default text the title and the times take one line each.
        assert_eq!(details_height(400.0, 1.0, 1.0), 27.0 + 6.0 + 21.0);
        // A narrow column wraps the minutes left onto a line of their own.
        assert_eq!(
            details_height(200.0, 1.0, 1.0),
            27.0 + 6.0 + 21.0 + 12.0 + 21.0
        );
        // A part exactly as wide as the row's rest stays; a hair wider wraps.
        let exact = TIMES_SPAN_W + TIMES_GAP + TIMES_PROGRESS_W + TIMES_GAP + TIMES_LEFT_W;
        assert_eq!(details_height(exact, 1.0, 1.0), 27.0 + 6.0 + 21.0);
        assert_eq!(
            details_height(exact - 0.25, 1.0, 1.0),
            27.0 + 6.0 + 21.0 + 12.0 + 21.0
        );
        // At AX3 in a 214-point column the span (440 points) would need
        // three lines; the app ellipsizes it after two, so two are budgeted,
        // then the progress on a line and the minutes left (235 points) on two.
        let ax3 = 40.0 / 17.0;
        let line = (HEADER_TIMES * ax3 * LINE_HEIGHT).ceil();
        assert_eq!(
            details_height(214.0, 1.0, ax3),
            (HEADER_TITLE * ax3 * LINE_HEIGHT).ceil()
                + 6.0
                + 2.0 * line
                + 12.0
                + line
                + 12.0
                + 2.0 * line
        );
    }

    #[test]
    fn beside_keeps_its_room_next_to_the_preview_it_draws() {
        // A short window whose header grows for the program widens its
        // preview: where the columns would then lack their room beside it,
        // the shortcuts go below.
        for (w, h, scale, font) in [
            (577.0, 240.0, 0.8, 1.0),
            (760.0, 480.0, 0.8, 1.0),
            (1280.0, 720.0, 0.875, 1.0),
            (960.0, 300.0, 0.8, 1.15),
        ] {
            let g = GuideHeader::of(w, h, scale, font);
            if g.shortcuts == Shortcuts::Beside {
                let room = w - 2.0 * PREVIEW_MARGIN - g.preview.0 - 2.0 * COLUMN_GAP * scale;
                assert!(
                    room >= (SHORTCUTS_W + DETAILS_MIN_W) * scale * font,
                    "{w}x{h}: {g:?}"
                );
            }
        }
    }

    #[test]
    fn a_phone_held_upright_stacks_a_full_width_preview() {
        // Phones at full size: the preview spans the window inside its
        // margins, the program and the shortcuts' row under it.
        for (w, h, want) in [
            (412.0, 915.0, (364.0, 388.0, 218.0)),
            (390.0, 844.0, (385.0, 366.0, 206.0)),
            (360.0, 800.0, (368.0, 336.0, 189.0)),
        ] {
            let g = GuideHeader::of(w, h, 1.0, 1.0);
            assert!(g.stacked(), "{w}x{h}: {g:?}");
            assert_eq!(g.shortcuts, Shortcuts::Below, "{w}x{h}");
            assert_eq!((g.height, g.preview.0, g.preview.1), want, "{w}x{h}");
            assert_eq!((g.row, g.row_gap, g.stack_gap), (48.0, 8.0, 12.0));
            let room =
                g.height - 2.0 * PREVIEW_MARGIN - g.preview.1 - g.stack_gap - g.row - g.row_gap;
            assert!(
                room >= details_height(g.preview.0, 1.0, 1.0),
                "{w}x{h}: {g:?}"
            );
        }
        // The largest text still stacks on a tall phone.
        let g = GuideHeader::of(412.0, 915.0, 1.0, 2.0);
        assert!(g.stacked(), "{g:?}");
        assert!(g.height <= (915.0 * LARGE_TEXT_MAX_SHARE).floor(), "{g:?}");
    }

    #[test]
    fn only_tall_compact_windows_stack() {
        // A phone on its side keeps the preview beside the program, with
        // the guide's rows under a third of the height.
        for (w, h, want) in [
            (915.0, 412.0, (136.0, 199.0, 112.0)),
            (780.0, 360.0, (119.0, 169.0, 95.0)),
        ] {
            let g = GuideHeader::of(w, h, 1.0, 1.0);
            assert!(!g.stacked(), "{w}x{h}: {g:?}");
            assert_eq!(g.shortcuts, Shortcuts::Beside, "{w}x{h}");
            assert_eq!((g.height, g.preview.0, g.preview.1), want, "{w}x{h}");
        }
        // A small desktop window (1.5 times as tall as wide), a portrait
        // tablet (medium) and a TV keep their arrangement.
        for (w, h, scale) in [
            (400.0, 600.0, 0.8),
            (800.0, 1280.0, 1.0),
            (960.0, 540.0, 0.8),
        ] {
            let g = GuideHeader::of(w, h, scale, 1.0);
            assert!(!g.stacked(), "{w}x{h}: {g:?}");
            assert_eq!(g.stack_gap, 0.0);
        }
        // A stacked header past its largest share does not stack: very
        // large text in a window just tall enough.
        let g = GuideHeader::of(360.0, 600.0, 1.0, 2.0);
        assert!(!g.stacked(), "{g:?}");
        assert!(g.preview.1 >= PREVIEW_MIN_H, "{g:?}");
    }

    #[test]
    fn no_scale_or_text_size_counts_as_one() {
        assert_eq!(
            GuideHeader::of(1280.0, 800.0, 0.0, f32::NAN),
            GuideHeader::of(1280.0, 800.0, 1.0, 1.0)
        );
    }
}
