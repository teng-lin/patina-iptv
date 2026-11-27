//! The guide's context menus as scrolling side panels, and
//! the program description popover. State lives in `iptv_presentation::menu`;
//! this opens, draws and performs it. Labels are iptv-rs's own wording.

mod actions;
mod render;
pub use actions::perform;
pub use render::layer;
pub(crate) use render::playlist_layer;

use crate::i18n::{tr, trf};
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
use nsh_core::Checked;
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
        ReorderChannels => tr("Reorder channel"),
        SortChannels => tr("Channel order"),
        CopyChannels => tr("Copy channels to a group…"),
        CreateGroup => tr("New group…"),
        GroupOptions => tr("Group options…"),
        GroupSettings => tr("Group settings"),
        SortGroups => tr("Group order"),
        ReorderGroups => tr("Reorder group"),
        ManageGroups => tr("Manage groups"),
        RenameGroup => tr("Rename group name"),
        TogglePlaylist(_) => "",
        ReorderPlaylists => tr("Reorder playlists"),
        PlaylistOrderAdded => tr("By date added"),
        PlaylistOrderManual => tr("Custom order"),
        PlaylistOrderByName => tr("By name"),
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
    let now = ui.guide.focus.with(|f| f.now);
    let playing_context = ui.chrome.mode.get_untracked() == ViewMode::Fullscreen
        && ui.playback.playing.get_untracked().is_some();
    let (channel, minute) = match (ui.chrome.mode.get(), ui.playback.playing.get()) {
        (ViewMode::Fullscreen, Some(ch)) => (ch, now),
        _ => {
            let Some(ch) = crate::lists::focused_channel(ui.guide) else {
                return; // an empty list
            };
            (ch, ui.guide.focus.with(|f| f.cursor))
        }
    };
    open_for(ui, channel, minute);
    ui.screens.menu.update(|menu| {
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
    ui.screens.menu.set(Some(menu));
}

fn program_rows(ui: Ui, origin: Origin, program: crate::guide::Program, list: &ListId) -> Vec<Row> {
    let now = ui.guide.focus.with(|focus| focus.now);
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
            _ => implemented(item, ui.playback.player_surface),
        },
    );
    let auxiliary = ui.playback.player_surface != 0;
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
    ui.screens.menu.set(Some(menu));
}

fn channel_rows(ui: Ui, origin: Origin, list: &ListId, current: bool) -> Vec<Row> {
    // Only the main window opens player windows.
    let new_window = ((crate::host::LINUX || crate::host::MACOS)
        && ui.playback.player_surface == 0)
        .then(|| crate::desktop::player_windows::can_open(ui, origin.channel));
    let scope = channel_scope(ui, list);
    let mut rows = channel_menu(
        current
            .then(|| crate::recordings_view::channel_menu_action(ui, origin))
            .flatten(),
        new_window,
        |item| {
            scope.as_ref().is_some_and(|(_, kind)| {
                item == MenuItem::ReorderChannels || *kind != Kind::Favorites
            })
        },
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
    if ui.screens.menu.with_untracked(|menu| {
        menu.as_ref()
            .and_then(|menu| menu.target.channel_key())
            .and_then(|key| before.find_channel(key))
            .is_some_and(shifted)
    }) {
        ui.screens.menu.set(None);
    }
    if ui
        .screens
        .description
        .get_untracked()
        .is_some_and(|origin| shifted(origin.channel))
    {
        ui.screens.description.set(None);
    }
    ui.screens
        .recordings
        .update(|state| state.invalidate_context(shifted));
}

/// Back from full options restores their compact context and its focus.
pub fn back(ui: Ui) {
    if let Some(parent) = ui.screens.menu.with(|menu| menu.as_ref()?.parent.clone()) {
        ui.screens.menu.set(Some(*parent));
        return;
    }
    close(ui);
}

/// Dismisses the entire menu or popover, keeping its guide selection.
pub fn close(ui: Ui) {
    let origin = ui
        .screens
        .menu
        .with(|m| {
            let menu = m.as_ref().filter(|menu| menu.list == current_list(ui))?;
            matches!(menu.target, Target::Program { .. })
                .then(|| program_origin(&menu.target))
                .flatten()
        })
        .or_else(|| ui.screens.description.get());
    ui.screens.menu.set(None);
    ui.screens.description.set(None);
    if let Some(origin) = origin
        && ui.chrome.mode.get() != ViewMode::Fullscreen
        && ui
            .guide
            .list
            .with(|list| list.row_of(origin.channel))
            .is_some()
    {
        crate::lists::tap(ui, origin.channel, origin.program_start);
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
        ui.guide.list.with_untracked(|list| list.entry()),
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
    heading_playlist(&list).is_some() || playlist_menu(&list) || channel_scope(ui, &list).is_some()
}

/// A focused group is actionable even when none of its channels are visible.
pub fn open_group_options(ui: Ui, entry: usize) -> bool {
    let schedule = crate::runtime_owner::schedule();
    if entry >= iptv_presentation::lists::entry_count(&*schedule) {
        return false;
    }
    let list = ListId::of(entry, &*schedule);
    show_group_options(ui, list, GroupMenuEntry::Heading)
}

pub(crate) fn open_playlist_options(ui: Ui, id: SourceId) {
    crate::playlists_view::open_only(
        ui,
        id,
        crate::playlists_view::Only::Actions {
            manual_order: crate::settings::playlists_by_hand(),
        },
    );
}

pub(crate) fn open_playlist_order(ui: Ui, id: SourceId) {
    let rows = vec![
        Row::Heading(Heading::PlaylistOrder),
        Row::Action {
            item: MenuItem::PlaylistOrderAdded,
            enabled: true,
        },
        Row::Action {
            item: MenuItem::PlaylistOrderByName,
            enabled: true,
        },
        Row::Action {
            item: MenuItem::PlaylistOrderManual,
            enabled: true,
        },
    ];
    let mut menu = Menu::new(
        rows,
        Target::Group,
        ListId::Group(format!("playlist:{}", id.0)),
    );
    let current = match crate::settings::choice(iptv_settings::keys::PLAYLIST_SORT) {
        "added" => MenuItem::PlaylistOrderAdded,
        "name" => MenuItem::PlaylistOrderByName,
        _ => MenuItem::PlaylistOrderManual,
    };
    menu.focus_on(current);
    ui.screens.menu.set(Some(menu));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupMenuEntry {
    Heading,
    Program,
}

fn show_group_options(ui: Ui, list: ListId, entry: GroupMenuEntry) -> bool {
    if let Some(id) = heading_playlist(&list) {
        if playlist_by_id(id).is_none() {
            return false;
        }
        crate::settings_view::settings_guarded(ui, move || {
            crate::playlists_view::open_only(ui, id, crate::playlists_view::Only::GuideActions)
        });
        return true;
    }
    let Some(mut rows) = group_option_rows(ui, &list) else {
        return false;
    };
    if entry == GroupMenuEntry::Program && group_target(channel_scope(ui, &list).as_ref()).is_some()
    {
        rows.push(Row::Action {
            item: MenuItem::GroupSettings,
            enabled: true,
        });
    }
    ui.screens
        .menu
        .set(Some(Menu::new(rows, Target::Group, list)));
    true
}

/// An empty named group cannot open preferences through a channel's menu.
fn empty_group_list(ui: Ui) -> Option<ListId> {
    if !ui.guide.list.with(|list| list.is_empty()) {
        return None;
    }
    ui.guide_data();
    let list = current_list(ui);
    group_target(channel_scope(ui, &list).as_ref()).map(|_| list)
}

pub(crate) fn empty_group_settings_available(ui: Ui) -> bool {
    empty_group_list(ui).is_some()
}

pub(crate) fn open_empty_group_settings(ui: Ui) -> bool {
    let Some(list) = empty_group_list(ui) else {
        return false;
    };
    open_group_settings(ui, list, None);
    true
}

fn open_group_settings(ui: Ui, list: ListId, parent: Option<Menu>) {
    let preferences = list_preferences(&list);
    let rows = vec![
        Row::Action {
            item: MenuItem::ShowFavoritesOnly,
            enabled: true,
        },
        Row::Action {
            item: MenuItem::GroupExternalPlayer,
            enabled: crate::host::EXTERNAL_PLAYER
                && (preferences.external_player || crate::host::external_player_available()),
        },
        Row::Action {
            item: MenuItem::GroupEpgOffset,
            enabled: true,
        },
        Row::Action {
            item: MenuItem::ManageGroups,
            enabled: true,
        },
    ];
    let mut menu = Menu::new(rows, Target::Group, list);
    menu.parent = parent.map(Box::new);
    ui.screens.menu.set(Some(menu));
}

fn group_option_rows(ui: Ui, list: &ListId) -> Option<Vec<Row>> {
    use MenuItem::*;
    let schedule = crate::runtime_owner::schedule();
    let entry = list.resolve(&*schedule);
    let scope = entry.and_then(|_| channel_scope(ui, list));
    let kind = scope.as_ref().map(|(_, kind)| kind);
    let mut rows: Vec<Row> = if playlist_menu(list) {
        let stored = crate::runtime().playlists().ok()?;
        if stored.is_empty() {
            return None;
        }
        stored
            .into_iter()
            .map(|playlist| Row::Action {
                item: TogglePlaylist(playlist.id),
                enabled: playlist.policy.enabled,
            })
            .collect()
    } else {
        kind?;
        Vec::new()
    };
    let can_reorder_playlists = rows.len() > 1 && crate::settings::playlists_by_hand();
    let named = group_target(scope.as_ref()).is_some();
    let reorderable = named
        && matches!(
            entry.map(Entry::at),
            Some(Entry::Group(group)) if matches!(schedule.group_label(group), GroupName::Named(_))
        );
    let can_manage = kind.is_some_and(|kind| *kind != Kind::Favorites);
    let renamed = matches!(
        entry.map(Entry::at),
        Some(Entry::Group(group)) if schedule.group_name_overridden(group)
    );
    rows.extend(
        group_options(|item| match item {
            ReorderPlaylists => can_reorder_playlists,
            RenameGroup | ResetGroupName | BlockGroup | HideGroup => named,
            ReorderGroups | SortGroups => reorderable,
            SortChannels | ManageGroups => can_manage,
            ReorderChannels => kind.is_some(),
            _ => false,
        })
        .into_iter()
        .map(|row| match row {
            Row::Action {
                item: ResetGroupName,
                ..
            } => Row::Action {
                item: ResetGroupName,
                enabled: renamed,
            },
            row => row,
        }),
    );
    Some(rows)
}

fn playlist_by_id(id: SourceId) -> Option<crate::data::Playlist> {
    crate::runtime()
        .playlists()
        .ok()?
        .into_iter()
        .find(|playlist| playlist.id == id)
}

fn playlist_menu(list: &ListId) -> bool {
    *list == ListId::All && crate::data::combined::active(&crate::runtime())
}

fn is_playlist_menu(menu: &Menu) -> bool {
    menu.target == Target::Group && playlist_menu(&menu.list)
}

/// Catalog changes dismiss channel and group actions. Playlist controls
/// stay open while their checkboxes update the guide.
pub(crate) fn catalog_changed(ui: Ui) {
    if ui
        .screens
        .menu
        .with_untracked(|menu| menu.as_ref().is_some_and(|menu| !is_playlist_menu(menu)))
    {
        ui.screens.menu.set(None);
    }
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
    ui.screens
        .menu
        .set(Some(Menu::new(rows, Target::Group, list)));
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
    ui.screens.menu.with(|menu| {
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
            Heading::PlaylistOrder => tr("Playlist order").to_owned(),
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
    ui.screens.menu.with(|menu| {
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
        String::new()
    })
}

/// Renders the current values on the rows that open or change preferences.
fn preference_label(item: MenuItem, list: &ListId) -> Option<String> {
    use MenuItem::*;
    let state_label = |on| if on { tr("On") } else { tr("Off") };
    Some(match item {
        SortGroups => group_order_label(list),
        SortChannels => {
            let sort = organization_view::sort_label(list_preferences(list).sort);
            trf("Channel order: {method}", &[("method", sort.into())])
        }
        ShowFavoritesOnly => {
            let state = state_label(list_preferences(list).favorites_only);
            trf("Show favorites only: {state}", &[("state", state.into())])
        }
        GroupExternalPlayer => {
            let state = state_label(list_preferences(list).external_player);
            trf("Use external player: {state}", &[("state", state.into())])
        }
        GroupEpgOffset => {
            let offset = offset_label(list_preferences(list).epg_offset_minutes);
            trf(
                "Guide time offset: {offset}",
                &[("offset", (&offset).into())],
            )
        }
        _ => return None,
    })
}

pub(crate) fn group_order_label(list: &ListId) -> String {
    let schedule = crate::runtime_owner::schedule();
    let order = match list.resolve(&*schedule).map(Entry::at) {
        Some(Entry::Group(group)) => schedule.groups_order(group),
        _ => Default::default(),
    };
    let method = organization_view::sort_label(order);
    trf("Group order: {method}", &[("method", method.into())])
}

fn bind_live_rows(ui: Ui) {
    let target = computed(move || {
        ui.screens.menu.with(|menu| {
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
            ui.screens.menu.set(None);
            return;
        };
        // The runtime's recording snapshot is published to the UI when it
        // reloads. Recheck the current action for the occurrence already open.
        ui.screens.recordings.with(|_| ());
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
                ui.screens.menu.set(None);
                return;
            }
            Target::Group => return,
        };
        if ui
            .screens
            .menu
            .with_untracked(|menu| menu.as_ref().is_some_and(|menu| menu.rows != rows))
        {
            ui.screens.menu.update(|menu| {
                if let Some(menu) = menu {
                    menu.replace_rows(rows);
                }
            });
        }
    });
}

/// Keep the aggregate menu synchronized with the stored playlists.
fn bind_playlist_rows(ui: Ui) {
    let list = computed(move || {
        ui.screens.menu.with(|menu| {
            menu.as_ref()
                .filter(|menu| is_playlist_menu(menu))
                .map(|menu| menu.list.clone())
        })
    });
    effect(move || {
        let Some(list) = list.get() else { return };
        ui.guide_data();
        ui.app.library.version.get();
        ui.app.settings.get();
        let rows = group_option_rows(ui, &list);
        if ui.screens.menu.with_untracked(|open| {
            open.as_ref()
                .is_some_and(|menu| menu.list == list && rows.as_ref() != Some(&menu.rows))
        }) {
            ui.screens.menu.update(|open| match rows {
                Some(rows) => {
                    if let Some(menu) = open {
                        menu.replace_rows(rows);
                    }
                }
                None => *open = None,
            });
        }
    });
}

pub(crate) fn playlist_popup_open(ui: Ui) -> bool {
    ui.screens
        .menu
        .with(|menu| menu.as_ref().is_some_and(is_playlist_menu))
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
    ui.screens
        .description_scroll
        .scroll_by(direction * crate::scale::now() * 96.0);
}

/// The description popover. Back or a tap closes it.
fn description(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let shown = move || ui.screens.description.get().is_some();
    let scrolling = ui.screens.description_scroll;
    effect(move || {
        if ui.screens.description.get().is_some() {
            scrolling.reset();
        }
    });
    let program = move || {
        ui.guide_data();
        ui.screens.description.get().map(|origin| {
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
                                let now = ui.guide.focus.with(|focus| focus.now);
                                ui.playback.player_surface == 0
                                    && crate::recordings_view::details_button()
                                    && ui.screens.description.get().is_some_and(|origin| {
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
                                        if let Some(origin) = ui.screens.description.get_untracked()
                                        {
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
        let minute = ui.guide.focus.with_untracked(|focus| focus.cursor);
        crate::lists::tap(ui, channel, minute);
        crate::browse_playback::hold(ui);
        let minute = ui.guide.focus.with_untracked(|focus| focus.cursor);
        open_channel_at(ui, channel, minute, current_list(ui), false);
    });
}

/// The timeline's shared context action: a remote's Menu or Long OK,
/// a touch hold, or a right-click. Keep the focused program and hold
/// autoplay while opening the same compact channel actions on every device.
pub fn open_timeline_context(ui: Ui) {
    crate::browse_playback::hold(ui);
    if let Some(channel) = crate::lists::focused_channel(ui.guide) {
        let minute = ui.guide.focus.with_untracked(|focus| focus.cursor);
        open_channel_at(ui, channel, minute, current_list(ui), false);
    } else {
        open_empty_group_settings(ui);
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
