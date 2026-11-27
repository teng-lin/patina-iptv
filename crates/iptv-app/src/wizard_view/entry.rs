//! The wizard's text field session. The shared sheet renders its input.

use std::rc::Rc;

use iptv_presentation::wizard::{Field, WizardEvent};
use nsh_core::prelude::*;

use super::words;
use crate::{
    Ui,
    chrome::input::dismiss_keyboard,
    chrome::ui_kit::entry::{Sheet, panel},
};

/// The field being typed into and its text so far.
#[derive(Clone, PartialEq, Eq)]
pub struct Editing {
    pub field: Field,
    pub text: String,
}

pub fn begin(ui: Ui, field: Field) {
    ui.screens.wizard.update(|w| {
        if let Some(open) = w {
            let text = open.wizard.value(field).to_owned();
            open.editing = Some(Editing { field, text });
        }
    });
}

pub fn is_editing(ui: Ui) -> bool {
    ui.screens
        .wizard
        .with(|w| w.as_ref().is_some_and(|o| o.editing.is_some()))
}

/// Ends entry and gives the wizard the text.
pub fn commit(ui: Ui) {
    let mut done = None;
    ui.screens
        .wizard
        .update(|w| done = w.as_mut().and_then(|o| o.editing.take()));
    dismiss_keyboard();
    if let Some(Editing { field, text }) = done {
        super::send(ui, WizardEvent::SetField { field, value: text });
    }
}

pub fn cancel(ui: Ui) {
    ui.screens
        .wizard
        .update(|w| w.iter_mut().for_each(|o| o.editing = None));
    dismiss_keyboard();
}

fn set_text(ui: Ui, text: &str) {
    ui.screens.wizard.update(|w| {
        if let Some(e) = w.as_mut().and_then(|o| o.editing.as_mut()) {
            e.text = text.to_owned();
        }
    });
}

/// The sheet; made when entry starts, so its input exists only then.
pub fn sheet(ui: Ui) -> impl IntoElement {
    let Some(field) = ui
        .screens
        .wizard
        .with_untracked(|w| w.as_ref().and_then(|o| o.editing.as_ref().map(|e| e.field)))
    else {
        return view();
    };
    let current = move || {
        ui.screens.wizard.with(|w| {
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
