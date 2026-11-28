//! Search and groups in one channel picker over the multiview session.
//! The portable picker owns selection, filtering and remote focus; the
//! guide's list and the persistent player surfaces stay in place.

use crate::chrome::i18n::{tr, trf};
use crate::guide::lists::entry_name;

use iptv_core::guide::Schedule;
use iptv_guide::combined::CombinedSchedule;
use iptv_nav::Action;
use iptv_presentation::channel_picker::{ChannelPicker, Pane};
use iptv_presentation::lists::{Entry, ListId, VisibleEntries};
use nsh_base::input;
use nsh_core::prelude::*;
use std::rc::Rc;

use crate::chrome::fonts::CONDENSED;
use crate::chrome::scale::Scale;
use crate::guide::list_rows::{self, Mark, ROW, ROW_TWO};
use crate::{Ui, chrome::ui_kit, clock};

/// An independent browser over the shared channel catalog. The portable
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

fn catalog(playback: crate::PlaybackUi) -> Rc<dyn Schedule> {
    playback
        .channel_picker
        .with(|p| p.as_ref().map(|p| p.catalog.clone()))
        .unwrap_or_else(crate::schedule)
}

fn visible_entries(ui: Ui) -> VisibleEntries {
    crate::guide::lists::entries_in(ui.app, &*catalog(ui.playback))
}

fn combined(catalog: &dyn Schedule) -> Option<&CombinedSchedule> {
    catalog.as_any()?.downcast_ref()
}

pub fn is_open(playback: crate::PlaybackUi) -> bool {
    playback.channel_picker.with(Option::is_some)
}

fn state<T>(playback: crate::PlaybackUi, f: impl FnOnce(&ChannelPicker) -> T) -> Option<T> {
    playback
        .channel_picker
        .with(|p| p.as_ref().map(|p| f(&p.state)))
}

pub fn open(ui: Ui) {
    let catalog = crate::wiring::runtime_owner::schedule();
    let group = ListId::of(ui.guide.list.with_untracked(|l| l.entry()), &*catalog);
    let channel =
        crate::playback::multiview::pick_from(ui.playback).and_then(|c| catalog.channel_key(c));
    ui.playback.channel_picker.set(Some(Screen {
        state: ChannelPicker::open(group, channel),
        catalog,
    }));
    refresh(ui);
}

pub fn close(ui: crate::PlaybackUi) {
    ui.channel_picker.set(None);
    crate::playback::multiview::list_closed(ui);
}

fn update(playback: crate::PlaybackUi, f: impl FnOnce(&mut ChannelPicker)) {
    playback.channel_picker.update(|p| {
        if let Some(p) = p {
            f(&mut p.state);
        }
    });
}

fn refresh(ui: Ui) {
    let catalog = catalog(ui.playback);
    let favorites = crate::guide::favorites::favorite_channels();
    let recent = crate::guide::favorites::recent_channels();
    let entries = crate::guide::lists::entries_in(ui.app, &*catalog);
    update(ui.playback, |p| {
        p.refresh(&*catalog, &favorites, &recent, entries)
    });
}

/// A new data generation keeps picker-local expansion by source identity.
/// Channel indices remain those of the shared catalog, so selecting a row
/// still routes playback to its current source and stream.
fn refresh_catalog(ui: Ui) {
    let current = crate::wiring::runtime_owner::schedule();
    let previous = catalog(ui.playback);
    let next: Rc<dyn Schedule> = match (combined(&*current), combined(&*previous)) {
        (Some(current), Some(previous)) => {
            Rc::new(current.with_browser_expansion(previous.collapsed_sources()))
        }
        _ => current,
    };
    ui.playback.channel_picker.update(|p| {
        if let Some(p) = p {
            p.catalog = next;
        }
    });
    refresh(ui);
}

fn type_query(ui: Ui, query: &str) {
    if state(ui.playback, |p| p.query() == query) == Some(false) {
        update(ui.playback, |p| p.type_query(query.to_owned()));
        refresh(ui);
    }
}

fn choose(ui: Ui, row: usize) {
    let key = state(ui.playback, |p| {
        p.channels().get(row).map(|c| c.key.clone())
    })
    .flatten();
    if let Some(channel) =
        key.and_then(|key| crate::wiring::runtime_owner::schedule().find_channel(&key))
    {
        update(ui.playback, |p| p.focus_row(row));
        if crate::playback::multiview::picked(ui, channel) {
            close(ui.playback);
        }
    } else {
        refresh_catalog(ui);
    }
}

fn group(ui: Ui, entry: usize, toggle: bool) {
    let catalog = catalog(ui.playback);
    update(ui.playback, |p| p.group(entry, &*catalog));
    if toggle
        && let Entry::Group(g) = Entry::at(entry)
        && let Some(all) = combined(&*catalog)
        && let Some(collapsed) = all.toggle_playlist(g)
    {
        let next = Rc::new(all.with_browser_expansion(collapsed));
        ui.playback.channel_picker.update(|p| {
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
            let catalog = catalog(ui.playback);
            let before = state(ui.playback, |p| p.entry(&*catalog));
            let entries = crate::guide::lists::entries_in(ui.app, &*catalog);
            update(ui.playback, |p| p.vertical(by, &*catalog, entries));
            if state(ui.playback, |p| p.entry(&*catalog)) != before {
                refresh(ui);
            }
        }
        Action::MoveLeft | Action::MoveRight => {
            let entries = visible_entries(ui);
            update(ui.playback, |p| {
                p.horizontal(action == Action::MoveRight, entries)
            });
        }
        Action::Select => match state(ui.playback, |p| p.pane) {
            Some(Pane::Search) => update(ui.playback, ChannelPicker::edit),
            Some(Pane::Clear) => type_query(ui, ""),
            Some(Pane::Close) => close(ui.playback),
            Some(Pane::Channels) => {
                if let Some(row) = state(ui.playback, ChannelPicker::row).flatten() {
                    choose(ui, row);
                }
            }
            Some(Pane::Groups) => {
                let catalog = catalog(ui.playback);
                if let Some(entry) = state(ui.playback, |p| p.entry(&*catalog)) {
                    if matches!(Entry::at(entry), Entry::Group(g) if catalog.group_is_playlist(g)) {
                        group(ui, entry, true);
                    } else {
                        update(ui.playback, ChannelPicker::results);
                    }
                }
            }
            None => {}
        },
        Action::Back => {
            let mut close_picker = false;
            update(ui.playback, |p| close_picker = p.back());
            if close_picker {
                close(ui.playback);
            } else {
                refresh(ui);
            }
        }
        Action::OpenSearch => update(ui.playback, ChannelPicker::edit),
        _ => {}
    }
}

fn focused(playback: crate::PlaybackUi, pane: Pane) -> bool {
    state(playback, |p| p.pane == pane).unwrap_or(false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    effect(move || {
        ui.app.data_version.get();
        if untracked(|| is_open(ui.playback)) {
            untracked(|| refresh_catalog(ui));
        }
    });
    let categories = computed(move || {
        crate::guide::lists::category_preferences(&crate::wiring::settings_wiring::read(ui.app))
    });
    effect(move || {
        categories.get();
        untracked(|| {
            if is_open(ui.playback) {
                refresh(ui);
            }
        });
    });
    effect(move || {
        let picking = ui
            .playback
            .multiview
            .with(|m| m.as_ref().is_some_and(|m| m.pick().is_some()));
        if !picking && ui.playback.channel_picker.with_untracked(Option::is_some) {
            ui.playback.channel_picker.set(None);
        }
    });
    when(move || is_open(ui.playback), move || screen(ui))
}

fn screen(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let rows = element_ref();
    let groups = element_ref();
    let row = computed(move || state(ui.playback, ChannelPicker::row).flatten());
    let entry = computed(move || {
        ui.app.data_version.get();
        state(ui.playback, |p| p.entry(&*catalog(ui.playback))).unwrap_or(0)
    });
    // A touch scroll does not move selection. Changing the group or query must
    // reveal it again even when the selected channel still occupies the same
    // row (or a query keeps the exact same results).
    let scroll_target = computed(move || {
        ui.app.data_version.get();
        let catalog = catalog(ui.playback);
        state(ui.playback, |p| {
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
        if let Some(position) = visible_entries(ui).position(entry.get()) {
            groups.scroll_to_index(position as u32, percentage(0.5), px(0.0), false);
        }
    });
    ui_kit::screen(ui, size)
        .accessibility_elements_hidden(move || {
            crate::chrome::overlay::Overlay::ChannelPicker.accessibility_covered(ui)
        })
        .test_id("multiview-picker")
        .child(header(ui, size))
        .child(ui_kit::caption(size, move || {
            if ui.keys_shown() {
                tr("▲▼ move   ◀▶ groups / channels   OK choose   Back clear search or close")
            } else {
                tr("Choose a group or search its channels, then tap a channel")
            }
        }))
        .child(
            view().flex_row().flex_1().gap(move || size.px(20.0))
                .child(
                    view().w(move || size.px(240.0)).gap(move || size.px(8.0))
                        .child(ui_kit::heading(size, move || tr("Groups")))
                        .child(
                            virtual_list(
                                move || visible_entries(ui).len(),
                                move |at| group_row(ui, at, size, entry),
                            ).element_ref(&groups).estimated_item_size(list_rows::pitch(size, ROW)).flex_1(),
                        ),
                )
                .child(
                    view().flex_1().gap(move || size.px(8.0))
                        .child(ui_kit::heading(size, move || {
                            ui.app.data_version.get();
                            let catalog = catalog(ui.playback);
                            state(ui.playback, |p| {
                                let name = entry_name(p.entry(&*catalog), &*catalog);
                                trf(
                                    "{name} · {count, plural, one {# channel} other {# channels}}",
                                    &[("name", (&name).into()), ("count", p.channels().len().into())],
                                )
                            }).unwrap_or_default()
                        }))
                        .child(when(
                            move || state(ui.playback, |p| p.channels().is_empty()).unwrap_or(false),
                            move || ui_kit::body(size, move || {
                                if state(ui.playback, |p| p.query().trim().is_empty()).unwrap_or(true) {
                                    tr("No channels in this group. Choose another group.")
                                } else {
                                    tr("No matching channels. Clear the search or choose another group.")
                                }
                            }),
                        ))
                        .child(
                            virtual_list(
                                move || state(ui.playback, |p| p.channels().len()).unwrap_or(0),
                                move |at| channel_row(ui, at, size, row),
                            ).test_id("multiview-picker-channels").element_ref(&rows).estimated_item_size(list_rows::pitch(size, ROW_TWO)).flex_1(),
                        ),
                ),
        )
}

fn header(ui: Ui, size: Scale) -> impl IntoElement {
    let field = element_ref();
    let editing = computed(move || {
        state(ui.playback, |p| p.keyboard && p.pane == Pane::Search).unwrap_or(false)
    });
    crate::chrome::input_kit::bind_field_focus(field, move || editing.get());
    crate::chrome::keyboard::on_dismissed(
        move || editing.get(),
        move || update(ui.playback, ChannelPicker::keyboard_hidden),
    );
    view()
        .flex_row()
        .items_center()
        .gap(move || size.px(12.0))
        .child(ui_kit::title(size, move || {
            ui.playback.multiview.with(|m| {
                if m.as_ref().is_some_and(|m| {
                    matches!(
                        m.pick(),
                        Some(iptv_presentation::multiview::Pick::Replace(_))
                    )
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
                .value(move || state(ui.playback, |p| p.query().to_owned()).unwrap_or_default())
                .placeholder(move || tr("Search this group by channel or number"))
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .return_key_type(ReturnKeyType::Search)
                .bg(move || ui_kit::field_fill(ui, focused(ui.playback, Pane::Search)))
                .border(move || size.px(2.0))
                .border_color(move || ui_kit::field_border(ui, focused(ui.playback, Pane::Search)))
                .on_change(move |text: &str| type_query(ui, text))
                .on_submit(move |_: &str| update(ui.playback, ChannelPicker::results))
                .on_focus(move || update(ui.playback, ChannelPicker::edit))
                .on_blur(move || update(ui.playback, ChannelPicker::keyboard_hidden)),
        )
        .child(ui_kit::button(
            ui,
            size,
            tr("Clear"),
            move || focused(ui.playback, Pane::Clear),
            move || type_query(ui, ""),
        ))
        .child(ui_kit::button(
            ui,
            size,
            tr("Close"),
            move || focused(ui.playback, Pane::Close),
            move || close(ui.playback),
        ))
}

fn group_row(ui: Ui, at: Position, size: Scale, entry: Memo<usize>) -> impl IntoElement {
    let visible = move || visible_entries(ui).entry(at.index());
    let selected = move || visible() == Some(entry.get());
    let mark = move || {
        Mark::of(
            ui.keys_shown() && focused(ui.playback, Pane::Groups) && selected(),
            selected(),
        )
    };
    let name = move || {
        visible().map_or_else(String::new, |entry| {
            crate::guide::lists::browser_entry_name(entry, &*catalog(ui.playback))
        })
    };
    let expanded =
        move || crate::guide::lists::playlist_expanded(visible()?, &*catalog(ui.playback));
    list_rows::row(size, ROW, mark)
        .flex_row()
        .items_center()
        .gap(move || size.px(8.0))
        .role(AccessibilityRole::Tab)
        .label(move || name().trim_start().to_owned())
        .accessibility_state(move || AccessibilityState {
            selected: selected(),
            expanded: expanded(),
            ..Default::default()
        })
        .on_press(move || {
            ui.touched();
            if let Some(entry) = visible() {
                group(ui, entry, true);
            }
        })
        .child(
            list_rows::name(size, name)
                .flex_1()
                .text_color(move || mark().text()),
        )
        .child(when(
            move || expanded().is_some(),
            move || {
                list_rows::triangle(
                    size,
                    move || crate::guide::lists::playlist_arrow(expanded()),
                )
                .test_id("playlist-disclosure")
                .flex_shrink_0()
                .text_color(move || mark().text())
            },
        ))
}

fn channel_row(ui: Ui, at: Position, size: Scale, row: Memo<Option<usize>>) -> impl IntoElement {
    let channel = move || state(ui.playback, |p| p.channels().get(at.index()).cloned()).flatten();
    let mark = move || {
        Mark::of(
            ui.keys_shown()
                && focused(ui.playback, Pane::Channels)
                && row.get() == Some(at.index()),
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
                    let now = ui.guide.focus.with(|f| f.now);
                    let catalog = catalog(ui.playback);
                    let program = catalog.program_at(channel.index, now);
                    format!(
                        "{}  {}",
                        clock(program.start),
                        crate::guide::listing_labels::listing(
                            catalog.listing(channel.index, program)
                        )
                    )
                },
                CONDENSED,
            )
            .text_color(move || mark().secondary()),
        )
}
