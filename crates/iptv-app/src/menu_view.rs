//! The guide's context menu (Long OK / Menu) as a scrolling side panel, and
//! the program description popover. State lives in `iptv_presentation::menu`;
//! this opens, draws and performs it. Labels are iptv-rs's own wording.

use crate::i18n::tr;
use crate::organization_view::{self, Kind};
use crate::runtime::model::SourceId;
use iptv_core::organization::{Block, BlockKind};
use iptv_library::ChannelScope;
use iptv_presentation::lists::{Entry, ListId};
use iptv_presentation::organization::Mode;

use iptv_presentation::menu::{
    Heading, Menu, MenuItem, Origin, Row, Target, channel_menu, group_options, program_menu,
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
            | MenuItem::ScheduleRecording
            | MenuItem::CustomRecording
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
        ExternalPlayer => tr("Open in another player"),
        WatchInNewWindow => tr("Watch in new window"),
        Reminder => tr("Remind me"),
        Record => tr("Record"),
        ScheduleRecording => tr("Schedule recording"),
        CustomRecording => tr("Schedule a recording…"),
        SaveToList => tr("Save to my list"),
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
        ResetGroupName => tr("Use provider name"),
        HideGroup => tr("Hide group"),
    }
}

fn title(channel: usize, p: crate::guide::Program) -> String {
    crate::listing_labels::listing(crate::schedule().listing(channel, p))
}

/// Long OK / Menu: the menu for the focused program (in full screen, the
/// playing channel's current program).
pub fn open(ui: Ui) {
    let now = ui.focus.with(|f| f.now);
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
}

pub fn open_for(ui: Ui, channel: usize, minute: i32) {
    let now = ui.focus.with(|f| f.now);
    let schedule = crate::schedule();
    let Some(key) = schedule.channel_key(channel) else {
        return;
    };
    let list = current_list(ui);
    let scope = channel_scope(ui, &list);
    let program = schedule.program_at(channel, minute);
    let mut rows = program_menu(program, schedule.catchup_days(channel), now, |item| {
        if matches!(
            item,
            MenuItem::Record | MenuItem::ScheduleRecording | MenuItem::CustomRecording
        ) {
            return ui.player_surface == 0 && crate::recordings_view::available(channel);
        }
        match item {
            MenuItem::ManageHidden | MenuItem::ManageBlocked | MenuItem::ReorderChannels => {
                scope.is_some()
            }
            MenuItem::GroupOptions => scope.is_some() || aggregate_options(&list),
            MenuItem::SortChannels => scope
                .as_ref()
                .is_some_and(|(_, kind)| *kind != Kind::Favorites),
            MenuItem::CopyChannels => copy_scope(ui, &list, scope.as_ref()).is_some(),
            MenuItem::CreateGroup => management_source(ui, &list, scope.as_ref()).is_some(),
            _ => implemented(item, ui.player_surface),
        }
    });
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
    if (crate::host::LINUX || crate::host::MACOS)
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
    let origin = Origin {
        channel,
        program_start: program.start,
    };
    crate::recordings_view::remember(ui, origin);
    let target = Target::Program {
        channel: key,
        start: program.start,
    };
    ui.menu.set(Some(Menu::new(rows, target, list)));
}

/// Opens the actions for a channel row in the guide or player list.
pub fn open_channel(ui: Ui, channel: usize) {
    let Some(key) = crate::schedule().channel_key(channel) else {
        return;
    };
    let rows = channel_menu(|item| match item {
        MenuItem::WatchInNewWindow => {
            (crate::host::LINUX || crate::host::MACOS)
                && crate::desktop::player_windows::can_open(ui, channel)
        }
        _ => implemented(item, ui.player_surface),
    });
    ui.menu.set(Some(Menu::new(
        rows,
        Target::Channel(key),
        current_list(ui),
    )));
}

fn menu_channel(target: &Target) -> Option<usize> {
    crate::schedule().find_channel(target.channel_key()?)
}

fn program_origin(target: &Target) -> Option<Origin> {
    let Target::Program { start, .. } = target else {
        return None;
    };
    Some(Origin {
        channel: menu_channel(target)?,
        program_start: *start,
    })
}

/// Closes the menu or popover, keeping focus on its guide selection.
pub fn close(ui: Ui) {
    let origin = ui
        .menu
        .with(|m| {
            let menu = m.as_ref().filter(|menu| menu.list == current_list(ui))?;
            program_origin(&menu.target)
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
    let Some((target, list)) = ui.menu.with(|menu| {
        let menu = menu.as_ref()?;
        menu.rows
            .contains(&Row::Action {
                item,
                enabled: true,
            })
            .then(|| (menu.target.clone(), menu.list.clone()))
    }) else {
        return;
    };
    ui.menu.set(None);
    let scope = list
        .resolve(&*crate::schedule())
        .and_then(|_| channel_scope(ui, &list));
    if target == Target::Group && scope.is_none() && !aggregate_options(&list) {
        return;
    }
    let channel = menu_channel(&target);
    if channel.is_none() && target != Target::Group {
        return;
    }
    match item {
        MenuItem::BlockChannel | MenuItem::BlockGroup => {
            if item == MenuItem::BlockChannel
                && channel.is_some_and(|ch| crate::schedule().channel_block(ch) == Block::Group)
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
        MenuItem::ManageBlocked | MenuItem::ManageHidden | MenuItem::ReorderChannels => {
            let mode = match item {
                MenuItem::ManageBlocked => Mode::Blocking,
                MenuItem::ManageHidden => Mode::Visibility,
                _ => Mode::Reorder,
            };
            if let Some(channel) = channel {
                open_channel_editor(ui, channel, mode, scope, list);
            } else if let Some((source, kind)) = scope {
                organization_view::open_guide_mode(ui, source, kind, None, mode, list);
            }
        }
        MenuItem::Record
        | MenuItem::CustomRecording
        | MenuItem::ScheduleRecording
        | MenuItem::Description
        | MenuItem::StartFromBeginning => {
            if let Some(origin) = program_origin(&target) {
                perform_program(ui, item, origin);
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
                let saved =
                    crate::runtime::commands::hide_channel(&crate::runtime(), source, target.id);
                crate::data::spawn_in_app(async move {
                    match saved.await {
                        Ok(()) => crate::data::organization::changed(&crate::runtime(), source),
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
                open_channel_editor(ui, channel, Mode::Options, scope, list);
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
        MenuItem::RenameGroup => {
            if let Some(target) = group_target(scope.as_ref()) {
                organization_view::rename_group(ui, SourceId(target.source), target.id);
            }
        }
        MenuItem::ResetGroupName => {
            if let Some((source, Kind::GroupChannels(id))) = scope {
                let owner = crate::runtime();
                let save = crate::runtime::commands::rename_organization_item(
                    &owner,
                    source,
                    Kind::Groups,
                    id.as_str().to_owned(),
                    String::new(),
                );
                crate::data::spawn_in_app(async move {
                    match save.await {
                        Ok(()) => crate::data::organization::changed(&owner, source),
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
                    MenuItem::ReorderGroups => organization_view::open_guide_mode(
                        ui,
                        *source,
                        Kind::Groups,
                        focus,
                        Mode::Reorder,
                        list,
                    ),
                    _ => organization_view::open(ui, *source, Kind::Groups, focus),
                }
            }
        }
        MenuItem::HideGroup => {
            if let Some(target) = group_target(scope.as_ref()) {
                let source = SourceId(target.source);
                let owner = crate::runtime();
                let save = crate::runtime::commands::hide_group(&owner, source, target.id);
                crate::data::spawn_in_app(async move {
                    match save.await {
                        Ok(()) => crate::data::organization::changed(&owner, source),
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

fn perform_program(ui: Ui, item: MenuItem, origin: Origin) {
    match item {
        MenuItem::Record => crate::recordings_view::record_origin(ui, origin),
        MenuItem::CustomRecording => crate::recordings_view::program(ui, origin, true),
        MenuItem::ScheduleRecording => crate::recordings_view::schedule_program(ui, origin),
        MenuItem::Description => ui.description.set(Some(origin)),
        MenuItem::StartFromBeginning => {
            let schedule = crate::schedule();
            let now = ui.focus.with_untracked(|focus| focus.now);
            let program = origin_program(&*schedule, origin);
            if program.start != origin.program_start
                || !program.can_replay(now, schedule.catchup_days(origin.channel))
                || !crate::player::allow_guide_channel(origin.channel)
            {
                return;
            }
            ui.play(
                origin.channel,
                crate::Playback::CatchUp {
                    start: origin.program_start,
                    offset: 0,
                },
            );
            ui.mode.set(ViewMode::Fullscreen);
            ui.show_playing();
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
    let schedule = crate::schedule();
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
    } else if *list == ListId::Recent && !crate::data::combined::active(&crate::runtime()) {
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
        &*crate::schedule(),
    )
}

fn channel_scope(ui: Ui, list: &ListId) -> Option<(SourceId, Kind)> {
    let owner = crate::runtime();
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

fn open_channel_editor(
    ui: Ui,
    channel: usize,
    mode: Mode,
    scope: Option<(SourceId, Kind)>,
    list: ListId,
) {
    let Some(target) = crate::parental_view::live_target(ui, channel) else {
        return;
    };
    let scope = scope.filter(|(source, _)| source.0 == target.source);
    let (source, kind) = match scope {
        Some(scope) => scope,
        None if mode == Mode::Options => (SourceId(target.source), Kind::Channels),
        None => return,
    };
    organization_view::open_guide_mode(ui, source, kind, Some(target.id), mode, list);
}

/// Whether entry `entry` of a groups column has group options (what
/// [`open_group_options`] would open): its row shows a ⋯ for them.
pub fn has_group_options(ui: Ui, entry: usize) -> bool {
    let schedule = crate::schedule();
    if entry >= iptv_presentation::lists::entry_count(&*schedule) {
        return false;
    }
    let list = ListId::of(entry, &*schedule);
    channel_scope(ui, &list).is_some() || aggregate_options(&list)
}

/// A focused group is actionable even when none of its channels are visible.
pub fn open_group_options(ui: Ui, entry: usize) -> bool {
    let schedule = crate::schedule();
    if entry >= iptv_presentation::lists::entry_count(&*schedule) {
        return false;
    }
    let list = ListId::of(entry, &*schedule);
    show_group_options(ui, list)
}

fn show_group_options(ui: Ui, list: ListId) -> bool {
    use MenuItem::*;

    let scope = channel_scope(ui, &list);
    let rows = if let Some((_, kind)) = scope.as_ref() {
        let named = group_target(scope.as_ref()).is_some();
        group_options(|item| match item {
            RenameGroup | BlockGroup | HideGroup => named,
            ResetGroupName => matches!(kind, Kind::GroupChannels(_)),
            ManageGroups | SortGroups | ReorderGroups | SortChannels => *kind != Kind::Favorites,
            ManageFavorites => false,
            _ => true,
        })
    } else if aggregate_options(&list) {
        group_options(|item| item == ManageFavorites)
    } else {
        return false;
    };
    ui.menu.set(Some(Menu::new(rows, Target::Group, list)));
    true
}

fn aggregate_options(list: &ListId) -> bool {
    matches!(list, ListId::All | ListId::Favorites)
        && crate::data::combined::active(&crate::runtime())
}

fn heading_text(ui: Ui, heading: Heading) -> String {
    ui.guide_data();
    ui.menu.with(|menu| {
        let Some(menu) = menu.as_ref() else {
            return String::new();
        };
        match heading {
            Heading::Program => program_origin(&menu.target).map_or_else(String::new, |origin| {
                let p = origin_program(&*crate::schedule(), origin);
                format!(
                    "{} · {}–{}",
                    title(origin.channel, p),
                    clock(p.start),
                    clock(p.end)
                )
            }),
            Heading::Channel => menu_channel(&menu.target).map_or_else(String::new, channel_name),
            Heading::Group => {
                let catalog = crate::schedule();
                menu.list
                    .resolve(&*catalog)
                    .map_or_else(String::new, |entry| {
                        crate::lists::entry_name(entry, &*catalog)
                    })
            }
        }
    })
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let open = move || ui.menu.with(Option::is_some);
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
                    label(item, favorite, blocked).to_owned()
                })
            }
            None => String::new(),
        };
        let name = list_rows::name(size, title)
            // Headings in the lists' heading size.
            .text_size(move || size.text_px(if heading() { SECTION } else { LIST_ITEM }))
            .text_color(move || match row() {
                Some(Row::Action { enabled: true, .. }) => mark().text(),
                Some(Row::Heading(_)) => mark().secondary(),
                _ => mark().disabled(),
            });
        list_rows::row(size, ROW, mark)
            .role(move || {
                if heading() {
                    AccessibilityRole::Header
                } else {
                    AccessibilityRole::Menuitem
                }
            })
            .label(title)
            .accessibility_state(move || AccessibilityState {
                selected: focused(),
                disabled: matches!(row(), Some(Row::Action { enabled: false, .. })),
                ..Default::default()
            })
            .child(name)
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
                .w(move || px(size.whole(PANEL_W)))
                .accessibility_elements_hidden(move || {
                    crate::overlays::Overlay::Menu.accessibility_hidden(ui)
                })
                .opacity(menu_opacity)
                .pt_safe_area()
                .px(px(12.0))
                .py(px(16.0))
                .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
                .child(
                    virtual_list(count, row)
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

/// The description popover: title, time range and text; no actions (those
/// live in the menu). Back or a tap closes it.
fn description(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let shown = move || ui.description.get().is_some();
    let program = move || {
        ui.guide_data();
        ui.description
            .get()
            .map(|origin| (origin.channel, origin_program(&*crate::schedule(), origin)))
    };
    view()
        .absolute()
        .inset_0()
        .items_end()
        .pt_safe_area()
        .px(px(40.0))
        .py(px(40.0))
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
            view()
                .w(px(480.0))
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
                            format!("{} · {}–{}", channel_name(ch), clock(p.start), clock(p.end))
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
                                    &*crate::schedule(),
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
                        ui.player_surface == 0
                            && program().is_some_and(|(ch, p)| {
                                crate::recordings_view::available(ch)
                                    && crate::guide::unix_of(&*crate::schedule(), p.end)
                                        > crate::time::wall_unix()
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
                                    crate::recordings_view::program(ui, origin, false);
                                }
                            },
                        )
                    },
                )),
        )
}

/// A channel header gesture selects its row without tuning or moving to now.
pub fn open_at(ui: Ui, channel: usize) {
    ui.touched();
    batch(|| {
        let minute = ui.focus.with_untracked(|focus| focus.cursor);
        crate::lists::tap(ui, channel, minute);
        crate::browse_playback::hold(ui);
        open_channel(ui, channel);
    });
}

/// A right-click or long press moved the guide's focus to a program:
/// its menu opens, and autoplay holds on it. The focus stays there when
/// the menu (or the details it opens) closes, and opening a menu never
/// starts playback (`docs/interaction.md`): nothing tunes it until the
/// viewer moves the focus again. A choice still plays.
pub fn open_focused(ui: Ui) {
    crate::browse_playback::hold(ui);
    open(ui);
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
