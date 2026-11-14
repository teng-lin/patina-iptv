//! Multiview on screen (`multiview.rs` runs it): over each tile's video a
//! frame (the active tile's a thin light outline: its sound plays), its
//! channel (with ⏸ while paused) and its own waiting or error state with
//! Retry; the pointer chrome and the key hint (`multiview_view/chrome.rs`);
//! a tile's menu as a panel at the right edge (`multiview_view/menu.rs`),
//! with no backdrop over the tiles; and the connection warning, centred
//! over a dimmed screen. There are only tiles: no empty or "add" cells.
//! Taps go to the tiles and menu items, a long press on a tile enlarges or
//! reduces it; the video surfaces are below (`video.rs`). On a detour (a
//! tile watched full screen) none of this shows.

use crate::i18n::{tr, trf};

mod chrome;
mod menu;

use iptv_core::multiview::{Dialog, MAX_SURFACES, WARNING_CHOICES, WarningChoice};

use nsh_core::prelude::*;

use crate::fonts::{OSD_DETAIL, OSD_TEXT, OSD_TITLE};
use crate::multiview::{self, State};
use crate::theme::{FOCUS_ALPHA, RADIUS, RADIUS_LARGE, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Ui, channel_name, tint};

const WHITE: u32 = 0xFFFFFF;
const PANEL_ALPHA: u8 = 0xE0;
/// The active tile's outline: thin, light grey; the others keep a clear
/// one as wide, so no tile's content moves when the sound does.
const FRAME: f32 = 2.0;
const FRAME_COLOUR: u32 = 0x9F9F9F;
/// A held press enlarges a tile after this long, ms.
const LONG_PRESS_MS: f32 = 800.0;

fn warning_label(choice: WarningChoice) -> &'static str {
    match choice {
        WarningChoice::AddAnyway => "Add it anyway",
        WarningChoice::Cancel => tr("Cancel"),
    }
}

fn with_state<T>(ui: Ui, f: impl FnOnce(&State) -> T) -> Option<T> {
    ui.multiview.with(|m| m.as_ref().map(f))
}

/// Why no more tiles fit, while none does.
fn device_limit(m: &State) -> Option<String> {
    (!m.can_add()).then(|| {
        trf(
            "This device plays up to {count} at once",
            &[("count", m.limits().decoders.into())],
        )
    })
}

/// What a tile shows over its video.
#[derive(Clone, Debug, PartialEq)]
enum TileStatus {
    Playing,
    Waiting(String),
    Failed(String),
}

fn tile_status(ui: Ui, surface: usize) -> TileStatus {
    let now = ui.now_ms();
    ui.player
        .stream(surface)
        .health
        .with(|h| match h.failure() {
            Some(f) => TileStatus::Failed(crate::player_panels::failure_message(&f.kind, false)),
            // A quick reconnect keeps the last frame without a label.
            _ => h.indicator(now).map_or(TileStatus::Playing, |w| {
                TileStatus::Waiting(crate::player_panels::waiting_text(&w))
            }),
        })
}

fn dialog_is(ui: Ui, f: impl Fn(&Dialog) -> bool) -> bool {
    with_state(ui, |m| m.dialog().is_some_and(&f)).unwrap_or(false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let open = move || multiview::is_open(ui);
    let tiles = move || with_state(ui, |m| m.tiles().len()).unwrap_or(0);
    view()
        .test_id("multiview-ui")
        .accessibility_elements_hidden(move || !open() || ui.any_overlay_open())
        .absolute()
        .inset_0()
        .pointer_events(move || {
            if open() {
                PointerEvents::BoxNone
            } else {
                PointerEvents::None
            }
        })
        .child(when(open, move || {
            view()
                .absolute()
                .inset_0()
                .pointer_events(PointerEvents::BoxNone)
                .children(
                    (0..MAX_SURFACES).map(move |i| when(move || i < tiles(), move || tile(ui, i))),
                )
                .child(chrome::toolbar(ui))
                .child(chrome::hint(ui))
                .child(when(
                    move || dialog_is(ui, |d| matches!(d, Dialog::Options { .. })),
                    move || menu::panel(ui),
                ))
                .child(when(
                    move || dialog_is(ui, |d| matches!(d, Dialog::Warning { .. })),
                    move || warning(ui),
                ))
        }))
}

/// Tile `index`: its frame, status, channel and pause mark, and its
/// pointer buttons.
fn tile(ui: Ui, index: usize) -> impl IntoElement {
    let rect = move || multiview::tile_rect(ui, index).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let surface = move || with_state(ui, |m| m.tiles().get(index).map(|t| t.surface)).flatten();
    let channel = move || with_state(ui, |m| m.tiles().get(index).map(|t| t.channel)).flatten();
    let active = move || with_state(ui, |m| m.active_index() == index).unwrap_or(false);
    let paused =
        move || with_state(ui, |m| m.tiles().get(index).is_some_and(|t| t.paused)).unwrap_or(false);
    let accent = move || ui.theme.with(|t| t.accent.main);
    let panel = move || tint(ui.theme.with(|t| t.background.panel), PANEL_ALPHA);
    let status = move || surface().map_or(TileStatus::Playing, |s| tile_status(ui, s));
    view()
        .test_id("multiview-tile")
        .accessibility_elements_hidden(move || {
            with_state(ui, |state| state.dialog().is_some()).unwrap_or(false)
        })
        .absolute()
        .left(move || px(rect().0))
        .top(move || px(rect().1))
        .w(move || px(rect().2))
        .h(move || px(rect().3))
        .rounded(px(RADIUS))
        .border(px(FRAME))
        .border_color(move || {
            if active() {
                rgb(FRAME_COLOUR)
            } else {
                tint(WHITE, 0)
            }
        })
        .items_center()
        .justify_center()
        .on_press(move || multiview::tap(ui, index))
        .on_long_press(move || multiview::long_press(ui, index))
        .long_press_delay(LONG_PRESS_MS)
        .child(when(
            move || matches!(status(), TileStatus::Waiting(_)),
            move || {
                text(move || match status() {
                    TileStatus::Waiting(t) => t,
                    _ => String::new(),
                })
                .px(px(14.0))
                .py(px(6.0))
                .rounded(px(RADIUS_LARGE))
                .bg(panel)
                .text_size(px(OSD_DETAIL))
                .text_color(rgb(WHITE))
            },
        ))
        .child(when(
            move || matches!(status(), TileStatus::Failed(_)),
            move || {
                view()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(18.0))
                    .py(px(14.0))
                    .rounded(px(RADIUS_LARGE))
                    .bg(panel)
                    .child(
                        text(move || match status() {
                            TileStatus::Failed(m) => m.to_owned(),
                            _ => String::new(),
                        })
                        .text_size(px(OSD_DETAIL))
                        .text_color(rgb(WHITE)),
                    )
                    .child(
                        text(move || tr("Try again"))
                            .px(px(16.0))
                            .py(px(6.0))
                            .rounded(px(RADIUS))
                            .text_size(px(OSD_DETAIL))
                            .text_color(rgb(WHITE))
                            .bg(move || tint(accent(), FOCUS_ALPHA))
                            .border(px(2.0))
                            .border_color(move || rgb(ui.theme.with(|t| t.accent.light)))
                            .on_press(move || {
                                ui.touched();
                                if let Some(s) = surface() {
                                    crate::player::retry_surface(ui, s);
                                }
                            }),
                    )
            },
        ))
        // The channel at the bottom left, ⏸ after it while the tile is paused.
        .child(
            view()
                .absolute()
                .left(px(8.0))
                .bottom(px(8.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(RADIUS))
                .bg(panel)
                .pointer_events(PointerEvents::None)
                .child(
                    text(move || {
                        ui.guide_data();
                        channel().map_or(String::new(), |ch| {
                            format!("{}  {}", crate::lists::number(ui, ch), channel_name(ch))
                        })
                    })
                    .number_of_lines(1)
                    .text_size(px(OSD_DETAIL))
                    .text_color(rgb(WHITE)),
                )
                .child(when(paused, || {
                    text("⏸")
                        .test_id("multiview-paused")
                        .text_size(px(OSD_DETAIL))
                        .text_color(rgb(WHITE))
                })),
        )
        .child(chrome::tile_buttons(ui, index))
}

/// The warning's title, message, choices and which is focused.
fn warning_content(ui: Ui) -> (String, String, usize) {
    let content = with_state(ui, |m| match m.dialog() {
        Some(Dialog::Warning { allowed, focus, .. }) => (
            trf(
                "{count, plural, one {Your account allows # stream at once} \
                 other {Your account allows # streams at once}}",
                &[("count", (*allowed).into())],
            ),
            "Another one may not play, or may stop one that is playing.".to_owned(),
            *focus,
        ),
        _ => (String::new(), String::new(), 0),
    });
    content.unwrap_or_default()
}

/// The connection warning, centred over a dimmed screen; its choices in
/// a row.
fn warning(ui: Ui) -> impl IntoElement {
    let content = computed(move || warning_content(ui));
    let panel = move || tint(ui.theme.with(|t| t.background.panel), 0xF0);
    let accent = move || ui.theme.with(|t| t.accent.main);
    let choice = move |i: usize| {
        let focused = move || ui.keys_shown() && content.with(|c| c.2 == i);
        text(warning_label(WARNING_CHOICES[i]))
            .border(px(2.0))
            .border_color(move || {
                tint(
                    ui.theme.with(|t| t.accent.light),
                    if focused() { 255 } else { 0 },
                )
            })
            .px(px(18.0))
            .py(px(8.0))
            .rounded(px(RADIUS))
            .text_size(px(OSD_TEXT))
            .text_color(rgb(WHITE))
            .bg(move || {
                if focused() {
                    tint(accent(), FOCUS_ALPHA)
                } else {
                    tint(WHITE, RESTING_ALPHA)
                }
            })
            .on_press(move || multiview::choose(ui, i))
    };
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .bg(tint(0x000000, 0x80))
        .pointer_events(PointerEvents::Auto)
        // A tap beside the dialog closes it.
        .on_press(move || multiview::tap(ui, usize::MAX))
        .child(
            view()
                .w(px(380.0))
                .gap(px(10.0))
                .px(px(24.0))
                .py(px(20.0))
                .rounded(px(RADIUS_LARGE))
                .bg(panel)
                .pointer_events(PointerEvents::Auto)
                .on_press(|| {})
                .child(
                    text(move || content.with(|c| c.0.clone()))
                        .text_size(px(OSD_TITLE))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(WHITE)),
                )
                .child(
                    text(move || content.with(|c| c.1.clone()))
                        .text_size(px(OSD_DETAIL))
                        .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA)),
                )
                .child(
                    view()
                        .mt(px(6.0))
                        .gap(px(8.0))
                        .flex_row()
                        .children((0..WARNING_CHOICES.len()).map(choice)),
                ),
        )
}
