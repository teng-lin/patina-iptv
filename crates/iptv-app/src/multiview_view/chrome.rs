//! Multiview's affordances over the tiles. A pointer (desktop mouse,
//! tablet touch; never a TV) gets, while it is active
//! (`multiview::chrome_shows`): a toolbar at the top centre (Add screen
//! while another fits, else the device's limit; exit multiview) and on
//! each tile, with two or more, buttons to enlarge or reduce it and to
//! remove it, named as the tile menu's items are. They are not mounted
//! while hidden, so they never take a press meant for a tile. Keys get a
//! short hint instead, which goes with the first key or after a few
//! seconds.

use nsh_core::platform::Idiom;
use nsh_core::prelude::*;

use super::{PANEL_ALPHA, WHITE, device_limit, with_state};
use crate::fonts::{OSD_BADGE, OSD_DETAIL};
use crate::i18n::tr;
use crate::multiview;
use crate::play_bar::icons::{self, Icon};
use crate::theme::{RADIUS_LARGE, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Ui, channel_name, tint};

/// The toolbar's band at the top centre (px): a tile's buttons keep out
/// of it.
const TOOLBAR_HALF_W: f32 = 200.0;
const TOOLBAR_H: f32 = 64.0;
/// Room between a tile's edge and its buttons, px.
const INSET: f32 = 8.0;

/// A tile button's side: a finger's target on touch devices, smaller
/// under a mouse.
fn target() -> f32 {
    if idiom() == Idiom::Desktop {
        36.0
    } else {
        44.0
    }
}

fn panel(ui: Ui) -> Rgba {
    tint(ui.theme.with(|t| t.background.panel), PANEL_ALPHA)
}

/// Add screen (or the device's limit) and Exit multiview, at the top
/// centre.
pub fn toolbar(ui: Ui) -> impl IntoElement {
    let shown = computed(move || multiview::chrome_shows(ui));
    let limit = move || with_state(ui, device_limit).flatten();
    let pill = move |words: &'static str, name: &'static str, act: fn(Ui)| {
        view()
            .role(AccessibilityRole::Button)
            .px(px(16.0))
            .py(px(8.0))
            .rounded(px(RADIUS_LARGE))
            .bg(tint(WHITE, RESTING_ALPHA))
            .hover(|s| s.bg(tint(WHITE, RESTING_ALPHA * 2)))
            // Its name in the interface language; the log keeps the source.
            .label(move || tr(name))
            .on_press(move || {
                log::info!("iptv multiview toolbar: {name}");
                act(ui);
            })
            .child(text(words).text_size(px(OSD_DETAIL)).text_color(rgb(WHITE)))
    };
    when(
        move || shown.get(),
        move || {
            view()
                .accessibility_elements_hidden(move || {
                    with_state(ui, |state| state.dialog().is_some()).unwrap_or(false)
                })
                .absolute()
                .top(px(12.0))
                .left(px(0.0))
                .right(px(0.0))
                .items_center()
                // Only the toolbar takes presses: beside it they reach the tiles.
                .pointer_events(PointerEvents::BoxNone)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(RADIUS_LARGE))
                        .bg(move || panel(ui))
                        .on_mouse_enter(move |_: &MouseEvent| crate::play_bar::hover(ui, true))
                        .on_mouse_leave(move |_: &MouseEvent| crate::play_bar::hover(ui, false))
                        .child(when(
                            move || limit().is_none(),
                            move || pill("+  Add screen", "Add screen", multiview::add),
                        ))
                        .child(when(
                            move || limit().is_some(),
                            move || {
                                text(move || limit().unwrap_or_default())
                                    .px(px(10.0))
                                    .text_size(px(OSD_DETAIL))
                                    .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA))
                            },
                        ))
                        .child(pill("Exit multiview", "Exit multiview", multiview::exit)),
                )
        },
    )
}

/// Whether a tile's buttons at its top right would lie under the
/// toolbar: then they go to its top left.
fn under_toolbar(tile: (f32, f32, f32, f32), buttons_w: f32, width: f32) -> bool {
    let right = tile.0 + tile.2 - INSET;
    let left = right - buttons_w;
    let centre = width / 2.0;
    tile.1 + INSET < TOOLBAR_H && left < centre + TOOLBAR_HALF_W && right > centre - TOOLBAR_HALF_W
}

/// Tile `index`'s enlarge (or reduce) and remove buttons, with two or
/// more tiles while the pointer chrome shows.
pub fn tile_buttons(ui: Ui, index: usize) -> impl IntoElement {
    let several = move || with_state(ui, |m| m.tiles().len() > 1).unwrap_or(false);
    let shown = computed(move || multiview::chrome_shows(ui) && several());
    let name = move || {
        ui.data_version.get();
        with_state(ui, |m| {
            m.tiles().get(index).map(|t| channel_name(t.channel))
        })
        .flatten()
        .unwrap_or_default()
    };
    let enlarged = move || with_state(ui, |m| m.enlarged() == Some(index)).unwrap_or(false);
    let size = target();
    let buttons_w = 2.0 * size + 6.0;
    // From the tile's left edge: at its top right, unless the toolbar is there.
    let left = move || {
        let rect = multiview::tile_rect(ui, index).unwrap_or((0.0, 0.0, 0.0, 0.0));
        if under_toolbar(rect, buttons_w, window().width.0) {
            INSET
        } else {
            rect.2 - INSET - buttons_w
        }
    };
    let button = move |icon: Box<dyn Fn() -> Icon>, act: fn(Ui, usize)| {
        view()
            .role(AccessibilityRole::Button)
            .w(px(size))
            .h(px(size))
            .rounded(px(size / 2.0))
            .items_center()
            .justify_center()
            .bg(move || panel(ui))
            .hover(|s| s.bg(tint(WHITE, RESTING_ALPHA * 3)))
            .on_press(move || act(ui, index))
            .child(
                svg(move || icons::src(icon()))
                    .size(px(size * 0.5))
                    .text_color(rgb(WHITE)),
            )
    };
    when(
        move || shown.get(),
        move || {
            view()
                .absolute()
                .top(px(INSET))
                .left(move || px(left()))
                .flex_row()
                .gap(px(6.0))
                .child(
                    button(
                        Box::new(move || {
                            if enlarged() {
                                Icon::ExitFullScreen
                            } else {
                                Icon::FullScreen
                            }
                        }),
                        multiview::toggle_enlarge,
                    )
                    .label(move || {
                        let what = if enlarged() {
                            "Reduce screen"
                        } else {
                            "Enlarge screen"
                        };
                        format!("{}: {}", tr(what), name())
                    }),
                )
                .child(
                    button(Box::new(|| Icon::Close), multiview::remove)
                        .label(move || format!("{}: {}", tr("Remove screen"), name())),
                )
        },
    )
}

/// The keys' hint at the top centre: OK opens the tile's menu, Back
/// watches it full screen. It goes with the first key or after a while.
pub fn hint(ui: Ui) -> impl IntoElement {
    let shown = move || {
        ui.keys_shown()
            && with_state(ui, |m| {
                m.hint() && m.dialog().is_none() && m.pick().is_none()
            })
            .unwrap_or(false)
    };
    when(shown, move || {
        view()
            .absolute()
            .top(px(12.0))
            .left(px(0.0))
            .right(px(0.0))
            .items_center()
            .pointer_events(PointerEvents::None)
            .child(
                text("OK: menu · Back: full screen")
                    .px(px(14.0))
                    .py(px(5.0))
                    .rounded(px(RADIUS_LARGE))
                    .bg(move || panel(ui))
                    .text_size(px(OSD_BADGE))
                    .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA)),
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_keep_out_of_the_toolbar() {
        let width = 1280.0;
        // Two by two on a 16:9 window: the top left tile's top right
        // corner lies under the toolbar, the top right tile's does not.
        assert!(under_toolbar((3.0, 3.0, 634.0, 354.0), 78.0, width));
        assert!(!under_toolbar((643.0, 3.0, 634.0, 354.0), 78.0, width));
        // Side by side, vertically centred: well below it.
        assert!(!under_toolbar((3.0, 183.0, 634.0, 354.0), 78.0, width));
    }
}
