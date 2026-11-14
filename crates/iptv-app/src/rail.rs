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

/// The rail's width now (0 while the guide has the focus).
pub fn width(ui: Ui) -> f32 {
    match ui.focus.with(|f| f.pane) {
        Pane::Guide => 0.0,
        Pane::Groups => RAIL_W,
        Pane::Rail => RAIL_OPEN_W,
    }
}

fn label(item: RailItem) -> &'static str {
    match item {
        RailItem::Search => tr("Search"),
        RailItem::Tv => tr("Live TV"),
        RailItem::Movies => tr("Films"),
        RailItem::Shows => tr("Series"),
        RailItem::Recordings => tr("Recordings"),
        RailItem::MyList => tr("Watchlist"),
        RailItem::Playlists => tr("Playlists"),
        RailItem::Settings => tr("Settings"),
    }
}

/// A 24-unit line drawing in the text colour.
fn icon(item: RailItem) -> ImageSrc {
    let body = match item {
        RailItem::Search => "<circle cx='10.5' cy='10.5' r='6'/><path d='M15 15l5.5 5.5'/>",
        RailItem::Tv => "<rect x='3' y='5' width='18' height='12' rx='2'/><path d='M8 21h8'/>",
        RailItem::Movies => {
            "<rect x='3' y='4' width='18' height='16' rx='2'/><path d='M8 4v16M16 4v16M3 12h18'/>"
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
    // Its place in the rail now (Movies and Shows come and go with VOD).
    let index = move || crate::lists::rail(ui).iter().position(|&i| i == item);
    let open = move || ui.focus.with(|f| f.pane) == Pane::Rail;
    let focused = move || {
        open() && ui.keys_shown() && index().is_some_and(|i| ui.focus.with(|f| f.rail) == i)
    };
    let colour = move || {
        if !item.enabled() {
            tint(WHITE, TEXT_DISABLED_ALPHA / 2)
        } else if focused() {
            rgb(FOCUSED_TEXT)
        } else if item == RailItem::Tv {
            rgb(ui.theme.with(|t| t.accent.light))
        } else {
            tint(WHITE, TEXT_SECONDARY_ALPHA)
        }
    };
    view()
        .role(AccessibilityRole::Tab)
        .accessibility_state(move || AccessibilityState {
            selected: item == RailItem::Tv,
            ..Default::default()
        })
        .h(move || px(if index().is_some() { ITEM_H } else { 0.0 }))
        .mx(px(6.0))
        .my(move || px(if index().is_some() { 2.0 } else { 0.0 }))
        .px(px(11.0))
        .flex_row()
        .items_center()
        .gap(px(14.0))
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
        .child(svg(icon(item)).size(px(ICON)).text_color(colour))
        .child(
            text(move || label(item))
                .number_of_lines(1)
                .accessibility_elements_hidden(move || !open())
                .opacity(move || if open() { 1.0_f32 } else { 0.0 })
                .text_size(px(TEXT_SIZE))
                .text_color(colour),
        )
}

/// The rail: top items, then Playlists and Settings at the bottom.
pub fn column(ui: Ui) -> impl IntoElement {
    let part = move |bottom: bool| {
        view().children(
            iptv_core::browse::rail_items(true)
                .into_iter()
                .filter(move |i| i.at_bottom() == bottom)
                .map(move |item| item_view(ui, item)),
        )
    };
    view()
        .w(move || px(width(ui)))
        .overflow_hidden()
        .py(px(8.0))
        .bg(move || ui.surface(ui.theme.with(|t| t.background.panel)))
        .child(part(false))
        .child(view().flex_1())
        .child(part(true))
}
