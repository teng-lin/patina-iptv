//! The guide's context menu (Long OK / Menu) as a scrolling side panel, and
//! the programme description popover. State lives in `iptv_core::menu`;
//! this opens, draws and performs it. Labels are iptv-rs's own wording.

use crate::i18n::tr;

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
fn implemented(item: MenuItem) -> bool {
    matches!(
        item,
        MenuItem::Search
            | MenuItem::Settings
            | MenuItem::Description
            | MenuItem::ToggleFavorite
            | MenuItem::StartFromBeginning
            | MenuItem::ManageFavorites
            | MenuItem::AssignEpg
            | MenuItem::HideChannel
            | MenuItem::ManageHidden
            | MenuItem::ReorderChannels
            | MenuItem::ChannelOptions
            | MenuItem::GroupOptions
            | MenuItem::CopyChannels
            | MenuItem::CreateGroup
    )
}

fn label(item: MenuItem, favorite: bool) -> &'static str {
    use MenuItem::*;
    match item {
        Search => tr("Search"),
        Settings => tr("Settings"),
        StartFromBeginning => tr("Play from the beginning"),
        ExternalPlayer => tr("Open in another player"),
        Reminder => tr("Remind me"),
        Record => tr("Record"),
        CustomRecording => "Schedule a recording…",
        SaveToList => "Save to my list",
        Description => tr("Details"),
        ToggleFavorite if favorite => tr("Remove from favourites"),
        ToggleFavorite => tr("Add to favourites"),
        BlockChannel => tr("Block channel"),
        HideChannel => tr("Hide channel"),
        AssignEpg => tr("Choose guide data…"),
        ChannelOptions => tr("Channel options…"),
        ManageFavorites => tr("Manage favourites…"),
        ManageBlocked => "Manage blocked channels…",
        ManageHidden => "Manage hidden channels…",
        ReorderChannels => tr("Reorder channels"),
        CopyChannels => "Copy channels to a group…",
        CreateGroup => tr("New group…"),
        GroupOptions => tr("Group options…"),
    }
}

fn title(channel: usize, p: crate::guide::Program) -> String {
    crate::schedule().title(channel, p)
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
    let program = crate::schedule().program_at(channel, minute);
    let rows = program_menu(
        program,
        crate::schedule().catchup_days(channel),
        now,
        |item| {
            implemented(item)
                && (!crate::data::combined::active(ui)
                    || matches!(
                        item,
                        MenuItem::Search
                            | MenuItem::Settings
                            | MenuItem::Description
                            | MenuItem::ToggleFavorite
                            | MenuItem::StartFromBeginning
                            | MenuItem::ManageFavorites
                            | MenuItem::AssignEpg
                    ))
        },
    );
    let origin = Origin {
        channel,
        program_start: program.start,
    };
    ui.menu.set(Some(Menu::new(rows, origin)));
}

/// Closes the menu or popover; guide focus stays on the programme it was
/// opened on (it does not jump to now).
pub fn close(ui: Ui) {
    let origin = ui
        .menu
        .with(|m| m.as_ref().map(|m| m.origin))
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
    let Some(origin) = ui.menu.with(|m| m.as_ref().map(|m| m.origin)) else {
        return;
    };
    ui.menu.set(None);
    match item {
        // Details replaces the menu; Back from it returns to the guide.
        MenuItem::Description => ui.description.set(Some(origin)),
        MenuItem::StartFromBeginning => {
            close(ui);
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
            if let (Some(source), Some(id)) = (
                ui.library.shown.get_untracked(),
                crate::schedule().channel_key(origin.channel),
            ) {
                let saved = crate::runtime::commands::hide_channel(source, id);
                crate::data::spawn_in_app(async move {
                    match saved.await {
                        Ok(()) => crate::data::organization::changed(ui, source),
                        Err(e) => log::warn!("iptv organization: {e}"),
                    }
                });
            }
        }
        MenuItem::CreateGroup | MenuItem::CopyChannels => {
            close(ui);
            if let Some(source) = ui.library.shown.get_untracked() {
                crate::custom_groups::open(ui, source, item == MenuItem::CopyChannels);
            }
        }
        MenuItem::ChannelOptions
        | MenuItem::ManageHidden
        | MenuItem::ReorderChannels
        | MenuItem::GroupOptions => {
            close(ui);
            if let Some(source) = ui.library.shown.get_untracked() {
                let groups = item == MenuItem::GroupOptions;
                let preferred = if groups {
                    Some(
                        crate::schedule().group_key(
                            ui.focus
                                .with(|f| f.group.saturating_sub(iptv_core::lists::FIRST_GROUP)),
                        ),
                    )
                } else {
                    crate::schedule().channel_key(origin.channel)
                };
                crate::organization_view::open(
                    ui,
                    source,
                    if groups {
                        crate::organization_view::Kind::Groups
                    } else if item == MenuItem::ReorderChannels {
                        use crate::organization_view::Kind;
                        use iptv_core::lists::Entry;
                        match Entry::at(ui.list.with_untracked(|l| l.entry())) {
                            Entry::Favorites => Kind::Favorites,
                            Entry::Group(g) => {
                                let key = crate::schedule().group_key(g);
                                if let Some(id) = key
                                    .strip_prefix("custom:")
                                    .and_then(|s| s.parse::<i64>().ok())
                                {
                                    Kind::CustomChannels(crate::runtime::model::CustomGroupId(id))
                                } else {
                                    Kind::GroupChannels(iptv_providers::GroupId::from_stored(key))
                                }
                            }
                            _ => Kind::Channels,
                        }
                    } else {
                        crate::organization_view::Kind::Channels
                    },
                    preferred,
                );
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

fn heading_text(ui: Ui, heading: Heading) -> String {
    let Some(origin) = ui.menu.with(|m| m.as_ref().map(|m| m.origin)) else {
        return String::new();
    };
    match heading {
        Heading::Program => {
            let p = crate::schedule().program_at(origin.channel, origin.program_start);
            format!(
                "{} · {}–{}",
                title(origin.channel, p),
                clock(p.start),
                clock(p.end)
            )
        }
        Heading::Channel => channel_name(origin.channel),
        Heading::Group => crate::lists::list_name(ui),
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
            ch.is_some_and(|ch| ui.favorites.with(|f| f.contains(&ch)))
        };
        let focused = move || ui.keys_shown() && focus.get() == Some(at.index());
        let mark = move || Mark::of(focused(), false);
        let heading = move || matches!(row(), Some(Row::Heading(_)));
        let name = list_rows::name(size, move || match row() {
            Some(Row::Heading(h)) => heading_text(ui, h),
            Some(Row::Action { item, .. }) => label(item, favorite()).to_owned(),
            None => String::new(),
        })
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
            .accessibility_state(move || AccessibilityState {
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
                .bg(move || rgb(ui.theme.with(|t| t.background.panel)))
                .child(
                    virtual_list(count, row)
                        .estimated_item_size(list_rows::pitch(size, ROW))
                        .element_ref(&list)
                        .flex_1(),
                ),
        )
        .child(description(ui))
}

/// The description popover: title, time range and text; no actions (those
/// live in the menu). Back or a tap closes it.
fn description(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let shown = move || ui.description.get().is_some();
    let program = move || {
        ui.description.get().map(|o| {
            (
                o.channel,
                crate::schedule().program_at(o.channel, o.program_start),
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
        .pointer_events(move || if shown() { PointerEvents::Auto } else { PointerEvents::None })
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
                .bg(move || rgb(ui.theme.with(|t| t.background.panel)))
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
                    text("A synthetic programme description for testing the guide. Real descriptions come from the EPG.")
                        .font_family(SANS)
                        .text_size(move || size.text_px(HEADER_BODY))
                        .text_color(rgb(LIST_TEXT)),
                ),
        )
}
