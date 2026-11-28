//! Actions consumed by the visible modal or overlay.
use super::{Action, Ui};
use crate::shell::overlays::Overlay;

pub(super) fn apply(ui: Ui, action: Action) -> bool {
    let top = ui.top_overlay();
    match top {
        Some(Overlay::QuitConfirm) => crate::shell::desktop::quit_confirm::key(ui, action),
        Some(Overlay::StoreStartupFailure) => {
            crate::shell::startup_view::store_failure_key(ui, action)
        }
        Some(Overlay::Vod) => crate::screens::vod_view::key(ui, action),
        Some(Overlay::Parental) => crate::screens::parental_view::key(ui, action),
        Some(Overlay::CastPairing) => crate::screens::cast_pairing::key(ui, action),
        Some(Overlay::RestoreReview) => crate::library_ui::backup::key(ui, action),
        Some(Overlay::Recordings) => crate::screens::recordings_view::key(ui, action),
        Some(Overlay::Wizard) => crate::library_ui::wizard_view::key(ui, action),
        Some(Overlay::GuideRepair) => crate::library_ui::guide_repair_view::key(ui, action),
        Some(Overlay::GuideInspection) => crate::library_ui::guide_inspect_view::key(ui, action),
        Some(Overlay::GuideSources) => crate::library_ui::epg_sources_view::key(ui, action),
        Some(Overlay::RemoteEditor) => crate::shell::remote_editor::key(ui, action),
        Some(Overlay::Organization) => crate::library_ui::organization_view::key(ui, action),
        Some(Overlay::CustomGroups) => crate::library_ui::custom_groups::key(ui, action),
        Some(Overlay::Playlists) => crate::library_ui::playlists_view::key(ui, action),
        Some(Overlay::AppUpdateNotice) => {
            crate::screens::settings_view::updates::notice_key(ui.screens, action)
        }
        Some(Overlay::AppUpdate) => crate::screens::settings_view::updates::key(ui, action),
        Some(Overlay::Settings) => crate::screens::settings_view::key(ui, action),
        Some(Overlay::GuideAssign) => crate::library_ui::guide_assign_view::key(ui, action),
        Some(Overlay::FavoritesEditor) => crate::guide::fav_editor::key(ui, action),
        Some(Overlay::ChannelPicker) => crate::playback::multiview_picker::key(ui, action),
        Some(Overlay::ChannelList) => crate::screens::channels_view::key(ui, action),
        Some(Overlay::Search) => crate::screens::search_view::key(ui, action),
        Some(Overlay::Chooser | Overlay::Leave) => chooser(ui, action, top == Some(Overlay::Leave)),
        Some(Overlay::Description) => description(ui, action),
        Some(Overlay::Menu | Overlay::PlaylistMenu) => menu(ui, action),
        None => return false,
    }
    true
}

fn chooser(ui: Ui, action: Action, leave: bool) {
    match action {
        Action::MoveUp => crate::shell::chooser::move_focus(ui.screens, -1),
        Action::MoveDown => crate::shell::chooser::move_focus(ui.screens, 1),
        Action::MoveLeft if leave => crate::shell::chooser::focus_leave(ui.screens, false),
        Action::MoveRight if leave => crate::shell::chooser::focus_leave(ui.screens, true),
        Action::Select => {
            if let Some(i) = ui
                .screens
                .chooser
                .with(|c| c.as_ref().map(|c| c.list.focus))
            {
                crate::shell::chooser::pick(ui, i);
            }
        }
        Action::Back | Action::ShowMenu => crate::shell::chooser::close(ui),
        _ => {}
    }
}

fn description(ui: Ui, action: Action) {
    // Select and Record press the details' Record button, which only a
    // program on now shows (`recordings_view::record_details`).
    if matches!(action, Action::Select | Action::Record)
        && let Some(origin) = ui.screens.description.get_untracked()
    {
        crate::screens::recordings_view::record_details(ui, origin);
        return;
    }
    // Back (or Menu) closes program details.
    match action {
        Action::MoveUp => crate::shell::menu_view::scroll_description(ui.screens, -1.0),
        Action::MoveDown => crate::shell::menu_view::scroll_description(ui.screens, 1.0),
        Action::Back => crate::shell::menu_view::back(ui),
        Action::ShowMenu => crate::shell::menu_view::close(ui),
        _ => {}
    }
}

fn menu(ui: Ui, action: Action) {
    match action {
        Action::MoveUp => ui
            .screens
            .menu
            .update(|m| m.as_mut().into_iter().for_each(|m| m.move_focus(-1))),
        Action::MoveDown => ui
            .screens
            .menu
            .update(|m| m.as_mut().into_iter().for_each(|m| m.move_focus(1))),
        Action::MoveLeft | Action::MoveRight => ui.screens.menu.update(|menu| {
            if let Some(menu) = menu {
                menu.step_group_offset(if action == Action::MoveLeft { -1 } else { 1 });
            }
        }),
        Action::Select => {
            if let Some(item) = ui
                .screens
                .menu
                .with(|m| m.as_ref().and_then(|m| m.focused()))
            {
                crate::shell::menu_view::perform(ui, item);
            }
        }
        Action::Back => crate::shell::menu_view::back(ui),
        Action::ShowMenu => crate::shell::menu_view::close(ui),
        _ => {}
    }
}
