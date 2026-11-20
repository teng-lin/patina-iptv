//! The navigation rail left of the groups column, on the panel colour: icons while the groups
//! have the focus, icons with labels while it has the focus itself. Items
//! without a screen in this build are dimmed and never take the focus
//! (`iptv_core::browse`); the focused one is the light pill the guide's
//! focused programme is. Left from the groups lands on Search. The icons
//! are our own simple line drawings.

use crate::i18n::tr;

use iptv_core::browse::RailItem;
use iptv_core::guide::Pane;
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;

use crate::theme::{
    FOCUSED_CELL, FOCUSED_TEXT, RADIUS, TEXT_DISABLED_ALPHA, TEXT_SECONDARY_ALPHA, TEXT_SIZE,
};
use crate::{Ui, tint};

const WHITE: u32 = 0xFFFFFF;
/// Width with icons only, and with labels.
pub const RAIL_W: f32 = 56.0;
pub const RAIL_OPEN_W: f32 = 196.0;
const ITEM_H: f32 = 44.0;
const ICON: f32 = 22.0;
const ITEM_MARGIN: f32 = 2.0;

fn item_height_for(available: f32, count: usize) -> f32 {
    let count = count.max(1) as f32;
    (available / count - ITEM_MARGIN * 2.0).clamp(0.0, ITEM_H)
}

fn item_height(ui: Ui) -> f32 {
    // Background recorder startup can mount the main window while it has no
    // surface. Keep the rail's hit targets usable when that window is shown.
    if window().height.0 <= 0.0 {
        return ITEM_H;
    }
    let inset = safe_area();
    let available =
        window().height.0 - crate::panels::header_now().0 - inset.top.0 - inset.bottom.0 - 16.0;
    item_height_for(available.max(0.0), crate::lists::rail(ui).len())
}

/// The rail's width now (0 while the guide has the focus).
pub fn width(ui: Ui) -> f32 {
    match ui.focus.with(|f| f.layout_pane()) {
        Pane::Guide | Pane::Preview => 0.0,
        Pane::Groups => RAIL_W,
        Pane::Rail => RAIL_OPEN_W,
    }
}

fn label(item: RailItem) -> &'static str {
    match item {
        RailItem::Search => tr("Search"),
        RailItem::Tv => tr("Live TV"),
        RailItem::Continue => tr("Continue watching"),
        RailItem::Recent => tr("Recently added"),
        RailItem::Movies => tr("Movies"),
        RailItem::Shows => tr("Shows"),
        RailItem::Recordings => tr("Recordings"),
        RailItem::MyList => tr("Watchlist"),
        RailItem::Playlists => tr("Playlists"),
        RailItem::Settings => tr("Settings"),
    }
}

/// A 24-unit line drawing in the text colour.
pub(crate) fn icon(item: RailItem) -> ImageSrc {
    let body = match item {
        RailItem::Search => "<circle cx='10.5' cy='10.5' r='6'/><path d='M15 15l5.5 5.5'/>",
        RailItem::Tv => "<rect x='3' y='5' width='18' height='12' rx='2'/><path d='M8 21h8'/>",
        // A play mark inside an unfinished ring.
        RailItem::Continue => "<path d='M12 3a9 9 0 1 1-8.5 6'/><path d='M10 8.5v7l5.5-3.5z'/>",
        // A title card with a new mark beside it.
        RailItem::Recent => {
            "<rect x='3' y='7' width='13' height='13' rx='2'/><path d='M19 2v7M15.5 5.5h7'/>"
        }
        RailItem::Movies => {
            "<rect x='3' y='9' width='18' height='12' rx='1.5'/><path d='M3 9l2-6h17l-2 6M7 3l4 6M14 3l4 6M3 14h18'/>"
        }
        RailItem::Shows => {
            "<rect x='3' y='7' width='18' height='13' rx='2'/><path d='M8 3l4 4 4-4'/>"
        }
        RailItem::Recordings => {
            "<circle cx='12' cy='12' r='8'/><circle cx='12' cy='12' r='3' fill='currentColor'/>"
        }
        RailItem::MyList => "<path d='M6 3h12v18l-6-4-6 4z'/>",
        RailItem::Playlists => "<path d='M4 6h12M4 11h12M4 16h7'/><path d='M15 14v6l5-3z'/>",
        RailItem::Settings => {
            "<circle cx='12' cy='12' r='3.5'/><path d='M12 2v4M12 18v4M2 12h4M18 12h4M4.9 4.9l2.8 2.8M16.3 16.3l2.8 2.8M4.9 19.1l2.8-2.8M16.3 7.7l2.8-2.8'/>"
        }
    };
    ImageSrc::svg_markup(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' width='24' height='24' \
         fill='none' stroke='currentColor' stroke-width='1.8' stroke-linecap='round' \
         stroke-linejoin='round'>{body}</svg>"
    ))
}

fn item_view(ui: Ui, item: RailItem) -> impl IntoElement {
    // Its place in the rail now (discovery shelves follow imported VOD).
    let index = move || crate::lists::rail(ui).iter().position(|&i| i == item);
    let open = move || ui.focus.with(|f| f.layout_pane()) == Pane::Rail;
    let focused = move || {
        open()
            && ui.keys_shown()
            && index().is_some()
            && ui.focus.with(|f| f.pane == Pane::Rail && f.rail == item)
    };
    let colour = move || {
        if !item.enabled() {
            tint(WHITE, TEXT_DISABLED_ALPHA / 2)
        } else if focused() {
            rgb(FOCUSED_TEXT)
        } else if item == RailItem::Tv {
            rgb(ui.app.theme.with(|t| t.accent.light))
        } else {
            tint(WHITE, TEXT_SECONDARY_ALPHA)
        }
    };
    view()
        .role(AccessibilityRole::Tab)
        .accessibility_elements_hidden(move || index().is_none())
        .accessibility_state(move || AccessibilityState {
            selected: item == RailItem::Tv,
            ..Default::default()
        })
        .h(move || {
            px(if index().is_some() {
                item_height(ui)
            } else {
                0.0
            })
        })
        .mx(px(6.0))
        .my(move || px(if index().is_some() { ITEM_MARGIN } else { 0.0 }))
        .px(px(11.0))
        .flex_row()
        .items_center()
        .gap(move || px(item_height(ui) * 0.4))
        .rounded(px(RADIUS))
        .overflow_hidden()
        .bg(move || {
            if focused() {
                rgb(FOCUSED_CELL)
            } else {
                tint(WHITE, 0)
            }
        })
        // Its name even while the rail shows the icons only (VoiceOver,
        // TalkBack, tests).
        .label(move || label(item))
        .on_press(move || {
            ui.touched();
            if let Some(i) = index() {
                crate::lists::tap_rail(ui, i);
            }
        })
        .child(
            svg(icon(item))
                .size(move || px(item_height(ui).min(ICON)))
                .text_color(colour),
        )
        .child(
            text(move || label(item))
                .number_of_lines(1)
                .accessibility_elements_hidden(move || !open())
                .opacity(move || if open() { 1.0_f32 } else { 0.0 })
                .text_size(move || px(TEXT_SIZE.min(item_height(ui) * 0.8)))
                .text_color(colour),
        )
}

/// The rail: top items, then Playlists and Settings at the bottom.
pub fn column(ui: Ui) -> impl IntoElement {
    effect(move || {
        let items = crate::lists::rail(ui);
        if ui
            .focus
            .with_untracked(|focus| !items.contains(&focus.rail) || !focus.rail.enabled())
        {
            ui.focus.update(|focus| focus.retain_rail(&items));
        }
    });
    let part = move |bottom: bool| {
        view().children(
            iptv_core::browse::rail_items(iptv_core::browse::VodAvailability {
                shown: true,
                any: true,
            })
            .into_iter()
            .filter(move |i| i.at_bottom() == bottom)
            .map(move |item| item_view(ui, item)),
        )
    };
    view()
        .test_id("navigation-rail")
        .w(move || px(width(ui)))
        .overflow_hidden()
        .py(px(8.0))
        .pb_safe_area()
        .bg(move || ui.surface(ui.app.theme.with(|t| t.background.panel)))
        .child(part(false))
        .child(view().flex_1())
        .child(part(true))
}

#[cfg(test)]
mod tests {
    use super::{ITEM_MARGIN, item_height_for};

    #[test]
    fn rail_rows_shrink_to_keep_bottom_actions_in_a_tv_height() {
        assert_eq!(item_height_for(346.0, 8), 39.25);
        assert!((item_height_for(346.0, 10) - 30.6).abs() < 0.001);
        assert_eq!(item_height_for(500.0, 8), 44.0);
        let short = item_height_for(201.0, 10);
        assert!((short - 16.1).abs() < 0.001);
        assert!(short + ITEM_MARGIN * 2.0 <= 201.0 / 10.0);
    }

    #[test]
    fn a_short_windows_header_leaves_the_rail_its_items() {
        // The guide header's minimum grows it in short windows, but never
        // so far that the rail's eight items lose their height (as
        // `item_height` measures the room under the header).
        for (w, h, font) in [
            (480.0, 180.0, 1.3),
            (480.0, 240.0, 1.0),
            (480.0, 300.0, 1.3),
            (960.0, 200.0, 1.0),
        ] {
            let header = iptv_core::layout::GuideHeader::of(w, h, 0.8, font).height;
            let item = item_height_for((h - header - 16.0).max(0.0), 8);
            assert!(item > 0.0, "{w}x{h} at {font}: header {header}");
        }
    }
}
