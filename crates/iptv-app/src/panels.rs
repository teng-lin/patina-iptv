//! The regions around the guide: the opaque backdrop under the video
//! layer, the groups column (shown with the rail while either has the
//! focus) and the geometry of the header (`header.rs`) and its preview.

use iptv_core::layout::{GuideHeader, Shortcuts};
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
/// The preview's margin and corner radius (dp); its geometry and the
/// header's come from `iptv_core::layout::GuideHeader`.
pub use iptv_core::layout::PREVIEW_MARGIN;
pub const PREVIEW_RADIUS: f32 = 10.0;

/// The header in the window now (reactive): the guide's scale and the
/// viewer's text size (the app's and the system's) decide where the shortcuts go and how tall their row is.
pub fn guide_header_now() -> GuideHeader {
    let w = window();
    GuideHeader::of(
        w.width.0,
        w.height.0,
        crate::scale::now(),
        crate::settings::text_factor(),
    )
}

/// Where the header puts its shortcuts now (reactive).
pub fn shortcuts_now() -> Shortcuts {
    guide_header_now().shortcuts
}

/// The header's height and the preview's width and height now (reactive).
pub fn header_now() -> (f32, f32, f32) {
    let g = guide_header_now();
    (g.height, g.preview.0, g.preview.1)
}

/// The rectangle the video layer occupies in guide mode (x, y, width,
/// height): the preview, below the status bar.
pub fn preview_rect() -> (f32, f32, f32, f32) {
    let top = safe_area().top.0;
    let (_, w, h) = header_now();
    (PREVIEW_MARGIN, top + PREVIEW_MARGIN, w, h)
}

/// The opaque colors of the regions, drawn under the video layer with the
/// same geometry as the foreground: the window color under the header,
/// the groups column and the guide; the panel color under the rail.
/// The foreground stays clear in guide mode, so the video shows wherever it is
/// laid out: today the preview rectangle.
pub fn backdrop(ui: Ui) -> impl IntoElement {
    let panel = move || rgb(ui.app.theme.with(|t| t.background.panel));
    let window = move || rgb(ui.app.theme.with(|t| t.background.window));
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

/// The groups column: Favorites, Recently watched, All channels, then the
/// playlist's groups, in the guide's type on the window color; the focus
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
                    if idiom() == nsh_core::platform::Idiom::Desktop {
                        crate::lists::activate_entry(ui, at.index());
                    } else {
                        crate::lists::tap_entry(ui, at.index());
                    }
                })
                .on_long_press(move || {
                    ui.touched();
                    crate::menu_view::open_group_options(ui, at.index());
                })
                .on_context_menu(move |_: &nsh_core::ContextMenuEvent| {
                    ui.touched();
                    crate::menu_view::open_group_options(ui, at.index());
                })
                .child(
                    // Re-read on new data: the recycled cell keeps its index.
                    list_rows::name(size, move || {
                        ui.guide_data();
                        crate::lists::browser_entry_name(at.index())
                    })
                    .text_color(move || match mark() {
                        Mark::Current => rgb(ui.app.theme.with(|t| t.accent.light)),
                        m => m.text(),
                    })
                    .mr(move || {
                        px(crate::channels_view::group_more_room(
                            ui,
                            current(),
                            at.index(),
                        ))
                    }),
                )
                .child(crate::channels_view::group_more(
                    ui,
                    size,
                    move || at.index(),
                    current,
                    || true,
                    move || {
                        ui.guide_data();
                        crate::lists::browser_entry_name(at.index())
                    },
                ))
        },
    )
    .test_id("guide-groups")
    .accessibility_elements_hidden(move || !crate::lists::columns_shown(ui))
    .estimated_item_size(list_rows::pitch(size, ROW))
    .element_ref(&groups)
    .w(move || px(groups_width(ui)))
    .py(px(6.0))
    .bg(move || ui.surface(ui.app.theme.with(|t| t.background.window)))
}
