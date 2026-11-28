//! The player's channel list (Left over full-screen video; Long Left
//! beside the video shrunk to a preview; the control row's Channels): the
//! list the guide shows (`Ui::list`: a group, All channels, Favorites,
//! Recently watched) numbered as in the guide, with what is on now. Left
//! opens the guide's groups column beside it; moving over its entries
//! changes the guide's list as the guide's own column does
//! (`lists::preview`). State is `iptv_presentation::channel_list`; this draws it,
//! takes its keys and plays what it picks. The video stays the one
//! persistent layer: preview mode only changes its rectangle
//! ([`preview_rect`]).

use crate::chrome::i18n::tr;
use crate::guide::lists::{browser_entry_name, visible_entries};

use crate::chrome::layout::ListPanel;
use iptv_nav::Action;
use iptv_presentation::channel_list::{ChannelList, ListMode, ListPane, Outcome};
use iptv_presentation::lists::{ALL, ChannelList as Rows, FAVORITES, VisibleEntries};
use nsh_core::prelude::*;

use crate::chrome::fonts::{CONDENSED, HEADER_TIMES, MEDIUM, SANS, SECTION, TITLE};
use crate::chrome::scale::Scale;
use crate::guide::list_rows::{self, INSET, Mark, ROW, ROW_TWO};
use crate::playback::panels::GROUPS_W;
use crate::theme::BRIGHT_TEXT;
use crate::{Ui, ViewMode, clock, tint};

/// The list's width at full size (scaled with the guide, as is the groups
/// column's, `panels::GROUPS_W`).
const LIST_W: f32 = 460.0;
/// Page Up/Down moves this many rows.
const PAGE: isize = 8;

pub fn is_open(playback: crate::PlaybackUi) -> bool {
    playback.channels.with(Option::is_some)
}

/// Opens the list over (or beside) full-screen video.
pub fn open(ui: Ui, mode: ListMode) {
    let mode = if mode == ListMode::Overlay
        && crate::wiring::settings::choice(iptv_settings::keys::GUIDE_LIST_BROWSE_MODE) == "preview"
    {
        ListMode::Preview
    } else {
        mode
    };
    if ui.chrome.mode.get_untracked() != ViewMode::Fullscreen {
        return;
    }
    ui.playback.panels.update(|p| p.hide());
    let list = ui.guide.list.get_untracked();
    // Picking for multiview: from the active tile's channel.
    let from =
        crate::playback::multiview::pick_from(ui.playback).or(ui.playback.playing.get_untracked());
    let state = ChannelList::open(mode, from, &list);
    ui.playback.channels.set(Some(state));
}

pub fn close(ui: crate::PlaybackUi) {
    ui.channels.set(None);
    crate::playback::multiview::list_closed(ui);
}

/// What OK on a channel compares with: nothing while picking for
/// multiview (every channel is a choice), else the playing channel.
fn playing(ui: crate::PlaybackUi) -> Option<usize> {
    if crate::playback::multiview::picking(ui) {
        None
    } else {
        ui.playing.get_untracked()
    }
}

fn groups_shown(playback: crate::PlaybackUi) -> bool {
    playback
        .channels
        .with(|c| c.is_some_and(|c| c.pane == ListPane::Groups))
}

/// The columns at the left in a window `width` points wide, kept inside
/// it (`crate::chrome::layout::ListPanel`).
fn panel(playback: crate::PlaybackUi, width: f32) -> ListPanel {
    let scale = crate::chrome::scale::now();
    let groups = if groups_shown(playback) {
        (GROUPS_W * scale).round()
    } else {
        0.0
    };
    ListPanel::of(width, (LIST_W * scale).round(), groups)
}

/// The columns in the window now (reactive).
fn panel_now(playback: crate::PlaybackUi) -> ListPanel {
    panel(playback, window().width.0)
}

/// The video's rectangle while the list shows beside it: right of the
/// list, 16:9, at the top. `None` in the overlay layout, and in a window
/// with no room beside the list (a phone held upright), where the video
/// stays full screen under the list. In a compact window the layout
/// decides, whatever `guide.list_browse_mode` says (#281): the video goes
/// beside the list wherever it has room.
pub fn preview_rect(
    playback: crate::PlaybackUi,
    width: f32,
    _height: f32,
) -> Option<(f32, f32, f32, f32)> {
    let preview = playback.channels.with(|c| {
        c.is_some_and(|c| {
            c.mode == ListMode::Preview
                || crate::wiring::device::window_class() == crate::chrome::layout::Class::Compact
        })
    });
    if !preview {
        return None;
    }
    panel(playback, width).preview(width)
}

/// Whether the list shows over the video rather than beside it (reactive).
fn over_video(playback: crate::PlaybackUi) -> bool {
    let w = window();
    is_open(playback) && preview_rect(playback, w.width.0, w.height.0).is_none()
}

/// Runs a step on the list's state with the guide's list and the groups
/// column's length, then does what it asks.
fn update(ui: Ui, f: impl FnOnce(&mut ChannelList, &Rows, VisibleEntries) -> Outcome) {
    let list = ui.guide.list.get_untracked();
    let entries = visible_entries(ui.app);
    let mut outcome = Outcome::Stay;
    ui.playback.channels.update(|c| {
        if let Some(c) = c.as_mut() {
            outcome = f(c, &list, entries);
        }
    });
    match outcome {
        Outcome::Stay => {}
        Outcome::Close => close(ui.playback),
        Outcome::Play(channel) if crate::playback::multiview::picking(ui.playback) => {
            if crate::playback::multiview::picked(ui, channel) {
                close(ui.playback);
            }
        }
        Outcome::Play(channel) => {
            log::info!("iptv channel list: play {channel}");
            crate::guide::lists::show_channel(ui, channel);
            crate::playback::browse_playback::select(ui);
        }
        Outcome::Preview(entry) => {
            crate::guide::lists::preview(ui, entry);
            let list = ui.guide.list.get_untracked();
            ui.playback
                .channels
                .update(|c| c.iter_mut().for_each(|c| c.shown(&list)));
        }
    }
}

/// The remote while the list shows.
pub fn key(ui: Ui, action: Action) {
    let playing = playing(ui.playback);
    let step = |by: isize| update(ui, move |c, l, n| c.move_focus(by, l, n));
    match action {
        Action::MoveUp => step(-1),
        Action::MoveDown => step(1),
        Action::PageUp => step(-PAGE),
        Action::PageDown => step(PAGE),
        Action::MoveLeft => update(ui, |c, l, entries| {
            if !entries.is_empty() {
                c.left(l);
            }
            Outcome::Stay
        }),
        Action::MoveRight => update(ui, |c, l, _| {
            c.right(l);
            Outcome::Stay
        }),
        Action::Select => update(ui, |c, l, _| c.select(playing, l)),
        Action::ShowMenu if !crate::playback::multiview::picking(ui.playback) => {
            if let Some(list) = ui.playback.channels.get_untracked() {
                match list.pane {
                    ListPane::Groups => {
                        crate::wiring::navigation::open_group_options(ui, list.entry);
                    }
                    ListPane::Channels => open_channel_menu(ui, list.row),
                }
            }
        }
        Action::Back => back(ui),
        // Picking for multiview: nothing plays here.
        Action::ChannelUp | Action::ChannelDown | Action::PlayPause
            if crate::playback::multiview::picking(ui.playback) => {}
        Action::ChannelUp | Action::ChannelDown => {
            crate::playback::player_keys::zap(ui, if action == Action::ChannelUp { -1 } else { 1 });
            if let Some(ch) = ui.playback.playing.get_untracked() {
                update(ui, |c, l, _| {
                    c.follow(ch, l);
                    Outcome::Stay
                });
            }
        }
        Action::PlayPause => ui.playback.paused.update(|p| *p = !*p),
        _ => {}
    }
}

pub fn back(ui: Ui) {
    update(ui, |state, _, _| state.back());
}

fn open_channel_menu(ui: Ui, row: usize) {
    if !crate::playback::multiview::picking(ui.playback)
        && let Some(channel) = ui.guide.list.with_untracked(|list| list.channel(row))
    {
        batch(|| {
            ui.playback.pending_zap.set(None);
            ui.playback.channels.update(|state| {
                if let Some(state) = state {
                    state.focus_row(row);
                }
            });
            crate::playback::browse_playback::hold(ui);
            crate::wiring::navigation::open_channel_menu(ui, channel);
        });
    }
}

/// What an empty list says.
fn empty_text(guide: crate::GuideUi) -> String {
    match guide.list.with(|l| (l.is_empty(), l.entry())) {
        (false, _) => String::new(),
        (true, FAVORITES) => tr("No favorites yet").to_owned(),
        (true, _) => tr("No channels here").to_owned(),
    }
}

pub fn layer(ui: Ui) -> impl IntoElement {
    // The list belongs to full-screen playback and to the data it indexes.
    effect(move || {
        if ui.chrome.mode.get() != ViewMode::Fullscreen && untracked(|| is_open(ui.playback)) {
            untracked(|| close(ui.playback));
        }
    });
    effect(move || {
        // Settings or a catalog refresh can remove the last group while
        // its column is open. Keep the channel list and its current row.
        if groups_shown(ui.playback) && visible_entries(ui.app).is_empty() {
            untracked(|| back(ui));
        }
    });
    let open = move || is_open(ui.playback);
    let state = move || ui.playback.channels.get();
    // Over the video in the overlay layout, and in the preview layout of a
    // window with no room for the video beside the list.
    let overlay = move || over_video(ui.playback);
    let groups = move || groups_shown(ui.playback);
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
        if let Some((entry, _)) = entry_focus.get()
            && let Some(position) = visible_entries(ui.app).position(entry)
        {
            entries.scroll_to_index(position as u32, percentage(0.5), px(0.0), false);
        }
    });
    let panel = move || {
        let alpha = if overlay() { 0xE0 } else { 0xFF };
        tint(ui.app.theme.with(|t| t.background.panel), alpha)
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
        .accessibility_elements_hidden(move || !groups())
        .w(move || {
            px(if groups() {
                panel_now(ui.playback).groups
            } else {
                0.0
            })
        })
        .opacity(move || if groups() { 1.0_f32 } else { 0.0 })
        .overflow_hidden()
        .pt_safe_area()
        .py(px(16.0))
        .bg(panel)
        .child(heading(Box::new(|| tr("Groups").to_owned())).px(move || size.px(INSET + 8.0)))
        .child(
            virtual_list(
                move || visible_entries(ui.app).len(),
                move |at| entry_row(ui, at, entry_focus, size),
            )
            .estimated_item_size(list_rows::pitch(size, ROW))
            .element_ref(&entries)
            .flex_1(),
        );
    let channels_column = crate::playback::browse_playback::watch_drags(view(), ui)
        .test_id("channel-list")
        .w(move || px(panel_now(ui.playback).list))
        .pt_safe_area()
        .px(px(12.0))
        .py(px(16.0))
        .bg(panel)
        .child(
            heading(Box::new(move || {
                if groups() || visible_entries(ui.app).is_empty() {
                    crate::guide::lists::list_name(ui)
                } else {
                    format!("◂  {}", crate::guide::lists::list_name(ui))
                }
            }))
            .on_press(move || {
                ui.touched();
                update(ui, |c, l, entries| {
                    if !entries.is_empty() {
                        c.left(l);
                    }
                    Outcome::Stay
                });
            }),
        )
        .child(when(
            move || ui.guide.list.with(|l| l.is_empty()),
            move || heading(Box::new(move || empty_text(ui.guide))),
        ))
        .child(
            virtual_list(
                move || ui.guide.list.with(|l| l.len()),
                move |at| channel_row(ui, at, row_focus, size),
            )
            .estimated_item_size(list_rows::pitch(size, ROW_TWO))
            .element_ref(&rows)
            // Scrolling never tunes (`browse_playback::dragged`).
            .on_scroll_drag_start(move |_: &Point<Points>| {
                crate::playback::browse_playback::dragged(ui)
            })
            .flex_1(),
        );
    // Mounted while closed: no view in it may say `PointerEvents::Auto`
    // (NSH's pointer events inherit as CSS's, so one would take presses
    // through the closed layer's `None`); in the open layer's `BoxNone`
    // they take presses by default.
    view()
        .test_id("channel-list-layer")
        .absolute()
        .inset_0()
        .flex_row()
        .accessibility_elements_hidden(move || {
            crate::chrome::overlay::Overlay::ChannelList.accessibility_hidden(ui)
        })
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
            // the focused channel's program; a tap on the preview goes
            // back to full screen.
            view()
                .test_id("channel-list-beside")
                .flex_1()
                .on_press(move || {
                    ui.touched();
                    close(ui.playback);
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
    let visible = move || visible_entries(ui.app).entry(at.index());
    let entry = move || visible().unwrap_or(ALL);
    let current = move || visible() == Some(ui.guide.list.with(|l| l.entry()));
    let focused = move || {
        ui.keys_shown() && visible().is_some_and(|entry| focus.get() == Some((entry, true)))
    };
    let name = move || {
        visible().map_or_else(String::new, |entry| {
            browser_entry_name(entry, &*crate::wiring::runtime_owner::schedule())
        })
    };
    let mark = move || Mark::of(focused(), current());
    list_rows::row(size, ROW, mark)
        .role(AccessibilityRole::Tab)
        .label(move || name().trim_start().to_owned())
        .accessibility_state(move || AccessibilityState {
            selected: current(),
            ..Default::default()
        })
        .mx(px(8.0))
        .on_press(move || {
            ui.touched();
            if let Some(entry) = visible() {
                update(ui, |c, l, _| c.tap_entry(entry, l));
            }
        })
        .on_long_press(move || {
            ui.touched();
            if !crate::playback::multiview::picking(ui.playback)
                && let Some(entry) = visible()
            {
                crate::wiring::navigation::open_group_options(ui, entry);
            }
        })
        .on_context_menu(move |_: &nsh_core::ContextMenuEvent| {
            ui.touched();
            if !crate::playback::multiview::picking(ui.playback)
                && let Some(entry) = visible()
            {
                crate::wiring::navigation::open_group_options(ui, entry);
            }
        })
        .child(
            list_rows::name(size, name)
                .text_color(move || mark().text())
                .mr(move || {
                    px(group_more_room(
                        ui,
                        current() && !crate::playback::multiview::picking(ui.playback),
                        entry(),
                    ))
                }),
        )
        .child(group_more(
            ui,
            size,
            entry,
            current,
            move || !crate::playback::multiview::picking(ui.playback),
            move || name().trim_start().to_owned(),
        ))
}

/// The ⋯ on the shown group's row of the player's channel list, for
/// touch and a pointer: its group options, which
/// long press, right-click, Menu and Shift+F10 open. Only the shown row
/// has one, so the narrow column keeps its names; a click shows another
/// group, which then has it. Keys hide it.
pub(crate) fn group_more(
    ui: Ui,
    size: Scale,
    entry: impl Fn() -> usize + Copy + 'static,
    current: impl Fn() -> bool + Copy + 'static,
    allowed: impl Fn() -> bool + Copy + 'static,
    name: impl Fn() -> String + 'static,
) -> impl IntoElement {
    let name = std::rc::Rc::new(name);
    when(
        move || {
            current()
                && allowed()
                && !ui.keys_shown()
                && crate::wiring::navigation::has_group_options(ui.app, entry())
        },
        move || {
            view()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .right(move || size.px(4.0))
                .justify_center()
                .child(
                    crate::chrome::ui_kit::round_more(
                        || GROUP_MORE,
                        crate::playback::play_bar::back_bg(ui),
                        {
                            let name = name.clone();
                            move || {
                                let name = name();
                                crate::chrome::i18n::trf(
                                    "More options for {name}",
                                    &[("name", name.as_str().into())],
                                )
                            }
                        },
                        move || {
                            ui.touched();
                            crate::wiring::navigation::open_group_options(ui, entry());
                        },
                    )
                    .test_id("group-more"),
                )
        },
    )
}

const GROUP_MORE: f32 = 28.0;

/// The room a group's name leaves at its end for the ⋯ on its row:
/// `shown` when [`group_more`]'s row and `allowed` conditions hold.
pub(crate) fn group_more_room(ui: Ui, shown: bool, entry: usize) -> f32 {
    if shown && !ui.keys_shown() && crate::wiring::navigation::has_group_options(ui.app, entry) {
        GROUP_MORE + 4.0
    } else {
        0.0
    }
}

/// A row of the list: its number (row + 1, as in the guide), name, the
/// playing marker, and what is on now.
fn channel_row(
    ui: Ui,
    at: Position,
    focus: Memo<Option<(usize, bool)>>,
    size: Scale,
) -> impl IntoElement {
    let channel = move || ui.guide.list.with(|l| l.channel(at.index()));
    let entry = move || {
        ui.app.data_version.get();
        let ch = channel()?;
        let s = crate::wiring::runtime_owner::schedule();
        let now = ui.guide.focus.with(|f| f.now);
        let playing = if ui.playback.playing.get() == Some(ch) {
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
            format!(
                "{}  {}",
                clock(p.start),
                crate::guide::listing_labels::listing(s.listing(ch, p))
            ),
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
            let playing = playing(ui.playback);
            update(ui, |c, l, _| c.tap_row(at.index(), playing, l));
        })
        .on_long_press(move || {
            ui.touched();
            open_channel_menu(ui, at.index());
        })
        .on_context_menu(move |_: &nsh_core::ContextMenuEvent| {
            ui.touched();
            open_channel_menu(ui, at.index());
        })
        .flex_row()
        .items_center()
        .child(when(
            move || is_open(ui.playback) && crate::guide::grid::show_channel_logos(ui.app),
            move || {
                image(crate::chrome::content_image::source(move || {
                    crate::guide::grid::channel_logo(ui.app, channel())
                }))
                .test_id("player-channel-logo")
                .accessibility_elements_hidden(true)
                .w(move || size.px(40.0))
                .h(move || size.px(36.0))
                .mr(move || size.px(8.0))
                .flex_shrink_0()
                .object_fit(ResizeMode::Contain)
            },
        ))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .mr(move || px(more_room(ui)))
                .child(
                    list_rows::name(size, move || entry().map_or(String::new(), |e| e.0))
                        .text_color(move || mark().text()),
                )
                .child(
                    list_rows::second(
                        size,
                        move || entry().map_or(String::new(), |e| e.1),
                        CONDENSED,
                    )
                    .text_color(move || mark().secondary()),
                ),
        )
        .child(when(
            move || more_shown(ui) && entry().is_some(),
            move || {
                let name = move || {
                    ui.app.data_version.get();
                    ui.guide
                        .list
                        .with(|l| l.channel(at.index()))
                        .map(crate::channel_name)
                        .unwrap_or_default()
                };
                view()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .right(move || size.px(INSET))
                    .justify_center()
                    .child(
                        crate::chrome::ui_kit::round_more(
                            || MORE,
                            crate::playback::play_bar::back_bg(ui),
                            move || {
                                let channel = name();
                                crate::chrome::i18n::trf(
                                    "More options for {channel}",
                                    &[("channel", channel.as_str().into())],
                                )
                            },
                            move || {
                                ui.touched();
                                open_channel_menu(ui, at.index());
                            },
                        )
                        .test_id("channel-list-more"),
                    )
            },
        ))
}

/// A row's ⋯ for touch and a pointer: the menu a long press, a
/// right-click or the remote's Menu key opens for its channel.
const MORE: f32 = 32.0;

/// Whether rows show their ⋯: while the list is open, for touch and a
/// pointer (keys have Menu), and not while multiview picks a channel,
/// where rows have no menu.
fn more_shown(ui: Ui) -> bool {
    is_open(ui.playback) && !ui.keys_shown() && !crate::playback::multiview::picking(ui.playback)
}

/// The room the row's text leaves at its end for the ⋯.
fn more_room(ui: Ui) -> f32 {
    if more_shown(ui) { MORE + 8.0 } else { 0.0 }
}

/// Under the preview: the focused channel and its program now.
/// In the guide header's type.
fn preview_info(ui: Ui, size: Scale) -> impl IntoElement {
    let info = move || {
        ui.app.data_version.get();
        let row = ui.playback.channels.get()?.row;
        let ch = ui.guide.list.with(|l| l.channel(row))?;
        let s = crate::wiring::runtime_owner::schedule();
        let now = ui.guide.focus.with(|f| f.now);
        let p = s.program_at(ch, now);
        Some((
            format!("{}  {}", Rows::number(row), s.channel_name(ch)),
            crate::guide::listing_labels::listing(s.listing(ch, p)),
            format!("{}–{}", clock(p.start), clock(p.end)),
        ))
    };
    let top = move || {
        let w = window();
        preview_rect(ui.playback, w.width.0, w.height.0).map_or(0.0, |r| r.1 + r.3 + 20.0)
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
