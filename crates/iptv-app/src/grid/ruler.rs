//! The time ruler over the grid: the date of the day shown and the time
//! (light accent), the touch steps of the time window and, once it left
//! now, a Now button over the channel column; the half-hour slot times
//! (grey) over the programmes and the now line's dot, sliding with them
//! (`pan.rs`); and a thin line under it all.

use super::BAND;
use iptv_core::timeline::MARGIN;
use nsh_core::prelude::*;

use super::{NOW_W, Pan, fill};
use crate::fonts::{RULER, SANS};
use crate::guide::{PAST, STEP};
use crate::scale::Scale;
use crate::theme::{RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Ui, clock, tint};

/// The ruler's height, the line under it and the now dot's diameter (the
/// height, dot and inset at full size, scaled with the grid).
pub const RULER_H: f32 = 36.0;
const SEPARATOR_H: f32 = 1.0;
const DOT: f32 = 8.0;
/// A slot time's inset from its half hour.
const TICK_INSET: f32 = 8.0;
const WHITE: u32 = 0xFFFFFF;

pub(super) fn ruler(
    ui: Ui,
    size: Scale,
    column: fn() -> f32,
    pan: Pan,
    now_x: Memo<Option<f32>>,
    width: Signal<f32>,
) -> impl IntoElement {
    let button = move |label: &'static str| {
        text(label)
            .role(AccessibilityRole::Button)
            .px(move || size.px(10.0))
            .rounded(move || size.px(4.0))
            .bg(tint(WHITE, RESTING_ALPHA))
            .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA))
            .font_family(SANS)
            .text_size(move || size.text_px(RULER))
    };
    let shift = move |label: &'static str, minutes: i32| {
        button(label).on_press(move || {
            ui.touched();
            let view = crate::lists::view(ui);
            ui.focus.update(|f| f.shift_window(minutes, &view));
        })
    };
    // Back to now, once the view left it.
    let now_button = when(
        move || pan.away(),
        move || button("Now").on_press(move || pan.to_now()),
    );
    // Enlarged text needs room for localized 12-hour labels (including
    // the day period). Use hourly ticks rather than ellipsizing minutes.
    let tick_step = move || {
        if crate::settings::font_factor() > 1.0 {
            STEP * 2
        } else {
            STEP
        }
    };
    let first = move || {
        let step = tick_step();
        pan.band().base.div_euclid(step) * step + step
    };
    let tick = move |i: i32| {
        // The epoch comes with the data; now decides which day is "today".
        text(move || {
            ui.guide_data();
            ui.focus.with(|f| f.now);
            clock(first() + i * tick_step())
        })
        .absolute()
        .left(move || {
            let at = (first() + i * tick_step() - pan.band().base) as f32;
            px(at * pan.scale() + size.of(TICK_INSET))
        })
        // Its slot's width, not its text's: sized to its text it would be
        // fitted to the room between it and the right edge of this strip,
        // which the slide does not move, and a time near the edge came out
        // as "2" or "…" (CoreText, Pango) though it showed on screen. The
        // strip's clip cuts what passes its edge, as the cells' titles.
        .w(move || px((tick_step() as f32 * pan.scale() - size.of(TICK_INSET)).max(0.0)))
        .number_of_lines(1)
        .font_family(SANS)
        .text_size(move || size.text_px(RULER))
        .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA))
    };
    let separator = move || fill(ui, ui.theme.with(|t| t.guide().separator));
    view()
        .child(
            view()
                .h(move || px(size.whole(RULER_H)))
                .flex_row()
                .items_center()
                .child(
                    // The date and time, then the touch steps of the window
                    // (an iPad drags and flings it, Now returns: its ‹ top
                    // left is the way back).
                    view()
                        .w(move || px(column()))
                        .flex_row()
                        .items_center()
                        .gap(move || size.px(6.0))
                        .px(move || size.px(16.0))
                        .child(date_time(ui, size, pan).flex_1())
                        .child(now_button)
                        .children(
                            (!crate::IOS)
                                .then(|| [shift("‹", -STEP), shift("›", STEP)])
                                .into_iter()
                                .flatten(),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .h_full()
                        .overflow_hidden()
                        .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0))
                        .child(
                            view()
                                .absolute()
                                .inset_0()
                                .justify_center()
                                .bind_translate_x(&pan.offset())
                                .children((0..=BAND / STEP).map(tick)),
                        ),
                ),
        )
        .child(view().h(px(SEPARATOR_H)).bg(separator))
        .child(
            // The now dot and the line through the separator under it, over
            // the programmes only, sliding with them.
            view()
                .absolute()
                .bottom(px(0.0))
                .h(move || px(SEPARATOR_H + size.of(DOT)))
                .left(move || px(column()))
                .right(px(0.0))
                .overflow_hidden()
                .pointer_events(PointerEvents::None)
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .bind_translate_x(&pan.offset())
                        .opacity(move || if now_x.get().is_some() { 1.0_f32 } else { 0.0 })
                        .child(
                            view()
                                .absolute()
                                .top(px(0.0))
                                .w(move || size.px(DOT))
                                .h(move || size.px(DOT))
                                .rounded(move || size.px(DOT / 2.0))
                                .left(move || px(now_x.get().unwrap_or(0.0) - size.of(DOT) / 2.0))
                                .bg(move || fill(ui, ui.theme.with(|t| t.guide().now_line))),
                        )
                        .child(
                            view()
                                .absolute()
                                .bottom(px(0.0))
                                .h(move || px(SEPARATOR_H + size.of(DOT) / 2.0))
                                .w(px(NOW_W))
                                .left(move || px(now_x.get().unwrap_or(0.0) - NOW_W / 2.0))
                                .bg(move || fill(ui, ui.theme.with(|t| t.guide().now_line))),
                        ),
                ),
        )
}

/// The date of the day shown (where now sits in its default window) and
/// the time now, at the left of the ruler in the light accent.
fn date_time(ui: Ui, size: Scale, pan: Pan) -> Text {
    text(move || {
        ui.guide_data();
        let now = ui.focus.with(|f| f.now);
        let shown = pan.band().base + MARGIN + PAST;
        let unix = crate::guide::unix_of(&*crate::schedule(), shown);
        format!(
            "{}  {}",
            iptv_core::clock::date(
                unix,
                crate::time::offset_minutes(unix),
                crate::i18n::clock_style()
            ),
            clock(now)
        )
    })
    .number_of_lines(1)
    .font_family(SANS)
    .text_size(move || size.text_px(RULER))
    .text_color(move || rgb(ui.theme.with(|t| t.guide().ruler_date)))
}
