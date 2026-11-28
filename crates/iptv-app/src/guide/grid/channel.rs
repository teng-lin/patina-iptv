//! A row's channel: its place in the list (right-aligned), its logo in a
//! box, and its name, cut with two dots where it is too long. The playing
//! channel's number, name and ▶ marker take the accent.

use nsh_core::prelude::*;

use super::{CELL_H, CELL_RADIUS, fill};
use crate::kit::fonts::{CHANNEL, SANS};
use crate::kit::scale::Scale;
use crate::theme::BRIGHT_TEXT;
use crate::{Ui, channel_name};

/// The logo box, as tall as a program: missing or broken logos leave
/// it empty (sizes at full size, scaled with the grid).
const LOGO_W: f32 = 60.0;
/// Inside the logo box around the logo.
const LOGO_PAD: f32 = 4.0;
/// Before the number, between the parts, after the name.
const PAD: f32 = 8.0;
const GAP: f32 = 14.0;
/// The ▶ marker after the playing channel's name (its text is empty on
/// the other rows, but still takes a gap).
const MARKER_W: f32 = 20.0;

/// Shown where a channel has no logo (an image needs a source).
pub(crate) use crate::kit::content_image::no_logo;

pub(crate) fn show_channel_logos(app: crate::SharedUi) -> bool {
    crate::wiring::settings_wiring::read(app).bool(
        iptv_settings::keys::GUIDE_SHOW_LOGOS,
        &iptv_settings::Target::Global,
    )
}

pub(crate) fn channel_logo(app: crate::SharedUi, channel: Option<usize>) -> String {
    app.guide_data();
    channel
        .and_then(|ch| crate::wiring::runtime_owner::schedule().channel_logo(ch))
        .unwrap_or_else(no_logo)
}

/// The width the name has in a column `column` wide after a number
/// `number` wide (`numbers.rs`), at size scale `scale`.
fn name_width(column: f32, number: f32, playing: bool, scale: f32) -> f32 {
    let marker = if playing { MARKER_W } else { 0.0 };
    let logo = (LOGO_W * scale).round();
    column - number - logo - scale * (2.0 * PAD + 3.0 * GAP + marker)
}

pub(super) fn cell(
    ui: Ui,
    at: Position,
    channel: Memo<Option<usize>>,
    (column, size): (fn() -> f32, Scale),
    number_w: Memo<f32>,
    editing: Memo<Option<bool>>,
) -> impl IntoElement {
    let playing = move || channel.get().is_some() && ui.playback.playing.get() == channel.get();
    let color = move || {
        if editing.get() == Some(false) {
            rgb(crate::theme::FOCUSED_TEXT)
        } else if playing() {
            rgb(ui.app.theme.with(|t| t.guide().playing))
        } else {
            rgb(BRIGHT_TEXT)
        }
    };
    let show_logo = move || show_channel_logos(ui.app);
    let logo = move || channel_logo(ui.app, channel.get());
    let is_selected = move || {
        if crate::library_ui::organization_view::guide_order_open(ui.screens) {
            editing.get() == Some(true)
        } else {
            ui.guide.focus.with(|f| f.channel == at.index())
        }
    };
    let grips = move || {
        crate::library_ui::organization_view::guide_order_open(ui.screens) && ui.drag_editing()
    };
    view()
        .accessible(true)
        .label(move || {
            ui.guide_data();
            channel.get().map(channel_name).unwrap_or_default()
        })
        .on_long_press(move || {
            if crate::library_ui::organization_view::guide_order_open(ui.screens) {
                return;
            }
            if let Some(ch) = channel.get() {
                crate::wiring::navigation::open_menu_at(ui, ch);
            }
        })
        .on_context_menu(move |_: &nsh_core::ContextMenuEvent| {
            // Every desktop: the menu itself leaves out what a platform
            // lacks (Open in new window is Linux and macOS only).
            if !crate::library_ui::organization_view::guide_order_open(ui.screens)
                && let Some(ch) = channel.get()
            {
                crate::wiring::navigation::open_menu_at(ui, ch);
            }
        })
        .role(move || {
            if crate::library_ui::organization_view::guide_order_open(ui.screens) {
                AccessibilityRole::Row
            } else {
                AccessibilityRole::Button
            }
        })
        .accessibility_state(move || AccessibilityState {
            selected: is_selected(),
            ..Default::default()
        })
        .rounded(px(CELL_RADIUS))
        .bg(move || {
            if editing.get() == Some(false) {
                rgb(crate::theme::FOCUSED_CELL)
            } else {
                crate::tint(0, 0)
            }
        })
        .w(move || px(column()))
        .px(move || size.px(PAD))
        .flex_row()
        .items_center()
        .gap(move || size.px(GAP))
        .on_press(move || {
            ui.touched();
            if crate::library_ui::organization_view::guide_order_open(ui.screens)
                || crate::guide::grid::closed_columns(ui)
            {
                return;
            }
            let view = crate::guide::lists::view(ui.guide);
            ui.guide.focus.update(|f| {
                let now = f.now;
                f.tap(at.index(), now, &view)
            });
            ui.watch();
        })
        .child(
            // The place in the list on screen.
            text(move || {
                crate::wiring::settings_wiring::read(ui.app);
                if crate::wiring::settings::flag(iptv_settings::keys::GUIDE_SHOW_CHANNEL_NUMBERS) {
                    (at.index() + 1).to_string()
                } else {
                    String::new()
                }
            })
            .test_id("guide-channel-number")
            .w(move || px(number_w.get()))
            .number_of_lines(1)
            .text_right()
            .font_family(SANS)
            .text_size(move || size.text_px(CHANNEL))
            .text_color(color),
        )
        .child(
            view()
                .test_id("guide-channel-logo")
                .w(move || px(if show_logo() { size.whole(LOGO_W) } else { 0.0 }))
                .h(move || px(size.whole(CELL_H)))
                .p(move || size.px(if show_logo() { LOGO_PAD } else { 0.0 }))
                .rounded(move || size.px(CELL_RADIUS))
                .bg(move || fill(ui, ui.app.theme.with(|t| t.guide().cell)))
                .items_center()
                .justify_center()
                .child(
                    image(crate::kit::content_image::source(logo))
                        .w(move || px(size.whole(LOGO_W) - 2.0 * size.of(LOGO_PAD)))
                        .h(move || px(size.whole(CELL_H) - 2.0 * size.of(LOGO_PAD)))
                        .object_fit(ResizeMode::Contain)
                        .opacity(move || if show_logo() { 1.0_f32 } else { 0.0_f32 }),
                ),
        )
        .child(
            text(move || {
                // Cells are made once per recycled slot: the index
                // alone does not change when the data does.
                ui.guide_data();
                let Some(ch) = channel.get() else {
                    return String::new();
                };
                let star = ui.app.favorites.with(|f| f.contains(&ch));
                let name = format!("{}{}", if star { "★ " } else { "" }, channel_name(ch));
                let scale = size.get();
                crate::kit::fonts::fit(
                    &name,
                    name_width(column(), number_w.get(), playing(), scale)
                        + if show_logo() { 0.0 } else { size.whole(LOGO_W) }
                        - if grips() {
                            size.whole(
                                crate::library_ui::organization_view::reorder_drag::GRIP_COLUMN,
                            )
                        } else {
                            0.0
                        },
                    CHANNEL * scale * crate::wiring::settings::font_factor(),
                )
            })
            .test_id("guide-channel-name")
            .flex_shrink(1.0)
            .number_of_lines(1)
            .font_family(SANS)
            .text_size(move || size.text_px(CHANNEL))
            .text_color(color),
        )
        .child(
            // The playing marker, after the name.
            crate::kit::list_rows::triangle(size, move || if playing() { "▶" } else { "" })
                .test_id("guide-playing-marker")
                .text_color(move || rgb(ui.app.theme.with(|t| t.guide().playing))),
        )
        .child(when(grips, move || {
            view()
                .test_id("guide-reorder-handle")
                .absolute()
                .right(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(
                    move || {
                        size.px(crate::library_ui::organization_view::reorder_drag::GRIP_COLUMN)
                    },
                )
                .items_center()
                .justify_center()
                .cursor(Cursor::Grab)
                .child(crate::library_ui::organization_view::reorder_drag::grip(
                    size,
                ))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_have_the_room_the_column_leaves() {
        // A 1280 dp window: 36 % is 461 dp; three digits take about 42.
        assert_eq!(
            name_width(461.0, 42.0, false, 1.0),
            461.0 - 16.0 - 42.0 - 60.0 - 3.0 * 14.0
        );
        assert_eq!(
            name_width(461.0, 42.0, true, 1.0),
            name_width(461.0, 42.0, false, 1.0) - MARKER_W
        );
        // Scaled: the logo in whole px, the gaps with it.
        assert_eq!(
            name_width(461.0, 42.0, false, 0.875),
            461.0 - 42.0 - 53.0 - 0.875 * (16.0 + 3.0 * 14.0)
        );
    }
}
