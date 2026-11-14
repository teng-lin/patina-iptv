//! The secondary screens' building blocks (search, settings and its
//! editors, the add-playlist wizard and its text sheet, the playlists and
//! guide sources screens, the guide-data chooser, the choosers over
//! video): screen titles, section headings, body and caption lines,
//! buttons, tabs, action rows, text fields and cards, all in the lists'
//! type (`fonts.rs`), pitch and focus pill (`list_rows.rs`), scaled with
//! the desktop window (`scale.rs`). Sizes live here and in `fonts.rs`
//! only: `tests/app/type_drift.rs` fails on a literal text size elsewhere.

use crate::i18n::tr;

use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;

use crate::fonts::{CAPTION, LIST_ITEM, LIST_SECONDARY, MEDIUM, SANS, SECTION, TITLE};
use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;
use crate::theme::{FOCUS_ALPHA, FOCUSED_TEXT, LIST_TEXT, RADIUS, RADIUS_LARGE, RESTING_ALPHA};
use crate::{Ui, tint};

/// A round ‹ for pointer and touch navigation: out of the
/// full-screen player, from the guide to its groups, from the groups to
/// the rail. `side` across on `bg`, its press reaching `slop` around it,
/// named `label` (VoiceOver, tests).
pub fn round_back(
    side: f32,
    slop: f32,
    bg: impl Fn() -> Rgba + 'static,
    label: impl Fn() -> &'static str + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let chevron = ImageSrc::svg_markup(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' width='24' height='24' \
         fill='none' stroke='currentColor' stroke-width='2.2' stroke-linecap='round' \
         stroke-linejoin='round'><path d='M15 4.5L7.5 12l7.5 7.5'/></svg>",
    );
    view()
        .role(AccessibilityRole::Button)
        .size(px(side))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(side / 2.0))
        .bg(bg)
        .hit_slop(EdgeInsets::all(px(slop)))
        .label(move || label().to_owned())
        .on_press(on_press)
        .child(
            svg(chevron)
                .size(px((side * 0.55).round()))
                .text_color(rgb(WHITE)),
        )
}

/// A button's or a tab's label: a list heading's size in Roboto Medium.
pub const BUTTON: f32 = SECTION;
/// The least height of anything tapped (dp at full size): buttons, tabs,
/// fields; rows keep theirs through their pitch (`list_rows.rs`).
pub const TOUCH: f32 = 48.0;
/// A heading's row in a list (settings, the key list).
pub const HEADING_ROW: f32 = 40.0;
/// A row with three lines (a playlist, a guide source): name, summary,
/// status.
pub const ROW_THREE: f32 = 80.0;
/// Problems: a light red.
pub use crate::theme::{ERROR, ERROR_FOCUSED};
/// A screen's margins (dp at full size).
pub const MARGIN_X: f32 = 48.0;
pub const MARGIN_Y: f32 = 24.0;

const WHITE: u32 = 0xFFFFFF;
/// A button's fill at rest.
const BUTTON_ALPHA: u8 = RESTING_ALPHA * 2;

/// Secondary text's colour (descriptions, hints, headings).
pub fn secondary() -> Rgba {
    Mark::Plain.secondary()
}

fn styled(size: Scale, content: impl IntoReactive<SharedString>, sp: f32) -> Text {
    text(content)
        .font_family(SANS)
        .text_size(move || size.text_px(sp))
}

/// A screen's, sheet's or dialog's title: the header's title size.
pub fn title(size: Scale, content: impl IntoReactive<SharedString>) -> Text {
    styled(size, content, TITLE)
        .number_of_lines(1)
        .text_color(rgb(LIST_TEXT))
}

/// A section's heading ("Recent searches", a settings section): the
/// lists' heading size, in the secondary colour.
pub fn heading(size: Scale, content: impl IntoReactive<SharedString>) -> Text {
    styled(size, content, SECTION).text_color(secondary())
}

/// A paragraph (what a dialog does, a description): an item's second
/// line's size, wrapping.
pub fn body(size: Scale, content: impl IntoReactive<SharedString>) -> Text {
    styled(size, content, LIST_SECONDARY).text_color(secondary())
}

/// Hints, status lines and badges.
pub fn caption(size: Scale, content: impl IntoReactive<SharedString>) -> Text {
    styled(size, content, CAPTION).text_color(secondary())
}

/// An item's label (a setting, an option, a field's value): one line at
/// the list item size.
pub fn label(size: Scale, content: impl IntoReactive<SharedString>) -> Text {
    list_rows::name(size, content).text_color(rgb(LIST_TEXT))
}

/// An item's second line (a value, a summary): one line.
pub fn detail(size: Scale, content: impl IntoReactive<SharedString>) -> Text {
    list_rows::second(size, content, SANS).text_color(secondary())
}

/// A button: the light pill while focused with the keys shown, a subtle
/// fill otherwise; at least [`TOUCH`] tall.
pub fn button(
    ui: Ui,
    size: Scale,
    label: impl IntoReactive<SharedString>,
    focused: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let mark = move || Mark::of(focused() && ui.keys_shown(), false);
    pill(
        ui,
        size,
        label,
        move || match mark() {
            Mark::Focused => Mark::Focused.bg(),
            _ => tint(WHITE, BUTTON_ALPHA),
        },
        move || mark().text(),
        on_press,
    )
}

/// A button inside a list row (a form field's Paste): on the row's light
/// pill while it is focused, the pill's dark text on a dark tint, so it
/// still reads there; otherwise as [`button`].
pub fn row_button(
    ui: Ui,
    size: Scale,
    label: impl IntoReactive<SharedString>,
    row: impl Fn() -> Mark + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    pill(
        ui,
        size,
        label,
        move || match row() {
            Mark::Focused => tint(FOCUSED_TEXT, BUTTON_ALPHA),
            _ => tint(WHITE, BUTTON_ALPHA),
        },
        move || row().text(),
        on_press,
    )
}

fn pill(
    ui: Ui,
    size: Scale,
    label: impl IntoReactive<SharedString>,
    bg: impl Fn() -> Rgba + 'static,
    text: impl Fn() -> Rgba + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    view()
        .role(AccessibilityRole::Button)
        .min_h(move || px(size.whole(TOUCH)))
        .px(move || size.px(20.0))
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .bg(bg)
        .on_press(move || {
            ui.touched();
            on_press();
        })
        .child(
            styled(size, label, BUTTON)
                .number_of_lines(1)
                .font_weight(MEDIUM)
                .text_color(text),
        )
}

/// A tab or chip (search's Channels and Programmes): the selected one in
/// the accent's light tone on a subtle fill, the focused one the pill.
pub fn tab(
    ui: Ui,
    size: Scale,
    label: impl IntoReactive<SharedString>,
    selected: impl Fn() -> bool + Copy + 'static,
    focused: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let pill = move || focused() && ui.keys_shown();
    view()
        .role(AccessibilityRole::Tab)
        .accessibility_state(move || AccessibilityState {
            selected: selected(),
            ..Default::default()
        })
        .min_h(move || px(size.whole(TOUCH)))
        .px(move || size.px(18.0))
        .justify_center()
        .rounded(px(RADIUS))
        .bg(move || {
            if pill() {
                Mark::Focused.bg()
            } else if selected() {
                Mark::Current.bg()
            } else {
                tint(WHITE, 0)
            }
        })
        .on_press(move || {
            ui.touched();
            on_press();
        })
        .child(
            styled(size, label, BUTTON)
                .number_of_lines(1)
                .font_weight(MEDIUM)
                .text_color(move || {
                    if pill() {
                        Mark::Focused.text()
                    } else if selected() {
                        rgb(ui.theme.with(|t| t.accent.light))
                    } else {
                        secondary()
                    }
                }),
        )
}

/// An action in a sheet's list (a playlist's actions, Keep and Delete):
/// a list row whose name is `color` (an error tone for what deletes).
pub fn action(
    ui: Ui,
    size: Scale,
    label: impl IntoReactive<SharedString>,
    focused: impl Fn() -> bool + Copy + 'static,
    color: u32,
    on_press: impl Fn() + 'static,
) -> View {
    let mark = move || Mark::of(focused() && ui.keys_shown(), false);
    list_rows::row(size, ROW, mark)
        .role(AccessibilityRole::Button)
        .on_press(move || {
            ui.touched();
            on_press();
        })
        .child(
            list_rows::name(size, label).text_color(move || match mark() {
                Mark::Focused => Mark::Focused.text(),
                _ => rgb(if color == WHITE { LIST_TEXT } else { color }),
            }),
        )
}

/// A field's padding above and below its line: with the item size's line
/// (about 1.2 × 20 sp) the field is [`TOUCH`] tall or more.
pub const FIELD_PAD_Y: f32 = 13.0;

/// A text field's type and box: the list item size, its padding making
/// it a touch target (no fixed height: the type sets it). The caller adds
/// its fill ([`field_fill`], or a sheet's border).
pub fn field(size: Scale, input: Input) -> Input {
    input
        .font_family(SANS)
        .text_size(move || size.text_px(LIST_ITEM))
        .text_color(rgb(LIST_TEXT))
        .placeholder_color(secondary())
        .px(move || size.px(14.0))
        .py(move || size.px(FIELD_PAD_Y))
        .rounded(px(RADIUS))
}

/// A field's light outline makes keyboard focus distinct from resting fills.
pub fn field_border(ui: Ui, focused: bool) -> Rgba {
    tint(
        ui.theme.with(|t| t.accent.light),
        if focused && ui.keys_shown() { 255 } else { 0 },
    )
}

/// Opaque dark accent for focused input, subtle resting fill otherwise.
pub fn field_fill(ui: Ui, focused: bool) -> Rgba {
    if focused && ui.keys_shown() {
        tint(ui.theme.with(|t| t.accent.main), FOCUS_ALPHA)
    } else {
        tint(WHITE, RESTING_ALPHA)
    }
}

/// A dialog's or sheet's card, `width` dp wide at full size.
pub fn card(ui: Ui, size: Scale, width: f32) -> View {
    view()
        .w(move || size.px(width))
        .max_w(percentage(0.95))
        .px(move || size.px(28.0))
        .py(move || size.px(24.0))
        .gap(move || size.px(8.0))
        .rounded(px(RADIUS_LARGE))
        .bg(move || rgb(ui.theme.with(|t| t.background.panel)))
}

/// A full-screen screen (search, playlists, guide sources): the window's
/// colour, the safe area, the margins.
pub fn screen(ui: Ui, size: Scale) -> View {
    view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .px(move || size.px(MARGIN_X))
        .py(move || size.px(MARGIN_Y))
        .gap(move || size.px(12.0))
        .bg(move || rgb(ui.theme.with(|t| t.background.window)))
}

/// A screen's top line: its title, then a Close button.
pub fn header(
    ui: Ui,
    size: Scale,
    content: impl IntoReactive<SharedString>,
    close: fn(Ui),
) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(move || size.px(12.0))
        .child(title(size, content).flex_1())
        .child(button(
            ui,
            size,
            move || tr("Close"),
            || false,
            move || close(ui),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::{HEADER_BODY, HEADER_TIMES, HEADER_TITLE};

    #[test]
    fn the_kit_uses_the_lists_scale() {
        assert_eq!(TITLE, HEADER_TITLE);
        assert_eq!(BUTTON, HEADER_TIMES);
        assert_eq!(LIST_SECONDARY, HEADER_BODY);
        const {
            assert!(TOUCH >= 48.0 && HEADING_ROW < ROW && ROW_THREE > list_rows::ROW_TWO);
        };
        // A field: Roboto's line (≥ 1.17 × its size) and the padding.
        const {
            assert!(1.17 * LIST_ITEM + 2.0 * FIELD_PAD_Y >= TOUCH);
        };
    }
}
