//! The regions around the guide: the opaque backdrop under the video
//! layer, the groups column (shown with the rail while either has the
//! focus) and the geometry of the header (`header.rs`) and its preview.

use crate::chrome::layout::GuideHeader;
use crate::guide::lists;
use nsh_core::prelude::*;

use crate::Ui;
use crate::chrome::scale::Scale;
use crate::guide::Pane;
use crate::guide::list_rows::{self, Mark, ROW};

/// The groups column's width at full size (scaled with the guide).
pub const GROUPS_W: f32 = 260.0;

/// The groups column's width now (0 while the guide has the focus).
pub fn groups_width(ui: crate::GuideUi) -> f32 {
    if crate::guide::lists::columns_shown(ui) {
        (GROUPS_W * crate::chrome::scale::now()).round()
    } else {
        0.0
    }
}
/// The preview's margin and corner radius (dp); its geometry and the
/// header's come from `crate::chrome::layout::GuideHeader`.
pub use crate::chrome::layout::PREVIEW_MARGIN;
pub const PREVIEW_RADIUS: f32 = 10.0;

/// The header in the window now (reactive): the guide's scale and the
/// viewer's text size decide the layout and how tall the header is.
pub fn guide_header_now() -> GuideHeader {
    let w = window();
    GuideHeader::of(
        w.width.0,
        w.height.0,
        crate::chrome::scale::now(),
        crate::wiring::settings::text_factor(),
    )
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
///
/// Where the video plane is under the whole page (`host::video_under_page`)
/// the backdrop cannot be under the video, so while a picture shows
/// (`video::backdrop_clear`) it leaves the video's rectangle clear and
/// keeps its colors everywhere else, as the native layers look: the
/// window color in strips around the rectangle (none in full screen) and
/// the rail's strip only where the video is not.
pub fn backdrop(ui: Ui) -> impl IntoElement {
    let panel = move || rgb(ui.app.theme.with(|t| t.background.panel));
    let window_color = move || rgb(ui.app.theme.with(|t| t.background.window));
    let clear = move || crate::playback::video::backdrop_clear(ui);
    let video_under_page = crate::host::video_under_page();
    let mut backdrop = view()
        .test_id("video-backdrop")
        .absolute()
        .inset_0()
        .pointer_events(PointerEvents::None)
        .bg(move || if clear() { rgba(0) } else { window_color() });
    if video_under_page {
        let strips = computed(move || {
            if clear() {
                let w = window();
                strips_around(crate::playback::video::rect(ui), (w.width.0, w.height.0))
            } else {
                [EMPTY; 4]
            }
        });
        backdrop = backdrop.children((0..4).map(move |side| {
            let strip = move || strips.get()[side];
            view()
                .absolute()
                .left(move || px(strip().0))
                .top(move || px(strip().1))
                .w(move || px(strip().2))
                .h(move || px(strip().3))
                .bg(window_color)
        }));
    }
    let rail_width = move || {
        let width = crate::chrome::rail::width(ui.guide);
        if video_under_page && clear() {
            let w = window();
            let top = safe_area().top.0 + header_now().0;
            let strip = (0.0, top, width, w.height.0 - top);
            if overlaps(strip, crate::playback::video::rect(ui)) {
                return 0.0;
            }
        }
        width
    };
    backdrop
        .child(
            view()
                .pt_safe_area()
                .child(view().h(move || px(header_now().0))),
        )
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(view().w(move || px(rail_width())).bg(panel)),
        )
}

/// A rectangle: x, y, width, height (px).
type Rect = (f32, f32, f32, f32);

const EMPTY: Rect = (0.0, 0.0, 0.0, 0.0);

/// The window (`size`: width, height) less `video`, as four rectangles:
/// above it, below it, and left and right of it between those two. Empty
/// where the video reaches the window's edge.
fn strips_around(video: Rect, size: (f32, f32)) -> [Rect; 4] {
    let (width, height) = size;
    let left = video.0.clamp(0.0, width);
    let top = video.1.clamp(0.0, height);
    let right = (video.0 + video.2).clamp(left, width);
    let bottom = (video.1 + video.3).clamp(top, height);
    [
        (0.0, 0.0, width, top),
        (0.0, bottom, width, height - bottom),
        (0.0, top, left, bottom - top),
        (right, top, width - right, bottom - top),
    ]
}

/// Whether two rectangles share any area.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.2 > 0.0
        && a.3 > 0.0
        && b.2 > 0.0
        && b.3 > 0.0
        && a.0 < b.0 + b.2
        && b.0 < a.0 + a.2
        && a.1 < b.1 + b.3
        && b.1 < a.1 + a.3
}

/// The groups column: Favorites, Recently watched, All channels, then the
/// playlist's groups, in the guide's type on the window color; the focus
/// is the guide's light pill. Moving over (or tapping) one shows its list.
pub fn group_column(ui: Ui, groups: ElementRef) -> impl IntoElement {
    let size = Scale::new();
    virtual_list(
        move || lists::visible_entries(ui.app).len(),
        move |at| {
            let entry = move || lists::visible_entries(ui.app).entry(at.index());
            let current = move || entry() == Some(ui.guide.focus.with(|f| f.group));
            let focused = move || {
                current() && ui.guide.focus.with(|f| f.pane) == Pane::Groups && ui.keys_shown()
            };
            let mark = move || Mark::of(focused(), current());
            let name = move || {
                ui.guide_data();
                entry().map_or_else(String::new, |entry| {
                    lists::browser_entry_name(entry, &*crate::wiring::runtime_owner::schedule())
                })
            };
            let expanded = move || {
                ui.guide_data();
                lists::playlist_expanded(entry()?, &*crate::wiring::runtime_owner::schedule())
            };
            let color = move || match mark() {
                Mark::Current => rgb(ui.app.theme.with(|t| t.accent.light)),
                m => m.text(),
            };
            list_rows::row(size, ROW, mark)
                .flex_row()
                .items_center()
                .gap(move || size.px(8.0))
                .role(AccessibilityRole::Tab)
                .label(move || name().trim_start().to_owned())
                .accessibility_state(move || AccessibilityState {
                    selected: current(),
                    expanded: expanded(),
                    ..Default::default()
                })
                .mx(px(8.0))
                .on_press(move || {
                    ui.touched();
                    if let Some(entry) = entry() {
                        if idiom() == nsh_core::platform::Idiom::Desktop {
                            lists::activate_entry(ui, entry);
                        } else {
                            lists::tap_entry(ui, entry);
                        }
                    }
                })
                .on_long_press(move || {
                    ui.touched();
                    if let Some(entry) = entry() {
                        crate::chrome::menu_view::open_group_options(ui, entry);
                    }
                })
                .on_context_menu(move |_: &nsh_core::ContextMenuEvent| {
                    ui.touched();
                    if let Some(entry) = entry() {
                        crate::chrome::menu_view::open_group_options(ui, entry);
                    }
                })
                .child(list_rows::name(size, name).flex_1().text_color(color))
                .child(when(
                    move || expanded().is_some(),
                    move || {
                        list_rows::triangle(size, move || {
                            crate::guide::lists::playlist_arrow(expanded())
                        })
                        .test_id("playlist-disclosure")
                        .flex_shrink_0()
                        .text_color(color)
                    },
                ))
        },
    )
    .test_id("guide-groups")
    .pointer_events(move || crate::guide_chrome_pointer_events(ui.screens))
    .accessibility_elements_hidden(move || !crate::guide::lists::columns_shown(ui.guide))
    .estimated_item_size(list_rows::pitch(size, ROW))
    .element_ref(&groups)
    .w(move || px(groups_width(ui.guide)))
    .py(px(6.0))
    .bg(move || ui.surface(ui.app.theme.with(|t| t.background.window)))
}

#[cfg(test)]
mod tests {
    use super::{EMPTY, overlaps, strips_around};

    const WINDOW: (f32, f32) = (960.0, 540.0);

    #[test]
    fn the_strips_cover_the_window_but_the_preview() {
        let preview = (12.0, 12.0, 320.0, 180.0);
        let strips = strips_around(preview, WINDOW);
        assert_eq!(
            strips,
            [
                (0.0, 0.0, 960.0, 12.0),
                (0.0, 192.0, 960.0, 348.0),
                (0.0, 12.0, 12.0, 180.0),
                (332.0, 12.0, 628.0, 180.0),
            ]
        );
        let covered: f32 = strips.iter().map(|s| s.2 * s.3).sum();
        assert_eq!(covered, 960.0 * 540.0 - 320.0 * 180.0);
        assert!(strips.iter().all(|s| !overlaps(*s, preview)));
    }

    #[test]
    fn full_screen_video_leaves_no_strip() {
        let strips = strips_around((0.0, 0.0, WINDOW.0, WINDOW.1), WINDOW);
        assert!(strips.iter().all(|s| s.2 * s.3 == 0.0), "{strips:?}");
        // A rectangle past the window's edges is clamped to it.
        let strips = strips_around((-5.0, -5.0, 1000.0, 600.0), WINDOW);
        assert!(strips.iter().all(|s| s.2 * s.3 == 0.0), "{strips:?}");
    }

    #[test]
    fn no_video_leaves_the_whole_window_opaque() {
        let strips = strips_around(EMPTY, WINDOW);
        assert_eq!(strips[1], (0.0, 0.0, 960.0, 540.0));
        let covered: f32 = strips.iter().map(|s| s.2 * s.3).sum();
        assert_eq!(covered, 960.0 * 540.0);
    }

    #[test]
    fn the_rail_strip_is_dropped_only_over_the_video() {
        let rail = (0.0, 60.0, 72.0, 480.0);
        assert!(overlaps(rail, (0.0, 0.0, 960.0, 540.0)), "full screen");
        assert!(!overlaps(rail, (12.0, 12.0, 320.0, 40.0)), "the preview");
        assert!(
            !overlaps((0.0, 60.0, 0.0, 480.0), (0.0, 0.0, 960.0, 540.0)),
            "no rail"
        );
    }
}
