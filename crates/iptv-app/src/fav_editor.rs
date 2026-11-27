//! The favorites editor overlay for the list on screen (from the menu's
//! "Manage favorites…"): a star per channel, OK toggles, Right adds the
//! list, Back leaves. Up from the first row focuses the toolbar.
//! "Remove all" asks for confirmation with Cancel focused. Changes are saved
//! by channel key (`favorites.rs`) at once.

use crate::i18n::{tr, trf};

use iptv_presentation::editor::FavoritesEditor;
use iptv_presentation::lists::{ChannelList, ListId};
use nsh_core::Checked;
use nsh_core::prelude::*;

use crate::Ui;
use crate::fonts::{CAPTION, LIST_ITEM, SANS, TITLE};
use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;
use crate::theme::LIST_TEXT;

/// Opens the editor on the list that owns the action, at its selected channel.
pub fn open(ui: Ui, list: &ListId, channel: Option<usize>) {
    let catalog = crate::runtime_owner::schedule();
    let Some(group) = list.resolve(&*catalog) else {
        return;
    };
    let channels = ui.guide.list.get_untracked();
    let channels = if channels.entry() == group {
        channels
    } else {
        std::rc::Rc::new(ChannelList::build(
            group,
            &*catalog,
            &crate::favorites::favorite_channels(),
            &crate::favorites::recent_channels(),
        ))
    };
    let mut editor = FavoritesEditor::new(group, channels.channels().to_vec());
    editor.focus = channel.and_then(|ch| channels.row_of(ch)).unwrap_or(0);
    ui.screens.editor.set(Some(editor));
}

pub fn close(ui: Ui) {
    ui.screens.editor.set(None);
}

pub fn move_focus(ui: Ui, step: isize) {
    ui.screens
        .editor
        .update(|e| e.iter_mut().for_each(|e| e.move_focus(step)));
}

fn with_favorites(
    ui: Ui,
    f: impl FnOnce(&FavoritesEditor, &mut std::collections::BTreeSet<usize>),
) {
    if let Some(editor) = ui.screens.editor.get() {
        crate::favorites::edit(ui, |fav| f(&editor, fav));
    }
}

pub fn toggle(ui: Ui) {
    with_favorites(ui, |e, fav| e.toggle(fav));
}

pub fn request_clear(ui: Ui) {
    ui.screens.editor.update(|editor| {
        if let Some(editor) = editor {
            editor.request_clear();
        }
    });
}

fn resolve_clear(ui: Ui, confirmed: bool) {
    if let Some(mut editor) = ui.screens.editor.get_untracked() {
        if confirmed {
            crate::favorites::edit(ui, |fav| editor.resolve_clear(fav, true));
        } else {
            editor.clear_confirmation = None;
        }
        ui.screens.editor.set(Some(editor));
    }
}

pub fn key(ui: Ui, action: iptv_nav::Action) {
    use iptv_nav::Action;
    let Some(editor) = ui.screens.editor.get_untracked() else {
        return;
    };
    if let Some(remove) = editor.clear_confirmation {
        match action {
            Action::MoveUp | Action::MoveDown | Action::MoveLeft | Action::MoveRight => {
                ui.screens
                    .editor
                    .update(|e| e.as_mut().unwrap().clear_confirmation = Some(!remove));
            }
            Action::Select => resolve_clear(ui, remove),
            Action::Back => resolve_clear(ui, false),
            _ => {}
        }
    } else if let Some(control) = editor.control {
        match action {
            Action::MoveLeft | Action::MoveRight => {
                let next = if action == Action::MoveLeft {
                    control.saturating_sub(1)
                } else {
                    (control + 1).min(2)
                };
                ui.screens
                    .editor
                    .update(|e| e.as_mut().unwrap().control = Some(next));
            }
            Action::MoveDown => ui
                .screens
                .editor
                .update(|e| e.as_mut().unwrap().control = None),
            Action::Select => match control {
                0 => set_group(ui),
                1 => request_clear(ui),
                _ => close(ui),
            },
            Action::Back => close(ui),
            _ => {}
        }
    } else {
        match action {
            Action::MoveUp if editor.focus == 0 => {
                ui.screens
                    .editor
                    .update(|e| e.as_mut().unwrap().control = Some(0));
            }
            Action::MoveUp => move_focus(ui, -1),
            Action::MoveDown => move_focus(ui, 1),
            Action::Select => toggle(ui),
            Action::MoveRight => set_group(ui),
            Action::Back => close(ui),
            _ => {}
        }
    }
}

pub fn set_group(ui: Ui) {
    with_favorites(ui, |e, fav| e.set_group(fav));
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let open = move || ui.screens.editor.with(Option::is_some);
    let size = Scale::new();
    let list = element_ref();
    let focus = computed(move || ui.screens.editor.with(|e| e.as_ref().map(|e| e.focus)));
    effect(move || {
        if let Some(i) = focus.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let count = move || {
        ui.screens
            .editor
            .with(|e| e.as_ref().map_or(0, FavoritesEditor::len))
    };
    let title = move || {
        ui.screens.editor.with(|e| {
            e.as_ref().map_or(String::new(), |e| {
                let starred = ui.app.favorites.with(|f| e.starred(f));
                let group = crate::lists::entry_name(e.group, &*crate::runtime_owner::schedule());
                trf(
                    "Favorites · {group} · {starred} of {total}",
                    &[
                        ("group", (&group).into()),
                        ("starred", starred.into()),
                        ("total", e.len().into()),
                    ],
                )
            })
        })
    };
    let button = move |label: &'static str, control: usize, act: fn(Ui)| {
        crate::ui_kit::button(
            ui,
            size,
            move || tr(label),
            move || {
                ui.screens.editor.with(|e| {
                    e.as_ref().is_some_and(|e| {
                        e.control == Some(control) && e.clear_confirmation.is_none()
                    })
                })
            },
            move || act(ui),
        )
    };
    let row = move |at: Position| {
        let channel = move || {
            ui.screens
                .editor
                .with(|e| e.as_ref().and_then(|e| e.channel(at.index())))
        };
        let starred =
            move || channel().is_some_and(|ch| ui.app.favorites.with(|f| f.contains(&ch)));
        let focused = move || {
            ui.keys_shown()
                && focus.get() == Some(at.index())
                && ui.screens.editor.with(|e| {
                    e.as_ref()
                        .is_some_and(|e| e.control.is_none() && e.clear_confirmation.is_none())
                })
        };
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
                ui.screens.editor.update(|e| {
                    e.iter_mut().for_each(|e| {
                        e.focus = at.index();
                        e.control = None;
                    })
                });
                toggle(ui);
            })
            .child(
                text(move || if starred() { "★" } else { "☆" })
                    .w(move || size.px(LIST_ITEM + 4.0))
                    .text_size(move || size.text_px(LIST_ITEM))
                    .text_color(move || {
                        if starred() && mark() != Mark::Focused {
                            rgb(ui.app.theme.with(|t| t.accent.light))
                        } else if starred() {
                            mark().text()
                        } else {
                            mark().secondary()
                        }
                    }),
            )
            .child(
                list_rows::name(size, move || {
                    channel().map_or(String::new(), |ch| {
                        crate::runtime_owner::schedule().channel_name(ch)
                    })
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
        .bg(move || rgb(ui.app.theme.with(|t| t.background.window)))
        .test_id("favorites-editor")
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::FavoritesEditor.accessibility_hidden(ui)
        })
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
                .accessibility_elements_hidden(move || ui.screens.editor.with(|e| e.as_ref().is_some_and(|e| e.clear_confirmation.is_some())))
                .pointer_events(move || {
                    if ui.screens.editor.with(|e| e.as_ref().is_some_and(|e| e.clear_confirmation.is_none())) {
                        PointerEvents::Auto
                    } else {
                        PointerEvents::None
                    }
                })
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
                .child(button("Add all", 0, set_group))
                .child(button("Remove all", 1, request_clear))
                .child(button("Done", 2, close)),
        )
        .child(
            text(move || {
                if ui.keys_shown() {
                    tr("OK add or remove   ▲ toolbar above first channel   ▶ add all in this list   Back done")
                } else {
                    tr("Tap a channel to add or remove it")
                }
            })
            .font_family(SANS)
            .text_size(move || size.text_px(CAPTION))
            .text_color(Mark::Plain.secondary())
            .accessibility_elements_hidden(move || ui.screens.editor.with(|e| e.as_ref().is_some_and(|e| e.clear_confirmation.is_some()))),
        )
        .child(
            virtual_list(count, row)
                .accessibility_elements_hidden(move || ui.screens.editor.with(|e| e.as_ref().is_some_and(|e| e.clear_confirmation.is_some())))
                .pointer_events(move || {
                    if ui.screens.editor.with(|e| e.as_ref().is_some_and(|e| e.clear_confirmation.is_none())) {
                        PointerEvents::Auto
                    } else {
                        PointerEvents::None
                    }
                })
                .estimated_item_size(list_rows::pitch(size, ROW))
                .element_ref(&list)
                .flex_1(),
        )
        .child(clear_confirmation(ui, size))
}

fn clear_confirmation(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || {
            ui.screens
                .editor
                .with(|e| e.as_ref().is_some_and(|e| e.clear_confirmation.is_some()))
        },
        move || {
            let (outer, card) = crate::ui_kit::entry::frame(ui, size);
            outer
                .on_press(|| {})
                .test_id("favorites-clear-confirmation")
                .child(
                    card.child(crate::ui_kit::title(size, move || {
                        tr("Remove all favorites in this list?")
                    }))
                    .child(crate::ui_kit::body(size, move || {
                        tr("Only channels in this list will be removed from favorites.")
                    }))
                    .child(crate::ui_kit::button(
                        ui,
                        size,
                        move || tr("Cancel"),
                        move || {
                            ui.screens.editor.with(|e| {
                                e.as_ref()
                                    .is_some_and(|e| e.clear_confirmation == Some(false))
                            })
                        },
                        move || resolve_clear(ui, false),
                    ))
                    .child(crate::ui_kit::button(
                        ui,
                        size,
                        move || tr("Remove all"),
                        move || {
                            ui.screens.editor.with(|e| {
                                e.as_ref()
                                    .is_some_and(|e| e.clear_confirmation == Some(true))
                            })
                        },
                        move || resolve_clear(ui, true),
                    )),
                )
        },
    )
}
