//! The guide's context menus as scrolling side panels, and
//! the program description popover. State lives in `iptv_presentation::menu`;
//! this opens, draws and performs it. Labels are iptv-rs's own wording.

use crate::i18n::tr;
use crate::organization_view::{self, Kind};
use crate::runtime::model::SourceId;
use iptv_core::guide::GroupName;
use iptv_core::organization::{Block, BlockKind, GroupPreference, GroupPreferences};
use iptv_library::ChannelScope;
use iptv_presentation::lists::{Entry, ListId};
use iptv_presentation::organization::Mode;

use iptv_presentation::menu::{
    Heading, Menu, MenuItem, Origin, Row, Target, channel_menu, group_offset_menu, group_options,
    program_menu,
};
use nsh_core::prelude::*;

use crate::fonts::{HEADER_BODY, HEADER_TIMES, LIST_ITEM, MEDIUM, SANS, SECTION, TITLE};
use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;
use crate::theme::{LIST_TEXT, RADIUS_LARGE};
use crate::{Ui, ViewMode, channel_name, clock, tint};

/// The panel's width at full size (scaled with the guide).
const PANEL_W: f32 = 400.0;

/// The actions this build can perform; the rest show disabled.
fn implemented(item: MenuItem, surface: usize) -> bool {
    if item == MenuItem::StartFromBeginning && surface != 0 {
        return false; // Auxiliary sessions currently play live streams only.
    }
    matches!(
        item,
        MenuItem::Search
            | MenuItem::Record
            | MenuItem::Settings
            | MenuItem::Description
            | MenuItem::BlockChannel
            | MenuItem::BlockGroup
            | MenuItem::ProtectChannel
            | MenuItem::ProtectGroup
            | MenuItem::ManageParentalBlocks
            | MenuItem::ManageBlocked
            | MenuItem::ToggleFavorite
            | MenuItem::StartFromBeginning
            | MenuItem::ManageFavorites
            | MenuItem::AssignEpg
            | MenuItem::HideChannel
            | MenuItem::HideGroup
            | MenuItem::ManageHidden
            | MenuItem::ReorderChannels
            | MenuItem::SortChannels
            | MenuItem::ChannelActions
            | MenuItem::ProgramActions
            | MenuItem::ChannelOptions
            | MenuItem::GroupOptions
            | MenuItem::CopyChannels
            | MenuItem::CreateGroup
    )
}

fn label(item: MenuItem, favorite: bool, blocked: bool) -> &'static str {
    use MenuItem::*;
    match item {
        Search => tr("Search"),
        Settings => tr("Settings"),
        StartFromBeginning => tr("Play from the beginning"),
        WatchInNewWindow => tr("Watch in new window"),
        Record => tr("Record"),
        RecordNow => tr("Record now"),
        CancelRecording => tr("Cancel recording"),
        StopRecording => tr("Stop recording"),
        Description => tr("Details"),
        ToggleFavorite if favorite => tr("Remove from favorites"),
        ToggleFavorite => tr("Add to favorites"),
        BlockChannel if blocked => tr("Unblock channel"),
        BlockChannel => tr("Block channel"),
        BlockGroup if blocked => tr("Unblock group"),
        BlockGroup => tr("Block group"),
        ProtectChannel => tr("Protect channel with PIN"),
        ProtectGroup => tr("Protect group with PIN"),
        ManageParentalBlocks => tr("Manage PIN-protected content…"),
        HideChannel => tr("Hide channel"),
        AssignEpg => tr("Choose guide data…"),
        ChannelOptions => tr("Channel options…"),
        ChannelActions => tr("Channel actions…"),
        ProgramActions => tr("More options"),
        ManageFavorites => tr("Manage favorites…"),
        ManageBlocked => tr("Manage blocked channels…"),
        ManageHidden => tr("Manage hidden channels…"),
        ReorderChannels => tr("Reorder channels"),
        SortChannels => tr("Channel order"),
        CopyChannels => tr("Copy channels to a group…"),
        CreateGroup => tr("New group…"),
        GroupOptions => tr("Group options…"),
        SortGroups => tr("Group order"),
        ReorderGroups => tr("Reorder groups"),
        ManageGroups => tr("Manage groups"),
        RenameGroup => tr("Rename group"),
        RenamePlaylist => tr("Rename playlist"),
        ReorderPlaylists => tr("Reorder playlists"),
        ResetGroupName => tr("Restore group name"),
        ShowFavoritesOnly => tr("Show favorites only"),
        GroupExternalPlayer => tr("Use external player"),
        StopExternalPlayer => tr("Turn off external player"),
        GroupEpgOffset => tr("Guide time offset"),
        DecreaseEpgOffset => tr("15 minutes earlier"),
        IncreaseEpgOffset => tr("15 minutes later"),
        ResetEpgOffset => tr("Reset to zero"),
        SaveEpgOffset => tr("Save"),
        HideGroup => tr("Hide group"),
    }
}

fn title(channel: usize, p: crate::guide::Program) -> String {
    crate::listing_labels::listing(crate::runtime_owner::schedule().listing(channel, p))
}

/// The full menu reached from the program header (in full screen, the
/// playing channel's current program).
pub fn open(ui: Ui) {
    let now = ui.focus.with(|f| f.now);
    let playing_context =
        ui.mode.get_untracked() == ViewMode::Fullscreen && ui.playing.get_untracked().is_some();
    let (channel, minute) = match (ui.mode.get(), ui.playing.get()) {
        (ViewMode::Fullscreen, Some(ch)) => (ch, now),
        _ => {
            let Some(ch) = crate::lists::focused_channel(ui) else {
                return; // an empty list
            };
            (ch, ui.focus.with(|f| f.cursor))
        }
    };
    open_for(ui, channel, minute);
    ui.menu.update(|menu| {
        if let Some(menu) = menu {
            menu.playing_context = playing_context;
        }
    });
}

pub fn open_for(ui: Ui, channel: usize, minute: i32) {
    open_program_at(ui, channel, minute, current_list(ui), false);
}

fn open_program_at(ui: Ui, channel: usize, minute: i32, list: ListId, playing_context: bool) {
    let schedule = crate::runtime_owner::schedule();
    let Some(key) = schedule.channel_key(channel) else {
        return;
    };
    let program = schedule.program_at(channel, minute);
    let origin = Origin {
        channel,
        program_start: program.start,
    };
    let rows = program_rows(ui, origin, program, &list);
    crate::recordings_view::remember(ui, origin);
    let target = Target::Program {
        channel: key,
        start: program.start,
    };
    let mut menu = Menu::new(rows, target, list);
    menu.playing_context = playing_context;
    ui.menu.set(Some(menu));
}

fn program_rows(ui: Ui, origin: Origin, program: crate::guide::Program, list: &ListId) -> Vec<Row> {
    let now = ui.focus.with(|focus| focus.now);
    let channel = origin.channel;
    let schedule = crate::runtime_owner::schedule();
    let scope = channel_scope(ui, list);
    let recording = crate::recordings_view::channel_menu_action(ui, origin).map(|action| {
        if action == MenuItem::RecordNow {
            MenuItem::Record
        } else {
            action
        }
    });
    let mut rows = program_menu(
        program,
        schedule.catchup_days(channel),
        now,
        recording,
        |item| match item {
            MenuItem::Record | MenuItem::StopRecording | MenuItem::CancelRecording => true,
            MenuItem::ManageHidden | MenuItem::ManageBlocked | MenuItem::ReorderChannels => {
                scope.is_some()
            }
            MenuItem::GroupOptions => scope.is_some(),
            MenuItem::SortChannels => scope
                .as_ref()
                .is_some_and(|(_, kind)| *kind != Kind::Favorites),
            MenuItem::CopyChannels => copy_scope(ui, list, scope.as_ref()).is_some(),
            MenuItem::CreateGroup => management_source(ui, list, scope.as_ref()).is_some(),
            _ => implemented(item, ui.player_surface),
        },
    );
    let auxiliary = ui.player_surface != 0;
    if auxiliary {
        // Auxiliary player windows play live streams only.
        rows.retain(|row| {
            !matches!(
                row,
                Row::Action {
                    item: MenuItem::StartFromBeginning,
                    ..
                }
            )
        });
    }
    if group_target(scope.as_ref()).is_none() {
        rows.retain(|row| {
            !matches!(
                row,
                Row::Action {
                    item: MenuItem::BlockGroup | MenuItem::HideGroup | MenuItem::ProtectGroup,
                    ..
                }
            )
        });
    }
    // Only the main window opens player windows.
    if (crate::host::LINUX || crate::host::MACOS)
        && !auxiliary
        && program.airing(now) == crate::guide::Airing::Live
    {
        rows.insert(
            3,
            Row::Action {
                item: MenuItem::WatchInNewWindow,
                enabled: crate::desktop::player_windows::can_open(ui, channel),
            },
        );
    }
    rows
}

/// The player list has no timeline cursor. Its channel actions use the live block.
pub fn open_channel(ui: Ui, channel: usize) {
    open_channel_at(
        ui,
        channel,
        crate::clock::now_minute(),
        current_list(ui),
        false,
    );
}

fn open_channel_at(ui: Ui, channel: usize, minute: i32, list: ListId, playing_context: bool) {
    let schedule = crate::runtime_owner::schedule();
    let Some(key) = schedule.channel_key(channel) else {
        return;
    };
    let program = schedule.program_at(channel, minute);
    let origin = Origin {
        channel,
        program_start: program.start,
    };
    let offset = crate::player::guide_offset(&*schedule, channel);
    let rows = channel_rows(ui, origin, &list, true);
    let mut menu = Menu::new(
        rows,
        Target::Channel {
            channel: key,
            program,
            offset,
        },
        list,
    );
    menu.playing_context = playing_context;
    ui.menu.set(Some(menu));
}

fn channel_rows(ui: Ui, origin: Origin, list: &ListId, current: bool) -> Vec<Row> {
    // Only the main window opens player windows.
    let new_window = ((crate::host::LINUX || crate::host::MACOS) && ui.player_surface == 0)
        .then(|| crate::desktop::player_windows::can_open(ui, origin.channel));
    let mut rows = channel_menu(
        current
            .then(|| crate::recordings_view::channel_menu_action(ui, origin))
            .flatten(),
        new_window,
        channel_scope(ui, list).is_some(),
    );
    if !current {
        for row in &mut rows {
            if let Row::Action {
                item: MenuItem::ProgramActions,
                enabled,
            } = row
            {
                *enabled = false;
            }
        }
    }
    rows
}

fn menu_channel(target: &Target) -> Option<usize> {
    crate::runtime_owner::schedule().find_channel(target.channel_key()?)
}

fn program_origin(target: &Target) -> Option<Origin> {
    let start = match target {
        Target::Program { start, .. } => *start,
        Target::Channel { program, .. } => program.start,
        Target::Group => return None,
    };
    Some(Origin {
        channel: menu_channel(target)?,
        program_start: start,
    })
}

pub(crate) fn invalidate_shifted_context(
    ui: Ui,
    before: &dyn crate::guide::Schedule,
    after: &dyn crate::guide::Schedule,
) {
    let shifted = |channel: usize| {
        before
            .channel_key(channel)
            .and_then(|key| after.find_channel(&key))
            .is_some_and(|next| {
                crate::player::guide_offset(before, channel)
                    != crate::player::guide_offset(after, next)
            })
    };
    if ui.menu.with_untracked(|menu| {
        menu.as_ref()
            .and_then(|menu| menu.target.channel_key())
            .and_then(|key| before.find_channel(key))
            .is_some_and(shifted)
    }) {
        ui.menu.set(None);
    }
    if ui
        .description
        .get_untracked()
        .is_some_and(|origin| shifted(origin.channel))
    {
        ui.description.set(None);
    }
    ui.recordings
        .update(|state| state.invalidate_context(shifted));
}

/// Back from full options restores their compact context and its focus.
pub fn back(ui: Ui) {
    if let Some(parent) = ui.menu.with(|menu| menu.as_ref()?.parent.clone()) {
        ui.menu.set(Some(*parent));
        return;
    }
    close(ui);
}

/// Dismisses the entire menu or popover, keeping its guide selection.
pub fn close(ui: Ui) {
    if let Some(list) = ui.menu.with(|menu| {
        let menu = menu.as_ref()?;
        menu.group_offset.map(|_| menu.list.clone())
    }) {
        if show_group_options(ui, list) {
            ui.menu.update(|menu| {
                if let Some(menu) = menu {
                    menu.focus = menu
                        .rows
                        .iter()
                        .position(|row| {
                            matches!(
                                row,
                                Row::Action {
                                    item: MenuItem::GroupEpgOffset,
                                    ..
                                }
                            )
                        })
                        .unwrap_or(menu.focus);
                }
            });
        } else {
            ui.menu.set(None);
        }
        return;
    }
    let origin = ui
        .menu
        .with(|m| {
            let menu = m.as_ref().filter(|menu| menu.list == current_list(ui))?;
            matches!(menu.target, Target::Program { .. })
                .then(|| program_origin(&menu.target))
                .flatten()
        })
        .or_else(|| ui.description.get());
    ui.menu.set(None);
    ui.description.set(None);
    if let Some(origin) = origin
        && ui.mode.get() != ViewMode::Fullscreen
        && ui.list.with(|list| list.row_of(origin.channel)).is_some()
    {
        crate::lists::tap(ui, origin.channel, origin.program_start);
    }
}

pub fn perform(ui: Ui, item: MenuItem) {
    let Some((target, list, offset, playing_context)) = ui.menu.with(|menu| {
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
    if offset.is_some() && item != MenuItem::SaveEpgOffset {
        ui.menu.update(|menu| {
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
    let parent = (item == MenuItem::ProgramActions)
        .then(|| ui.menu.get_untracked())
        .flatten()
        .map(|mut parent| {
            // Touch and pointer activation do not move key focus first.
            // Back returns to the action that opened the full options.
            if let Some(focus) = parent.rows.iter().position(|row| {
                *row == Row::Action {
                    item,
                    enabled: true,
                }
            }) {
                parent.focus = focus;
            }
            parent
        });
    ui.menu.set(None);
    let scope = list
        .resolve(&*crate::runtime_owner::schedule())
        .and_then(|_| channel_scope(ui, &list));
    if target == Target::Group && scope.is_none() {
        return;
    }
    let channel = menu_channel(&target);
    if channel.is_none() && target != Target::Group {
        return;
    }
    if item == MenuItem::ProgramActions {
        let Some(origin) = program_origin(&target) else {
            return;
        };
        let Target::Channel {
            program, offset, ..
        } = &target
        else {
            return;
        };
        let schedule = crate::runtime_owner::schedule();
        if origin_program(&*schedule, origin) != *program
            || crate::player::guide_offset(&*schedule, origin.channel) != *offset
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
        ui.menu.update(|menu| {
            if let Some(menu) = menu {
                menu.parent = parent.map(Box::new);
            }
        });
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
        let schedule = crate::runtime_owner::schedule();
        if crate::player::guide_offset(&*schedule, origin.channel) != offset
            || origin_program(&*schedule, origin) != program
            || !crate::recordings_view::perform_channel_action(ui, origin, item)
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
                    crate::runtime_owner::schedule().channel_block(ch) == Block::Group
                })
            {
                crate::settings_view::notify(tr("This channel is blocked by a group."));
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
                crate::parental_view::block(ui, target, true);
            }
        }
        MenuItem::ManageParentalBlocks => crate::parental_view::manage(ui),
        MenuItem::ReorderChannels => {
            if let Some((source, kind)) = scope {
                let focus = channel
                    .and_then(|ch| crate::parental_view::live_target(ui, ch))
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
                crate::desktop::player_windows::open(ui, channel);
            }
        }
        MenuItem::ManageFavorites => crate::fav_editor::open(ui, &list, channel),
        MenuItem::Search => crate::search_view::open(ui),
        MenuItem::Settings => crate::settings_view::open(ui),
        MenuItem::ToggleFavorite => {
            if let Some(channel) = channel {
                crate::favorites::toggle(ui, channel);
            }
        }
        MenuItem::HideChannel => {
            if let Some(target) = channel.and_then(|ch| crate::parental_view::live_target(ui, ch)) {
                let source = SourceId(target.source);
                let saved = crate::runtime::commands::hide_channel(
                    &crate::runtime_owner::runtime(),
                    source,
                    target.id,
                );
                crate::tasks::spawn_in_app(async move {
                    match saved.await {
                        Ok(()) => crate::data::organization_changed(
                            &crate::runtime_owner::runtime(),
                            source,
                        ),
                        Err(_) => crate::settings_view::notify(tr(
                            "The change could not be saved. Reopen the list and try again.",
                        )),
                    }
                });
            }
        }
        MenuItem::CreateGroup => {
            if let Some(source) = management_source(ui, &list, scope.as_ref()) {
                crate::custom_groups::open(ui, source);
            }
        }
        MenuItem::CopyChannels => {
            if let Some((source, channels)) = copy_scope(ui, &list, scope.as_ref()) {
                crate::custom_groups::open_copy(ui, source, channels);
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
        MenuItem::GroupOptions => {
            show_group_options(ui, list);
        }
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
            ui.menu.set(Some(group_offset_menu(list, minutes)));
        }
        MenuItem::RenamePlaylist | MenuItem::ReorderPlaylists => {
            if let Some(id) = heading_playlist(&list) {
                let only = if item == MenuItem::RenamePlaylist {
                    crate::playlists_view::Only::Rename
                } else {
                    crate::playlists_view::Only::Reorder
                };
                crate::playlists_view::open_only(ui, id, only);
            }
        }
        MenuItem::RenameGroup => {
            if let Some(target) = group_target(scope.as_ref()) {
                organization_view::rename_group(ui, SourceId(target.source), target.id);
            }
        }
        MenuItem::ResetGroupName => {
            if let Some((source, Kind::GroupChannels(id))) = scope {
                let owner = crate::runtime_owner::runtime();
                let save = crate::runtime::commands::rename_organization_item(
                    &owner,
                    source,
                    Kind::Groups,
                    id.as_str().to_owned(),
                    String::new(),
                );
                crate::tasks::spawn_in_app(async move {
                    match save.await {
                        Ok(()) => crate::data::organization_changed(&owner, source),
                        Err(_) => crate::settings_view::notify(tr(
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
                let owner = crate::runtime_owner::runtime();
                let save = crate::runtime::commands::hide_group(&owner, source, target.id);
                crate::tasks::spawn_in_app(async move {
                    match save.await {
                        Ok(()) => crate::data::organization_changed(&owner, source),
                        Err(_) => crate::settings_view::notify(tr(
                            "The change could not be saved. Reopen the list and try again.",
                        )),
                    }
                });
            }
        }
        // The chooser opens over the guide; the guide's focus stays.
        MenuItem::AssignEpg => {
            if let Some(channel) = channel {
                crate::guide_assign_view::open(ui, channel);
            }
        }
        other => {
            log::info!("iptv menu {other:?} not implemented yet");
        }
    }
}

fn perform_program(ui: Ui, item: MenuItem, origin: Origin, list: ListId, playing_context: bool) {
    match item {
        MenuItem::Record => crate::recordings_view::record_origin(ui, origin),
        MenuItem::StopRecording | MenuItem::CancelRecording => {
            let program = origin_program(&*crate::runtime_owner::schedule(), origin);
            if program.start == origin.program_start {
                crate::recordings_view::perform_channel_action(ui, origin, item);
            }
        }
        MenuItem::Description => ui.description.set(Some(origin)),
        MenuItem::StartFromBeginning => {
            let schedule = crate::runtime_owner::schedule();
            let now = ui.focus.with_untracked(|focus| focus.now);
            let program = origin_program(&*schedule, origin);
            if program.start != origin.program_start
                || !program.can_replay(now, schedule.catchup_days(origin.channel))
                || !crate::player::allow_guide_channel(origin.channel)
            {
                return;
            }
            crate::archive_playback::select(ui, list, origin.channel, program, playing_context);
        }
        _ => {}
    }
}

fn group_target(scope: Option<&(SourceId, Kind)>) -> Option<iptv_core::parental::Target> {
    let (source, kind) = scope?;
    let id = match kind {
        Kind::GroupChannels(id) => id.as_str().to_owned(),
        Kind::CustomChannels(id) => format!("custom:{}", id.0),
        _ => return None,
    };
    Some(iptv_core::parental::Target {
        source: source.0,
        kind: iptv_core::parental::ContentKind::Group,
        id,
    })
}

fn block_target(
    ui: Ui,
    item: MenuItem,
    channel: Option<usize>,
    scope: Option<&(SourceId, Kind)>,
) -> Option<iptv_core::parental::Target> {
    match item {
        MenuItem::BlockChannel | MenuItem::ProtectChannel => {
            crate::parental_view::live_target(ui, channel?)
        }
        MenuItem::BlockGroup | MenuItem::ProtectGroup => group_target(scope),
        _ => None,
    }
}

fn directly_blocked(list: &ListId, item: MenuItem, channel: Option<usize>) -> bool {
    let schedule = crate::runtime_owner::schedule();
    match item {
        MenuItem::BlockChannel => {
            channel.is_some_and(|ch| schedule.channel_block(ch) == Block::Channel)
        }
        MenuItem::BlockGroup => {
            matches!(list.resolve(&*schedule).map(Entry::at), Some(Entry::Group(group)) if schedule.group_blocked(group))
        }
        _ => false,
    }
}

fn management_source(ui: Ui, list: &ListId, scope: Option<&(SourceId, Kind)>) -> Option<SourceId> {
    if let Some((source, _)) = scope {
        Some(*source)
    } else if *list == ListId::Recent
        && !crate::data::combined::active(&crate::runtime_owner::runtime())
    {
        ui.app.library.shown.get_untracked()
    } else {
        None
    }
}

fn copy_scope(
    ui: Ui,
    list: &ListId,
    scope: Option<&(SourceId, Kind)>,
) -> Option<(SourceId, ChannelScope)> {
    let source = management_source(ui, list, scope)?;
    let channels = match scope.map(|(_, kind)| kind) {
        Some(Kind::GroupChannels(id)) => ChannelScope::Group(id.clone()),
        Some(Kind::CustomChannels(id)) => ChannelScope::Custom(*id),
        _ => ChannelScope::All,
    };
    Some((source, channels))
}

fn current_list(ui: Ui) -> ListId {
    ListId::of(
        ui.list.with_untracked(|list| list.entry()),
        &*crate::runtime_owner::schedule(),
    )
}

fn channel_scope(ui: Ui, list: &ListId) -> Option<(SourceId, Kind)> {
    let owner = crate::runtime_owner::runtime();
    match list {
        ListId::Group(key) => {
            if let Some(source) = key.strip_prefix("playlist:") {
                return Some((SourceId(source.parse().ok()?), Kind::Channels));
            }
            let (source, id) = crate::data::combined::owner(&owner, key)?;
            Some((source, Kind::group_channels(&id)))
        }
        ListId::All | ListId::Favorites if !crate::data::combined::active(&owner) => {
            let kind = if *list == ListId::Favorites {
                Kind::Favorites
            } else {
                Kind::Channels
            };
            Some((ui.app.library.shown.get_untracked()?, kind))
        }
        _ => None,
    }
}

fn open_channel_editor(ui: Ui, channel: usize, mode: Mode, scope: Option<(SourceId, Kind)>) {
    let Some(target) = crate::parental_view::live_target(ui, channel) else {
        return;
    };
    let scope = scope.filter(|(source, _)| source.0 == target.source);
    let (source, kind) = match scope {
        Some(scope) => scope,
        None if mode == Mode::Options => (SourceId(target.source), Kind::Channels),
        None => return,
    };
    organization_view::open_mode(ui, source, kind, Some(target.id), mode);
}

/// Whether entry `entry` of a groups column has group options (what
/// [`open_group_options`] would open): its row shows a ⋯ for them.
pub fn has_group_options(ui: Ui, entry: usize) -> bool {
    let schedule = crate::runtime_owner::schedule();
    if entry >= iptv_presentation::lists::entry_count(&*schedule) {
        return false;
    }
    let list = ListId::of(entry, &*schedule);
    channel_scope(ui, &list).is_some()
}

/// A focused group is actionable even when none of its channels are visible.
pub fn open_group_options(ui: Ui, entry: usize) -> bool {
    let schedule = crate::runtime_owner::schedule();
    if entry >= iptv_presentation::lists::entry_count(&*schedule) {
        return false;
    }
    let list = ListId::of(entry, &*schedule);
    show_group_options(ui, list)
}

fn show_group_options(ui: Ui, list: ListId) -> bool {
    use MenuItem::*;
    let scope = channel_scope(ui, &list);
    let Some((_, kind)) = scope.as_ref() else {
        return false;
    };
    let named = group_target(scope.as_ref()).is_some();
    let schedule = crate::runtime_owner::schedule();
    let reorderable = named
        && matches!(
            list.resolve(&*schedule).map(Entry::at),
            Some(Entry::Group(group)) if matches!(schedule.group_label(group), GroupName::Named(_))
        );
    let playlist = heading_playlist(&list).is_some();
    let rows = group_options(|item| match item {
        // A playlist's heading (the combined view): its own name, and the
        // order of the playlists, which it shows with another one.
        RenamePlaylist => playlist,
        ReorderPlaylists => playlist && crate::settings::playlists_by_hand(),
        RenameGroup | BlockGroup | HideGroup | ShowFavoritesOnly => named,
        ResetGroupName | GroupEpgOffset => matches!(kind, Kind::GroupChannels(_)),
        // Hidden without a player to hand streams to, unless it is on:
        // then turning it off stays reachable.
        GroupExternalPlayer => {
            named
                && crate::host::EXTERNAL_PLAYER
                && (list_preferences(&list).external_player
                    || crate::host::external_player_available())
        }
        ReorderGroups => reorderable,
        ManageGroups => !reorderable && *kind != Kind::Favorites,
        SortChannels => *kind != Kind::Favorites,
        _ => true,
    });
    ui.menu.set(Some(Menu::new(rows, Target::Group, list)));
    true
}

/// The playlist whose heading `list` is (the combined view's groups
/// column), if it is one.
fn heading_playlist(list: &ListId) -> Option<SourceId> {
    match list {
        ListId::Group(key) => Some(SourceId(key.strip_prefix("playlist:")?.parse().ok()?)),
        _ => None,
    }
}

/// Offers to turn off `list`'s external player after a channel chosen in
/// it played here because no player could take it.
pub(crate) fn offer_external_player_reset(ui: Ui, list: ListId) {
    let rows = vec![Row::Action {
        item: MenuItem::StopExternalPlayer,
        enabled: true,
    }];
    ui.menu.set(Some(Menu::new(rows, Target::Group, list)));
}

fn list_preferences(list: &ListId) -> GroupPreferences {
    let schedule = crate::runtime_owner::schedule();
    match list.resolve(&*schedule).map(Entry::at) {
        Some(Entry::Group(group)) => schedule.group_preferences(group),
        Some(Entry::All) => GroupPreferences {
            sort: schedule.channel_sort(),
            ..GroupPreferences::default()
        },
        _ => GroupPreferences::default(),
    }
}

fn set_group_preference(target: iptv_core::parental::Target, preference: GroupPreference) {
    let source = SourceId(target.source);
    let owner = crate::runtime_owner::runtime();
    let save =
        crate::runtime::commands::set_group_preference(&owner, source, target.id, preference);
    crate::tasks::spawn_in_app(async move {
        match save.await {
            Ok(()) => crate::data::organization_changed(&owner, source),
            Err(_) => crate::settings_view::notify(tr(
                "The change could not be saved. Reopen the list and try again.",
            )),
        }
    });
}

fn offset_label(minutes: i32) -> String {
    let sign = match minutes.signum() {
        -1 => "−",
        1 => "+",
        _ => "",
    };
    let minutes = minutes.abs();
    format!("{sign}{}:{:02}", minutes / 60, minutes % 60)
}

fn heading_text(ui: Ui, heading: Heading) -> String {
    ui.guide_data();
    ui.menu.with(|menu| {
        let Some(menu) = menu.as_ref() else {
            return String::new();
        };
        if let Some(offset) = menu.group_offset {
            return format!("{} · {}", tr("Guide time offset"), offset_label(offset));
        }
        match heading {
            Heading::Program => program_origin(&menu.target).map_or_else(String::new, |origin| {
                let p = origin_program(&*crate::runtime_owner::schedule(), origin);
                format!(
                    "{} · {}–{}",
                    title(origin.channel, p),
                    clock(p.start),
                    clock(p.end)
                )
            }),
            Heading::Channel => menu_channel(&menu.target).map_or_else(String::new, channel_name),
            Heading::Group => {
                let catalog = crate::runtime_owner::schedule();
                menu.list
                    .resolve(&*catalog)
                    .map_or_else(String::new, |entry| {
                        crate::lists::entry_name(entry, &*catalog)
                    })
            }
        }
    })
}

fn action_detail(ui: Ui, item: MenuItem) -> String {
    ui.guide_data();
    ui.menu.with(|menu| {
        let Some(menu) = menu.as_ref() else {
            return String::new();
        };
        if let Target::Channel { program, .. } = menu.target
            && matches!(
                item,
                MenuItem::RecordNow | MenuItem::CancelRecording | MenuItem::StopRecording
            )
            && let Some(channel) = menu_channel(&menu.target)
        {
            return format!(
                "{} · {}–{}",
                title(channel, program),
                clock(program.start),
                clock(program.end),
            );
        }
        if menu.target == Target::Group {
            let preferences = list_preferences(&menu.list);
            match item {
                MenuItem::SortChannels => {
                    return organization_view::sort_label(preferences.sort).to_owned();
                }
                MenuItem::ShowFavoritesOnly | MenuItem::GroupExternalPlayer => {
                    let enabled = if item == MenuItem::ShowFavoritesOnly {
                        preferences.favorites_only
                    } else {
                        preferences.external_player
                    };
                    return if enabled { tr("On") } else { tr("Off") }.to_owned();
                }
                MenuItem::GroupEpgOffset => return offset_label(preferences.epg_offset_minutes),
                _ => {}
            }
        }
        String::new()
    })
}

fn bind_live_rows(ui: Ui) {
    let target = computed(move || {
        ui.menu.with(|menu| {
            let menu = menu.as_ref()?;
            menu.target.channel_key()?;
            Some((menu.target.clone(), menu.list.clone()))
        })
    });
    effect(move || {
        let Some((target, list)) = target.get() else {
            return;
        };
        ui.guide_data();
        let Some(origin) = program_origin(&target) else {
            ui.menu.set(None);
            return;
        };
        // The runtime's recording snapshot is published to the UI when it
        // reloads. Recheck the current action for the occurrence already open.
        ui.recordings.with(|_| ());
        let schedule = crate::runtime_owner::schedule();
        let program = origin_program(&*schedule, origin);
        let rows = match target {
            Target::Program { start, .. } if program.start == start => {
                program_rows(ui, origin, program, &list)
            }
            Target::Channel {
                program: opened,
                offset,
                ..
            } => channel_rows(
                ui,
                origin,
                &list,
                opened == program
                    && offset == crate::player::guide_offset(&*schedule, origin.channel),
            ),
            Target::Program { .. } => {
                // The catalog no longer has this occurrence. Leave the
                // guide's current focus in place.
                ui.menu.set(None);
                return;
            }
            Target::Group => return,
        };
        if ui
            .menu
            .with_untracked(|menu| menu.as_ref().is_some_and(|menu| menu.rows != rows))
        {
            ui.menu.update(|menu| {
                if let Some(menu) = menu {
                    menu.replace_rows(rows);
                }
            });
        }
    });
}

pub fn layer(ui: Ui) -> impl IntoElement {
    bind_live_rows(ui);
    let open = move || ui.menu.with(Option::is_some);
    let fixed_heading = move || {
        ui.menu.with(|menu| match menu.as_ref()?.target {
            Target::Channel { .. } => Some(Heading::Channel),
            Target::Group => Some(Heading::Group),
            Target::Program { .. } => None,
        })
    };
    let count = move || ui.menu.with(|m| m.as_ref().map_or(0, |m| m.rows.len()));
    let list = element_ref();
    let size = Scale::new();
    let focus = computed(move || ui.menu.with(|m| m.as_ref().map(|m| m.focus)));
    effect(move || {
        if let Some(i) = focus.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let row = move |at: Position| {
        let row = move || {
            ui.menu
                .with(|m| m.as_ref().and_then(|m| m.rows.get(at.index()).copied()))
        };
        let focused = move || ui.keys_shown() && focus.get() == Some(at.index());
        let mark = move || Mark::of(focused(), false);
        let heading = move || matches!(row(), Some(Row::Heading(_)));
        let title = move || match row() {
            Some(Row::Heading(h)) => heading_text(ui, h),
            Some(Row::Action { item, .. }) => {
                ui.guide_data();
                ui.menu.with(|menu| {
                    let Some(menu) = menu.as_ref() else {
                        return String::new();
                    };
                    let channel = menu_channel(&menu.target);
                    let favorite = item == MenuItem::ToggleFavorite
                        && channel.is_some_and(|ch| ui.app.favorites.with(|f| f.contains(&ch)));
                    let blocked = directly_blocked(&menu.list, item, channel);
                    match (item, &menu.target) {
                        (MenuItem::ReorderChannels, Target::Channel { .. }) => {
                            tr("Reorder channel")
                        }
                        (MenuItem::WatchInNewWindow, Target::Channel { .. }) => {
                            tr("Open in new window")
                        }
                        _ => label(item, favorite, blocked),
                    }
                    .to_owned()
                })
            }
            None => String::new(),
        };
        let subtitle = move || {
            let Some(Row::Action { item, .. }) = row() else {
                return String::new();
            };
            action_detail(ui, item)
        };
        let accessible_label = move || {
            let subtitle = subtitle();
            if subtitle.is_empty() {
                title()
            } else {
                format!("{} · {}", title(), subtitle)
            }
        };
        let name = list_rows::name(size, title)
            // Headings in the lists' heading size.
            .text_size(move || size.text_px(if heading() { SECTION } else { LIST_ITEM }))
            .text_color(move || match row() {
                Some(Row::Action { enabled: true, .. }) => mark().text(),
                Some(Row::Heading(_)) => mark().secondary(),
                _ => mark().disabled(),
            });
        list_rows::row_with(
            size,
            move || {
                if subtitle().is_empty() {
                    ROW
                } else {
                    list_rows::ROW_TWO
                }
            },
            mark,
        )
        .justify_center()
        .role(move || {
            if heading() {
                AccessibilityRole::Header
            } else {
                AccessibilityRole::Menuitem
            }
        })
        .label(accessible_label)
        .accessibility_state(move || AccessibilityState {
            selected: focused(),
            disabled: matches!(row(), Some(Row::Action { enabled: false, .. })),
            ..Default::default()
        })
        .child(name)
        .child(when(
            move || !subtitle().is_empty(),
            move || crate::ui_kit::caption(size, subtitle).text_color(move || mark().secondary()),
        ))
        .on_press(move || {
            ui.touched();
            let picked = ui.menu.with(|m| m.as_ref()?.action_at(at.index()));
            if let Some(item) = picked {
                perform(ui, item);
            }
        })
    };
    let menu_opacity = move || if open() { 1.0_f32 } else { 0.0 };
    view()
        .absolute()
        .inset_0()
        .flex_row()
        // The description popover is in it and shows without the menu.
        .test_id("menu-layer")
        .accessibility_elements_hidden(move || {
            (!open() && !ui.description.with(Option::is_some))
                || crate::overlays::Overlay::Description.accessibility_covered(ui)
        })
        .opacity(move || {
            if open() || ui.description.with(Option::is_some) {
                1.0_f32
            } else {
                0.0
            }
        })
        .pointer_events(move || {
            if open() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .child(
            view()
                .flex_1()
                .bg(tint(0x000000, 0x80))
                .accessibility_elements_hidden(move || {
                    crate::overlays::Overlay::Menu.accessibility_hidden(ui)
                })
                .opacity(menu_opacity)
                .on_press(move || {
                    ui.touched();
                    close(ui);
                }),
        )
        .child(
            view()
                .test_id("menu-panel")
                .w(move || px(size.whole(PANEL_W).min(window().width.0)))
                .accessibility_elements_hidden(move || {
                    crate::overlays::Overlay::Menu.accessibility_hidden(ui)
                })
                .opacity(menu_opacity)
                .pt_safe_area()
                .px(px(12.0))
                .py(px(16.0))
                .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
                .child(when(
                    move || fixed_heading().is_some(),
                    move || {
                        view()
                            .flex_shrink(0.0)
                            .px(move || size.px(list_rows::INSET))
                            .py(move || size.px(12.0))
                            .bg(move || rgb(ui.app.theme.with(|theme| theme.background.alt)))
                            .child(
                                crate::ui_kit::title(size, move || {
                                    fixed_heading()
                                        .map_or_else(String::new, |h| heading_text(ui, h))
                                })
                                .role(AccessibilityRole::Header)
                                .number_of_lines(1),
                            )
                    },
                ))
                .child(
                    virtual_list(count, row)
                        .keys(move || {
                            ui.menu.with(|menu| {
                                menu.iter()
                                    .flat_map(|menu| &menu.rows)
                                    .map(|row| match *row {
                                        Row::Action { item, .. } => Ok(item),
                                        Row::Heading(heading) => Err(heading),
                                    })
                                    .collect::<Vec<_>>()
                            })
                        })
                        .estimated_item_size(list_rows::pitch(size, ROW))
                        .element_ref(&list)
                        .flex_1(),
                ),
        )
        .child(description(ui))
}

fn origin_program(schedule: &dyn crate::guide::Schedule, origin: Origin) -> crate::guide::Program {
    let minute = schedule
        .listing_start(origin.channel)
        .map_or(origin.program_start, |start| {
            origin.program_start.max(start)
        });
    schedule.program_at(origin.channel, minute)
}

pub(crate) fn scroll_description(ui: Ui, direction: f32) {
    ui.description_scroll
        .scroll_by(direction * crate::scale::now() * 96.0);
}

/// The description popover. Back or a tap closes it.
fn description(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let shown = move || ui.description.get().is_some();
    let scrolling = ui.description_scroll;
    effect(move || {
        if ui.description.get().is_some() {
            scrolling.reset();
        }
    });
    let program = move || {
        ui.guide_data();
        ui.description.get().map(|origin| {
            (
                origin.channel,
                origin_program(&*crate::runtime_owner::schedule(), origin),
            )
        })
    };
    view()
        .absolute()
        .inset_0()
        .items_end()
        .pt_safe_area()
        .px(px(40.0))
        .pt(px(40.0))
        .pb(move || {
            px(if crate::play_bar::shows(ui) {
                crate::play_bar::height(ui) + 8.0
            } else {
                40.0
            })
        })
        .bg(move || tint(0x000000, if shown() { 0x80 } else { 0 }))
        .test_id("description-layer")
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::Description.accessibility_hidden(ui)
        })
        .opacity(move || if shown() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if shown() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .on_press(move || {
            ui.touched();
            close(ui);
        })
        .child(
            scrolling
                .view()
                .test_id("description-scroll")
                .max_h(percentage(1.0))
                .max_w(percentage(1.0))
                .w(px(480.0))
                .child(
                    view()
                        .gap(px(10.0))
                        .p(px(24.0))
                        .rounded(px(RADIUS_LARGE))
                        .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
                        .child(
                            text(move || program().map_or(String::new(), |(ch, p)| title(ch, p)))
                                .font_family(SANS)
                                .font_weight(MEDIUM)
                                .text_size(move || size.text_px(TITLE))
                                .text_color(rgb(LIST_TEXT)),
                        )
                        .child(
                            text(move || {
                                program().map_or(String::new(), |(ch, p)| {
                                    format!(
                                        "{} · {}–{}",
                                        channel_name(ch),
                                        clock(p.start),
                                        clock(p.end)
                                    )
                                })
                            })
                            .font_family(SANS)
                            .text_size(move || size.text_px(HEADER_TIMES))
                            .text_color(Mark::Plain.secondary()),
                        )
                        .child(
                            text(move || {
                                program()
                                    .and_then(|(channel, program)| {
                                        crate::header::program_description(
                                            &*crate::runtime_owner::schedule(),
                                            channel,
                                            program,
                                        )
                                    })
                                    .unwrap_or_default()
                            })
                            .font_family(SANS)
                            .text_size(move || size.text_px(HEADER_BODY))
                            .text_color(rgb(LIST_TEXT)),
                        )
                        .child(when(
                            move || {
                                // Only a program on now records (Record now); a
                                // future one is not scheduled (#273).
                                ui.guide_data();
                                let now = ui.focus.with(|focus| focus.now);
                                ui.player_surface == 0
                                    && crate::recordings_view::details_button()
                                    && ui.description.get().is_some_and(|origin| {
                                        crate::recordings_view::can_record_now(origin, now)
                                    })
                            },
                            move || {
                                crate::ui_kit::button(
                                    ui,
                                    size,
                                    tr("Record"),
                                    || true,
                                    move || {
                                        if let Some(origin) = ui.description.get_untracked() {
                                            crate::recordings_view::record_details(ui, origin);
                                        }
                                    },
                                )
                            },
                        )),
                ),
        )
}

/// A channel header gesture selects its row without tuning or moving to now.
pub fn open_at(ui: Ui, channel: usize) {
    ui.touched();
    batch(|| {
        let minute = ui.focus.with_untracked(|focus| focus.cursor);
        crate::lists::tap(ui, channel, minute);
        crate::browse_playback::hold(ui);
        let minute = ui.focus.with_untracked(|focus| focus.cursor);
        open_channel_at(ui, channel, minute, current_list(ui), false);
    });
}

/// The timeline's shared context action: a remote's Menu or Long OK,
/// a touch hold, or a right-click. Keep the focused program and hold
/// autoplay while opening the same compact channel actions on every device.
pub fn open_timeline_context(ui: Ui) {
    crate::browse_playback::hold(ui);
    if let Some(channel) = crate::lists::focused_channel(ui) {
        let minute = ui.focus.with_untracked(|focus| focus.cursor);
        open_channel_at(ui, channel, minute, current_list(ui), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_up_is_available_only_on_the_primary_surface() {
        assert!(implemented(MenuItem::StartFromBeginning, 0));
        for surface in 1..crate::player::SURFACES {
            assert!(!implemented(MenuItem::StartFromBeginning, surface));
            assert!(implemented(MenuItem::Description, surface));
        }
    }
}
