//! Shared routes into library screens and the runtime's parental gate.

use crate::runtime::model::SourceId;
use iptv_core::parental::Target;
use iptv_presentation::parental::Phase;

use crate::Ui;

/// Always re-resolve durable ancestry before executing a content action.
pub(crate) fn request(screens: crate::ScreensUi, target: Target, done: impl FnOnce() + 'static) {
    request_if(screens, target, || true, done);
}

/// A superseded content choice must not open a late PIN prompt or resume it.
pub(crate) fn request_if(
    ui: crate::ScreensUi,
    target: Target,
    current: impl Fn() -> bool + 'static,
    done: impl FnOnce() + 'static,
) {
    crate::wiring::tasks::spawn_in_app(async move {
        let owner = crate::wiring::runtime_owner::runtime();
        let result = owner.parental_check(&target).await;
        if !current() {
            return;
        }
        let done = move || {
            if current() {
                done();
            }
        };
        match result {
            Ok(()) => done(),
            Err(crate::runtime::parental::Error::Locked) => {
                let phase = if owner.parental_has_pin() {
                    Phase::Unlock
                } else {
                    Phase::New
                };
                crate::screens::parental_view::show(ui, target, phase, done);
            }
            Err(_) => crate::chrome::toast::notify(crate::chrome::i18n::tr(
                "Parental settings are unavailable",
            )),
        }
    });
}
/// Runs `then` now, or after the PIN while the settings are protected
/// and locked.
pub(crate) fn settings_guarded(ui: crate::ScreensUi, then: impl FnOnce() + 'static) {
    if crate::wiring::settings::choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR)
        == "blocked_content_and_settings"
        && !crate::runtime().parental_settings_allowed()
    {
        crate::screens::parental_view::protected_settings(ui, then);
    } else {
        then();
    }
}

// Guide settings opens the shown playlist's editors, or the playlist
// picker when no source is currently selected.
pub(crate) fn guide_sources(ui: Ui, playlist: Option<SourceId>) {
    if let Some(playlist) = playlist {
        crate::library_ui::epg_sources_view::open(ui.screens, playlist);
    } else {
        crate::library_ui::playlists_view::open(ui);
    }
}

pub(crate) fn add_playlist(ui: Ui) {
    if crate::runtime().parental_has_pin() && !crate::runtime().parental_settings_allowed() {
        request(ui.screens, Target::settings(), move || {
            crate::library_ui::wizard_view::open_add(ui)
        });
    } else {
        crate::library_ui::wizard_view::open_add(ui);
    }
}

/// Opens the playlist manager above the Settings screen.
pub(crate) fn manage_playlists(screens: crate::ScreensUi) {
    crate::library_ui::playlists_view::choose_for_management(screens);
}

pub(crate) fn reorder_playlists(ui: Ui) {
    crate::library_ui::organization_view::open_playlist_reorder(ui, None);
}

// Shell dispatch entry points exposed to feature trees without naming `shell/`.
pub(crate) use crate::chrome::chooser::{
    Kind as ChooserKind, close as close_chooser, layer as chooser_layer, open as open_chooser,
    open_more as open_more_chooser, picture_identity, picture_name,
    show_receivers as show_cast_receivers,
};
pub(crate) use crate::chrome::desktop::{
    close_after_shutdown, focus_main, keep_reachable as keep_window_reachable, pointer_entered,
    pointer_forget, with_window,
};
pub(crate) use crate::chrome::input::{
    bind as bind_input, context as input_context, window as input_window,
};
pub(crate) use crate::chrome::menu_view::{
    close as close_menu, empty_group_settings_available, group_order_label, has_group_options,
    layer as menu_layer, offer_external_player_reset, open as open_menu, open_at as open_menu_at,
    open_channel as open_channel_menu, open_empty_group_settings, open_group_options,
    open_playlist_options, open_playlist_order, open_timeline_context, perform as perform_menu,
};
pub(crate) use crate::chrome::remote_editor::{media_toggle_override, open as open_remote_editor};
pub(crate) use crate::{pointer_moved, toggle_panels};
