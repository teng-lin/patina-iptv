//! The guide grid: a time ruler, one virtualized row per channel of the list
//! on screen (`lists.rs`) with its programmes across the full remaining
//! width, and a now-line. Rows are numbered by their place in the list;
//! `at.index()` is a row, the catalog channel comes from the list.
//!
//! The look: programmes are rounded cells on the window colour; the focused
//! programme is a light pill with dark text, the rest of its row lighter
//! cells with white titles, every other row dimmed (grey titles). Sizes are
//! dp at full size, the same on TV, tablet and a full-screen desktop; a
//! smaller desktop window scales them (`scale.rs`; `fonts.rs` has the type).
//!
//! The timeline pans by touch and mouse drags (`pan.rs`): the rows hold
//! the programmes of a band a little wider than the window and slide them,
//! with the ruler's times and the now line, by one animated offset.

mod cell;
mod channel;
use cell::RowFocus;
pub(crate) use channel::no_logo;
pub(crate) mod keys;
mod pan;
mod ruler;
mod wheel;

const BAND: i32 = 360 + 2 * iptv_core::timeline::MARGIN;
use pan::Pan;

use iptv_core::lists::{FAVORITES, RECENT};
use nsh_core::prelude::*;

use crate::fonts::CHANNEL;
use crate::guide::Pane;
use crate::scale::Scale;
use crate::theme::{RADIUS, TEXT_SECONDARY_ALPHA};
use crate::{Ui, ViewMode, tint};

/// Row pitch; a programme's height (the rest is the gap between rows), the
/// gap between programmes, their corner radius and text inset (full size:
/// [`Scale`] scales them, the pitch and height in whole px).
pub const ROW_H: f32 = 50.0;
const CELL_H: f32 = 46.0;
const CELL_GAP: f32 = 3.0;
const CELL_RADIUS: f32 = 4.0;
const CELL_INSET: f32 = 14.0;
/// The channel column: its share of the window's width, within bounds
/// (scaled).
const CHANNEL_SHARE: f32 = 0.36;
const CHANNEL_MIN: f32 = 234.0;
const CHANNEL_MAX: f32 = 576.0;
/// The now line's width.
const NOW_W: f32 = 2.0;
/// A past programme the row cannot replay (no catch-up, or before its
/// archive): shown when the view is panned back for rows that can, dimmed
/// to this opacity, and never focused by the keys (`guide::floor`).
const PAST_DIM: f32 = 0.4;

/// Cells a row can hold: about 12 per window of programmes, across the
/// band.
const MAX_CELLS: usize = 40;
const WHITE: u32 = 0xFFFFFF;

/// The channel column's width now (reactive: the window's size).
pub fn channel_width() -> f32 {
    let scale = crate::scale::now();
    (window().width.0 * CHANNEL_SHARE).clamp(CHANNEL_MIN * scale, CHANNEL_MAX * scale)
}

/// An opaque guide colour; over full-screen video (overlay) at the
/// overlay's alpha (`appearance.ui_transparency`).
pub(crate) fn fill(ui: Ui, rgb_hex: u32) -> Rgba {
    let alpha = match ui.mode.get() {
        ViewMode::Overlay => crate::settings::overlay_alpha(&crate::settings::read(ui)),
        ViewMode::Guide | ViewMode::Fullscreen => 0xFF,
    };
    tint(rgb_hex, alpha)
}

pub fn guide(ui: Ui, rows: ElementRef) -> impl IntoElement {
    // The programme area's width: the window always spans all of it.
    let width = signal(0.0_f32);
    let size = Scale::new();
    let pan = Pan::new(ui, width);
    let now = computed(move || ui.focus.with(|f| f.now));
    let focused = computed(move || {
        ui.guide_data();
        let view = crate::lists::view(ui);
        ui.focus
            .with(|f| (f.pane == Pane::Guide).then(|| (f.channel, f.program(&view).start)))
    });
    // Read where used: a memo of it went stale on window resizes.
    let column: fn() -> f32 = channel_width;
    // Rows are numbered by place: the list's length is its widest number.
    let number_w = computed(move || {
        crate::settings::read(ui);
        if crate::settings::flag(iptv_settings::keys::BoolKey("guide.show_channel_numbers")) {
            crate::numbers::column_width(
                ui.list.with(|l| l.len()),
                size.of(CHANNEL) * crate::settings::font_factor(),
            )
        } else {
            0.0
        }
    });
    // Now in the band (the slide brings it on screen or past the edge).
    let now_x = computed(move || {
        let at = now.get() - pan.band().base;
        let rows = ui.list.with(|l| !l.is_empty());
        (rows && (0..=BAND).contains(&at)).then(|| at as f32 * pan.scale())
    });
    scroll_to_focus_on_new_list(ui, rows);
    view()
        .flex_1()
        .bg(move || ui.surface(ui.theme.with(|t| t.background.window)))
        .child(ruler::ruler(ui, size, column, pan, now_x, width))
        .child(
            wheel::trackball(
                pan,
                pan::panning(view(), pan)
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        virtual_list(
                            move || ui.list.with(|l| l.len()),
                            move |at| row(ui, at, (column, pan, size), focused, number_w),
                        )
                        .estimated_item_size(move || px(size.whole(ROW_H)))
                        .element_ref(&rows)
                        .on_viewable_items_change(move |v: &ViewableItems| {
                            ui.visible_rows.set(v.viewable)
                        })
                        .flex_1(),
                    )
                    .child(
                        // Now, over the programmes only, sliding with them.
                        view()
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .left(move || px(column()))
                            .right(px(0.0))
                            .overflow_hidden()
                            .pointer_events(PointerEvents::None)
                            .child(
                                view()
                                    .absolute()
                                    .top(px(0.0))
                                    .h_full()
                                    .w(px(NOW_W))
                                    .test_id("guide-now-line")
                                    .opacity(
                                        move || if now_x.get().is_some() { 1.0_f32 } else { 0.0 },
                                    )
                                    .left(move || px(now_x.get().unwrap_or(0.0) - NOW_W / 2.0))
                                    .bind_translate_x(&pan.offset())
                                    .bg(move || rgb(ui.theme.with(|t| t.guide().now_line)))
                                    .child(
                                        view()
                                            .absolute()
                                            .left(px(0.0))
                                            .top(px(0.0))
                                            .w(px(NOW_W / 2.0))
                                            .h_full()
                                            .bg(rgb(crate::theme::FOCUSED_TEXT)),
                                    ),
                            ),
                    )
                    .child(empty_state(ui))
                    .child(hint_card(ui)),
            )
            .flex_1(),
        )
}

/// Over an empty list: what it would hold and how to fill it.
fn empty_state(ui: Ui) -> impl IntoElement {
    let empty = move || ui.list.with(|l| l.is_empty());
    let message = move || match ui.list.with(|l| l.entry()) {
        FAVORITES => {
            "No favourite channels yet.\nHold OK (or press Menu) on a channel and choose \
             “Add to favourites”, or star channels with “Manage favourites…”."
        }
        RECENT => "Channels you watch will appear here.",
        _ => "This group has no channels.",
    };
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .px(px(40.0))
        .opacity(move || if empty() { 1.0_f32 } else { 0.0 })
        .pointer_events(PointerEvents::None)
        .child(
            text(message)
                .text_size(px(16.0))
                .text_center()
                .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA)),
        )
}

/// After a group was chosen: what the keys do here, until a key or Back.
fn hint_card(ui: Ui) -> impl IntoElement {
    let shown = move || ui.focus.with(|f| f.hint) && ui.keys_shown();
    view()
        .absolute()
        .right(px(16.0))
        .bottom(px(16.0))
        .px(px(16.0))
        .py(px(12.0))
        .gap(px(4.0))
        .rounded(px(RADIUS))
        .bg(move || tint(ui.theme.with(|t| t.background.alt), 0xF0))
        .opacity(move || if shown() { 1.0_f32 } else { 0.0 })
        .pointer_events(PointerEvents::None)
        .children(hint_lines().map(|line| text(line).text_size(px(13.0)).text_color(rgb(WHITE))))
}

/// What the keys do in the guide: a remote's, or a desktop keyboard's
/// (`iptv_nav::Profile`).
fn hint_lines() -> [&'static str; 4] {
    match crate::input::PROFILE {
        iptv_nav::Profile::Remote => [
            "◀  ·  replays (catch-up), then groups",
            "◀ in the groups  ·  search",
            "Hold ◀  ·  groups      Search  ·  search",
            "Hold OK  ·  menu",
        ],
        iptv_nav::Profile::Keyboard if cfg!(target_os = "macos") => [
            "◀  ·  replays (catch-up), then groups",
            "Tab  ·  groups      / or ⌘F  ·  search",
            "Home  ·  now      Shift ◀ ▶  ·  hours      ⌘ ◀ ▶  ·  days",
            "◀ in the groups  ·  search      m  ·  menu",
        ],
        iptv_nav::Profile::Keyboard => [
            "◀  ·  replays (catch-up), then groups",
            "Tab  ·  groups      / or Ctrl+F  ·  search",
            "Home  ·  now      Shift ◀ ▶  ·  hours      Ctrl ◀ ▶  ·  days",
            "◀ in the groups  ·  search      m  ·  menu",
        ],
    }
}

/// New guide data or a new list shows the focused row (the focus is kept
/// by identity, but the list keeps its scroll offset across a count
/// change).
fn scroll_to_focus_on_new_list(ui: Ui, list: ElementRef) {
    effect(move || {
        ui.guide_data();
        ui.list.with(|_| ());
        let row = ui.focus.with_untracked(|f| f.channel);
        list.scroll_to_index(
            row.saturating_sub(2) as u32,
            percentage(0.0),
            px(0.0),
            false,
        );
    });
}

/// The column's width, the panned timeline and the size scale.
type Geometry = (fn() -> f32, Pan, Scale);

/// An iPad's tap on the guide beside the open groups column (and rail)
/// closes them, as a tap beside a side panel does there, and does nothing
/// else (it plays nothing). Whether they were open. Elsewhere a tap or
/// click keeps acting at once.
pub(crate) fn closed_columns(ui: Ui) -> bool {
    if !crate::IOS || ui.focus.with_untracked(|f| f.pane) == Pane::Guide {
        return false;
    }
    ui.focus.update(|f| f.close_columns());
    true
}

fn row(
    ui: Ui,
    at: Position,
    (column, pan, size): Geometry,
    focused: Memo<Option<(usize, i32)>>,
    number_w: Memo<f32>,
) -> impl IntoElement {
    // The catalog channel behind this row of the list on screen.
    let channel = computed(move || ui.list.with(|l| l.channel(at.index())));
    let programs = computed(move || {
        ui.guide_data();
        let base = pan.band().base;
        channel.get().map_or_else(Vec::new, |ch| {
            crate::schedule().programs_between(ch, base, pan.minutes())
        })
    });
    // The focused programme's start when this row has the focus (a memo
    // per row: a key moving the focus re-runs the cells of the two rows
    // it leaves and enters only, not every row's).
    let focus = computed(move || {
        focused
            .get()
            .and_then(|(row, start)| (row == at.index()).then_some(start))
    });
    // The row with the focus: lighter cells, white titles.
    let lit = computed(move || focus.get().is_some());
    // Now, for the past cells the row cannot replay (a memo: keys move
    // the focus without the cells' dimming running again).
    let now = computed(move || ui.focus.with(|f| f.now));
    let selected = computed(move || {
        ui.guide_data();
        let view = crate::lists::view(ui);
        ui.focus
            .with(|f| (f.channel == at.index()).then(|| f.program(&view).start))
    });
    let focus = RowFocus {
        start: focus,
        lit,
        selected,
    };
    let cell = move |i| cell::cell(ui, (at, i), (programs, channel), focus, now, (pan, size));
    view()
        .test_id("guide-row")
        .h(move || px(size.whole(ROW_H)))
        .flex_row()
        .child(channel::cell(ui, at, channel, (column, size), number_w))
        .child(
            view().flex_1().overflow_hidden().child(
                view()
                    .absolute()
                    .inset_0()
                    .bind_translate_x(&pan.offset())
                    .children((0..17).map(cell))
                    .child(when(
                        move || pan.minutes() > iptv_core::timeline::BAND,
                        move || {
                            view()
                                .absolute()
                                .inset_0()
                                .pointer_events(PointerEvents::BoxNone)
                                .children((17..MAX_CELLS).map(cell))
                        },
                    )),
            ),
        )
}
