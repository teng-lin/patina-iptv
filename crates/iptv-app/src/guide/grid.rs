//! The guide grid: a time ruler, one virtualized row per channel of the list
//! on screen (`lists.rs`) with its programs across the full remaining
//! width, and a now-line. Rows are numbered by their place in the list;
//! `at.index()` is a row, the catalog channel comes from the list.
//!
//! The look: programs are rounded cells on the window color; the focused
//! program is a light pill with dark text, the rest of its row lighter
//! cells with white titles, every other row dimmed (gray titles). Sizes are
//! dp at full size, the same on TV, tablet and a full-screen desktop; a
//! smaller desktop window scales them (`scale.rs`; `fonts.rs` has the type).
//!
//! The timeline pans by touch and mouse drags (`pan.rs`): the rows hold
//! the programs of a band a little wider than the window and slide them,
//! with the ruler's times and the now line, by one animated offset.

use crate::chrome::i18n::tr;
use crate::library_ui::organization_view;

mod cell;
mod channel;
use cell::RowFocus;
pub(crate) use channel::{channel_logo, no_logo, show_channel_logos};
pub(crate) mod keys;
mod pan;
mod ruler;
mod wheel;

const BAND: i32 = 360 + 2 * crate::guide::timeline::MARGIN;
use pan::Pan;

use iptv_presentation::lists::{FAVORITES, RECENT};
use nsh_core::prelude::*;

use crate::chrome::fonts::CHANNEL;
use crate::chrome::scale::Scale;
use crate::guide::Pane;
use crate::theme::{RADIUS, TEXT_SECONDARY_ALPHA};
use crate::{Ui, ViewMode, tint};

/// A program's height (the rest is the gap between rows), the
/// gap between programs, their corner radius and text inset (full size:
/// [`Scale`] scales them, the pitch and height in whole px).
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
/// A past program the row cannot replay (no catch-up, or before its
/// archive): shown when the view is panned back for rows that can, dimmed
/// to this opacity, and never focused by the keys (`guide::floor`).
const PAST_DIM: f32 = 0.4;

/// Cells a row can hold: about 12 per window of programs, across the
/// band.
const MAX_CELLS: usize = 40;
const WHITE: u32 = 0xFFFFFF;

/// The channel column's width now (reactive: the window's size).
pub fn channel_width() -> f32 {
    let scale = crate::chrome::scale::now();
    (window().width.0 * CHANNEL_SHARE).clamp(CHANNEL_MIN * scale, CHANNEL_MAX * scale)
}

/// An opaque guide color; over full-screen video (overlay) at the
/// overlay's alpha (`appearance.ui_transparency`).
pub(crate) fn fill(ui: Ui, rgb_hex: u32) -> Rgba {
    let alpha = match ui.chrome.mode.get() {
        ViewMode::Overlay => {
            crate::theme::overlay_alpha(&crate::wiring::settings_wiring::read(ui.app))
        }
        ViewMode::Guide | ViewMode::Fullscreen => 0xFF,
    };
    tint(rgb_hex, alpha)
}

pub fn guide(ui: Ui, rows: ElementRef) -> impl IntoElement {
    // The program area's width: the window always spans all of it.
    let width = signal(0.0_f32);
    let size = Scale::new();
    let pan = Pan::new(ui, width);
    let drag = organization_view::reorder_drag::DragList::new(ui, size, rows).guide(channel_width);
    let now = computed(move || ui.guide.focus.with(|f| f.now));
    let order_scope = computed(move || organization_view::guide_order_scope(ui.screens));
    let order_keys = computed(move || {
        order_scope.with(|scope| {
            scope
                .as_ref()
                .map(|(source, list)| organization_view::guide_order_keys(ui, *source, list))
        })
    });
    effect(move || {
        if let Some(keys) = order_keys.get() {
            untracked(|| organization_view::invalidate_changed_guide_order(ui, &keys));
        }
    });
    let order = computed(move || {
        order_keys.with(|keys| organization_view::guide_order(ui.screens, keys.as_ref()?))
    });
    let order_focus = computed(move || organization_view::guide_order_focus(ui.screens));
    let count = computed(move || {
        order.with(|order| {
            order
                .as_ref()
                .map_or_else(|| ui.guide.list.with(|l| l.len()), Vec::len)
        })
    });
    let focused = computed(move || {
        if order.with(Option::is_some) {
            return None;
        }
        ui.guide_data();
        let view = crate::guide::lists::view(ui.guide);
        ui.guide
            .focus
            .with(|f| (f.pane == Pane::Guide).then(|| (f.channel, f.program(&view).start)))
    });
    // Read where used: a memo of it went stale on window resizes.
    let column: fn() -> f32 = channel_width;
    // Rows are numbered by place: the list's length is its widest number.
    let number_w = computed(move || {
        crate::wiring::settings_wiring::read(ui.app);
        if crate::wiring::settings::flag(iptv_settings::keys::GUIDE_SHOW_CHANNEL_NUMBERS) {
            crate::guide::numbers::column_width(
                ui.guide.list.with(|l| l.len()),
                size.of(CHANNEL) * crate::wiring::settings::font_factor(),
            )
        } else {
            0.0
        }
    });
    // Now in the band (the slide brings it on screen or past the edge).
    let now_x = computed(move || {
        let at = now.get() - pan.band().base;
        let rows = ui.guide.list.with(|l| !l.is_empty());
        (rows && (0..=BAND).contains(&at)).then(|| at as f32 * pan.scale())
    });
    scroll_to_focus_on_new_list(ui, rows, order_focus);
    let was_reordering = std::cell::Cell::new(false);
    effect(move || {
        let draft = order_focus.get();
        let was_reordering = was_reordering.replace(draft.is_some());
        if let Some((row, _)) = draft {
            if !untracked(|| drag.active()) {
                rows.scroll_to_index(row as u32, percentage(0.5), px(0.0), false);
            }
        } else if was_reordering {
            scroll_to_focused_row(ui.guide, rows);
        }
    });
    let list_body = pan::panning(view(), pan)
        .flex_1()
        .overflow_hidden()
        .child(
            drag.with_grips(
                view().flex_1().overflow_hidden().child(
                    virtual_list(
                        move || count.get(),
                        move |at| {
                            let channel = computed(move || {
                                order.with(|order| match order {
                                    Some(channels) => channels.get(at.index()).copied(),
                                    None => ui.guide.list.with(|l| l.channel(at.index())),
                                })
                            });
                            let editing = computed(move || {
                                order_focus.get().and_then(|(row, selected)| {
                                    (row == at.index()).then_some(selected)
                                })
                            });
                            row(
                                ui,
                                at,
                                (column, pan, size),
                                focused,
                                number_w,
                                (channel, editing),
                            )
                        },
                    )
                    .estimated_item_size(move || px(size.whole(crate::guide::list_rows::ROW)))
                    .element_ref(&rows)
                    .on_scroll(move |event: &ScrollEvent| drag.scrolled(*event))
                    // Scrolling the channels never tunes: a preview
                    // waiting to start stops (`browse_playback`).
                    .on_scroll_drag_start(move |_: &Point<Points>| {
                        crate::playback::browse_playback::dragged(ui)
                    })
                    .on_viewable_items_change(move |v: &ViewableItems| {
                        if !organization_view::guide_order_open(ui.screens) {
                            ui.guide.visible_rows.set(v.viewable);
                        }
                    })
                    .flex_1(),
                ),
            ),
        )
        .child(
            // Now, over the programs only, sliding with them.
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
                        .opacity(move || if now_x.get().is_some() { 1.0_f32 } else { 0.0 })
                        .left(move || px(now_x.get().unwrap_or(0.0) - NOW_W / 2.0))
                        .bind_translate_x(&pan.offset())
                        .bg(move || rgb(ui.app.theme.with(|t| t.guide().now_line)))
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
        .child(empty_state(ui, size))
        .child(hint_card(ui));
    view()
        .flex_1()
        .bg(move || ui.surface(ui.app.theme.with(|t| t.background.window)))
        .child(ruler::ruler(ui, size, column, pan, now_x, width))
        .child(if crate::host::is_tui_mode() {
            // `nsh-platform-tui` does not implement `StickyCells`, which
            // `wheel::trackball` relies on to pin the grid inside its
            // 20,000 pt horizontal scroll buffer.
            list_body.into_element()
        } else {
            wheel::trackball(pan, list_body).flex_1().into_element()
        })
}

/// Over an empty list: what it would hold and how to fill it.
fn empty_state(ui: Ui, size: Scale) -> impl IntoElement {
    let empty = move || ui.guide.list.with(|l| l.is_empty());
    let settings = computed(move || crate::wiring::navigation::empty_group_settings_available(ui));
    let message = move || match ui.guide.list.with(|l| l.entry()) {
        FAVORITES => tr(
            "No favorite channels yet. Hold OK (or press Menu) on a channel and choose “Add to favorites”, or star channels with “Manage favorites…”.",
        ),
        RECENT => tr("Channels you watch will appear here."),
        _ => tr("This group has no channels."),
    };
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .px(px(40.0))
        .gap(px(12.0))
        .accessibility_elements_hidden(move || !empty())
        .opacity(move || if empty() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if settings.get() {
                PointerEvents::BoxNone
            } else {
                PointerEvents::None
            }
        })
        .child(
            text(message)
                .text_size(px(16.0))
                .text_center()
                .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA)),
        )
        .child(when(
            move || settings.get(),
            move || {
                crate::chrome::ui_kit::button(
                    ui,
                    size,
                    move || tr("Group settings"),
                    move || ui.guide.focus.with(|f| f.pane == Pane::Guide),
                    move || {
                        crate::wiring::navigation::open_empty_group_settings(ui);
                    },
                )
            },
        ))
}

/// After a group was chosen: what the keys do here, until a key or Back.
fn hint_card(ui: Ui) -> impl IntoElement {
    let shown = move || ui.guide.focus.with(|f| f.hint) && ui.keys_shown();
    view()
        .absolute()
        .right(px(16.0))
        .bottom(px(16.0))
        .px(px(16.0))
        .py(px(12.0))
        .gap(px(4.0))
        .rounded(px(RADIUS))
        .bg(move || tint(ui.app.theme.with(|t| t.background.alt), 0xF0))
        .accessibility_elements_hidden(move || !shown())
        .opacity(move || if shown() { 1.0_f32 } else { 0.0 })
        .pointer_events(PointerEvents::None)
        .children((0..4).map(|i| {
            text(move || hint_lines()[i])
                .text_size(px(13.0))
                .text_color(rgb(WHITE))
        }))
}

/// What the keys do in the guide: a remote's, or a desktop keyboard's
/// (`iptv_nav::Profile`).
fn hint_lines() -> [&'static str; 4] {
    match crate::chrome::input_kit::PROFILE {
        iptv_nav::Profile::Remote => [
            tr("◀  ·  earlier programs, then groups"),
            tr("◀ in the groups  ·  search"),
            tr("Hold ◀  ·  groups      Search  ·  search"),
            tr("Hold OK  ·  menu"),
        ],
        iptv_nav::Profile::Keyboard if crate::host::MACOS => [
            tr("◀  ·  earlier programs, then groups"),
            tr("Tab  ·  groups      / or ⌘F  ·  search"),
            tr("Home  ·  now      Shift ◀ ▶  ·  hours      ⌘ ◀ ▶  ·  days"),
            tr("◀ in the groups  ·  search      m  ·  menu"),
        ],
        iptv_nav::Profile::Keyboard => [
            tr("◀  ·  earlier programs, then groups"),
            tr("Tab  ·  groups      / or Ctrl+F  ·  search"),
            tr("Home  ·  now      Shift ◀ ▶  ·  hours      Ctrl ◀ ▶  ·  days"),
            tr("◀ in the groups  ·  search      m  ·  menu"),
        ],
    }
}

/// A new schedule or a new list shows the focused row (the focus is kept
/// by identity, but the list keeps its scroll offset across a count
/// change). A day of listings paged in (a timeline pan reaching it) keeps
/// the rows where a finger scrolled them (#88).
fn scroll_to_focus_on_new_list(ui: Ui, list: ElementRef, order_focus: Memo<Option<(usize, bool)>>) {
    effect(move || {
        ui.app.data_version.get();
        ui.guide.list.with(|_| ());
        if order_focus.get_untracked().is_none() {
            scroll_to_focused_row(ui.guide, list);
        }
    });
}

fn scroll_to_focused_row(guide: crate::GuideUi, list: ElementRef) {
    let row = guide.focus.with_untracked(|f| f.channel);
    list.scroll_to_index(
        row.saturating_sub(2) as u32,
        percentage(0.0),
        px(0.0),
        false,
    );
}

/// The column's width, the panned timeline and the size scale.
type Geometry = (fn() -> f32, Pan, Scale);

/// An iPad's tap on the guide beside the open groups column (and rail)
/// closes them, as a tap beside a side panel does there, and does nothing
/// else (it plays nothing). Whether they were open. Elsewhere a tap or
/// click keeps acting at once.
pub(crate) fn closed_columns(ui: Ui) -> bool {
    if !crate::IOS || ui.guide.focus.with_untracked(|f| f.pane) == Pane::Guide {
        return false;
    }
    ui.guide.focus.update(|f| f.close_columns());
    true
}

fn row(
    ui: Ui,
    at: Position,
    (column, pan, size): Geometry,
    focused: Memo<Option<(usize, i32)>>,
    number_w: Memo<f32>,
    (channel, editing): (Memo<Option<usize>>, Memo<Option<bool>>),
) -> impl IntoElement {
    let programs = computed(move || {
        ui.guide_data();
        let base = pan.band().base;
        channel.get().map_or_else(Vec::new, |ch| {
            crate::wiring::runtime_owner::schedule().programs_between(ch, base, pan.minutes())
        })
    });
    // The focused program's start when this row has the focus (a memo
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
    let now = computed(move || ui.guide.focus.with(|f| f.now));
    let selected = computed(move || {
        if organization_view::guide_order_open(ui.screens) {
            return None;
        }
        ui.guide_data();
        let view = crate::guide::lists::view(ui.guide);
        ui.guide
            .focus
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
        .h(move || px(size.whole(crate::guide::list_rows::ROW)))
        .flex_row()
        .child(channel::cell(
            ui,
            at,
            channel,
            (column, size),
            number_w,
            editing,
        ))
        .child(
            view()
                .flex_1()
                .overflow_hidden()
                .accessibility_elements_hidden(move || {
                    organization_view::guide_order_open(ui.screens)
                })
                .pointer_events(move || {
                    if organization_view::guide_order_open(ui.screens) {
                        PointerEvents::None
                    } else {
                        PointerEvents::Auto
                    }
                })
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .bind_translate_x(&pan.offset())
                        .children((0..17).map(cell))
                        .child(when(
                            move || pan.minutes() > crate::guide::timeline::BAND,
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
        .child(organization_view::order_outline(ui, move || {
            editing.get() == Some(true)
        }))
}
