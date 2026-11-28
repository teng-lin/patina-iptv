//! Receiver approval belongs to the window that chose the destination.
use crate::{
    Ui,
    chrome::i18n::tr,
    chrome::overlays::Overlay,
    chrome::scale::Scale,
    chrome::ui_kit,
    chrome::ui_kit::entry::{frame, sheet_buttons, sheet_field},
    wiring::runtime_owner::runtime,
};
use iptv_core::cast::{Error, Receiver};
use iptv_presentation::cast::{Effect, Prompt, State};
use iptv_runtime::cast::{PairingPrompt, pairing_prompt};
use nsh_core::prelude::*;

pub(crate) fn prompt(cast: crate::screens::casting::CastUi, ticket: u64) -> PairingPrompt {
    let (prompt, request) = pairing_prompt();
    crate::wiring::tasks::spawn_in_app(async move {
        if let Ok(request) = request.await
            && runtime().cast_intent_current(ticket)
            && cast.busy.try_get() == Some(true)
        {
            cast.pairing.replace(Some(request));
            cast.state.update(|state| state.open_pin(ticket));
        }
    });
    prompt
}

pub(crate) fn offer_forget(
    cast: crate::screens::casting::CastUi,
    ticket: u64,
    receiver: Receiver,
    reason: Error,
) {
    cast.pairing.replace(None);
    cast.state
        .update(|state| state.offer_forget(ticket, receiver, reason));
}

pub(crate) fn is_open(cast: crate::screens::casting::CastUi) -> bool {
    cast.prompt.with(Option::is_some)
}

pub(crate) fn close_for(cast: crate::screens::casting::CastUi, ticket: u64) {
    if untracked(|| cast.prompt.try_get()).is_some()
        && cast.state.with_untracked(State::pairing_ticket) == Some(ticket)
    {
        close(cast);
    }
}

pub(crate) fn close(cast: crate::screens::casting::CastUi) {
    cast.pairing.replace(None);
    cast.state.update(State::close_pairing);
    dismiss_keyboard();
}

fn submit(cast: crate::screens::casting::CastUi) {
    if cast.prompt.get_untracked() != Some(Prompt::Pin) {
        return;
    }
    let mut effect = None;
    cast.state.update(|state| effect = state.submit_pin());
    let Some(Effect::SubmitPin { ticket, pin }) = effect else {
        crate::chrome::toast::notify(tr("Enter the four digits shown on the receiver."));
        return;
    };
    if let Some(request) = cast.pairing.replace(None) {
        close(cast);
        if runtime().cast_intent_current(ticket) {
            let _ = request.submit(pin);
        }
    }
}

fn forget(ui: Ui) {
    if ui.cast.busy.get_untracked() {
        return;
    }
    let Some((ticket, receiver)) = ui.cast.state.with_untracked(State::forget_receiver) else {
        return;
    };
    close(ui.cast);
    if !runtime().cast_intent_current(ticket) {
        return;
    }
    ui.cast.state.update(|state| state.busy = true);
    crate::wiring::tasks::spawn_in_app(async move {
        let result = runtime().forget_cast_pairing(receiver).await;
        if ui.cast.busy.try_get().is_some() {
            ui.cast.state.update(|state| state.busy = false);
            if result.is_err() {
                crate::chrome::toast::notify(tr("Could not forget the AirPlay pairing."));
            }
            crate::screens::casting::finish_pending_stop(ui);
        }
    });
}

pub(crate) fn key(ui: Ui, action: iptv_nav::Action) {
    use iptv_nav::Action;
    if let Some(Prompt::Forget { selected, .. }) = ui.cast.prompt.get_untracked() {
        match action {
            Action::MoveLeft | Action::MoveRight => ui
                .cast
                .state
                .update(|state| state.select_forget(action == Action::MoveRight)),
            Action::Select if selected => forget(ui),
            Action::Select | Action::Back | Action::ShowMenu => close(ui.cast),
            _ => {}
        }
        return;
    }
    match action {
        Action::Back | Action::ShowMenu => close(ui.cast),
        Action::Select => submit(ui.cast),
        _ => {}
    }
}

pub(crate) fn layer(ui: Ui) -> impl IntoElement {
    when(
        move || is_open(ui.cast),
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
                            move || matches!(ui.cast.prompt.get(), Some(Prompt::Forget { .. })),
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
        matches!(
            ui.cast.prompt.get(),
            Some(Prompt::Forget { selected: true, .. })
        )
    };
    view()
        .gap(move || scale.px(14.0))
        .child(ui_kit::title(scale, move || {
            let storage = matches!(
                ui.cast.prompt.get(),
                Some(Prompt::Forget {
                    reason: Error::PairingInvalid,
                    ..
                })
            );
            if storage {
                tr("The saved AirPlay pairing is invalid.")
            } else {
                tr("The AirPlay receiver identity changed.")
            }
        }))
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
                    move || close(ui.cast),
                ))
                .child(
                    ui_kit::button(ui, scale, tr("Forget pairing"), wants_forget, move || {
                        forget(ui)
                    })
                    .disabled(move || ui.cast.busy.get()),
                ),
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
                .on_change(move |text: &str| ui.cast.state.update(|state| state.set_pin(text)))
                .on_submit(move |_: &str| submit(ui.cast)),
        )
        .child(sheet_buttons(
            ui,
            scale,
            |ui| close(ui.cast),
            |ui| submit(ui.cast),
        ))
}
