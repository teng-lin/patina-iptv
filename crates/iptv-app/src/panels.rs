//! The regions around the guide: the opaque backdrop under the video
//! layer, the groups column (shown with the rail while either has the
//! focus) and the geometry of the header (`header.rs`) and its preview.

use iptv_core::lists::entry_count;
use nsh_core::prelude::*;

use crate::Ui;
use crate::guide::Pane;
use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;

/// The groups column's width at full size (scaled with the guide).
pub const GROUPS_W: f32 = 260.0;

/// The groups column's width now (0 while the guide has the focus).
pub fn groups_width(ui: Ui) -> f32 {
    if crate::lists::columns_shown(ui) {
        (GROUPS_W * crate::scale::now()).round()
    } else {
        0.0
    }
}
/// The header's share of the window's height; the preview (16:9) at its
/// top left: its largest share of the window's width, its margin and
/// corner radius (dp).
pub const HEADER_SHARE: f32 = 0.33;
pub const PREVIEW_SHARE: f32 = 0.32;
pub const PREVIEW_MARGIN: f32 = 12.0;
pub const PREVIEW_RADIUS: f32 = 10.0;

/// The header's height and the preview's width and height in a window
/// `w` × `h`: the preview as tall as the header leaves it, narrowed to its
/// share of the width.
pub fn header_size(w: f32, h: f32) -> (f32, f32, f32) {
    let header = (h * HEADER_SHARE).round();
    let tall = (header - 2.0 * PREVIEW_MARGIN).max(0.0);
    let width = (tall * 16.0 / 9.0).min(w * PREVIEW_SHARE).round();
    (header, width, (width * 9.0 / 16.0).round())
}

/// [`header_size`] of the window now (reactive).
pub fn header_now() -> (f32, f32, f32) {
    let w = window();
    header_size(w.width.0, w.height.0)
}

/// The rectangle the video layer occupies in guide mode (x, y, width,
/// height): the preview, below the status bar.
pub fn preview_rect() -> (f32, f32, f32, f32) {
    let top = safe_area().top.0;
    let (_, w, h) = header_now();
    (PREVIEW_MARGIN, top + PREVIEW_MARGIN, w, h)
}

/// The opaque colours of the regions, drawn under the video layer with the
/// same geometry as the foreground: the window colour under the header,
/// the groups column and the guide; the panel colour under the rail.
/// The foreground stays clear in guide mode, so the video shows wherever it is
/// laid out: today the preview rectangle.
pub fn backdrop(ui: Ui) -> impl IntoElement {
    let panel = move || rgb(ui.theme.with(|t| t.background.panel));
    let window = move || rgb(ui.theme.with(|t| t.background.window));
    view()
        .absolute()
        .inset_0()
        .pointer_events(PointerEvents::None)
        .bg(window)
        .child(
            view()
                .pt_safe_area()
                .child(view().h(move || px(header_now().0))),
        )
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(view().w(move || px(crate::rail::width(ui))).bg(panel)),
        )
}

/// The groups column: Favourites, Recently watched, All channels, then the
/// playlist's groups, in the guide's type on the window colour; the focus
/// is the guide's light pill. Moving over (or tapping) one shows its list.
pub fn group_column(ui: Ui, groups: ElementRef) -> impl IntoElement {
    // New data: the selected group stays in view.
    effect(move || {
        ui.guide_data();
        let group = ui.focus.with_untracked(|f| f.group);
        groups.scroll_to_index(group as u32, percentage(0.5), px(0.0), false);
    });
    let size = Scale::new();
    virtual_list(
        move || {
            ui.guide_data();
            entry_count(&*crate::schedule())
        },
        move |at| {
            let current = move || ui.focus.with(|f| f.group) == at.index();
            let focused =
                move || current() && ui.focus.with(|f| f.pane) == Pane::Groups && ui.keys_shown();
            let mark = move || Mark::of(focused(), current());
            list_rows::row(size, ROW, mark)
                .role(AccessibilityRole::Tab)
                .accessibility_state(move || AccessibilityState {
                    selected: current(),
                    ..Default::default()
                })
                .mx(px(8.0))
                .on_press(move || {
                    ui.touched();
                    crate::lists::tap_entry(ui, at.index());
                })
                .child(
                    // Re-read on new data: the recycled cell keeps its index.
                    list_rows::name(size, move || {
                        ui.guide_data();
                        crate::lists::browser_entry_name(at.index())
                    })
                    .text_color(move || match mark() {
                        Mark::Current => rgb(ui.theme.with(|t| t.accent.light)),
                        m => m.text(),
                    }),
                )
        },
    )
    .test_id("guide-groups")
    .estimated_item_size(list_rows::pitch(size, ROW))
    .element_ref(&groups)
    .w(move || px(groups_width(ui)))
    .py(px(6.0))
    .bg(move || ui.surface(ui.theme.with(|t| t.background.window)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_preview_keep_their_shares() {
        // Tablet (1920 × 1200 at 1.5 px/dp), desktop window, TV (1080p at 2).
        assert_eq!(header_size(1280.0, 800.0), (264.0, 410.0, 231.0));
        assert_eq!(header_size(1280.0, 720.0), (238.0, 380.0, 214.0));
        assert_eq!(header_size(960.0, 540.0), (178.0, 274.0, 154.0));
        assert_eq!(header_size(1920.0, 1080.0), (356.0, 590.0, 332.0));
        for (w, h) in [
            (1280.0, 800.0),
            (1280.0, 720.0),
            (960.0, 540.0),
            (760.0, 480.0),
        ] {
            let (header, pw, ph) = header_size(w, h);
            assert!(ph + 2.0 * PREVIEW_MARGIN <= header + 1.0, "{w}x{h}");
            assert!(pw <= w * PREVIEW_SHARE + 0.5, "{w}x{h}");
        }
    }
}
