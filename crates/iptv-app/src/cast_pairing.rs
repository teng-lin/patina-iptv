//! Receiver approval belongs to the window that chose the destination.
use crate::{
    Ui,
    i18n::tr,
    overlays::Overlay,
    runtime_owner::runtime,
    scale::Scale,
    ui_kit,
    wizard_view::entry::{frame, sheet_buttons, sheet_field},
};
use iptv_core::cast::Receiver;
use iptv_runtime::cast::{PairingPrompt, PairingRequest, pairing_prompt};
use nsh_core::prelude::*;
use std::{cell::RefCell, rc::Rc};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone)]
pub(crate) struct Pending {
    ticket: u64,
    request: Request,
}

#[derive(Clone)]
enum Request {
    Pin(Rc<RefCell<InputState>>),
    Forget { receiver: Receiver, selected: bool },
}

struct InputState {
    request: Option<PairingRequest>,
    pin: Zeroizing<String>,
}

pub(crate) fn prompt(ui: Ui, ticket: u64) -> PairingPrompt {
    let (prompt, request) = pairing_prompt();
    crate::tasks::spawn_in_app(async move {
        if let Ok(request) = request.await
            && runtime().cast_intent_current(ticket)
            && ui.cast_busy.try_get() == Some(true)
        {
            ui.cast_pairing.try_set(Some(Pending {
                ticket,
                request: Request::Pin(Rc::new(RefCell::new(InputState {
                    request: Some(request),
                    pin: Zeroizing::new(String::new()),
                }))),
            }));
        }
    });
    prompt
}

pub(crate) fn offer_forget(ui: Ui, ticket: u64, receiver: Receiver) {
    ui.cast_pairing.set(Some(Pending {
        ticket,
        request: Request::Forget {
            receiver,
            selected: false,
        },
    }));
}

pub(crate) fn is_open(ui: Ui) -> bool {
    ui.cast_pairing.with(Option::is_some)
}

pub(crate) fn close_for(ui: Ui, ticket: u64) {
    if ui
        .cast_pairing
        .try_get()
        .flatten()
        .is_some_and(|p| p.ticket == ticket)
    {
        close(ui);
    }
}

pub(crate) fn close(ui: Ui) {
    ui.cast_pairing.set(None);
    dismiss_keyboard();
}

fn submit(ui: Ui) {
    let Some(Pending {
        request: Request::Pin(input),
        ..
    }) = ui.cast_pairing.get_untracked()
    else {
        return;
    };
    let mut input = input.borrow_mut();
    if input.pin.len() == 4
        && let Some(request) = input.request.take()
    {
        let pin = std::mem::take(&mut input.pin);
        drop(input);
        close(ui);
        let _ = request.submit(pin);
    }
}

fn forget(ui: Ui) {
    let Some(Pending {
        ticket,
        request: Request::Forget { receiver, .. },
    }) = ui.cast_pairing.get_untracked()
    else {
        return;
    };
    close(ui);
    if !runtime().cast_intent_current(ticket) {
        return;
    }
    ui.cast_busy.set(true);
    crate::tasks::spawn_in_app(async move {
        let result = runtime().forget_cast_pairing(receiver).await;
        if ui.cast_busy.try_set(false)
            && let Err(error) = result
        {
            crate::casting::report(error);
        }
    });
}

pub(crate) fn key(ui: Ui, action: iptv_nav::Action) {
    use iptv_nav::Action;
    if let Some(Pending {
        request: Request::Forget { selected, .. },
        ..
    }) = ui.cast_pairing.get_untracked()
    {
        match action {
            Action::MoveLeft | Action::MoveRight => ui.cast_pairing.update(|open| {
                if let Some(Pending {
                    request: Request::Forget { selected, .. },
                    ..
                }) = open
                {
                    *selected = action == Action::MoveRight;
                }
            }),
            Action::Select if selected => forget(ui),
            Action::Select | Action::Back | Action::ShowMenu => close(ui),
            _ => {}
        }
        return;
    }
    match action {
        Action::Back | Action::ShowMenu => close(ui),
        Action::Select => submit(ui),
        _ => {}
    }
}

pub(crate) fn layer(ui: Ui) -> impl IntoElement {
    when(
        move || is_open(ui),
        move || {
            let scale = Scale::new();
            let (outer, card) = frame(ui, scale);
            outer
                .accessibility_elements_hidden(move || {
                    Overlay::CastPairing.accessibility_hidden(ui)
                })
                .child(
                    card.child(
                        when(
                            move || {
                                ui.cast_pairing.with(|open| {
                                    matches!(
                                        open,
                                        Some(Pending {
                                            request: Request::Forget { .. },
                                            ..
                                        })
                                    )
                                })
                            },
                            move || forget_card(ui, scale),
                        )
                        .otherwise(move || pin_card(ui, scale)),
                    ),
                )
        },
    )
}

fn forget_card(ui: Ui, scale: Scale) -> View {
    let wants_forget = move || {
        ui.cast_pairing.with(|open| {
            matches!(
                open,
                Some(Pending {
                    request: Request::Forget { selected: true, .. },
                    ..
                })
            )
        })
    };
    view()
        .gap(move || scale.px(14.0))
        .child(ui_kit::title(
            scale,
            tr("The AirPlay receiver identity changed."),
        ))
        .child(ui_kit::body(
            scale,
            tr("Forget this pairing to choose the receiver again."),
        ))
        .child(
            view()
                .flex_row()
                .gap(move || scale.px(12.0))
                .justify_end()
                .child(ui_kit::button(
                    ui,
                    scale,
                    tr("Cancel"),
                    move || !wants_forget(),
                    move || close(ui),
                ))
                .child(ui_kit::button(
                    ui,
                    scale,
                    tr("Forget pairing"),
                    wants_forget,
                    move || forget(ui),
                )),
        )
}

fn pin_card(ui: Ui, scale: Scale) -> View {
    view()
        .gap(move || scale.px(14.0))
        .child(ui_kit::title(scale, tr("Pair with AirPlay")))
        .child(ui_kit::body(
            scale,
            tr("Approve pairing on the receiver and enter the four-digit code it shows."),
        ))
        .child(
            sheet_field(ui, scale)
                .test_id("cast-pairing-pin")
                .label(|| tr("AirPlay PIN"))
                .placeholder(tr("AirPlay PIN"))
                .auto_focus(true)
                .masked(true)
                .keyboard_type(KeyboardType::Number)
                .max_length(4)
                .auto_correct(false)
                .on_change(move |text: &str| {
                    if let Some(Pending {
                        request: Request::Pin(input),
                        ..
                    }) = ui.cast_pairing.get_untracked()
                    {
                        let mut input = input.borrow_mut();
                        input.pin.zeroize();
                        if text.len() <= 4 && text.bytes().all(|c| c.is_ascii_digit()) {
                            input.pin.push_str(text);
                        }
                    }
                })
                .on_submit(move |_: &str| submit(ui)),
        )
        .child(sheet_buttons(ui, scale, close, submit))
}
