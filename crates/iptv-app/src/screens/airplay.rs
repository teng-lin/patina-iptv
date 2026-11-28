//! Apple owns the named-device picker. Its transient controls share one
//! persistent runtime player; the play bar remains the playback remote.
use crate::{Ui, wiring::runtime_owner::runtime};
use nsh_core::prelude::*;
use serde_json::json;

pub(crate) fn direct() -> bool {
    runtime().native_cast_view().is_some() && !runtime().network_cast_available()
}

pub(crate) fn button(ui: Ui, size: f32, id: &'static str) -> View {
    view().w(px(size)).h(px(size)).child(
        native("tv.cast-picker")
            .test_id(id)
            .accessibility_state(move || AccessibilityState {
                busy: ui.cast.busy.get(),
                has_popup: true,
                expanded: Some(ui.cast.picker_open.get()),
                ..Default::default()
            })
            .w(px(size))
            .h(px(size))
            .label(move || crate::chrome::i18n::tr("Cast to TV"))
            .prop("owner", move || json!(runtime().native_cast_view()))
            .prop("accent", move || {
                json!(ui.app.theme.with(|t| t.accent.light))
            }),
    )
}

pub(crate) fn picker(ui: Ui, _shown: impl Fn() -> bool + Copy + 'static) -> View {
    if ui.playback.player_surface != 0
        || crate::playback::pip::in_window(ui.playback)
        || runtime().native_cast_view().is_none()
    {
        return view();
    }
    let pending_choice = std::cell::RefCell::new(None);
    // Return the serial as soon as this handoff settles (or immediately if
    // rejected). Native cleanup only touches that choice's priming item.
    let choice_reply = signal(None::<(u64, bool)>);
    view()
        // This host is always mounted, including while the play bar disappears.
        .w(px(0.0))
        .h(px(0.0))
        .accessibility_elements_hidden(true)
        .pointer_events(PointerEvents::None)
        .child(
            native("tv.airplay")
                .w(px(0.0))
                .h(px(0.0))
                .prop("owner", move || json!(runtime().native_cast_view()))
                .prop("finishedChoice", move || {
                    json!(choice_reply.get().and_then(|(serial, started)| {
                        (!started || !ui.cast.busy.get()).then_some(serial)
                    }))
                })
                .on_native_event(move |event: &NativeEvent| match event.name.as_ref() {
                    "routes-open" => {
                        pending_choice.replace(Some((
                            runtime().cast_intent(),
                            ui.playback.player.main().key.get_untracked(),
                        )));
                        ui.cast.state.update(|state| state.picker_open = true);
                        ui.touched();
                    }
                    "routes-closed" => {
                        ui.cast.state.update(|state| state.picker_open = false);
                        if !ui.cast.busy.get_untracked() {
                            crate::wiring::navigation::close_chooser(ui);
                        }
                    }
                    "route-selected" => {
                        let valid = pending_choice.take().is_some_and(|(ticket, channel)| {
                            runtime().cast_intent_current(ticket)
                                && channel.is_some()
                                && channel == ui.playback.player.main().key.get_untracked()
                                && crate::screens::casting::available(ui)
                        });
                        let started = if valid && !ui.cast.busy.get_untracked() {
                            crate::screens::casting::select_airplay(ui);
                            ui.cast.busy.get_untracked()
                        } else {
                            false
                        };
                        choice_reply.set(event.data.as_u64().map(|serial| (serial, started)));
                    }
                    "local-selected" => crate::screens::casting::play_here(ui),
                    _ => {}
                }),
        )
}
