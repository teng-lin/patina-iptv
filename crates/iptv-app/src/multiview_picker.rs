//! Search and groups in one channel picker over the multiview session.
//! The portable picker owns selection, filtering and remote focus; the
//! guide's list and the persistent player surfaces stay in place.

use crate::i18n::{tr, trf};
use crate::lists::entry_name;

use iptv_core::channel_picker::{ChannelPicker, Pane};
use iptv_core::guide::Schedule;
use iptv_core::lists::{Entry, ListId, entry_count};
use iptv_guide::combined::CombinedSchedule;
use iptv_nav::Action;
use nsh_base::input;
use nsh_core::prelude::*;
use std::rc::Rc;

use crate::fonts::CONDENSED;
use crate::list_rows::{self, Mark, ROW, ROW_TWO};
use crate::scale::Scale;
use crate::{Ui, clock, ui_kit};

/// An independent browser over the shared channel catalogue. The portable
/// schedule owns playlist expansion; replacing this snapshot never changes
/// the guide's selected category or its persisted expansion preferences.
#[derive(Clone)]
pub struct Screen {
    state: ChannelPicker,
    catalog: Rc<dyn Schedule>,
}

impl PartialEq for Screen {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && Rc::ptr_eq(&self.catalog, &other.catalog)
    }
}

fn catalog(ui: Ui) -> Rc<dyn Schedule> {
    ui.channel_picker
        .with(|p| p.as_ref().map(|p| p.catalog.clone()))
        .unwrap_or_else(crate::schedule)
}

fn combined(catalog: &dyn Schedule) -> Option<&CombinedSchedule> {
    catalog.as_any()?.downcast_ref()
}

pub fn is_open(ui: Ui) -> bool {
    ui.channel_picker.with(Option::is_some)
}

fn state<T>(ui: Ui, f: impl FnOnce(&ChannelPicker) -> T) -> Option<T> {
    ui.channel_picker.with(|p| p.as_ref().map(|p| f(&p.state)))
}

pub fn open(ui: Ui) {
    let catalog = crate::schedule();
    let group = ListId::of(ui.list.with_untracked(|l| l.entry()), &*catalog);
    let channel = crate::multiview::pick_from(ui).and_then(|c| catalog.channel_key(c));
    ui.channel_picker.set(Some(Screen {
        state: ChannelPicker::open(group, channel),
        catalog,
    }));
    refresh(ui);
}

pub fn close(ui: Ui) {
    ui.channel_picker.set(None);
    crate::multiview::list_closed(ui);
}

fn update(ui: Ui, f: impl FnOnce(&mut ChannelPicker)) {
    ui.channel_picker.update(|p| {
        if let Some(p) = p {
            f(&mut p.state);
        }
    });
}

fn refresh(ui: Ui) {
    let catalog = catalog(ui);
    let favorites = crate::favorites::favorite_channels();
    let recent = crate::favorites::recent_channels();
    update(ui, |p| p.refresh(&*catalog, &favorites, &recent));
}

/// A new data generation keeps picker-local expansion by source identity.
/// Channel indices remain those of the shared catalogue, so selecting a row
/// still routes playback to its current source and stream.
fn refresh_catalog(ui: Ui) {
    let current = crate::schedule();
    let previous = catalog(ui);
    let next: Rc<dyn Schedule> = match (combined(&*current), combined(&*previous)) {
        (Some(current), Some(previous)) => {
            Rc::new(current.with_browser_expansion(previous.collapsed_sources()))
        }
        _ => current,
    };
    ui.channel_picker.update(|p| {
        if let Some(p) = p {
            p.catalog = next;
        }
    });
    refresh(ui);
}

fn type_query(ui: Ui, query: &str) {
    if state(ui, |p| p.query() == query) == Some(false) {
        update(ui, |p| p.type_query(query.to_owned()));
        refresh(ui);
    }
}

fn choose(ui: Ui, row: usize) {
    let key = state(ui, |p| p.channels().get(row).map(|c| c.key.clone())).flatten();
    if let Some(channel) = key.and_then(|key| crate::schedule().find_channel(&key)) {
        update(ui, |p| p.focus_row(row));
        crate::multiview::picked(ui, channel);
        close(ui);
    } else {
        refresh_catalog(ui);
    }
}

fn group(ui: Ui, entry: usize, toggle: bool) {
    let catalog = catalog(ui);
    update(ui, |p| p.group(entry, &*catalog));
    if toggle
        && let Entry::Group(g) = Entry::at(entry)
        && let Some(all) = combined(&*catalog)
        && let Some(collapsed) = all.toggle_playlist(g)
    {
        let next = Rc::new(all.with_browser_expansion(collapsed));
        ui.channel_picker.update(|p| {
            if let Some(p) = p {
                p.catalog = next;
            }
        });
    }
    refresh(ui);
}

pub fn key(ui: Ui, action: Action) {
    match action {
        Action::MoveUp | Action::MoveDown | Action::PageUp | Action::PageDown => {
            let by = match action {
                Action::MoveUp => -1,
                Action::MoveDown => 1,
                Action::PageUp => -8,
                _ => 8,
            };
            let catalog = catalog(ui);
            let before = state(ui, |p| p.entry(&*catalog));
            update(ui, |p| p.vertical(by, &*catalog));
            if state(ui, |p| p.entry(&*catalog)) != before {
                refresh(ui);
            }
        }
        Action::MoveLeft | Action::MoveRight => {
            update(ui, |p| p.horizontal(action == Action::MoveRight));
        }
        Action::Select => match state(ui, |p| p.pane) {
            Some(Pane::Search) => update(ui, ChannelPicker::edit),
            Some(Pane::Clear) => type_query(ui, ""),
            Some(Pane::Close) => close(ui),
            Some(Pane::Channels) => {
                if let Some(row) = state(ui, ChannelPicker::row).flatten() {
                    choose(ui, row);
                }
            }
            Some(Pane::Groups) => {
                let catalog = catalog(ui);
                if let Some(entry) = state(ui, |p| p.entry(&*catalog)) {
                    if matches!(Entry::at(entry), Entry::Group(g) if catalog.group_is_playlist(g)) {
                        group(ui, entry, true);
                    } else {
                        update(ui, ChannelPicker::results);
                    }
                }
            }
            None => {}
        },
        Action::Back => {
            let mut close_picker = false;
            update(ui, |p| close_picker = p.back());
            if close_picker {
                close(ui);
            } else {
                refresh(ui);
            }
        }
        Action::OpenSearch => update(ui, ChannelPicker::edit),
        _ => {}
    }
}

fn focused(ui: Ui, pane: Pane) -> bool {
    state(ui, |p| p.pane == pane).unwrap_or(false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    effect(move || {
        ui.data_version.get();
        if untracked(|| is_open(ui)) {
            untracked(|| refresh_catalog(ui));
        }
    });
    effect(move || {
        let picking = ui
            .multiview
            .with(|m| m.as_ref().is_some_and(|m| m.pick().is_some()));
        if !picking && ui.channel_picker.with_untracked(Option::is_some) {
            ui.channel_picker.set(None);
        }
    });
    when(move || is_open(ui), move || screen(ui))
}

fn screen(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let rows = element_ref();
    let groups = element_ref();
    let row = computed(move || state(ui, ChannelPicker::row).flatten());
    let entry = computed(move || {
        ui.data_version.get();
        state(ui, |p| p.entry(&*catalog(ui))).unwrap_or(0)
    });
    // A touch scroll does not move selection. Changing the group or query must
    // reveal it again even when the selected channel still occupies the same
    // row (or a query keeps the exact same results).
    let scroll_target = computed(move || {
        ui.data_version.get();
        let catalog = catalog(ui);
        state(ui, |p| {
            let row = p.row()?;
            Some((
                ListId::of(p.entry(&*catalog), &*catalog),
                p.channels().get(row)?.key.clone(),
                p.query().to_owned(),
                row,
            ))
        })
        .flatten()
    });
    effect(move || {
        if let Some((_, _, _, row)) = scroll_target.get() {
            rows.scroll_to_index(row as u32, percentage(0.5), px(0.0), false);
        }
    });
    effect(move || {
        groups.scroll_to_index(entry.get() as u32, percentage(0.5), px(0.0), false);
    });
    ui_kit::screen(ui, size)
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::ChannelPicker.accessibility_covered(ui)
        })
        .test_id("multiview-picker")
        .child(header(ui, size))
        .child(ui_kit::caption(size, move || {
            if ui.keys_shown() {
                "▲▼ move   ◀▶ groups / channels   OK choose   Back clear search or close"
            } else {
                "Choose a group or search its channels, then tap a channel"
            }
        }))
        .child(
            view().flex_row().flex_1().gap(move || size.px(20.0))
                .child(
                    view().w(move || size.px(240.0)).gap(move || size.px(8.0))
                        .child(ui_kit::heading(size, move || tr("Groups")))
                        .child(
                            virtual_list(
                                move || { ui.data_version.get(); entry_count(&*catalog(ui)) },
                                move |at| group_row(ui, at, size, entry),
                            ).element_ref(&groups).estimated_item_size(list_rows::pitch(size, ROW)).flex_1(),
                        ),
                )
                .child(
                    view().flex_1().gap(move || size.px(8.0))
                        .child(ui_kit::heading(size, move || {
                            ui.data_version.get();
                            let catalog = catalog(ui);
                            state(ui, |p| {
                                let name = entry_name(p.entry(&*catalog), &*catalog);
                                trf(
                                    "{name} · {count, plural, one {# channel} other {# channels}}",
                                    &[("name", (&name).into()), ("count", p.channels().len().into())],
                                )
                            }).unwrap_or_default()
                        }))
                        .child(when(
                            move || state(ui, |p| p.channels().is_empty()).unwrap_or(false),
                            move || ui_kit::body(size, move || {
                                if state(ui, |p| p.query().trim().is_empty()).unwrap_or(true) {
                                    "No channels in this group. Choose another group."
                                } else {
                                    "No matching channels. Clear the search or choose another group."
                                }
                            }),
                        ))
                        .child(
                            virtual_list(
                                move || state(ui, |p| p.channels().len()).unwrap_or(0),
                                move |at| channel_row(ui, at, size, row),
                            ).test_id("multiview-picker-channels").element_ref(&rows).estimated_item_size(list_rows::pitch(size, ROW_TWO)).flex_1(),
                        ),
                ),
        )
}

fn header(ui: Ui, size: Scale) -> impl IntoElement {
    let field = element_ref();
    let editing =
        computed(move || state(ui, |p| p.keyboard && p.pane == Pane::Search).unwrap_or(false));
    effect(move || {
        if editing.get() {
            field.focus();
        } else {
            field.blur();
        }
    });
    let was_up = std::cell::Cell::new(false);
    effect(move || {
        let up = keyboard_height() > px(0.0);
        if was_up.replace(up) && !up {
            update(ui, ChannelPicker::keyboard_hidden);
        }
    });
    view()
        .flex_row()
        .items_center()
        .gap(move || size.px(12.0))
        .child(ui_kit::title(size, move || {
            ui.multiview.with(|m| {
                if m.as_ref().is_some_and(|m| {
                    matches!(m.pick(), Some(iptv_core::multiview::Pick::Replace(_)))
                }) {
                    tr("Change channel")
                } else {
                    tr("Add screen")
                }
            })
        }))
        .child(
            ui_kit::field(size, input())
                .element_ref(&field)
                .flex_1()
                .test_id("multiview-channel-search")
                .value(move || state(ui, |p| p.query().to_owned()).unwrap_or_default())
                .placeholder("Search this group by channel or number")
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .return_key_type(ReturnKeyType::Search)
                .bg(move || ui_kit::field_fill(ui, focused(ui, Pane::Search)))
                .border(move || size.px(2.0))
                .border_color(move || ui_kit::field_border(ui, focused(ui, Pane::Search)))
                .on_change(move |text: &str| type_query(ui, text))
                .on_submit(move |_: &str| update(ui, ChannelPicker::results))
                .on_focus(move || update(ui, ChannelPicker::edit))
                .on_blur(move || update(ui, ChannelPicker::keyboard_hidden)),
        )
        .child(ui_kit::button(
            ui,
            size,
            tr("Clear"),
            move || focused(ui, Pane::Clear),
            move || type_query(ui, ""),
        ))
        .child(ui_kit::button(
            ui,
            size,
            tr("Close"),
            move || focused(ui, Pane::Close),
            move || close(ui),
        ))
}

fn group_name(ui: Ui, entry: usize) -> String {
    let catalog = catalog(ui);
    let name = entry_name(entry, &*catalog);
    match Entry::at(entry) {
        Entry::Group(g) if catalog.group_is_playlist(g) => {
            let arrow = if catalog.group_is_collapsed(g) {
                "▸"
            } else {
                "▾"
            };
            format!("{arrow}  {name}")
        }
        Entry::Group(g) if catalog.group_depth(g) > 0 => format!("    {name}"),
        _ => name,
    }
}

fn group_row(ui: Ui, at: Position, size: Scale, entry: Memo<usize>) -> impl IntoElement {
    let mark = move || {
        Mark::of(
            ui.keys_shown() && focused(ui, Pane::Groups) && entry.get() == at.index(),
            entry.get() == at.index(),
        )
    };
    list_rows::row(size, ROW, mark)
        .on_press(move || {
            ui.touched();
            group(ui, at.index(), true);
        })
        .child(
            list_rows::name(size, move || {
                ui.data_version.get();
                group_name(ui, at.index())
            })
            .text_color(move || mark().text()),
        )
}

fn channel_row(ui: Ui, at: Position, size: Scale, row: Memo<Option<usize>>) -> impl IntoElement {
    let channel = move || state(ui, |p| p.channels().get(at.index()).cloned()).flatten();
    let mark = move || {
        Mark::of(
            ui.keys_shown() && focused(ui, Pane::Channels) && row.get() == Some(at.index()),
            false,
        )
    };
    list_rows::row(size, ROW_TWO, mark)
        .on_press(move || {
            ui.touched();
            choose(ui, at.index());
        })
        .child(
            list_rows::name(size, move || {
                channel().map_or(String::new(), |c| format!("{}  {}", c.number, c.name))
            })
            .text_color(move || mark().text()),
        )
        .child(
            list_rows::second(
                size,
                move || {
                    let Some(channel) = channel() else {
                        return String::new();
                    };
                    let now = ui.focus.with(|f| f.now);
                    let catalog = catalog(ui);
                    let programme = catalog.program_at(channel.index, now);
                    format!(
                        "{}  {}",
                        clock(programme.start),
                        catalog.title(channel.index, programme)
                    )
                },
                CONDENSED,
            )
            .text_color(move || mark().secondary()),
        )
}
