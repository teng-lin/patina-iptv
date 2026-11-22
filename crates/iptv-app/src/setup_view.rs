//! Shared layout for library setup, restore review, and startup recovery.

use nsh_core::prelude::*;

use crate::Ui;
use crate::fonts::{LIBRARY_TITLE, MEDIUM, SANS};
use crate::scale::Scale;
use crate::theme::{BRIGHT_TEXT, RADIUS_LARGE};
use crate::ui_kit::{self, FocusScroll};

pub(crate) use crate::ui_kit::primary_button as primary;

const PANEL_WIDTH: f32 = 1120.0;
const MIN_SPLIT_WIDTH: f32 = 740.0;

fn split(size: Scale) -> bool {
    window().width.0 >= size.of(MIN_SPLIT_WIDTH) * crate::settings::text_factor().max(1.0)
}

pub(crate) fn frame(
    ui: Ui,
    size: Scale,
    section: impl IntoReactive<SharedString>,
    scrolling: FocusScroll,
    content: impl IntoElement,
) -> View {
    view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .pb_safe_area()
        .px_safe_area()
        .bg(move || rgb(ui.app.theme.with(|theme| theme.background.window)))
        .child(
            view()
                .flex_1()
                .w_full()
                .max_w(move || size.px(PANEL_WIDTH))
                .self_center()
                .px(move || size.px(if split(size) { 48.0 } else { 20.0 }))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(move || size.px(12.0))
                        .py(move || size.px(22.0))
                        .child(ui_kit::title(size, crate::startup::APP_NAME).font_weight(MEDIUM))
                        .child(ui_kit::body(size, section)),
                )
                .child(
                    scrolling.view().test_id("setup-scroll").flex_1().child(
                        view().pb(move || size.px(20.0)).child(
                            view()
                                .rounded(px(RADIUS_LARGE))
                                .p(move || size.px(if split(size) { 36.0 } else { 22.0 }))
                                .bg(move || rgb(ui.app.theme.with(|theme| theme.background.panel)))
                                .child(content),
                        ),
                    ),
                ),
        )
}

pub(crate) fn columns(size: Scale, intro: View, content: View) -> View {
    view()
        .gap(move || size.px(if split(size) { 48.0 } else { 22.0 }))
        .style_when(move || split(size), |style| style.flex_row().items_start())
        .child(intro.style_when(
            move || split(size),
            |style| style.flex_1().flex_grow(0.85).min_w(px(0.0)),
        ))
        .child(content.style_when(
            move || split(size),
            |style| style.flex_1().flex_grow(1.15).min_w(px(0.0)),
        ))
}

pub(crate) fn intro(
    size: Scale,
    title: impl IntoReactive<SharedString>,
    detail: impl IntoReactive<SharedString>,
) -> View {
    view()
        .gap(move || size.px(12.0))
        .child(
            text(title)
                .role(AccessibilityRole::Header)
                .font_family(SANS)
                .font_weight(MEDIUM)
                .text_size(move || size.text_px(LIBRARY_TITLE))
                .text_color(rgb(BRIGHT_TEXT)),
        )
        .child(ui_kit::body(size, detail))
}
