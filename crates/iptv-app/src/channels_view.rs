//! The player's channel list (Left over full-screen video; Long Left
//! beside the video shrunk to a preview; the control row's Channels): the
//! list the guide shows (`Ui::list`: a group, All channels, Favourites,
//! Recently watched) numbered as in the guide, with what is on now. Left
//! opens the guide's groups column beside it; moving over its entries
//! changes the guide's list as the guide's own column does
//! (`lists::preview`). State is `iptv_core::channel_list`; this draws it,
//! takes its keys and plays what it picks. The video stays the one
//! persistent layer: preview mode only changes its rectangle
//! ([`preview_rect`]).

use crate::i18n::tr;
use crate::lists::entry_name;

use iptv_core::channel_list::{ChannelList, ListMode, ListPane, Outcome};
use iptv_core::lists::{ChannelList as Rows, Entry, FAVORITES, entry_count};
use iptv_nav::Action;
use nsh_core::prelude::*;

use crate::fonts::{CONDENSED, HEADER_TIMES, MEDIUM, SANS, SECTION, TITLE};
use crate::list_rows::{self, INSET, Mark, ROW, ROW_TWO};
use crate::panels::GROUPS_W;
use crate::scale::Scale;
use crate::theme::BRIGHT_TEXT;
use crate::{Ui, ViewMode, clock, tint};

/// The list's width at full size (scaled with the guide, as is the groups
/// column's, `panels::GROUPS_W`).
const LIST_W: f32 = 460.0;
/// Page Up/Down moves this many rows.
const PAGE: isize = 8;

pub fn is_open(ui: Ui) -> bool {
    ui.channels.with(Option::is_some)
}

/// Opens the list over (or beside) full-screen video.
pub fn open(ui: Ui, mode: ListMode) {
    let mode = if mode == ListMode::Overlay
        && crate::settings::choice(iptv_settings::keys::EnumKey("guide.list_browse_mode"))
            == "preview"
    {
        ListMode::Preview
    } else {
        mode
    };
    if ui.mode.get_untracked() != ViewMode::Fullscreen {
        return;
    }
    ui.panels.update(|p| p.hide());
    let list = ui.list.get_untracked();
    // Picking for multiview: from the active tile's channel.
    let from = crate::multiview::pick_from(ui).or(ui.playing.get_untracked());
    let state = ChannelList::open(mode, from, &list);
    ui.channels.set(Some(state));
}

pub fn close(ui: Ui) {
    ui.channels.set(None);
    crate::multiview::list_closed(ui);
}

/// What OK on a channel compares with: nothing while picking for
/// multiview (every channel is a choice), else the playing channel.
fn playing(ui: Ui) -> Option<usize> {
    if crate::multiview::picking(ui) {
        None
    } else {
        ui.playing.get_untracked()
    }
}

fn groups_shown(ui: Ui) -> bool {
    ui.channels
        .with(|c| c.is_some_and(|c| c.pane == ListPane::Groups))
}

/// The width the list takes at the left.
fn panel_width(ui: Ui) -> f32 {
    let scale = crate::scale::now();
    (LIST_W * scale).round()
        + if groups_shown(ui) {
            (GROUPS_W * scale).round()
        } else {
            0.0
        }
}

/// The video's rectangle while the list shows beside it: right of the
/// list, 16:9, at the top.
pub fn preview_rect(ui: Ui, width: f32, _height: f32) -> Option<(f32, f32, f32, f32)> {
    let preview = ui
        .channels
        .with(|c| c.is_some_and(|c| c.mode == ListMode::Preview));
    preview.then(|| {
        let x = panel_width(ui) + 40.0;
        let w = (width - x - 40.0).max(160.0);
        (x, 40.0, w, w * 9.0 / 16.0)
    })
}

/// Runs a step on the list's state with the guide's list and the groups
/// column's length, then does what it asks.
fn update(ui: Ui, f: impl FnOnce(&mut ChannelList, &Rows, usize) -> Outcome) {
    let list = ui.list.get_untracked();
    let entries = entry_count(&*crate::schedule());
    let mut outcome = Outcome::Stay;
    ui.channels.update(|c| {
        if let Some(c) = c.as_mut() {
            outcome = f(c, &list, entries);
        }
    });
    match outcome {
        Outcome::Stay => {}
        Outcome::Close => close(ui),
        Outcome::Play(channel) if crate::multiview::picking(ui) => {
            crate::multiview::picked(ui, channel);
            close(ui);
        }
        Outcome::Play(channel) => {
            log::info!("iptv channel list: play {channel}");
            crate::lists::show_channel(ui, channel);
            crate::browse_playback::select(ui);
        }
        Outcome::Preview(entry) => {
            crate::lists::preview(ui, entry);
            let list = ui.list.get_untracked();
            ui.channels
                .update(|c| c.iter_mut().for_each(|c| c.shown(&list)));
        }
    }
}

/// The remote while the list shows.
pub fn key(ui: Ui, action: Action) {
    let playing = playing(ui);
    let step = |by: isize| update(ui, move |c, l, n| c.move_focus(by, l, n));
    match action {
        Action::MoveUp => step(-1),
        Action::MoveDown => step(1),
        Action::PageUp => step(-PAGE),
        Action::PageDown => step(PAGE),
        Action::MoveLeft => update(ui, |c, l, _| {
            c.left(l);
            Outcome::Stay
        }),
        Action::MoveRight => update(ui, |c, l, _| {
            c.right(l);
            Outcome::Stay
        }),
        Action::Select => update(ui, |c, l, _| c.select(playing, l)),
        Action::Back => update(ui, |c, _, _| c.back()),
        // Picking for multiview: nothing plays here.
        Action::ChannelUp | Action::ChannelDown | Action::PlayPause
            if crate::multiview::picking(ui) => {}
        Action::ChannelUp | Action::ChannelDown => {
            crate::player_keys::zap(ui, if action == Action::ChannelUp { -1 } else { 1 });
            if let Some(ch) = ui.playing.get_untracked() {
                update(ui, |c, l, _| {
                    c.follow(ch, l);
                    Outcome::Stay
                });
            }
        }
        Action::PlayPause => ui.paused.update(|p| *p = !*p),
        _ => {}
    }
}

/// What an empty list says.
fn empty_text(ui: Ui) -> String {
    match ui.list.with(|l| (l.is_empty(), l.entry())) {
        (false, _) => String::new(),
        (true, FAVORITES) => "No favourites yet".to_owned(),
        (true, _) => "No channels here".to_owned(),
    }
}

pub fn layer(ui: Ui) -> impl IntoElement {
    // The list belongs to full-screen playback and to the data it indexes.
    effect(move || {
        if ui.mode.get() != ViewMode::Fullscreen && untracked(|| is_open(ui)) {
            untracked(|| close(ui));
        }
    });
    let shown = std::cell::Cell::new(None);
    effect(move || {
        let version = ui.data_version.get();
        if shown.replace(Some(version)).is_some_and(|v| v != version) && untracked(|| is_open(ui)) {
            untracked(|| close(ui));
        }
    });
    let open = move || is_open(ui);
    let state = move || ui.channels.get();
    let overlay = move || state().is_some_and(|c| c.mode == ListMode::Overlay);
    let groups = move || groups_shown(ui);
    let row_focus = computed(move || state().map(|c| (c.row, c.pane == ListPane::Channels)));
    let entry_focus = computed(move || state().map(|c| (c.entry, c.pane == ListPane::Groups)));
    let rows = element_ref();
    let entries = element_ref();
    effect(move || {
        if let Some((i, _)) = row_focus.get() {
            rows.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    effect(move || {
        if let Some((i, _)) = entry_focus.get() {
            entries.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let panel = move || {
        let alpha = if overlay() { 0xE0 } else { 0xFF };
        tint(ui.theme.with(|t| t.background.panel), alpha)
    };
    let size = Scale::new();
    let heading = move |s: Box<dyn Fn() -> String>| {
        list_rows::small(size, s, SECTION)
            .px(move || size.px(INSET))
            .py(px(10.0))
            .number_of_lines(1)
    };
    let groups_column = view()
        .test_id("channel-list-groups")
        .w(move || px(if groups() { size.whole(GROUPS_W) } else { 0.0 }))
        .opacity(move || if groups() { 1.0_f32 } else { 0.0 })
        .overflow_hidden()
        .pt_safe_area()
        .py(px(16.0))
        .bg(panel)
        .child(heading(Box::new(|| tr("Groups").to_owned())).px(move || size.px(INSET + 8.0)))
        .child(
            virtual_list(
                move || {
                    ui.data_version.get();
                    entry_count(&*crate::schedule())
                },
                move |at| entry_row(ui, at, entry_focus, size),
            )
            .estimated_item_size(list_rows::pitch(size, ROW))
            .element_ref(&entries)
            .flex_1(),
        );
    let channels_column = view()
        .test_id("channel-list")
        .w(move || px(size.whole(LIST_W)))
        .pt_safe_area()
        .px(px(12.0))
        .py(px(16.0))
        .bg(panel)
        .child(
            heading(Box::new(move || {
                if groups() {
                    crate::lists::list_name(ui)
                } else {
                    format!("◂  {}", crate::lists::list_name(ui))
                }
            }))
            .on_press(move || {
                ui.touched();
                update(ui, |c, l, _| {
                    c.left(l);
                    Outcome::Stay
                });
            }),
        )
        .child(when(
            move || ui.list.with(|l| l.is_empty()),
            move || heading(Box::new(move || empty_text(ui))),
        ))
        .child(
            virtual_list(
                move || ui.list.with(|l| l.len()),
                move |at| channel_row(ui, at, row_focus, size),
            )
            .estimated_item_size(list_rows::pitch(size, ROW_TWO))
            .element_ref(&rows)
            .flex_1(),
        );
    // Mounted while closed: no view in it may say `PointerEvents::Auto`
    // (NSH's pointer events inherit as CSS's, so one would take presses
    // through the closed layer's `None`); in the open layer's `BoxNone`
    // they take presses by default.
    view()
        .absolute()
        .inset_0()
        .flex_row()
        .opacity(move || if open() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if open() {
                PointerEvents::BoxNone
            } else {
                PointerEvents::None
            }
        })
        .child(groups_column)
        .child(channels_column)
        .child(
            // Over the video: a tap closes the list. Beside the preview:
            // the focused channel's programme; a tap on the preview goes
            // back to full screen.
            view()
                .test_id("channel-list-beside")
                .flex_1()
                .on_press(move || {
                    ui.touched();
                    close(ui);
                })
                .child(when(move || !overlay(), move || preview_info(ui, size))),
        )
}

/// An entry of the groups column; the list shown is marked. Cells are
/// recycled: `at` is read where it is used.
fn entry_row(
    ui: Ui,
    at: Position,
    focus: Memo<Option<(usize, bool)>>,
    size: Scale,
) -> impl IntoElement {
    let current = move || ui.list.with(|l| l.entry()) == at.index();
    let focused = move || ui.keys_shown() && focus.get() == Some((at.index(), true));
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
            update(ui, |c, l, _| c.tap_entry(at.index(), l));
        })
        .child(
            list_rows::name(size, move || {
                ui.data_version.get();
                let schedule = crate::schedule();
                let name = entry_name(at.index(), &*schedule);
                if let Entry::Group(g) = Entry::at(at.index())
                    && schedule.group_depth(g) > 0
                {
                    format!("    {name}")
                } else {
                    name
                }
            })
            .text_color(move || mark().text()),
        )
}

/// A row of the list: its number (row + 1, as in the guide), name, the
/// playing marker, and what is on now.
fn channel_row(
    ui: Ui,
    at: Position,
    focus: Memo<Option<(usize, bool)>>,
    size: Scale,
) -> impl IntoElement {
    let entry = move || {
        ui.data_version.get();
        let ch = ui.list.with(|l| l.channel(at.index()))?;
        let s = crate::schedule();
        let now = ui.focus.with(|f| f.now);
        let playing = if ui.playing.get() == Some(ch) {
            "  ▶"
        } else {
            ""
        };
        let p = s.program_at(ch, now);
        Some((
            format!(
                "{}  {}{playing}",
                Rows::number(at.index()),
                s.channel_name(ch)
            ),
            format!("{}  {}", clock(p.start), s.title(ch, p)),
        ))
    };
    let focus = move || focus.get().filter(|f| f.0 == at.index());
    let focused = move || ui.keys_shown() && focus().is_some_and(|f| f.1);
    // While the groups have the focus, the row it returns to stays marked.
    let resting = move || focus().is_some_and(|f| !f.1);
    let is_selected = move || focus().is_some();
    let mark = move || Mark::of(focused(), resting());
    list_rows::row(size, ROW_TWO, mark)
        .role(AccessibilityRole::Row)
        .accessibility_state(move || AccessibilityState {
            selected: is_selected(),
            ..Default::default()
        })
        .on_press(move || {
            ui.touched();
            let playing = playing(ui);
            update(ui, |c, l, _| c.tap_row(at.index(), playing, l));
        })
        .child(
            list_rows::name(size, move || entry().map_or(String::new(), |e| e.0))
                .text_color(move || mark().text()),
        )
        .child(
            // What is on: a programme title, in the grid's type.
            list_rows::second(
                size,
                move || entry().map_or(String::new(), |e| e.1),
                CONDENSED,
            )
            .text_color(move || mark().secondary()),
        )
}

/// Under the preview: the focused channel and its programme now.
/// In the guide header's type.
fn preview_info(ui: Ui, size: Scale) -> impl IntoElement {
    let info = move || {
        ui.data_version.get();
        let row = ui.channels.get()?.row;
        let ch = ui.list.with(|l| l.channel(row))?;
        let s = crate::schedule();
        let now = ui.focus.with(|f| f.now);
        let p = s.program_at(ch, now);
        Some((
            format!("{}  {}", Rows::number(row), s.channel_name(ch)),
            s.title(ch, p),
            format!("{}–{}", clock(p.start), clock(p.end)),
        ))
    };
    let top = move || {
        let w = window();
        preview_rect(ui, w.width.0, w.height.0).map_or(0.0, |r| r.1 + r.3 + 20.0)
    };
    view()
        .pt(move || px(top()))
        .px(px(40.0))
        .gap(px(6.0))
        .pointer_events(PointerEvents::None)
        .child(list_rows::small(
            size,
            move || info().map_or(String::new(), |i| i.0),
            HEADER_TIMES,
        ))
        .child(
            text(move || info().map_or(String::new(), |i| i.1))
                .font_family(SANS)
                .font_weight(MEDIUM)
                .text_size(move || size.text_px(TITLE))
                .text_color(rgb(BRIGHT_TEXT)),
        )
        .child(
            text(move || info().map_or(String::new(), |i| i.2))
                .font_family(SANS)
                .text_size(move || size.text_px(HEADER_TIMES))
                .text_color(rgb(BRIGHT_TEXT)),
        )
}
