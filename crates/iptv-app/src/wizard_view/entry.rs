//! Text entry for one wizard field: a sheet with one NSH input, focused
//! when it opens so the platform keyboard (the TV's IME) comes up. Return
//! or Done keeps the text, Back keeps it too (as dismissing a keyboard
//! does), Cancel restores the field. The settings screen edits its text
//! values in the same sheet ([`panel`]), the playlists screen through
//! [`text_sheet`].

use crate::i18n::tr;

use std::rc::Rc;

use iptv_core::wizard::{Field, WizardEvent};
use nsh_core::element::input;
use nsh_core::prelude::*;

use super::words;
use crate::scale::Scale;
use crate::ui_kit::{self, ERROR};
use crate::{Ui, tint};

/// The sheet's width (dp at full size).
const SHEET_W: f32 = 720.0;

/// The field being typed into and its text so far.
#[derive(Clone, PartialEq, Eq)]
pub struct Editing {
    pub field: Field,
    pub text: String,
}

pub fn begin(ui: Ui, field: Field) {
    ui.wizard.update(|w| {
        if let Some(open) = w {
            let text = open.wizard.value(field).to_owned();
            open.editing = Some(Editing { field, text });
        }
    });
}

pub fn is_editing(ui: Ui) -> bool {
    ui.wizard
        .with(|w| w.as_ref().is_some_and(|o| o.editing.is_some()))
}

/// Ends entry and gives the wizard the text.
pub fn commit(ui: Ui) {
    let mut done = None;
    ui.wizard
        .update(|w| done = w.as_mut().and_then(|o| o.editing.take()));
    dismiss_keyboard();
    if let Some(Editing { field, text }) = done {
        super::send(ui, WizardEvent::SetField { field, value: text });
    }
}

pub fn cancel(ui: Ui) {
    ui.wizard
        .update(|w| w.iter_mut().for_each(|o| o.editing = None));
    dismiss_keyboard();
}

fn set_text(ui: Ui, text: &str) {
    ui.wizard.update(|w| {
        if let Some(e) = w.as_mut().and_then(|o| o.editing.as_mut()) {
            e.text = text.to_owned();
        }
    });
}

fn button(ui: Ui, size: Scale, label: &'static str, act: fn(Ui)) -> impl IntoElement {
    ui_kit::button(ui, size, move || tr(label), || false, move || act(ui))
}

/// A sheet's field: the kit's, outlined in the accent on the window's
/// colour.
pub(crate) fn sheet_field(ui: Ui, size: Scale) -> Input {
    ui_kit::field(size, input())
        .border_1()
        .border_color(move || rgb(ui.app.theme.with(|t| t.accent.light)))
        .bg(move || rgb(ui.app.theme.with(|t| t.background.window)))
}

/// A text sheet's frame: near the top (the keyboard takes the lower
/// half), its card holding `content`.
pub(crate) fn frame(ui: Ui, size: Scale) -> (View, View) {
    let outer = view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .items_center()
        .pt(move || size.px(40.0))
        .bg(tint(0x000000, 0xA0));
    (
        outer,
        ui_kit::card(ui, size, SHEET_W).gap(move || size.px(14.0)),
    )
}

/// Cancel and Done under a sheet's field.
pub(crate) fn sheet_buttons(ui: Ui, size: Scale, cancel: fn(Ui), done: fn(Ui)) -> View {
    view()
        .flex_row()
        .gap(move || size.px(12.0))
        .justify_end()
        .child(button(ui, size, "Cancel", cancel))
        .child(button(ui, size, "Done", done))
}

/// The sheet; made when entry starts, so its input exists only then.
pub fn sheet(ui: Ui) -> impl IntoElement {
    let Some(field) = ui
        .wizard
        .with_untracked(|w| w.as_ref().and_then(|o| o.editing.as_ref().map(|e| e.field)))
    else {
        return view();
    };
    let current = move || {
        ui.wizard.with(|w| {
            w.as_ref()
                .and_then(|o| o.editing.as_ref().map(|e| e.text.clone()))
                .unwrap_or_default()
        })
    };
    let keyboard = if field.is_url() {
        KeyboardType::Url
    } else {
        KeyboardType::Default
    };
    panel(
        ui,
        Sheet {
            label: words::label(field).to_owned(),
            placeholder: words::placeholder(field),
            keyboard,
            masked: field == Field::Password,
            current: Rc::new(current),
            note: Rc::new(String::new),
            set: Rc::new(move |t: &str| set_text(ui, t)),
            done: commit,
            cancel,
        },
    )
}

/// A text sheet whose owner keeps the text in its own state (the
/// playlists screen): plain functions instead of closures.
#[derive(Clone, Copy)]
pub struct TextSheet {
    pub label: &'static str,
    pub placeholder: &'static str,
    /// A link: the keyboard offers URL keys.
    pub url: bool,
    /// A password: typed characters are hidden.
    pub masked: bool,
    pub text: fn(Ui) -> String,
    pub set_text: fn(Ui, &str),
    /// Return, Done (and the owner's Back) keep the text.
    pub commit: fn(Ui),
    pub cancel: fn(Ui),
}

/// [`panel`] for a [`TextSheet`]. Make it when entry starts.
pub fn text_sheet(ui: Ui, sheet: TextSheet) -> View {
    let TextSheet {
        label,
        placeholder,
        url,
        masked,
        text,
        set_text,
        commit,
        cancel,
    } = sheet;
    panel(
        ui,
        Sheet {
            label: label.to_owned(),
            placeholder,
            keyboard: if url {
                KeyboardType::Url
            } else {
                KeyboardType::Default
            },
            masked,
            current: Rc::new(move || text(ui)),
            note: Rc::new(String::new),
            set: Rc::new(move |t: &str| set_text(ui, t)),
            done: commit,
            cancel,
        },
    )
}

/// What a text sheet edits.
pub(crate) struct Sheet {
    pub label: String,
    pub placeholder: &'static str,
    pub keyboard: KeyboardType,
    pub masked: bool,
    /// The text so far.
    pub current: Rc<dyn Fn() -> String>,
    /// A line under the field (a problem with the text); empty for none.
    pub note: Rc<dyn Fn() -> String>,
    /// Typing.
    pub set: Rc<dyn Fn(&str)>,
    /// Return or Done.
    pub done: fn(Ui),
    pub cancel: fn(Ui),
}

/// A text sheet: the label, one input focused at once (the platform
/// keyboard comes up), an optional note, Cancel and Done.
pub(crate) fn panel(ui: Ui, sheet: Sheet) -> View {
    let Sheet {
        label,
        placeholder,
        keyboard,
        masked,
        current,
        note,
        set,
        done,
        cancel,
    } = sheet;
    let has_note = {
        let note = note.clone();
        move || !note().is_empty()
    };
    let size = Scale::new();
    let (outer, card) = frame(ui, size);
    outer.child(
        card.child(ui_kit::title(size, label))
            .child(
                sheet_field(ui, size)
                    .value(move || current())
                    .on_change(move |t: &str| set(t))
                    .on_submit(move |_: &str| done(ui))
                    .auto_focus(true)
                    .masked(masked)
                    .keyboard_type(keyboard)
                    .auto_capitalize(AutoCapitalize::None)
                    .auto_correct(false)
                    .placeholder(placeholder),
            )
            .child(when(has_note, move || {
                let note = note.clone();
                ui_kit::caption(size, move || note()).text_color(rgb(ERROR))
            }))
            .child(sheet_buttons(ui, size, cancel, done)),
    )
}
