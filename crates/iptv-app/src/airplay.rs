//! Apple owns the named-device picker. Its transient controls share one
//! persistent runtime player; the play bar remains the playback remote.
use crate::{Ui, runtime_owner::runtime};
use nsh_core::prelude::*;
use serde_json::json;

pub(crate) fn direct() -> bool {
    runtime().native_cast_view().is_some() && !runtime().chromecast_available()
}

pub(crate) fn button(ui: Ui, size: f32) -> View {
    view().w(px(size)).h(px(size)).child(
        native("tv.cast-picker")
            .test_id("play-bar-cast")
            .w(px(size))
            .h(px(size))
            .label(move || crate::i18n::tr("Cast to TV"))
            .prop("owner", move || json!(runtime().native_cast_view()))
            .prop("accent", move || {
                json!(ui.app.theme.with(|t| t.accent.light))
            }),
    )
}

pub(crate) fn picker(ui: Ui, _shown: impl Fn() -> bool + Copy + 'static) -> View {
    if ui.player_surface != 0 || crate::pip::in_window(ui) || runtime().native_cast_view().is_none()
    {
        return view();
    }
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
                .on_native_event(move |event: &NativeEvent| match event.name.as_ref() {
                    "routes-open" => {
                        ui.cast_picker_open.set(true);
                        ui.touched();
                        // A Mac negotiates video on the AVPlayer before routing.
                        // On iPad, local mpv already owns the shared audio session;
                        // wait for the user to actually choose an external device.
                        if cfg!(target_os = "macos") {
                            crate::casting::select_airplay(ui);
                        }
                    }
                    "routes-closed" => {
                        ui.cast_picker_open.set(false);
                        if !ui.cast_busy.get_untracked() {
                            crate::chooser::close(ui);
                        }
                    }
                    "route-selected" => crate::casting::select_airplay(ui),
                    "local-selected" => crate::casting::play_here(ui),
                    _ => {}
                }),
        )
}
