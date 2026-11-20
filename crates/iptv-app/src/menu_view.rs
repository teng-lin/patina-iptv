//! The guide's context menu (Long OK / Menu) as a scrolling side panel, and
//! the programme description popover. State lives in `iptv_core::menu`;
//! this opens, draws and performs it. Labels are iptv-rs's own wording.

use crate::i18n::tr;
use crate::organization_view::{self, Kind};
use crate::runtime::model::SourceId;
use iptv_core::lists::{Entry, ListId};
use iptv_core::organization::{Block, BlockKind, Mode};

use iptv_core::menu::{Heading, Menu, MenuItem, Origin, Row, program_menu};
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
        ToggleFavorite if favorite => tr("Remove from favourites"),
        ToggleFavorite => tr("Add to favourites"),
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
        ManageFavorites => tr("Manage favourites…"),
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
        HideGroup => tr("Hide group"),
    }
}

fn title(channel: usize, p: crate::guide::Program) -> String {
    crate::schedule().title_in(channel, p, crate::i18n::language())
}

/// Long OK / Menu: the menu for the focused programme (in full screen, the
/// playing channel's current programme).
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
    let list = current_list(ui);
    let scope = channel_scope(ui, &list);
    let program = crate::schedule().program_at(channel, minute);
    let mut rows = program_menu(
        program,
        crate::schedule().catchup_days(channel),
        now,
        |item| {
            if matches!(item, MenuItem::BlockGroup | MenuItem::ProtectGroup)
                && group_target(scope.as_ref()).is_none()
            {
                return false;
            }
            if matches!(
                item,
                MenuItem::Record | MenuItem::ScheduleRecording | MenuItem::CustomRecording
            ) {
                return ui.player_surface == 0 && crate::recordings_view::available(channel);
            }
            match item {
                MenuItem::ManageHidden
                | MenuItem::ManageBlocked
                | MenuItem::ReorderChannels
                | MenuItem::GroupOptions => scope.is_some(),
                MenuItem::SortChannels => scope
                    .as_ref()
                    .is_some_and(|(_, kind)| *kind != Kind::Favorites),
                MenuItem::CopyChannels | MenuItem::CreateGroup => {
                    !crate::data::combined::active(&crate::runtime())
                }
                _ => implemented(item, ui.player_surface),
            }
        },
    );
    if cfg!(any(target_os = "linux", target_os = "macos"))
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
    let mut menu = Menu::new(rows, origin);
    menu.list = Some(list);
    ui.menu.set(Some(menu));
}

/// Closes the menu or popover; guide focus stays on the programme it was
/// opened on (it does not jump to now).
pub fn close(ui: Ui) {
    let origin = ui
        .menu
        .with(|m| m.as_ref().filter(|m| !m.group_context).map(|m| m.origin))
        .or_else(|| ui.description.get());
    ui.menu.set(None);
    ui.description.set(None);
    if let Some(origin) = origin
        && ui.mode.get() != ViewMode::Fullscreen
    {
        crate::lists::tap(ui, origin.channel, origin.program_start);
    }
}

pub fn perform(ui: Ui, item: MenuItem) {
    let Some((origin, list, group_context)) = ui.menu.with(|menu| {
        let menu = menu.as_ref()?;
        Some((menu.origin, menu.list.clone(), menu.group_context))
    }) else {
        return;
    };
    ui.menu.set(None);
    let list = list.unwrap_or_else(|| current_list(ui));
    let scope = channel_scope(ui, &list);
    match item {
        MenuItem::BlockChannel | MenuItem::BlockGroup => {
            if item == MenuItem::BlockChannel
                && crate::schedule().channel_block(origin.channel) == Block::Group
            {
                crate::settings_view::notify(tr("This channel is blocked by a group."));
                return;
            }
            if let Some(target) = block_target(ui, item, origin.channel, scope.as_ref()) {
                let kind = if item == MenuItem::BlockGroup {
                    BlockKind::Group
                } else {
                    BlockKind::Channel
                };
                let blocked = !directly_blocked(&list, item, origin.channel);
                organization_view::set_block(ui, SourceId(target.source), kind, target.id, blocked);
            }
        }
        MenuItem::ProtectChannel | MenuItem::ProtectGroup => {
            if let Some(target) = block_target(ui, item, origin.channel, scope.as_ref()) {
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
            if group_context {
                if let Some((source, kind)) = scope {
                    organization_view::open_guide_mode(ui, source, kind, None, mode, list);
                }
            } else {
                open_channel_editor(ui, origin.channel, mode, scope, list);
            }
        }
        MenuItem::Record => crate::recordings_view::record_origin(ui, origin),
        MenuItem::CustomRecording => crate::recordings_view::programme(ui, origin, true),
        MenuItem::ScheduleRecording => crate::recordings_view::schedule_programme(ui, origin),
        // Details replaces the menu; Back from it returns to the guide.
        MenuItem::Description => ui.description.set(Some(origin)),
        MenuItem::WatchInNewWindow => {
            crate::desktop::player_windows::open(ui, origin.channel);
        }
        MenuItem::StartFromBeginning => {
            close(ui);
            let schedule = crate::schedule();
            let now = ui.focus.with_untracked(|focus| focus.now);
            let program = origin_programme(&*schedule, origin);
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
        MenuItem::ManageFavorites => {
            close(ui);
            crate::fav_editor::open(ui);
        }
        MenuItem::Search => {
            close(ui);
            crate::search_view::open(ui);
        }
        MenuItem::Settings => {
            close(ui);
            crate::settings_view::open(ui);
        }
        MenuItem::ToggleFavorite => {
            close(ui);
            crate::favorites::toggle(ui, origin.channel);
        }
        MenuItem::HideChannel => {
            close(ui);
            if let Some(target) = crate::parental_view::live_target(ui, origin.channel) {
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
        MenuItem::CreateGroup | MenuItem::CopyChannels => {
            close(ui);
            if let Some(source) = ui.app.library.shown.get_untracked() {
                crate::custom_groups::open(ui, source, item == MenuItem::CopyChannels);
            }
        }
        MenuItem::ChannelOptions => {
            open_channel_editor(ui, origin.channel, Mode::Options, scope, list);
        }
        MenuItem::SortChannels => {
            if let Some((source, kind)) = scope {
                organization_view::open_sort(ui, source, kind, Some(list));
            }
        }
        MenuItem::GroupOptions => {
            show_group_options(ui, list);
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
            close(ui);
            crate::guide_assign_view::open(ui, origin.channel);
        }
        other => {
            log::info!("iptv menu {other:?} not implemented yet");
            close(ui);
        }
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
    channel: usize,
    scope: Option<&(SourceId, Kind)>,
) -> Option<iptv_core::parental::Target> {
    match item {
        MenuItem::BlockChannel | MenuItem::ProtectChannel => {
            crate::parental_view::live_target(ui, channel)
        }
        MenuItem::BlockGroup | MenuItem::ProtectGroup => group_target(scope),
        _ => None,
    }
}

fn directly_blocked(list: &ListId, item: MenuItem, channel: usize) -> bool {
    let schedule = crate::schedule();
    match item {
        MenuItem::BlockChannel => schedule.channel_block(channel) == Block::Channel,
        MenuItem::BlockGroup => {
            matches!(Entry::at(list.find(&*schedule)), Entry::Group(group) if schedule.group_blocked(group))
        }
        _ => false,
    }
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
    channel_scope(ui, &ListId::of(entry, &*crate::schedule())).is_some()
}

/// A focused group is actionable even when none of its channels are visible.
pub fn open_group_options(ui: Ui, entry: usize) -> bool {
    let list = ListId::of(entry, &*crate::schedule());
    show_group_options(ui, list)
}

fn show_group_options(ui: Ui, list: ListId) -> bool {
    use MenuItem::*;

    let scope = channel_scope(ui, &list);
    let Some((_, kind)) = scope.as_ref() else {
        return false;
    };
    let mut rows = vec![Row::Heading(Heading::Group)];
    let mut add = |item| {
        rows.push(Row::Action {
            item,
            enabled: true,
        })
    };
    if *kind != Kind::Favorites {
        add(SortGroups);
        add(ReorderGroups);
        add(SortChannels);
    }
    add(ReorderChannels);
    if group_target(scope.as_ref()).is_some() {
        add(HideGroup);
        add(BlockGroup);
    }
    add(ManageHidden);
    add(ManageBlocked);
    if *kind != Kind::Favorites {
        add(ManageGroups);
    }
    let origin = Origin {
        channel: crate::lists::focused_channel(ui).unwrap_or_default(),
        program_start: ui.focus.with_untracked(|focus| focus.now),
    };
    let mut menu = Menu::new(rows, origin);
    menu.list = Some(list);
    menu.group_context = true;
    ui.menu.set(Some(menu));
    true
}

fn heading_text(ui: Ui, heading: Heading) -> String {
    let Some(origin) = ui.menu.with(|m| m.as_ref().map(|m| m.origin)) else {
        return String::new();
    };
    match heading {
        Heading::Program => {
            let p = origin_programme(&*crate::schedule(), origin);
            format!(
                "{} · {}–{}",
                title(origin.channel, p),
                clock(p.start),
                clock(p.end)
            )
        }
        Heading::Channel => channel_name(origin.channel),
        Heading::Group => {
            ui.guide_data();
            let catalog = crate::schedule();
            let entry = ui.menu.with(|menu| {
                menu.as_ref()?
                    .list
                    .as_ref()
                    .map(|list| list.find(&*catalog))
            });
            entry.map_or_else(
                || crate::lists::list_name(ui),
                |entry| crate::lists::entry_name(entry, &*catalog),
            )
        }
    }
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
        let favorite = move || {
            let ch = ui.menu.with(|m| m.as_ref().map(|m| m.origin.channel));
            ch.is_some_and(|ch| ui.app.favorites.with(|f| f.contains(&ch)))
        };
        let focused = move || ui.keys_shown() && focus.get() == Some(at.index());
        let mark = move || Mark::of(focused(), false);
        let heading = move || matches!(row(), Some(Row::Heading(_)));
        let title = move || match row() {
            Some(Row::Heading(h)) => heading_text(ui, h),
            Some(Row::Action { item, .. }) => {
                ui.guide_data();
                let blocked = ui.menu.with(|menu| {
                    menu.as_ref().is_some_and(|menu| {
                        menu.list
                            .as_ref()
                            .is_some_and(|list| directly_blocked(list, item, menu.origin.channel))
                    })
                });
                label(item, favorite(), blocked).to_owned()
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
                let picked = ui
                    .menu
                    .with(|m| m.clone())
                    .and_then(|mut m| m.tap(at.index()));
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

fn origin_programme(
    schedule: &dyn crate::guide::Schedule,
    origin: Origin,
) -> crate::guide::Program {
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
        ui.description.get().map(|origin| {
            (
                origin.channel,
                origin_programme(&*crate::schedule(), origin),
            )
        })
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
                            .and_then(|(channel, programme)| {
                                crate::header::programme_description(
                                    &*crate::schedule(),
                                    channel,
                                    programme,
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
                                    crate::recordings_view::programme(ui, origin, false);
                                }
                            },
                        )
                    },
                )),
        )
}

/// Context-click targets a channel without tuning the main player.
pub fn open_at(ui: Ui, channel: usize, minute: i32) {
    ui.touched();
    crate::lists::tap(ui, channel, minute);
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
