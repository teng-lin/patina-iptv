//! Platform Back handler, top-level exit confirmation, and step-back routing.

use nsh_core::prelude::*;

use crate::guide::{Pane, fav_editor, lists};
use crate::kit::{mode, overlay::Overlay};
use crate::library_ui::{
    backup, custom_groups, epg_sources_view, guide_assign_view, guide_inspect_view,
    guide_repair_view, organization_view, playlists_view, wizard_view,
};
use crate::playback::{browse_playback, external_player, multiview, multiview_picker, play_bar};
use crate::screens::{
    cast_pairing, channels_view, parental_view, recordings_view, search_view, settings_view,
    vod_view,
};
use crate::shell::{
    chooser, desktop, exit_confirmation, input, menu_view, remote_editor, startup_view,
};
use crate::{Ui, ViewMode};

/// How the host leaves the app after the viewer confirmed it, where it
/// asks first ([`confirm_leaving`]).
static LEAVE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Makes Back at the top level ask before the app leaves: a Yes/No prompt
/// (No focused; Back closes it), and Yes calls `leave`. For a host whose
/// platform asks for that (Samsung's Tizen TVs: an exit confirmation on
/// Return at the app's first screen, `hosts/tizen-nsh`); elsewhere the
/// platform's Back leaves at once. Only the first call counts; make it
/// before the app starts.
pub fn confirm_leaving(leave: fn()) {
    let _ = LEAVE.set(leave);
}

/// Whether Back at the top level asks before leaving ([`confirm_leaving`]).
fn asks_before_leaving() -> bool {
    LEAVE.get().is_some()
}

/// Where Back would leave the app: the prompt instead, where the host asks
/// first ([`confirm_leaving`]); `false` (leave) elsewhere.
pub(crate) fn ask_before_leaving(ui: Ui) -> bool {
    asks_before_leaving() && chooser::ask_to_leave(ui)
}

/// Yes in the prompt [`confirm_leaving`] asked for: the host leaves.
pub(crate) fn leave() {
    if let Some(leave) = LEAVE.get() {
        leave();
    }
}

/// Install the platform Back handler while there is an in-app step to take
/// or an exit confirmation to handle. A pending Long Back release takes
/// precedence over both. The default exit is delegated to the platform,
/// so Android handhelds can preview the gesture back to their launcher.
pub(crate) fn bind_back(ui: Ui) {
    let back_target = computed(move || {
        let panels = ui.playback.panels.with(|p| p.shown)
            != iptv_presentation::panels::Shown::Hidden
            || play_bar::shows(ui);
        let menu = ui.screens.menu.with(Option::is_some)
            || ui.screens.description.with(Option::is_some)
            || ui.screens.chooser.with(Option::is_some)
            || ui.screens.editor.with(Option::is_some)
            || ui.guide.focus.with(|f| f.hint)
            || channels_view::is_open(ui.playback)
            || multiview_picker::is_open(ui.playback);
        (
            ui.chrome.mode.get(),
            ui.guide.focus.with(|f| f.pane),
            panels || menu,
        )
    });
    effect(move || {
        let top = ui.top_overlay();
        let rail_root = top.is_none()
            && !desktop::fullscreen()
            && match back_target.get() {
                (ViewMode::Guide, Pane::Rail, false) => true,
                (ViewMode::Overlay, Pane::Rail, false) => ui.playback.playing.with(Option::is_none),
                _ => false,
            };
        let double_back = rail_root
            && exit_confirmation::enabled(ui.app)
            && lifecycle() == nsh_core::platform::Lifecycle::Active
            && !ui.playback.channel_number.with(|entry| entry.active());
        if !double_back {
            ui.chrome.exit_deadline.set(None);
        }
        let leaves = match top {
            Some(Overlay::Wizard) => wizard_view::back_leaves(ui.screens),
            Some(Overlay::StoreStartupFailure) => {
                startup_view::store_failure_back_leaves(ui.chrome)
            }
            _ => rail_root,
        } && !ui.playback.channel_number.with(|entry| entry.active())
            && !ui.chrome.back_guard.with(iptv_nav::BackGuard::armed);
        if !leaves || asks_before_leaving() || double_back {
            on_back_press(move || {
                input::swallow_back(ui)
                    || if double_back && leaves {
                        exit_confirmation::press(ui)
                    } else {
                        go_back(ui)
                    }
            });
        }
    });
}

/// One step back (the platform's Back, or a key standing in for it);
/// `false` where Back leaves the app. Where the host asks before leaving
/// ([`confirm_leaving`]), every such last step (the guide's rail, the
/// first run's welcome screen, the store failure screen) opens the prompt
/// instead, over every other layer.
pub(crate) fn go_back(ui: Ui) -> bool {
    external_player::cancel_pending(ui.cast);
    if ui.playback.channel_number.with(|entry| entry.active()) {
        ui.playback.channel_number.update(|entry| {
            entry.take();
        });
        return true;
    }
    ui.playback.pending_zap.set(None);
    browse_playback::activity(ui);
    untracked(|| {
        let top = ui.top_overlay();
        let panels = ui.playback.panels.with(|p| p.shown)
            != iptv_presentation::panels::Shown::Hidden
            || play_bar::shows(ui);
        let menu = ui.screens.menu.with(Option::is_some)
            || ui.screens.description.with(Option::is_some)
            || ui.screens.chooser.with(Option::is_some)
            || ui.screens.editor.with(Option::is_some);
        match (
            ui.chrome.mode.get(),
            ui.guide.focus.with(|f| f.pane),
            panels || menu,
        ) {
            _ if top == Some(Overlay::QuitConfirm) => desktop::quit_confirm::back(ui.chrome),
            _ if top == Some(Overlay::Leave) => {
                chooser::close(ui);
                true
            }
            _ if top == Some(Overlay::StoreStartupFailure) => {
                startup_view::store_failure_back(ui.chrome)
            }
            _ if top == Some(Overlay::Parental) => {
                parental_view::close(ui);
                true
            }
            _ if top == Some(Overlay::CastPairing) => {
                cast_pairing::close(ui.cast);
                true
            }
            _ if top == Some(Overlay::RestoreReview) => backup::back(ui.screens),
            _ if top == Some(Overlay::AppUpdateNotice) => {
                settings_view::updates::dismiss_notice(ui.screens);
                true
            }
            _ if top == Some(Overlay::AppUpdate) => {
                settings_view::updates::close(ui.screens);
                true
            }
            _ if top == Some(Overlay::Wizard) => wizard_view::back(ui),
            _ if top == Some(Overlay::Vod) => vod_view::back(ui),
            _ if top == Some(Overlay::Recordings) => recordings_view::back(ui.screens),
            _ if top == Some(Overlay::GuideSources) => epg_sources_view::back(ui),
            _ if top == Some(Overlay::GuideRepair) => guide_repair_view::back(ui),
            _ if top == Some(Overlay::GuideInspection) => guide_inspect_view::back(ui),
            _ if top == Some(Overlay::RemoteEditor) => remote_editor::back(ui.screens),
            _ if top == Some(Overlay::Organization) => organization_view::back(ui),
            _ if top == Some(Overlay::CustomGroups) => custom_groups::back(ui.screens),
            _ if top == Some(Overlay::Playlists) => playlists_view::back(ui),
            _ if top == Some(Overlay::Settings) => settings_view::back(ui),
            _ if top == Some(Overlay::GuideAssign) => guide_assign_view::back(ui),
            _ if top == Some(Overlay::FavoritesEditor) => {
                fav_editor::key(ui, iptv_nav::Action::Back);
                true
            }
            _ if top == Some(Overlay::Chooser) => {
                chooser::close(ui);
                true
            }
            _ if matches!(
                top,
                Some(Overlay::Menu | Overlay::PlaylistMenu | Overlay::Description)
            ) =>
            {
                menu_view::back(ui);
                true
            }
            _ if top == Some(Overlay::Search) => {
                search_view::apply(ui, iptv_presentation::search::SearchAction::Back);
                true
            }
            _ if top == Some(Overlay::ChannelList) => {
                channels_view::back(ui);
                true
            }
            _ if top == Some(Overlay::ChannelPicker) => {
                multiview_picker::key(ui, iptv_nav::Action::Back);
                true
            }
            // The operating system's full screen is restored first; the
            // next Back leaves the video for where it was chosen.
            _ if ui.playback.player_surface == 0 && desktop::fullscreen() => {
                desktop::set_fullscreen(false);
                true
            }
            (ViewMode::Fullscreen, _, false)
                if ui.playback.player.recorded.with(Option::is_some) =>
            {
                recordings_view::open(ui);
                true
            }
            // A movie or an episode: back to its details (#147).
            (ViewMode::Fullscreen, _, false) if vod_view::return_from_player(ui) => true,
            // A dialog closes, else the active tile goes full screen.
            _ if multiview::is_open(ui.playback) => multiview::back(ui),
            // A tile watched full screen: back to the grid as it was.
            _ if multiview::detour(ui.playback) => multiview::resume(ui),
            (ViewMode::Fullscreen, _, true) => {
                ui.playback.panels.update(|p| p.hide());
                ui.playback
                    .play_bar
                    .update(iptv_presentation::play_bar::PlayBar::hide);
                true
            }
            // Hint, past program → now, guide → groups → rail → leave.
            (ViewMode::Overlay, _, _) if ui.playback.playing.get_untracked().is_some() => {
                ui.chrome.mode.apply(mode::Action::Back);
                true
            }
            (ViewMode::Guide | ViewMode::Overlay, _, _) => lists::back(ui),
            _ => {
                ui.chrome.mode.apply(mode::Action::Back);
                true
            }
        }
    }) || ask_before_leaving(ui)
}
