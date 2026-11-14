//! The favourites editor overlay for the list on screen (from the menu's
//! "Manage favourites…"): a star per channel, OK toggles, Left removes the
//! whole list, Right adds it, Back leaves; persistent hints and, for touch,
//! a tap per row plus "Add all" / "Remove all" buttons. Changes are saved
//! by channel key (`favorites.rs`) at once.

use crate::i18n::{tr, trf};

use iptv_core::editor::FavoritesEditor;
use nsh_core::Checked;
use nsh_core::prelude::*;

use crate::Ui;
use crate::fonts::{CAPTION, LIST_ITEM, SANS, TITLE};
use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;
use crate::theme::LIST_TEXT;

/// Opens the editor on the list on screen, at the focused channel.
pub fn open(ui: Ui) {
    let (group, channels) = ui.list.with(|l| (l.entry(), l.channels().to_vec()));
    let mut editor = FavoritesEditor::new(group, channels);
    editor.focus = ui
        .focus
        .with(|f| f.channel)
        .min(editor.len().saturating_sub(1));
    ui.editor.set(Some(editor));
}

pub fn close(ui: Ui) {
    ui.editor.set(None);
}

pub fn move_focus(ui: Ui, step: isize) {
    ui.editor
        .update(|e| e.iter_mut().for_each(|e| e.move_focus(step)));
}

fn with_favorites(
    ui: Ui,
    f: impl FnOnce(&FavoritesEditor, &mut std::collections::BTreeSet<usize>),
) {
    if let Some(editor) = ui.editor.get() {
        crate::favorites::edit(ui, |fav| f(&editor, fav));
    }
}

pub fn toggle(ui: Ui) {
    with_favorites(ui, |e, fav| e.toggle(fav));
}

pub fn clear_group(ui: Ui) {
    with_favorites(ui, |e, fav| e.clear_group(fav));
}

pub fn set_group(ui: Ui) {
    with_favorites(ui, |e, fav| e.set_group(fav));
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let open = move || ui.editor.with(Option::is_some);
    let size = Scale::new();
    let list = element_ref();
    let focus = computed(move || ui.editor.with(|e| e.as_ref().map(|e| e.focus)));
    effect(move || {
        if let Some(i) = focus.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let count = move || {
        ui.editor
            .with(|e| e.as_ref().map_or(0, FavoritesEditor::len))
    };
    let title = move || {
        ui.editor.with(|e| {
            e.as_ref().map_or(String::new(), |e| {
                let starred = ui.favorites.with(|f| e.starred(f));
                let group = crate::lists::entry_name(e.group, &*crate::schedule());
                trf(
                    "Favourites · {group} · {starred} of {total}",
                    &[
                        ("group", (&group).into()),
                        ("starred", starred.into()),
                        ("total", e.len().into()),
                    ],
                )
            })
        })
    };
    let button = move |label: &'static str, act: fn(Ui)| {
        crate::ui_kit::button(ui, size, move || tr(label), || false, move || act(ui))
    };
    let row = move |at: Position| {
        let channel = move || {
            ui.editor
                .with(|e| e.as_ref().and_then(|e| e.channel(at.index())))
        };
        let starred = move || channel().is_some_and(|ch| ui.favorites.with(|f| f.contains(&ch)));
        let focused = move || ui.keys_shown() && focus.get() == Some(at.index());
        let mark = move || Mark::of(focused(), false);
        list_rows::row(size, ROW, mark)
            .role(AccessibilityRole::Checkbox)
            .accessibility_state(move || AccessibilityState {
                checked: Some(if starred() {
                    Checked::True
                } else {
                    Checked::False
                }),
                ..Default::default()
            })
            .flex_row()
            .items_center()
            .justify_start()
            .gap(px(16.0))
            .on_press(move || {
                ui.touched();
                ui.editor
                    .update(|e| e.iter_mut().for_each(|e| e.focus = at.index()));
                toggle(ui);
            })
            .child(
                text(move || if starred() { "★" } else { "☆" })
                    .w(move || size.px(LIST_ITEM + 4.0))
                    .text_size(move || size.text_px(LIST_ITEM))
                    .text_color(move || {
                        if starred() && mark() != Mark::Focused {
                            rgb(ui.theme.with(|t| t.accent.light))
                        } else if starred() {
                            mark().text()
                        } else {
                            mark().secondary()
                        }
                    }),
            )
            .child(
                list_rows::name(size, move || {
                    channel().map_or(String::new(), |ch| crate::schedule().channel_name(ch))
                })
                .text_color(move || mark().text()),
            )
    };
    view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .px(px(48.0))
        .py(px(24.0))
        .gap(px(12.0))
        .bg(move || rgb(ui.theme.with(|t| t.background.window)))
        .opacity(move || if open() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if open() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(
                    text(title)
                        .flex_1()
                        .font_family(SANS)
                        .text_size(move || size.text_px(TITLE))
                        .text_color(rgb(LIST_TEXT)),
                )
                .child(button("Add all", set_group))
                .child(button("Remove all", clear_group))
                .child(button("Done", close)),
        )
        .child(
            text(move || {
                if ui.keys_shown() {
                    "OK add or remove   ◀ remove all in this list   ▶ add all in this list   Back done"
                } else {
                    "Tap a channel to add or remove it"
                }
            })
            .font_family(SANS)
            .text_size(move || size.text_px(CAPTION))
            .text_color(Mark::Plain.secondary()),
        )
        .child(
            virtual_list(count, row)
                .estimated_item_size(list_rows::pitch(size, ROW))
                .element_ref(&list)
                .flex_1(),
        )
}
