//! A tile's menu: a panel at the right edge, vertically centred, over the
//! tiles without dimming them (they stay in view, and a tap on one closes
//! the menu). Its rows are the lists' (`list_rows.rs`): the guide's pitch
//! and type, the focus its light pill with dark text while the keys are
//! shown; the panel is the theme's panel colour. Sizes are at full size,
//! scaled with the guide (`scale.rs`): on a TV (× 0.8) the panel is
//! 208 dp wide, 8 dp from the edge, with 40 dp rows.

use crate::i18n::tr;

use iptv_core::multiview::{Dialog, TileAction};
use nsh_core::prelude::*;

use super::with_state;
use crate::list_rows::{self, Mark, ROW};
use crate::multiview;
use crate::scale::Scale;
use crate::theme::RESTING_ALPHA;
use crate::{Ui, tint};

/// The panel's width, its distance from the right edge, the room around
/// its rows and its corners (dp at full size).
pub const PANEL_W: f32 = 260.0;
pub const MARGIN: f32 = 10.0;
const PAD: f32 = 10.0;
const CORNER: f32 = 10.0;
/// The most items a menu has (a failed tile among two or more that fit).
const ITEMS: usize = 8;

const WHITE: u32 = 0xFFFFFF;

pub fn action_label(action: TileAction) -> &'static str {
    match action {
        TileAction::Retry => tr("Try again"),
        TileAction::Add => tr("Add screen"),
        TileAction::SearchAdd => tr("Search and add"),
        TileAction::Replace => tr("Change channel"),
        TileAction::Pause => tr("Pause"),
        TileAction::Play => tr("Play"),
        TileAction::Enlarge => tr("Enlarge screen"),
        TileAction::Reduce => tr("Reduce screen"),
        TileAction::FullScreen => tr("Full screen"),
        TileAction::Remove => tr("Remove screen"),
    }
}

/// The open menu's items and focus.
fn content(ui: Ui) -> (Vec<TileAction>, usize) {
    with_state(ui, |m| match m.dialog() {
        Some(Dialog::Options { items, focus, .. }) => (items.clone(), *focus),
        _ => (Vec::new(), 0),
    })
    .unwrap_or_default()
}

pub fn panel(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let content = computed(move || content(ui));
    let item = move |i: usize| {
        let action = move || content.with(|c| c.0.get(i).copied());
        let mark = move || Mark::of(ui.keys_shown() && content.with(|c| c.1 == i), false);
        when(
            move || action().is_some(),
            move || {
                list_rows::row(size, ROW, mark)
                    .role(AccessibilityRole::Menuitem)
                    .hover(|s| s.bg(tint(WHITE, RESTING_ALPHA)))
                    .on_press(move || multiview::choose(ui, i))
                    .child(
                        list_rows::name(size, move || action().map_or("", action_label))
                            .text_color(move || mark().text()),
                    )
            },
        )
    };
    // A band down the right edge that takes no presses itself: beside the
    // panel they reach the tiles.
    view()
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .right(move || size.px(MARGIN))
        .justify_center()
        .pointer_events(PointerEvents::BoxNone)
        .child(
            view()
                .test_id("multiview-menu")
                .w(move || size.px(PANEL_W))
                .p(move || size.px(PAD))
                .rounded(move || size.px(CORNER))
                .bg(move || rgb(ui.theme.with(|t| t.background.panel)))
                .pointer_events(PointerEvents::Auto)
                // Between the rows: nothing (not the tile under it).
                .on_press(|| {})
                .children((0..ITEMS).map(item)),
        )
}
