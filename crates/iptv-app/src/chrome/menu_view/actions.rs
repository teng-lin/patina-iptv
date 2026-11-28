//! Menu action admission, revalidation, and dispatch.
use super::*;

pub fn perform(ui: Ui, item: MenuItem) {
    let Some((target, list, offset, playing_context)) = ui.screens.menu.with(|menu| {
        let menu = menu.as_ref()?;
        menu.rows
            .contains(&Row::Action {
                item,
                enabled: true,
            })
            .then(|| {
                (
                    menu.target.clone(),
                    menu.list.clone(),
                    menu.group_offset,
                    menu.playing_context,
                )
            })
    }) else {
        return;
    };
    let playlist_order = match item {
        MenuItem::PlaylistOrderAdded => Some("added"),
        MenuItem::PlaylistOrderByName => Some("name"),
        MenuItem::PlaylistOrderManual => Some("manual"),
        _ => None,
    };
    if let Some(value) = playlist_order {
        ui.screens.menu.set(None);
        crate::wiring::navigation::settings_guarded(ui.screens, move || {
            if let Err(error) = crate::screens::settings_view::save_setting(
                iptv_settings::keys::PLAYLIST_SORT.as_str(),
                iptv_settings::Value::Enum(value.to_owned()),
            ) {
                crate::screens::settings_view::notify_setting_error(error);
            }
        });
        return;
    }
    if let MenuItem::TogglePlaylist(id) = item {
        focus_item(ui.screens, item);
        if playlist_by_id(id).is_some_and(|p| p.policy.enabled) {
            crate::screens::settings_view::set_playlist_visibility(ui, id, false);
        }
        return;
    }
    if offset.is_some() && item != MenuItem::SaveEpgOffset {
        ui.screens.menu.update(|menu| {
            if let Some(menu) = menu {
                match item {
                    MenuItem::DecreaseEpgOffset => menu.step_group_offset(-1),
                    MenuItem::IncreaseEpgOffset => menu.step_group_offset(1),
                    MenuItem::ResetEpgOffset => menu.group_offset = Some(0),
                    _ => {}
                }
            }
        });
        return;
    }
    let parent = matches!(
        item,
        MenuItem::ProgramActions | MenuItem::GroupSettings | MenuItem::GroupEpgOffset
    )
    .then(|| ui.screens.menu.get_untracked())
    .flatten()
    .map(|mut parent| {
        // Touch and pointer activation do not move key focus first.
        // Back returns to the action that opened the full options.
        parent.focus_on(item);
        parent
    });
    ui.screens.menu.set(None);
    let scope = list
        .resolve(&*crate::wiring::runtime_owner::schedule())
        .and_then(|_| channel_scope(ui.app, &list));
    if target == Target::Group && scope.is_none() && item != MenuItem::ReorderPlaylists {
        return;
    }
    let channel = menu_channel(&target);
    if channel.is_none() && target != Target::Group {
        return;
    }
    dispatch(
        ui,
        item,
        Selection {
            target,
            list,
            scope,
            channel,
            offset,
            playing_context,
            parent,
        },
    );
}

struct Selection {
    target: Target,
    list: ListId,
    scope: Option<(SourceId, Kind)>,
    channel: Option<usize>,
    offset: Option<i32>,
    playing_context: bool,
    parent: Option<Menu>,
}

fn dispatch(ui: Ui, item: MenuItem, selection: Selection) {
    let Selection {
        target,
        list,
        scope,
        channel,
        offset,
        playing_context,
        parent,
    } = selection;
    if item == MenuItem::ProgramActions {
        open_child_program(ui, target, list, playing_context, parent);
        return;
    }
    if let Target::Channel {
        program, offset, ..
    } = target
        && matches!(
            item,
            MenuItem::RecordNow | MenuItem::CancelRecording | MenuItem::StopRecording
        )
    {
        let Some(origin) = program_origin(&target) else {
            return;
        };
        let schedule = crate::wiring::runtime_owner::schedule();
        if crate::playback::player::guide_offset(&*schedule, origin.channel) != offset
            || origin_program(&*schedule, origin) != program
            || !crate::screens::recordings_view::perform_channel_action(ui, origin, item)
        {
            open_channel_at(ui, origin.channel, program.start, list, playing_context);
        }
        return;
    }
    match item {
        MenuItem::ChannelActions => {
            if let Some(origin) = program_origin(&target) {
                open_channel_at(
                    ui,
                    origin.channel,
                    origin.program_start,
                    list,
                    playing_context,
                );
            }
        }
        MenuItem::BlockChannel | MenuItem::BlockGroup => {
            if item == MenuItem::BlockChannel
                && channel.is_some_and(|ch| {
                    crate::wiring::runtime_owner::schedule().channel_block(ch) == Block::Group
                })
            {
                crate::chrome::toast::notify(tr("This channel is blocked by a group."));
                return;
            }
            if let Some(target) = block_target(ui, item, channel, scope.as_ref()) {
                let kind = if item == MenuItem::BlockGroup {
                    BlockKind::Group
                } else {
                    BlockKind::Channel
                };
                let blocked = !directly_blocked(&list, item, channel);
                organization_view::set_block(ui, SourceId(target.source), kind, target.id, blocked);
            }
        }
        MenuItem::ProtectChannel | MenuItem::ProtectGroup => {
            if let Some(target) = block_target(ui, item, channel, scope.as_ref()) {
                crate::screens::parental_view::block(ui, target, true);
            }
        }
        MenuItem::ManageParentalBlocks => crate::screens::parental_view::manage(ui.screens),
        MenuItem::ReorderChannels => {
            if let Some((source, kind)) = scope {
                let focus = channel
                    .and_then(|ch| crate::screens::parental_view::live_target(ui.app, ch))
                    .filter(|target| target.source == source.0)
                    .map(|target| target.id);
                organization_view::open_context_reorder(ui, source, kind, focus, list);
            }
        }
        MenuItem::ManageBlocked | MenuItem::ManageHidden => {
            let mode = if item == MenuItem::ManageBlocked {
                Mode::Blocking
            } else {
                Mode::Visibility
            };
            if let Some(channel) = channel {
                open_channel_editor(ui, channel, mode, scope);
            } else if let Some((source, kind)) = scope {
                organization_view::open_mode(ui, source, kind, None, mode);
            }
        }
        MenuItem::Record
        | MenuItem::StopRecording
        | MenuItem::CancelRecording
        | MenuItem::Description
        | MenuItem::StartFromBeginning => {
            if let Some(origin) = program_origin(&target) {
                perform_program(ui, item, origin, list, playing_context);
            }
        }
        MenuItem::WatchInNewWindow => {
            if let Some(channel) = channel {
                crate::chrome::desktop::player_windows::open(ui, channel);
            }
        }
        MenuItem::ManageFavorites => crate::guide::fav_editor::open(ui, &list, channel),
        MenuItem::Search => crate::screens::search_view::open(ui),
        MenuItem::Settings => crate::screens::settings_view::open(ui),
        MenuItem::ToggleFavorite => {
            if let Some(channel) = channel {
                crate::guide::favorites::toggle(ui, channel);
            }
        }
        MenuItem::HideChannel => hide_channel(ui.app, channel),
        MenuItem::CreateGroup => {
            if let Some(source) = management_source(ui.app, &list, scope.as_ref()) {
                crate::library_ui::custom_groups::open(ui, source);
            }
        }
        MenuItem::CopyChannels => {
            if let Some((source, channels)) = copy_scope(ui.app, &list, scope.as_ref()) {
                crate::library_ui::custom_groups::open_copy(ui, source, channels);
            }
        }
        MenuItem::ChannelOptions => {
            if let Some(channel) = channel {
                open_channel_editor(ui, channel, Mode::Options, scope);
            }
        }
        MenuItem::SortChannels => {
            if let Some((source, kind)) = scope {
                organization_view::open_sort(ui, source, kind, Some(list));
            }
        }
        MenuItem::GroupOptions
        | MenuItem::GroupSettings
        | MenuItem::ShowFavoritesOnly
        | MenuItem::GroupExternalPlayer
        | MenuItem::StopExternalPlayer
        | MenuItem::SaveEpgOffset
        | MenuItem::GroupEpgOffset
        | MenuItem::ReorderPlaylists
        | MenuItem::RenameGroup
        | MenuItem::ResetGroupName
        | MenuItem::SortGroups
        | MenuItem::ReorderGroups
        | MenuItem::ManageGroups
        | MenuItem::HideGroup => group(ui, item, list, scope, offset, parent),
        // The chooser opens over the guide; the guide's focus stays.
        MenuItem::AssignEpg => {
            if let Some(channel) = channel {
                crate::library_ui::guide_assign_view::open(ui, channel);
            }
        }
        other => {
            log::info!("iptv menu {other:?} not implemented yet");
        }
    }
}

fn focus_item(screens: crate::ScreensUi, item: MenuItem) {
    screens.menu.update(|menu| {
        if let Some(menu) = menu {
            menu.focus_on(item);
        }
    });
}

fn perform_program(ui: Ui, item: MenuItem, origin: Origin, list: ListId, playing_context: bool) {
    match item {
        MenuItem::Record => crate::screens::recordings_view::record_origin(ui, origin),
        MenuItem::StopRecording | MenuItem::CancelRecording => {
            let program = origin_program(&*crate::wiring::runtime_owner::schedule(), origin);
            if program.start == origin.program_start {
                crate::screens::recordings_view::perform_channel_action(ui, origin, item);
            }
        }
        MenuItem::Description => ui.screens.description.set(Some(origin)),
        MenuItem::StartFromBeginning => {
            let schedule = crate::wiring::runtime_owner::schedule();
            let now = ui.guide.focus.with_untracked(|focus| focus.now);
            let program = origin_program(&*schedule, origin);
            if program.start != origin.program_start
                || !program.can_replay(now, schedule.catchup_days(origin.channel))
                || !crate::playback::player::allow_guide_channel(origin.channel)
            {
                return;
            }
            crate::playback::archive_playback::select(
                ui,
                list,
                origin.channel,
                program,
                playing_context,
            );
        }
        _ => {}
    }
}

fn open_child_program(
    ui: Ui,
    target: Target,
    list: ListId,
    playing_context: bool,
    parent: Option<Menu>,
) {
    let Some(origin) = program_origin(&target) else {
        return;
    };
    let Target::Channel {
        program, offset, ..
    } = &target
    else {
        return;
    };
    let schedule = crate::wiring::runtime_owner::schedule();
    if origin_program(&*schedule, origin) != *program
        || crate::playback::player::guide_offset(&*schedule, origin.channel) != *offset
    {
        open_channel_at(
            ui,
            origin.channel,
            origin.program_start,
            list,
            playing_context,
        );
        return;
    }
    open_program_at(
        ui,
        origin.channel,
        origin.program_start,
        list,
        playing_context,
    );
    ui.screens.menu.update(|menu| {
        if let Some(menu) = menu {
            menu.parent = parent.map(Box::new);
        }
    });
}

fn hide_channel(app: crate::SharedUi, channel: Option<usize>) {
    if let Some(target) = channel.and_then(|ch| crate::screens::parental_view::live_target(app, ch))
    {
        let source = SourceId(target.source);
        let saved = crate::runtime::commands::hide_channel(
            &crate::wiring::runtime_owner::runtime(),
            source,
            target.id,
        );
        crate::wiring::tasks::spawn_in_app(async move {
            match saved.await {
                Ok(()) => crate::wiring::data::organization_changed(
                    &crate::wiring::runtime_owner::runtime(),
                    source,
                ),
                Err(_) => crate::chrome::toast::notify(tr(
                    "The change could not be saved. Reopen the list and try again.",
                )),
            }
        });
    }
}

fn group(
    ui: Ui,
    item: MenuItem,
    list: ListId,
    scope: Option<(SourceId, Kind)>,
    offset: Option<i32>,
    parent: Option<Menu>,
) {
    match item {
        MenuItem::GroupOptions => {
            show_group_options(ui, list, GroupMenuEntry::Program);
        }
        MenuItem::GroupSettings => open_group_settings(ui.screens, list, parent),
        MenuItem::ShowFavoritesOnly
        | MenuItem::GroupExternalPlayer
        | MenuItem::StopExternalPlayer
        | MenuItem::SaveEpgOffset => {
            if let Some(target) = group_target(scope.as_ref()) {
                let current = list_preferences(&list);
                let preference = match (item, offset) {
                    (MenuItem::ShowFavoritesOnly, _) => {
                        GroupPreference::FavoritesOnly(!current.favorites_only)
                    }
                    (MenuItem::GroupExternalPlayer, _) => {
                        GroupPreference::ExternalPlayer(!current.external_player)
                    }
                    (MenuItem::StopExternalPlayer, _) => GroupPreference::ExternalPlayer(false),
                    (MenuItem::SaveEpgOffset, Some(offset)) => {
                        GroupPreference::EpgOffsetMinutes(offset)
                    }
                    _ => return,
                };
                set_group_preference(target, preference);
            }
        }
        MenuItem::GroupEpgOffset => {
            let minutes = list_preferences(&list).epg_offset_minutes;
            let mut menu = group_offset_menu(list, minutes);
            menu.parent = parent.map(Box::new);
            ui.screens.menu.set(Some(menu));
        }
        MenuItem::ReorderPlaylists => {
            let preferred = heading_playlist(&list);
            crate::wiring::navigation::settings_guarded(ui.screens, move || {
                organization_view::open_playlist_reorder(ui, preferred);
            });
        }
        MenuItem::RenameGroup => {
            if let Some(target) = group_target(scope.as_ref()) {
                organization_view::rename_group(ui, SourceId(target.source), target.id);
            }
        }
        MenuItem::ResetGroupName => {
            if let Some(target) = group_target(scope.as_ref()) {
                let source = SourceId(target.source);
                let owner = crate::wiring::runtime_owner::runtime();
                let save = crate::runtime::commands::rename_organization_item(
                    &owner,
                    source,
                    Kind::Groups,
                    target.id,
                    String::new(),
                );
                crate::wiring::tasks::spawn_in_app(async move {
                    match save.await {
                        Ok(()) => crate::wiring::data::organization_changed(&owner, source),
                        Err(_) => crate::chrome::toast::notify(tr(
                            "The change could not be saved. Reopen the list and try again.",
                        )),
                    }
                });
            }
        }
        MenuItem::SortGroups | MenuItem::ReorderGroups | MenuItem::ManageGroups => {
            if let Some((source, _)) = scope.as_ref() {
                let focus = group_target(scope.as_ref()).map(|target| target.id);
                match item {
                    MenuItem::SortGroups => {
                        organization_view::open_sort(ui, *source, Kind::Groups, Some(list));
                    }
                    MenuItem::ReorderGroups => {
                        organization_view::open_group_reorder(ui, *source, focus, list);
                    }
                    _ => organization_view::open(ui, *source, Kind::Groups, focus),
                }
            }
        }
        MenuItem::HideGroup => {
            if let Some(target) = group_target(scope.as_ref()) {
                let source = SourceId(target.source);
                let owner = crate::wiring::runtime_owner::runtime();
                let save = crate::runtime::commands::hide_group(&owner, source, target.id);
                crate::wiring::tasks::spawn_in_app(async move {
                    match save.await {
                        Ok(()) => crate::wiring::data::organization_changed(&owner, source),
                        Err(_) => crate::chrome::toast::notify(tr(
                            "The change could not be saved. Reopen the list and try again.",
                        )),
                    }
                });
            }
        }
        _ => {}
    }
}
