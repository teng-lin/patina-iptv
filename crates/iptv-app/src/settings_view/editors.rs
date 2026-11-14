//! The settings screen's editors and dialogs, over the screen: a list for
//! a choice (colours with their swatch), a stepper for numbers and
//! durations, the text sheet the wizard uses, and the confirmation of a
//! command. Each says what it is about; the remote drives them through
//! `iptv_settings::screen`, taps directly.

use crate::i18n::{tr, trf};

use std::rc::Rc;

use iptv_settings::screen::{Editor, Effect, grid};
use iptv_settings::{Setting, Value, ValueType};
use nsh_core::Checked;
use nsh_core::prelude::*;

use super::view::swatch_colour;
use super::{WHITE, state, update, words};
use crate::fonts::TITLE;
use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;
use crate::theme::{LIST_TEXT, RADIUS, RESTING_ALPHA};
use crate::ui_kit;
use crate::wizard_view::entry::{self, Sheet};
use crate::{Ui, tint};

/// The side panels' widths and the dialogs' (dp at full size).
const CHOICE_W: f32 = 400.0;
const DIALOG_W: f32 = 480.0;

/// Which editor is open (rebuilt only when this changes).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Choice,
    Number,
    Text,
    Confirm,
}

fn editor<R>(ui: Ui, f: impl FnOnce(&Editor) -> R) -> Option<R> {
    state(ui, |s| s.editor().map(f)).flatten()
}

fn setting(ui: Ui) -> Option<&'static Setting> {
    editor(ui, Editor::setting).flatten()
}

/// What the open editor is about: its setting's or command's name.
fn subject(ui: Ui) -> &'static str {
    editor(ui, |e| words::label(e.key())).unwrap_or("")
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let kind = computed(move || {
        editor(ui, |e| match e {
            Editor::Choice { .. } => Kind::Choice,
            Editor::Number { .. } => Kind::Number,
            Editor::Text { .. } => Kind::Text,
            Editor::Confirm { .. } => Kind::Confirm,
        })
    });
    view()
        .test_id("settings-editor")
        .absolute()
        .inset_0()
        .child(dynamic(move || {
            let shown = kind.get();
            untracked(move || match shown {
                Some(Kind::Choice) => choice(ui).into_element(),
                Some(Kind::Number) => stepper(ui).into_element(),
                Some(Kind::Text) => text_sheet(ui).into_element(),
                Some(Kind::Confirm) => confirm(ui).into_element(),
                None => view().into_element(),
            })
        }))
}

fn cancel(ui: Ui) {
    update(ui, |screen, _| {
        screen.cancel();
        Effect::None
    });
}

fn keep(ui: Ui) {
    update(ui, |screen, _| screen.commit());
}

/// A dimmed backdrop: a tap outside the editor cancels it.
fn backdrop(ui: Ui) -> View {
    view()
        .absolute()
        .inset_0()
        .bg(tint(0x000000, 0x60))
        .on_press(move || {
            ui.touched();
            cancel(ui);
        })
}

fn title(ui: Ui, size: Scale) -> impl IntoElement {
    ui_kit::title(size, move || subject(ui))
        .px(move || size.px(14.0))
        .py(move || size.px(8.0))
}

fn button(ui: Ui, size: Scale, label: &'static str, act: fn(Ui)) -> impl IntoElement {
    ui_kit::button(ui, size, move || tr(label), || false, move || act(ui))
}

/// A focused entry's mark (the keys shown).
fn entry_mark(ui: Ui, focused: bool) -> Mark {
    Mark::of(focused && ui.keys_shown(), false)
}

/// A side panel at the right (a choice, the key list) over a backdrop.
fn side(ui: Ui, size: Scale, width: f32) -> View {
    view()
        .w(move || size.px(width))
        .pt_safe_area()
        .p(move || size.px(14.0))
        .gap(move || size.px(4.0))
        .bg(move || rgb(ui.theme.with(|t| t.background.alt)))
}

/// A dialog in the middle over a backdrop.
fn dialog(ui: Ui, size: Scale) -> View {
    ui_kit::card(ui, size, DIALOG_W)
        .gap(move || size.px(14.0))
        .bg(move || rgb(ui.theme.with(|t| t.background.alt)))
}

/// A choice: the variants, the current one marked; on the right like the
/// settings panel itself.
fn choice(ui: Ui) -> impl IntoElement {
    let variants = move || {
        setting(ui)
            .and_then(|s| s.ty.editor_choices())
            .map_or(&[][..], |c| c.variants)
    };
    let size = Scale::new();
    let list = element_ref();
    let focus = computed(move || {
        editor(ui, |e| match e {
            Editor::Choice { focus, .. } => Some(*focus),
            _ => None,
        })
        .flatten()
    });
    effect(move || {
        if let Some(i) = focus.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let entry = move |at: Position| {
        let variant = move || variants().get(at.index()).copied();
        let selected = move || {
            editor(ui, |e| match e {
                Editor::Choice { selected, .. } => *selected == at.index(),
                _ => false,
            }) == Some(true)
        };
        let swatch = move || {
            let s = setting(ui)?;
            swatch_colour(&s.ty, variant()?)
        };
        let label = move || {
            let (Some(s), Some(v)) = (setting(ui), variant()) else {
                return String::new();
            };
            let family = s.ty.editor_choices().map_or("", |c| c.family);
            format!(
                "{}{}",
                if selected() { "✓  " } else { "    " },
                words::choice(family, v)
            )
        };
        let mark = move || entry_mark(ui, focus.get() == Some(at.index()));
        list_rows::row(size, ROW, mark)
            .role(AccessibilityRole::Option)
            .accessibility_state(move || AccessibilityState {
                selected: selected(),
                checked: Some(if selected() {
                    Checked::True
                } else {
                    Checked::False
                }),
                ..Default::default()
            })
            .flex_row()
            .items_center()
            .justify_start()
            .gap(move || size.px(10.0))
            .on_press(move || {
                ui.touched();
                let i = at.index();
                update(ui, move |screen, _| screen.pick(i));
            })
            .child(
                list_rows::name(size, label)
                    .flex_1()
                    .text_color(move || mark().text()),
            )
            .child(
                view()
                    .w(move || size.px(18.0))
                    .h(move || size.px(18.0))
                    .rounded(px(4.0))
                    .border(px(1.0))
                    .border_color(move || mark().text())
                    .bg(move || swatch().map_or(tint(WHITE, 0), rgb))
                    .opacity(move || if swatch().is_some() { 1.0_f32 } else { 0.0 }),
            )
    };
    view()
        .absolute()
        .inset_0()
        .flex_row()
        .justify_end()
        .child(backdrop(ui))
        .child(
            side(ui, size, CHOICE_W).child(title(ui, size)).child(
                virtual_list(move || variants().len(), entry)
                    .estimated_item_size(list_rows::pitch(size, ROW))
                    .element_ref(&list)
                    .flex_1(),
            ),
        )
}

/// The stepper's value and its range, in the setting's unit.
fn number_text(ui: Ui) -> (String, String) {
    let Some((s, value)) = editor(ui, |e| match e {
        Editor::Number { setting, value } => Some((*setting, *value)),
        _ => None,
    })
    .flatten() else {
        return Default::default();
    };
    let value = match s.ty {
        ValueType::Int { .. } => Value::Int(value),
        _ => Value::Duration(value),
    };
    let shown = words::value(s, &value);
    let range = grid(&s.ty).map_or(String::new(), |(min, max, step)| {
        trf(
            "From {min} to {max}, in steps of {step}",
            &[
                ("min", (&words::amount(&s.ty, min)).into()),
                ("max", (&words::amount(&s.ty, max)).into()),
                ("step", (&words::amount(&s.ty, step)).into()),
            ],
        )
    });
    (shown, range)
}

/// − or + beside the stepper's value (touch and mouse).
fn step_button(ui: Ui, size: Scale, label: &'static str, act: impl Fn(Ui) + 'static) -> View {
    view()
        .role(AccessibilityRole::Button)
        .label(label)
        .w(move || size.px(56.0))
        .h(move || size.px(ui_kit::TOUCH))
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .bg(tint(WHITE, RESTING_ALPHA * 2))
        .on_press(move || {
            ui.touched();
            act(ui);
        })
        .child(
            text(label)
                .text_size(move || size.text_px(TITLE))
                .text_color(rgb(LIST_TEXT)),
        )
}

/// A number or duration: − value +, the range, Cancel and Keep.
fn stepper(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let step = |by: i64| {
        move |ui: Ui| {
            update(ui, move |screen, _| {
                screen.step(by);
                Effect::None
            })
        }
    };
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .child(backdrop(ui))
        .child(
            dialog(ui, size)
                .child(title(ui, size))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(move || size.px(20.0))
                        .child(step_button(ui, size, "−", step(-1)))
                        .child(
                            ui_kit::title(size, move || format!("◀  {}  ▶", number_text(ui).0))
                                .text_color(move || rgb(ui.theme.with(|t| t.accent.light))),
                        )
                        .child(step_button(ui, size, "+", step(1))),
                )
                .child(ui_kit::caption(size, move || number_text(ui).1).px(move || size.px(14.0)))
                .child(
                    view()
                        .flex_row()
                        .gap(move || size.px(12.0))
                        .justify_end()
                        .child(button(ui, size, "Cancel", cancel))
                        .child(button(ui, size, "Keep", keep)),
                ),
        )
}

/// Text: the wizard's entry sheet.
fn text_sheet(ui: Ui) -> View {
    let current = move || {
        editor(ui, |e| match e {
            Editor::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .unwrap_or_default()
    };
    let note = move || {
        let invalid = editor(ui, |e| matches!(e, Editor::Text { invalid: true, .. }));
        if invalid == Some(true) {
            tr("That is not a valid value here").to_owned()
        } else {
            String::new()
        }
    };
    entry::panel(
        ui,
        Sheet {
            label: subject(ui).to_owned(),
            placeholder: "",
            keyboard: KeyboardType::Default,
            masked: false,
            current: Rc::new(current),
            note: Rc::new(note),
            set: Rc::new(move |t: &str| {
                let t = t.to_owned();
                update(ui, move |screen, _| {
                    screen.set_text(&t);
                    Effect::None
                });
            }),
            done: keep,
            cancel,
        },
    )
}

/// The question before a command that cannot be undone.
fn question(key: &str) -> &'static str {
    match key {
        "general.restore" => tr(
            "Choose a backup to apply when you next open Patina. You can cancel the scheduled restore before closing the app.",
        ),
        "epg.clear" => tr(
            "The stored programmes are deleted. The guide stays empty until it is downloaded again.",
        ),
        _ => tr("This cannot be undone."),
    }
}

/// "Really?" before a command: what it does, Cancel and the command.
fn confirm(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let key = move || editor(ui, Editor::key).unwrap_or("");
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .child(backdrop(ui))
        .child(
            dialog(ui, size)
                .child(ui_kit::title(size, move || format!("{}?", subject(ui))))
                .child(ui_kit::body(size, move || question(key())))
                .child(
                    view()
                        .flex_row()
                        .gap(move || size.px(12.0))
                        .justify_end()
                        .child(button(ui, size, "Cancel", cancel))
                        // OK goes ahead: the focused button.
                        .child(ui_kit::button(
                            ui,
                            size,
                            move || subject(ui),
                            || true,
                            move || keep(ui),
                        )),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::question;

    #[test]
    fn a_confirmation_says_what_goes() {
        assert!(question("epg.clear").contains("stored programmes are deleted"));
        assert_eq!(
            question("general.reset_watch_time"),
            "This cannot be undone."
        );
    }
}
