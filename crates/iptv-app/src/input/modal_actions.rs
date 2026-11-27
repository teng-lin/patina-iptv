//! Actions consumed by the visible modal or overlay.
use super::{Action, Ui};
use crate::overlays::Overlay;

pub(super) fn apply(ui: Ui, action: Action) -> bool {
    let top = ui.top_overlay();
    match top {
        Some(Overlay::QuitConfirm) => crate::desktop::quit_confirm::key(ui, action),
        Some(Overlay::StoreStartupFailure) => crate::startup_view::store_failure_key(ui, action),
        Some(Overlay::Vod) => crate::vod_view::key(ui, action),
        Some(Overlay::Parental) => crate::parental_view::key(ui, action),
        Some(Overlay::CastPairing) => crate::cast_pairing::key(ui, action),
        Some(Overlay::RestoreReview) => crate::backup::key(ui, action),
        Some(Overlay::Recordings) => crate::recordings_view::key(ui, action),
        Some(Overlay::Wizard) => crate::wizard_view::key(ui, action),
        Some(Overlay::GuideRepair) => crate::guide_repair_view::key(ui, action),
        Some(Overlay::GuideInspection) => crate::guide_inspect_view::key(ui, action),
        Some(Overlay::GuideSources) => crate::epg_sources_view::key(ui, action),
        Some(Overlay::RemoteEditor) => crate::remote_editor::key(ui, action),
        Some(Overlay::Organization) => crate::organization_view::key(ui, action),
        Some(Overlay::CustomGroups) => crate::custom_groups::key(ui, action),
        Some(Overlay::Playlists) => crate::playlists_view::key(ui, action),
        Some(Overlay::AppUpdateNotice) => crate::settings_view::updates::notice_key(ui, action),
        Some(Overlay::AppUpdate) => crate::settings_view::updates::key(ui, action),
        Some(Overlay::Settings) => crate::settings_view::key(ui, action),
        Some(Overlay::GuideAssign) => crate::guide_assign_view::key(ui, action),
        Some(Overlay::FavoritesEditor) => crate::fav_editor::key(ui, action),
        Some(Overlay::ChannelPicker) => crate::multiview_picker::key(ui, action),
        Some(Overlay::ChannelList) => crate::channels_view::key(ui, action),
        Some(Overlay::Search) => crate::search_view::key(ui, action),
        Some(Overlay::Chooser | Overlay::Leave) => chooser(ui, action, top == Some(Overlay::Leave)),
        Some(Overlay::Description) => description(ui, action),
        Some(Overlay::Menu | Overlay::PlaylistMenu) => menu(ui, action),
        _ => return false,
    }
    true
}

fn chooser(ui: Ui, action: Action, leave: bool) {
    match action {
        Action::MoveUp => crate::chooser::move_focus(ui, -1),
        Action::MoveDown => crate::chooser::move_focus(ui, 1),
        Action::MoveLeft if leave => crate::chooser::focus_leave(ui, false),
        Action::MoveRight if leave => crate::chooser::focus_leave(ui, true),
        Action::Select => {
            if let Some(i) = ui
                .screens
                .chooser
                .with(|c| c.as_ref().map(|c| c.list.focus))
            {
                crate::chooser::pick(ui, i);
            }
        }
        Action::Back | Action::ShowMenu => crate::chooser::close(ui),
        _ => {}
    }
}

fn description(ui: Ui, action: Action) {
    // Select and Record press the details' Record button, which only a
    // program on now shows (`recordings_view::record_details`).
    if matches!(action, Action::Select | Action::Record)
        && let Some(origin) = ui.screens.description.get_untracked()
    {
        crate::recordings_view::record_details(ui, origin);
        return;
    }
    // Back (or Menu) closes program details.
    match action {
        Action::MoveUp => crate::menu_view::scroll_description(ui, -1.0),
        Action::MoveDown => crate::menu_view::scroll_description(ui, 1.0),
        Action::Back => crate::menu_view::back(ui),
        Action::ShowMenu => crate::menu_view::close(ui),
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
                crate::menu_view::perform(ui, item);
            }
        }
        Action::Back => crate::menu_view::back(ui),
        Action::ShowMenu => crate::menu_view::close(ui),
        _ => {}
    }
}
