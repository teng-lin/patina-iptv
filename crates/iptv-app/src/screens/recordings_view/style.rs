//! Recording panels use flat, edge-to-edge rows and a quiet slate focus fill.
//! All text uses Patina's bundled type scale; no external UI assets are used.
use super::*;

pub(super) const PANEL: u32 = 0x2B343E;
const FOCUS: u32 = 0x414D57;
pub(super) const WINDOW: u32 = 0x222C36;

pub(super) fn row(size: Scale, height: f32, focused: impl Fn() -> bool + Copy + 'static) -> View {
    view()
        .min_h(move || size.px(height))
        .px(move || size.px(24.0))
        .justify_center()
        .bg(move || rgb(if focused() { FOCUS } else { PANEL }))
        .role(AccessibilityRole::Button)
        .accessibility_state(move || AccessibilityState {
            selected: focused(),
            ..Default::default()
        })
}

pub(super) fn action(
    ui: Ui,
    size: Scale,
    label: impl IntoReactive<SharedString>,
    focused: impl Fn() -> bool + Copy + 'static,
    act: impl Fn() + 'static,
) -> View {
    row(size, 56.0, focused)
        .child(ui_kit::body(size, label).text_color(rgb(crate::theme::LIST_TEXT)))
        .on_press(move || {
            ui.touched();
            act();
        })
}

pub(super) fn header(ui: Ui, size: Scale, title: &'static str) -> View {
    view()
        .flex_row()
        .items_center()
        .min_h(move || size.px(72.0))
        .px(move || size.px(24.0))
        .gap(move || size.px(16.0))
        .child(ui_kit::title(size, title).flex_1())
        .child(
            view()
                .min_w(move || size.px(ui_kit::TOUCH))
                .min_h(move || size.px(ui_kit::TOUCH))
                .items_center()
                .justify_center()
                .role(AccessibilityRole::Button)
                .label(tr("Close"))
                .on_press(move || close(ui))
                .child(ui_kit::body(size, tr("Close"))),
        )
}
