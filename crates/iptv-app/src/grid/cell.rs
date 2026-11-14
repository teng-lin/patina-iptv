//! One programme cell of a guide row: a rounded box at its minutes in the
//! row's band with its title, coloured by the focus.
//!
//! Kept cheap for the keys, which move the focus many times a second on a
//! TV box (`tests/app/guide_scroll_cost.rs`):
//!
//! - a cell reads its row's focus only ([`RowFocus`]): a move re-runs the
//!   cells of the two rows it leaves and enters;
//! - a slot without a programme is invisible, so the platform does not
//!   draw it (a row has [`super::MAX_CELLS`] slots, most rows use a few);
//! - a past programme the row cannot replay is dimmed by colours mixed
//!   with the window's, as the dimmed group would show on it, rather than
//!   by the view's opacity, which Android draws through an offscreen layer
//!   per cell (over full-screen video, where the guide is translucent,
//!   the opacity stays).

use nsh_core::prelude::*;

use super::{CELL_GAP, CELL_H, CELL_INSET, CELL_RADIUS, PAST_DIM, ROW_H, WHITE, pan::Pan};
use crate::fonts::{CELL_TITLE, CONDENSED};
use crate::guide::Program;
use crate::scale::Scale;
use crate::theme::{
    BRIGHT_TEXT, FOCUSED_CELL, FOCUSED_TEXT, GuideColors, TEXT_SECONDARY_ALPHA, blend, with_alpha,
};
use crate::{Ui, ViewMode};

/// What a row's cells read of the focus: the focused programme's start
/// when the row has it (`None` otherwise), and whether the row has it.
#[derive(Clone, Copy)]
pub(super) struct RowFocus {
    pub start: Memo<Option<i32>>,
    pub lit: Memo<bool>,
    pub selected: Memo<Option<i32>>,
}

/// How a cell looks, from the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Look {
    /// The focused programme: a light pill, dark title.
    Focused,
    /// Another programme of the focused row: a lighter cell, white title.
    Lit,
    /// Any other row's: the cell colour, grey title.
    Rest,
}

/// A cell's background and title colours (`0xRRGGBBAA`) and opacity, for
/// `look`, dimmed or not, on the guide's colours `g` (the window colour
/// `window` under them), opaque or over video (`overlay`, the cells' alpha
/// then).
pub(super) fn colours(
    look: Look,
    dim: bool,
    g: &GuideColors,
    window: u32,
    overlay: Option<u8>,
) -> (u32, u32, f32) {
    let (cell, title) = match look {
        Look::Focused => (FOCUSED_CELL, with_alpha(FOCUSED_TEXT, 0xFF)),
        Look::Lit => (g.row_cell, with_alpha(BRIGHT_TEXT, 0xFF)),
        Look::Rest => (g.cell, with_alpha(WHITE, TEXT_SECONDARY_ALPHA)),
    };
    if overlay.is_some() && matches!(look, Look::Focused) {
        return (
            with_alpha(FOCUSED_CELL, 255),
            with_alpha(FOCUSED_TEXT, 255),
            1.0,
        );
    }
    match overlay {
        Some(alpha) => (
            with_alpha(cell, alpha),
            title,
            if dim { PAST_DIM } else { 1.0 },
        ),
        None if dim => {
            // The cell, and its title drawn on it, at PAST_DIM over the
            // window: what the dimmed group shows there.
            let a = (PAST_DIM * 255.0).round() as u8;
            let text = blend(cell, title >> 8, title as u8);
            (
                with_alpha(blend(window, cell, a), 0xFF),
                with_alpha(blend(window, text, a), 0xFF),
                1.0,
            )
        }
        None => (with_alpha(cell, 0xFF), title, 1.0),
    }
}

pub(super) fn cell(
    ui: Ui,
    (at, i): (Position, usize),
    (programs, channel): (Memo<Vec<Program>>, Memo<Option<usize>>),
    focus: RowFocus,
    now: Memo<i32>,
    (pan, size): (Pan, Scale),
) -> impl IntoElement {
    let program = move || programs.with(|ps| ps.get(i).copied());
    let empty = move || program().is_none() || channel.get().is_none();
    let geometry = move || {
        program().map_or((0.0, 0.0), |p: Program| {
            // At rest a cell starts at the window's edge, so its title
            // shows; its place is in the band, which slides.
            let base = pan.band().base;
            let (start, end) = (p.start.max(pan.clip()), p.end.min(base + pan.minutes()));
            let scale = pan.scale();
            (
                (start - base) as f32 * scale,
                (end - start).max(0) as f32 * scale,
            )
        })
    };
    // A memo each: the colours below re-run when one changes, not on every
    // change of what they are made from.
    let look = computed(move || match program() {
        Some(p) if focus.start.get() == Some(p.start) && ui.keys_shown() => Look::Focused,
        _ if focus.lit.get() => Look::Lit,
        _ => Look::Rest,
    });
    let dim = computed(move || match (program(), channel.get()) {
        (Some(p), Some(ch)) => {
            let now = now.get();
            p.end <= now && !crate::in_archive(ch, p, now)
        }
        _ => false,
    });
    let colours = computed(move || {
        let overlay = match ui.mode.get() {
            ViewMode::Overlay => Some(crate::settings::overlay_alpha(&crate::settings::read(ui))),
            ViewMode::Guide | ViewMode::Fullscreen => None,
        };
        let (g, window) = ui.theme.with(|t| (t.guide(), t.background.window));
        colours(look.get(), dim.get(), &g, window, overlay)
    });
    let title_text = move || {
        // Equal programme minutes on new data still re-read the title.
        ui.guide_data();
        match (channel.get(), program()) {
            (Some(ch), Some(p)) => crate::schedule().title(ch, p),
            _ => String::new(),
        }
    };
    let is_selected = move || match program() {
        Some(p) => focus.selected.get() == Some(p.start),
        None => false,
    };
    view()
        .test_id("guide-programme")
        .accessible(true)
        .role(AccessibilityRole::Cell)
        .accessibility_state(move || AccessibilityState {
            selected: is_selected(),
            ..Default::default()
        })
        .label(title_text)
        .absolute()
        .top(move || px(((size.whole(ROW_H) - size.whole(CELL_H)) / 2.0).floor()))
        .h(move || px(size.whole(CELL_H)))
        .left(move || px(geometry().0 + size.of(CELL_GAP) / 2.0))
        .w(move || px((geometry().1 - size.of(CELL_GAP)).max(0.0)))
        .opacity(move || if empty() { 0.0 } else { colours.get().2 })
        .style_when(empty, |s| s.invisible())
        .rounded(move || size.px(CELL_RADIUS))
        .px(move || size.px(CELL_INSET))
        .justify_center()
        .overflow_hidden()
        .bg(move || rgba(colours.get().0))
        .on_press(move || {
            let Some(p) = program() else { return };
            ui.touched();
            if super::closed_columns(ui) {
                return;
            }
            let view = crate::lists::view(ui);
            ui.focus.update(|f| f.tap(at.index(), p.start, &view));
            ui.watch();
        })
        .child(
            text(title_text)
                .number_of_lines(1)
                .text_overflow(nsh_core::EllipsizeMode::Tail)
                .font_family(CONDENSED)
                .text_size(move || size.text_px(CELL_TITLE))
                .text_color(move || rgba(colours.get().1)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    /// `over` (`0xRRGGBBAA`) drawn on opaque `under` (`0xRRGGBB`).
    fn over(under: u32, over: u32) -> u32 {
        blend(under, over >> 8, over as u8)
    }

    #[test]
    fn focused_cell_stays_opaque_over_video() {
        let g = Theme::default().guide();
        for alpha in [51, 128, 204, 255] {
            assert_eq!(
                colours(Look::Focused, false, &g, g.window, Some(alpha)),
                (
                    with_alpha(FOCUSED_CELL, 255),
                    with_alpha(FOCUSED_TEXT, 255),
                    1.0
                )
            );
        }
    }

    #[test]
    fn a_dimmed_cell_shows_what_the_dimmed_group_showed_on_the_window() {
        let t = Theme::default();
        let (g, window) = (t.guide(), t.background.window);
        let a = (PAST_DIM * 255.0).round() as u8;
        for look in [Look::Focused, Look::Lit, Look::Rest] {
            let (cell, title, opacity) = colours(look, false, &g, window, None);
            let (dim_cell, dim_title, dim_opacity) = colours(look, true, &g, window, None);
            assert_eq!((opacity, dim_opacity), (1.0, 1.0), "{look:?}: no layer");
            // The group at PAST_DIM over the window: the cell, and the
            // title drawn on the cell.
            let group_cell = blend(window, cell >> 8, a);
            let group_title = blend(window, over(cell >> 8, title), a);
            assert_eq!(dim_cell, with_alpha(group_cell, 0xFF), "{look:?}");
            assert_eq!(dim_title, with_alpha(group_title, 0xFF), "{look:?}");
        }
        // Undimmed: the colours as they were.
        let rest = colours(Look::Rest, false, &g, window, None);
        assert_eq!(rest.0, with_alpha(g.cell, 0xFF));
        assert_eq!(rest.1, with_alpha(WHITE, TEXT_SECONDARY_ALPHA));
        let focused = colours(Look::Focused, false, &g, window, None);
        assert_eq!(focused.1, with_alpha(FOCUSED_TEXT, 0xFF));
    }

    #[test]
    fn over_video_a_dimmed_cell_keeps_its_opacity() {
        let t = Theme::default();
        let (g, window) = (t.guide(), t.background.window);
        let (cell, title, opacity) = colours(Look::Rest, true, &g, window, Some(0xC0));
        assert_eq!(cell, with_alpha(g.cell, 0xC0));
        assert_eq!(title, with_alpha(WHITE, TEXT_SECONDARY_ALPHA));
        assert_eq!(opacity, PAST_DIM);
    }
}
