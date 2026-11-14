//! The lists' rows (the groups columns, the player's channel list, search,
//! the favourites editor, the menu) in the guide's look: its type
//! (`fonts.rs`: Roboto for names, Roboto Condensed for programme titles),
//! its row pitch, both scaled with it on the desktop (`scale.rs`), and its
//! selection: while the keys are shown, the focused row is the guide's
//! light pill with near-black text. The list shown (a group, the row the
//! focus returns to) keeps a subtle fill.

use nsh_core::prelude::*;

use crate::fonts::{LIST_ITEM, LIST_SECONDARY, SANS};
use crate::scale::Scale;
use crate::theme::{
    FOCUSED_CELL, FOCUSED_TEXT, LIST_TEXT, RADIUS, RESTING_ALPHA, TEXT_DISABLED_ALPHA,
    TEXT_SECONDARY_ALPHA,
};
use crate::tint;

/// Row pitch of one-line rows: the guide's (at full size; scaled in whole
/// px). Rows with a second line take [`ROW_TWO`].
pub const ROW: f32 = crate::grid::ROW_H;
pub const ROW_TWO: f32 = 54.0;
/// Above and below a row's fill within its pitch (its fill stays a 48 dp
/// touch target); the text's inset from the fill's sides.
pub const GAP: f32 = 1.0;
pub const INSET: f32 = 14.0;

const WHITE: u32 = 0xFFFFFF;

/// How a row is marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Plain,
    /// The list shown, or the row the focus returns to.
    Current,
    /// The focus, while the keys are shown.
    Focused,
}

impl Mark {
    /// `focused` wins over `current`; callers include whether the keys
    /// are shown in `focused`.
    pub fn of(focused: bool, current: bool) -> Self {
        match (focused, current) {
            (true, _) => Mark::Focused,
            (false, true) => Mark::Current,
            (false, false) => Mark::Plain,
        }
    }

    pub fn bg(self) -> Rgba {
        match self {
            Mark::Focused => rgb(FOCUSED_CELL),
            Mark::Current => tint(WHITE, RESTING_ALPHA),
            Mark::Plain => tint(WHITE, 0),
        }
    }

    /// A name's colour.
    pub fn text(self) -> Rgba {
        match self {
            Mark::Focused => rgb(FOCUSED_TEXT),
            _ => rgb(LIST_TEXT),
        }
    }

    /// A second line's colour.
    pub fn secondary(self) -> Rgba {
        match self {
            Mark::Focused => tint(FOCUSED_TEXT, TEXT_SECONDARY_ALPHA),
            _ => tint(WHITE, TEXT_SECONDARY_ALPHA),
        }
    }

    /// A disabled item's colour.
    pub fn disabled(self) -> Rgba {
        match self {
            Mark::Focused => tint(FOCUSED_TEXT, TEXT_DISABLED_ALPHA),
            _ => tint(WHITE, TEXT_DISABLED_ALPHA),
        }
    }
}

/// A list's virtual-list estimate: the pitch `pitch` at the scale now.
pub fn pitch(size: Scale, pitch: f32) -> impl Fn() -> Points + Copy {
    move || px(size.whole(pitch))
}

/// A row `pitch` apart from the next (its fill [`GAP`] in from both), its
/// content centred vertically, marked by `mark`.
pub fn row(size: Scale, pitch: f32, mark: impl Fn() -> Mark + 'static) -> View {
    row_with(size, move || pitch, mark)
}

/// [`row`] whose pitch changes (a settings row gains a second line).
pub fn row_with(
    size: Scale,
    pitch: impl Fn() -> f32 + 'static,
    mark: impl Fn() -> Mark + 'static,
) -> View {
    view()
        .role(AccessibilityRole::Row)
        .h(move || px(size.whole(pitch()) - 2.0 * size.whole(GAP)))
        .my(move || px(size.whole(GAP)))
        .px(move || size.px(INSET))
        .justify_center()
        .rounded(px(RADIUS))
        .bg(move || mark().bg())
}

/// A name in a list: one line of Roboto at the guide's channel name size.
pub fn name(size: Scale, content: impl IntoReactive<SharedString>) -> Text {
    text(content)
        .number_of_lines(1)
        .font_family(SANS)
        .text_size(move || size.text_px(LIST_ITEM))
}

/// A second line in `family` (Roboto Condensed for programme titles).
pub fn second(size: Scale, content: impl IntoReactive<SharedString>, family: &'static str) -> Text {
    text(content)
        .number_of_lines(1)
        .font_family(family)
        .text_size(move || size.text_px(LIST_SECONDARY))
}

/// Text at `sp` (scaled) in Roboto, in the secondary colour: headings,
/// hints and status lines.
pub fn small(size: Scale, content: impl IntoReactive<SharedString>, sp: f32) -> Text {
    text(content)
        .font_family(SANS)
        .text_size(move || size.text_px(sp))
        .text_color(Mark::Plain.secondary())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::{CHANNEL, HEADER_BODY};

    #[test]
    fn lists_use_the_guides_type_and_pitch() {
        assert_eq!(LIST_ITEM, CHANNEL);
        assert_eq!(LIST_SECONDARY, HEADER_BODY);
        assert_eq!(ROW, crate::grid::ROW_H);
        // Touch targets: at least 48 dp (Android keeps the full sizes).
        const {
            assert!(ROW - 2.0 * GAP >= 48.0 && ROW_TWO - 2.0 * GAP >= 48.0);
        };
    }

    #[test]
    fn the_focus_is_the_guides_light_pill() {
        assert_eq!(Mark::of(true, true), Mark::Focused);
        assert_eq!(Mark::of(false, true), Mark::Current);
        assert_eq!(Mark::of(false, false), Mark::Plain);
        assert_eq!(Mark::Focused.bg(), rgb(FOCUSED_CELL));
        assert_eq!(Mark::Focused.text(), rgb(FOCUSED_TEXT));
        assert_eq!(Mark::Plain.text(), rgb(LIST_TEXT));
    }
}
