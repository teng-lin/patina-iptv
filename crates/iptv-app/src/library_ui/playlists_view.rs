//! The playlists screen: the stored playlists (name, type, channel count,
//! last update or last failure, the one on screen marked), "Refresh all"
//! (with two or more) and "Add playlist". OK (or Menu, or a tap) on a
//! playlist opens its actions: show it, refresh it now, rename it, change
//! its provider details (link, server and login, portal and MAC address:
//! checked by importing them, kept only if that works), its User-Agent,
//! its streams at once (Automatic, the allowance its provider reports and
//! shown beside it, or a number), its guide sources (`epg_sources_view`),
//! delete it after a confirmation
//! that states what goes. `state.rs` and `details.rs` own
//! focus and flow; `data::library` does the work.
//!
//! Remote: Up/Down move, OK acts, Menu opens a playlist's actions, Back
//! closes the open sheet, then the screen. Touch and mouse: every row and
//! action is tappable; a tap beside a sheet closes it. Text entry uses the
//! wizard's sheet (`ui_kit::entry`). Wording is iptv-rs's own.

use crate::chrome::i18n::{tr, trf};
use crate::chrome::input::dismiss_keyboard;

use iptv_presentation::library::details;
mod details_card;
use iptv_presentation::library::playlists as state;
mod words;

use crate::runtime::model::SourceId;
use iptv_nav::Action;
use nsh_core::prelude::*;

pub use state::Only;
pub use state::Screen as Open;
use state::{Act, EditKind, Effect, Item, Key, Layer, Screen};
pub(crate) use words::masked_link;

use crate::chrome::scale::Scale;
use crate::chrome::ui_kit::entry::{self, TextSheet};
use crate::chrome::ui_kit::{self, ERROR, ROW_THREE};
use crate::guide::list_rows::{self, Mark};
use crate::wiring::data::{self, Activity, Playlist};
use crate::{Ui, ViewMode, tint};

const WHITE: u32 = 0xFFFFFF;
const CARD_W: f32 = 640.0;

/// What a playlist's row says under its name: its type and channels,
/// then what its last refresh did (running, updated, failed, details
/// refused) and whether it is on screen; and whether that is a failure.
/// Reactive: the library's activity and the playlist on screen.
pub(crate) fn summary(ui: Ui, playlist: &Playlist) -> (String, bool) {
    let activity = ui
        .app
        .library
        .activity
        .with(|a| a.get(&playlist.id).cloned());
    let (status, failed) = words::status(playlist, activity.as_ref(), crate::time::wall_unix());
    let shown = !ui.app.library.all.get() && ui.app.library.shown.get() == Some(playlist.id);
    let parts = [
        words::summary(playlist),
        status,
        if shown {
            tr("On screen").to_owned()
        } else {
            String::new()
        },
    ];
    let line = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    (line, failed)
}

/// Whether the screen is open for one task over another screen.
pub(crate) fn task_open(screens: crate::ScreensUi) -> bool {
    state(screens, |s| s.opened_for().is_some()).unwrap_or(false)
}

// --- Actions -------------------------------------------------------------------

/// Opens the screen (the navigation rail's Playlists).
pub fn open(ui: Ui) {
    if is_open(ui.screens) || crate::library_ui::wizard_view::is_open(ui.screens) {
        return;
    }
    let mut screen = Screen::new(load(), ui.app.library.shown.get_untracked());
    if ui.app.library.all.get_untracked() {
        screen.focus_all();
    }
    ui.screens.playlists.set(Some(screen));
    if ui.chrome.mode.get_untracked() == ViewMode::Fullscreen {
        ui.chrome
            .mode
            .apply(crate::chrome::mode::Action::ReturnToBrowse);
    }
}

pub(crate) fn choose_for_management(screens: crate::ScreensUi) {
    screens.playlists.set(Some(Screen::settings_picker(load())));
}

/// Opens a playlist task from Settings or a guide heading.
pub fn open_only(screens: crate::ScreensUi, id: SourceId, only: Only) {
    if is_open(screens) || crate::library_ui::wizard_view::is_open(screens) {
        return;
    }
    if let Some(screen) = Screen::only(load(), id, only) {
        log::info!("iptv playlists: {only:?} of {}", id.0);
        screens.playlists.set(Some(screen));
    }
}

/// Opens the screen at playlist `id`'s movie and series import (an empty
/// Movies or Shows screen's "Playlist settings"). Switching it stays the
/// viewer's choice.
pub fn open_on_demand(ui: Ui, id: SourceId) {
    open(ui);
    ui.screens.playlists.update(|s| {
        if let Some(s) = s {
            s.open_actions(id, Act::OnDemand);
        }
    });
}

pub fn close(screens: crate::ScreensUi) {
    if screens.playlists.with_untracked(Option::is_some) {
        screens.playlists.set(None);
        dismiss_keyboard();
    }
}

pub fn is_open(screens: crate::ScreensUi) -> bool {
    screens.playlists.with(Option::is_some)
}

fn load() -> Vec<Playlist> {
    crate::wiring::runtime_owner::runtime()
        .playlists()
        .unwrap_or_else(|e| {
            log::warn!("iptv playlists: the store is unavailable: {e}");
            Vec::new()
        })
}

fn state<R>(screens: crate::ScreensUi, f: impl FnOnce(&Screen) -> R) -> Option<R> {
    screens.playlists.with(|s| s.as_ref().map(f))
}

/// Changes the state and carries out what it answers.
fn update(ui: Ui, f: impl FnOnce(&mut Screen) -> Option<Effect>) {
    let mut effect = None;
    let mut finished = false;
    ui.screens.playlists.update(|s| {
        if let Some(s) = s {
            effect = f(s);
            finished = s.finished();
        }
    });
    // Opened for one task, which is over: back to where it came from.
    if finished {
        close(ui.screens);
    }
    if let Some(effect) = effect {
        perform(ui, effect);
    }
}

fn editing(screens: crate::ScreensUi) -> bool {
    state(screens, |s| s.editing().is_some()).unwrap_or(false)
}

/// A remote action while the screen is open.
pub fn key(ui: Ui, action: Action) {
    let key = match action {
        Action::MoveUp => Key::Up,
        Action::MoveDown => Key::Down,
        Action::MoveLeft => Key::Left,
        Action::MoveRight => Key::Right,
        Action::Select => Key::Ok,
        Action::Back => Key::Back,
        Action::ShowMenu => Key::Menu,
        _ => return,
    };
    if editing(ui.screens) {
        // The input has the keys while focused; these come once it lost it.
        if matches!(key, Key::Ok | Key::Back) {
            commit(ui);
        }
        return;
    }
    update(ui, |s| s.key(key));
}

/// The platform's Back: ends text entry, closes a sheet, then the screen.
pub fn back(ui: Ui) -> bool {
    key(ui, Action::Back);
    true
}

fn commit(ui: Ui) {
    dismiss_keyboard();
    update(ui, Screen::commit);
}

fn cancel(ui: Ui) {
    dismiss_keyboard();
    update(ui, |s| {
        s.cancel_edit();
        None
    });
}

fn perform(ui: Ui, effect: Effect) {
    if matches!(effect, Effect::Policy(..)) {
        crate::wiring::navigation::settings_guarded(ui.screens, move || apply_effect(ui, effect));
        return;
    }
    if matches!(
        &effect,
        Effect::Add
            | Effect::Delete(_)
            | Effect::OpenDetails(_)
            | Effect::TryDetails(..)
            | Effect::SetUserAgent(..)
            | Effect::ToggleOnDemand(_)
            | Effect::CycleStreams(_)
    ) && crate::wiring::runtime_owner::runtime().parental_has_pin()
        && !crate::wiring::runtime_owner::runtime().parental_settings_allowed()
    {
        crate::wiring::navigation::request(
            ui.screens,
            iptv_core::parental::Target::settings(),
            move || perform(ui, effect),
        );
        return;
    }
    apply_effect(ui, effect);
}

fn apply_effect(ui: Ui, effect: Effect) {
    crate::wiring::tasks::spawn_in_app(async move {
        log::info!("iptv playlists: {}", effect_name(&effect));
        let changes_library = matches!(effect, Effect::Delete(_) | Effect::Policy(..));
        let result = match effect {
            Effect::Reorder(id) => {
                close(ui.screens);
                crate::library_ui::organization_view::open_playlist_reorder(ui, Some(id));
                Ok(())
            }
            Effect::PlaylistOrder(id) => {
                crate::chrome::menu_view::open_playlist_order(ui.screens, id);
                Ok(())
            }
            Effect::ReorderGroups(id) | Effect::GroupOrder(id) => {
                let list = iptv_presentation::lists::ListId::Group(format!("playlist:{}", id.0));
                if matches!(effect, Effect::ReorderGroups(_)) {
                    crate::library_ui::organization_view::open_group_reorder(ui, id, None, list);
                } else {
                    crate::library_ui::organization_view::open_sort(
                        ui,
                        id,
                        crate::library_ui::organization_view::Kind::Groups,
                        Some(list),
                    );
                }
                Ok(())
            }
            Effect::AllPlaylists => {
                close(ui.screens);
                data::switch_all(&crate::wiring::runtime_owner::runtime());
                Ok(())
            }
            Effect::Add => {
                crate::library_ui::wizard_view::open_add(ui);
                Ok(())
            }
            Effect::Show(id) => {
                close(ui.screens);
                data::switch_to(&crate::wiring::runtime_owner::runtime(), id);
                Ok(())
            }
            Effect::Refresh(id) => {
                data::refresh(&crate::wiring::runtime_owner::runtime(), id);
                Ok(())
            }
            Effect::Policy(id, edit) => {
                data::update_policy(&crate::wiring::runtime_owner::runtime(), id, move |p| {
                    edit.apply(p)
                })
                .await
            }
            Effect::ToggleOnDemand(id) => {
                let on = on_demand(id);
                if let Err(e) = crate::wiring::runtime_owner::runtime().set_playlist_setting(
                    id,
                    iptv_settings::keys::PLAYLISTS_INCLUDE_VOD.as_str(),
                    iptv_settings::Value::Bool(!on),
                ) {
                    crate::screens::settings_view::notify_setting_error(e);
                }
                Ok(())
            }
            Effect::CycleStreams(id) => {
                let next = state::next_streams(own_streams(id));
                if let Err(e) =
                    crate::wiring::runtime_owner::runtime().set_connection_limit(id, next)
                {
                    crate::screens::settings_view::notify_setting_error(e);
                }
                Ok(())
            }
            Effect::CustomGroups(id) => {
                crate::library_ui::custom_groups::open(ui, id);
                Ok(())
            }
            Effect::Organize(id, groups) => {
                crate::library_ui::organization_view::open(
                    ui,
                    id,
                    if groups {
                        crate::library_ui::organization_view::Kind::Groups
                    } else {
                        crate::library_ui::organization_view::Kind::Channels
                    },
                    None,
                );
                Ok(())
            }
            Effect::Rename(id, name) => {
                data::rename(&crate::wiring::runtime_owner::runtime(), id, &name).await
            }
            Effect::Guides(id) => {
                crate::library_ui::epg_sources_view::open(ui.screens, id);
                Ok(())
            }
            Effect::Delete(id) => data::delete(&crate::wiring::runtime_owner::runtime(), id).await,
            Effect::RefreshAll => {
                data::refresh_all(&crate::wiring::runtime_owner::runtime());
                Ok(())
            }
            Effect::OpenDetails(id) => {
                if let Some(source) = crate::wiring::runtime_owner::runtime().source_details(id) {
                    update(ui, |s| {
                        s.open_details(source);
                        None
                    });
                }
                Ok(())
            }
            Effect::TryDetails(id, source) => {
                data::try_details(&crate::wiring::runtime_owner::runtime(), id, *source);
                Ok(())
            }
            Effect::SetUserAgent(id, agent) => {
                data::set_user_agent(&crate::wiring::runtime_owner::runtime(), id, agent);
                Ok(())
            }
            Effect::Close => {
                close(ui.screens);
                Ok(())
            }
        };
        if let Err(e) = result {
            log::warn!("iptv playlists: the store refused it: {e}");
            if e.settings_locked() {
                crate::screens::settings_view::notify_setting_error(
                    crate::wiring::settings::SaveError::ParentalLocked,
                );
            } else if changes_library {
                crate::chrome::toast::notify(tr(
                    "The change could not be saved. Reopen the list and try again.",
                ));
            }
        }
    });
}

/// For the log: the effect without names or links.
fn effect_name(effect: &Effect) -> String {
    match effect {
        Effect::Add => "add".to_owned(),
        Effect::Reorder(id) => format!("reorder playlist {}", id.0),
        Effect::PlaylistOrder(_) | Effect::GroupOrder(_) => "playlist order".to_owned(),
        Effect::ReorderGroups(id) => format!("reorder playlist {}", id.0),
        Effect::AllPlaylists => "show all playlists".to_owned(),
        Effect::Show(id) => format!("show {}", id.0),
        Effect::Refresh(id) => format!("refresh {}", id.0),
        Effect::Policy(id, _) => format!("playlist policy {}", id.0),
        Effect::ToggleOnDemand(id) => format!("movie and series import of {}", id.0),
        Effect::CycleStreams(id) => format!("streams at once of {}", id.0),
        Effect::CustomGroups(id) => format!("custom groups {}", id.0),
        Effect::Organize(id, _) => format!("organize {}", id.0),
        Effect::Rename(id, _) => format!("rename {}", id.0),
        Effect::Guides(id) => format!("guide sources of {}", id.0),
        Effect::Delete(id) => format!("delete {}", id.0),
        Effect::RefreshAll => "refresh all".to_owned(),
        Effect::OpenDetails(id) => format!("details of {}", id.0),
        Effect::TryDetails(id, _) => format!("try new details of {}", id.0),
        Effect::SetUserAgent(id, agent) => {
            format!("User-Agent of {}: set {}", id.0, agent.is_some())
        }
        Effect::Close => "close".to_owned(),
    }
}

/// Whether playlist `id` imports its movies and series (its own setting,
/// else the global one).
fn on_demand(id: SourceId) -> bool {
    crate::wiring::settings::get().bool(
        iptv_settings::keys::PLAYLISTS_INCLUDE_VOD,
        &iptv_settings::Target::playlist(id.0.to_string()),
    )
}

/// Playlist `id`'s own streams-at-once ceiling; `None` is Automatic (the
/// allowance its provider reports).
fn own_streams(id: SourceId) -> Option<u32> {
    let at = iptv_settings::Target::playlist(id.0.to_string());
    match crate::wiring::settings::get()
        .stored(iptv_settings::keys::PLAYLISTS_MAX_CONNECTIONS.as_str(), &at)
    {
        Some(iptv_settings::Value::Int(n)) => u32::try_from(*n).ok(),
        _ => None,
    }
}

// --- View ----------------------------------------------------------------------

/// A row's or action's mark: the pill while focused with the keys shown.
fn mark(chrome: crate::ChromeUi, focused: bool) -> Mark {
    Mark::of(focused && chrome.keys_shown(), false)
}

fn contents_hidden(screens: crate::ScreensUi) -> bool {
    state(screens, |screen| {
        screen.layer() != Layer::List || screen.editing().is_some()
    })
    .unwrap_or(false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    // What is stored changed (refreshed, renamed, deleted, added): reload.
    effect(move || {
        ui.app.library.version.get();
        ui.app.settings.get();
        if ui.screens.playlists.with_untracked(Option::is_some) {
            let rows = load();
            ui.screens.playlists.update(|s| {
                if let Some(s) = s {
                    s.set_rows(rows);
                }
            });
        }
    });
    // Opened for one task, which ended however it ended (a key, a check
    // of new details, a playlist deleted elsewhere): back to where it
    // came from, so no empty layer covers it.
    effect(move || {
        if state(ui.screens, Screen::finished).unwrap_or(false) {
            untracked(|| close(ui.screens));
        }
    });
    // New details checked: saved closes the form, refused keeps it.
    effect(move || {
        let ended = ui.app.library.activity.with(|a| {
            a.iter()
                .filter_map(|(id, act)| match act {
                    Activity::Done(Ok(_)) => Some((*id, true)),
                    Activity::Done(Err(_)) | Activity::Rejected(_) => Some((*id, false)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        untracked(|| {
            ui.screens.playlists.update(|s| {
                if let Some(s) = s {
                    ended
                        .iter()
                        .for_each(|(id, saved)| s.details_checked(*id, *saved));
                }
            });
        });
    });
    // What is typed into, if anything: the text sheet is made when it
    // changes (not as the text does).
    let entry = computed(move || {
        state(ui.screens, |s| s.editing().map(|e| (e.kind, e.prefilled))).flatten()
    });
    let size = Scale::new();
    let only = computed(move || state(ui.screens, |s| s.opened_for().is_some()));
    dynamic(move || match only.get() {
        None => view(),
        // Opened for one task: its sheet over what opened it.
        Some(true) => untracked(move || {
            view()
                .test_id("playlists-task")
                .absolute()
                .inset_0()
                .child(sheet(ui, size))
                .child(dynamic(move || {
                    let shown = entry.get();
                    untracked(move || match shown {
                        Some((kind, prefilled)) => edit_sheet(ui, kind, prefilled),
                        None => view(),
                    })
                }))
        }),
        Some(false) => untracked(move || {
            ui_kit::screen(ui, size)
                .test_id("playlists-screen")
                .accessibility_elements_hidden(move || {
                    crate::chrome::overlays::Overlay::Playlists.accessibility_covered(ui)
                })
                .child(
                    ui_kit::header(ui, size, move || tr("Playlists"), |ui| close(ui.screens))
                        .accessibility_elements_hidden(move || contents_hidden(ui.screens)),
                )
                .child(
                    ui_kit::caption(size, move || hint(ui))
                        .accessibility_elements_hidden(move || contents_hidden(ui.screens)),
                )
                .child(list(ui, size))
                .child(sheet(ui, size))
                .child(dynamic(move || {
                    let shown = entry.get();
                    untracked(move || match shown {
                        Some((kind, prefilled)) => edit_sheet(ui, kind, prefilled),
                        None => view(),
                    })
                }))
        }),
    })
}

fn yes_no(yes: bool) -> &'static str {
    if yes { tr("Yes") } else { tr("No") }
}

pub(crate) fn hint(ui: Ui) -> &'static str {
    let layer = state(ui.screens, Screen::layer).unwrap_or(Layer::List);
    match (ui.keys_shown(), layer) {
        (true, Layer::List) => tr("▲▼ choose   OK actions   Back close"),
        (true, Layer::Actions { .. }) => tr("▲▼ choose   OK do it   Back close the sheet"),
        (true, Layer::Confirm { .. }) => tr("▲▼ choose   OK confirm   Back keep it"),
        (true, Layer::Details { .. }) => {
            tr("▲▼ choose   OK change or check   Back leave unchanged")
        }
        (false, _) => tr("Tap a playlist for what you can do with it"),
    }
}

fn list(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    let count = move || state(ui.screens, |s| s.items().len()).unwrap_or(0);
    let focused_index = computed(move || {
        state(ui.screens, |s| {
            s.items().iter().position(|i| *i == s.focus())
        })
        .flatten()
    });
    effect(move || {
        if let Some(i) = focused_index.get()
            && ui.keys_shown()
        {
            rows.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let all_index = computed(move || {
        state(ui.screens, |s| {
            s.items()
                .iter()
                .position(|item| *item == Item::AllPlaylists)
        })
        .flatten()
    });
    // A cell is reused for other positions; its handlers and test ID must follow.
    virtual_list(count, move |at| {
        dynamic(move || {
            let index = at.index();
            let is_all = all_index.get() == Some(index);
            untracked(move || row(ui, size, index, focused_index, is_all))
        })
    })
    .test_id("playlists-list")
    .accessibility_elements_hidden(move || contents_hidden(ui.screens))
    .estimated_item_size(list_rows::pitch(size, ROW_THREE))
    .element_ref(&rows)
    .flex_1()
}

fn row(
    ui: Ui,
    size: Scale,
    index: usize,
    focused_index: Memo<Option<usize>>,
    is_all: bool,
) -> impl IntoElement {
    let playlist = move || state(ui.screens, |s| s.rows().get(index).cloned()).flatten();
    let item = move || state(ui.screens, |s| s.items().get(index).copied()).flatten();
    let refreshing = move || {
        ui.app.library.activity.with(|a| {
            a.values()
                .filter(|act| matches!(act, Activity::Running(_) | Activity::Checking(_)))
                .count()
        })
    };
    let shown = move || {
        playlist()
            .is_some_and(|p| !ui.app.library.all.get() && ui.app.library.shown.get() == Some(p.id))
            || (item() == Some(Item::AllPlaylists) && ui.app.library.all.get())
    };
    let activity = move |id: SourceId| ui.app.library.activity.with(|a| a.get(&id).cloned());
    let status = move || match (playlist(), item()) {
        (Some(p), _) => words::status(
            &p,
            activity(p.id).as_ref(),
            crate::guide::clock::wall_unix(),
        ),
        (None, Some(Item::RefreshAll)) => (words::refreshing(refreshing()), false),
        _ => (String::new(), false),
    };
    let title = move || match (playlist(), item()) {
        (Some(p), _) => p.name,
        (None, Some(Item::RefreshAll)) => tr("Refresh all playlists").to_owned(),
        (None, Some(Item::AllPlaylists)) => tr("All playlists").to_owned(),
        _ => tr("+ Add playlist").to_owned(),
    };
    let summary = move || match (playlist(), item()) {
        (Some(p), _) => words::summary(&p),
        (None, Some(Item::RefreshAll)) => {
            tr("Each one downloads again; the one on screen updates in place").to_owned()
        }
        (None, Some(Item::AllPlaylists)) => tr("Browse enabled playlists together").to_owned(),
        _ => tr("M3U link or file, Xtream Codes login, Stalker portal").to_owned(),
    };
    let mark = move || mark(ui.chrome, focused_index.get() == Some(index));
    list_rows::row(size, ROW_THREE, mark)
        .test_id(if is_all {
            "playlists-all"
        } else {
            "playlist-row"
        })
        .flex_row()
        .items_center()
        .justify_start()
        .gap(move || size.px(14.0))
        .on_press(move || {
            ui.touched();
            if let Some(tapped) = item() {
                update(ui, |s| s.tap(tapped));
            }
        })
        .child(
            view()
                .w(move || size.px(8.0))
                .h(move || size.px(40.0))
                .rounded(px(4.0))
                .bg(move || {
                    if shown() {
                        rgb(ui.app.theme.with(|t| {
                            if mark() == Mark::Focused {
                                t.accent.main
                            } else {
                                t.accent.light
                            }
                        }))
                    } else {
                        tint(WHITE, 0)
                    }
                }),
        )
        .child(
            view()
                .flex_1()
                .child(ui_kit::label(size, title).text_color(move || mark().text()))
                .child(ui_kit::detail(size, summary).text_color(move || mark().secondary()))
                .child(
                    ui_kit::caption(size, move || status().0)
                        .number_of_lines(1)
                        .text_color(move || match (status().1, mark()) {
                            (true, Mark::Focused) => rgb(ui_kit::ERROR_FOCUSED),
                            (true, _) => rgb(ERROR),
                            (false, m) => m.secondary(),
                        }),
                ),
        )
        .child(
            ui_kit::caption(size, move || if shown() { tr("On screen") } else { "" }).text_color(
                move || match mark() {
                    Mark::Focused => Mark::Focused.text(),
                    _ => rgb(ui.app.theme.with(|t| t.accent.light)),
                },
            ),
        )
}

/// The open sheet (actions or delete confirmation) over the list; rebuilt
/// only when another sheet opens.
fn sheet(ui: Ui, size: Scale) -> impl IntoElement {
    // Which sheet, not where its focus is.
    let open = computed(move || match state(ui.screens, Screen::layer) {
        Some(Layer::Actions { id, .. }) => Some(Sheet::Actions(id)),
        Some(Layer::Confirm { id, .. }) => Some(Sheet::Confirm(id)),
        Some(Layer::Details { id }) => Some(Sheet::Details(id)),
        Some(Layer::List) | None => None,
    });
    dynamic(move || {
        let shown = open.get();
        untracked(move || match shown {
            Some(Sheet::Actions(id))
                if matches!(
                    state(ui.screens, Screen::opened_for).flatten(),
                    Some(Only::Actions { .. } | Only::GuideActions | Only::Management)
                ) =>
            {
                backdrop(ui, actions_card(ui, size, id).into_element()).items_end()
            }
            Some(Sheet::Actions(id)) => backdrop(ui, actions_card(ui, size, id).into_element()),
            Some(Sheet::Confirm(id)) => backdrop(ui, confirm_card(ui, size, id).into_element()),
            Some(Sheet::Details(id)) => {
                backdrop(ui, details_card::card(ui, size, id).into_element())
            }
            None => view(),
        })
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sheet {
    Actions(SourceId),
    Confirm(SourceId),
    Details(SourceId),
}

fn backdrop(ui: Ui, card: AnyElement) -> View {
    view()
        .accessibility_elements_hidden(move || {
            state(ui.screens, |screen| screen.editing().is_some()).unwrap_or(false)
        })
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(tint(0x000000, 0xA0))
                .on_press(move || {
                    ui.touched();
                    update(ui, Screen::dismiss);
                }),
        )
        .child(card)
}

fn card(ui: Ui, size: Scale) -> View {
    ui_kit::card(ui, size, CARD_W)
        .gap(move || size.px(6.0))
        // Taps on the card stay on it.
        .on_press(|| {})
}

fn actions_card(ui: Ui, size: Scale, id: SourceId) -> impl IntoElement {
    let playlist = move || state(ui.screens, |s| s.row(id).cloned()).flatten();
    let running = move || {
        ui.app.library.activity.with(|a| {
            matches!(
                a.get(&id),
                Some(Activity::Running(_) | Activity::Checking(_))
            )
        })
    };
    let task = state(ui.screens, Screen::opened_for).flatten();
    let guide = task == Some(Only::GuideActions);
    let contextual = guide || matches!(task, Some(Only::Actions { .. }));
    let side_panel = contextual || task == Some(Only::Management);
    let acts = computed(move || state(ui.screens, |s| s.acts(id)).unwrap_or_default());
    let notice = move || state(ui.screens, |s| s.notice()).flatten();
    let act_row = move |act: Act| {
        let focused = move || {
            state(ui.screens, |s| {
                s.layer() == Layer::Actions { id, focus: act }
            })
            .unwrap_or(false)
        };
        let label = move || {
            let shown = ui.app.library.shown.get() == Some(id);
            let kind = playlist().map(|p| p.kind);
            match (act, playlist()) {
                (Act::Rename, _) if guide => tr("Rename playlist").to_owned(),
                (Act::Delete, _) if guide => tr("Delete playlist").to_owned(),
                (Act::Hide, _) if guide => tr("Hide playlist").to_owned(),
                (Act::Refresh, _) if guide && !running() => tr("Refresh Now").to_owned(),
                (Act::RefreshInterval, Some(p)) => {
                    let every = match p.policy.refresh_seconds {
                        0 => tr("Off").to_owned(),
                        n => trf(
                            "{count, plural, one {every hour} other {every # hours}}",
                            &[("count", (n / 3600).into())],
                        ),
                    };
                    trf(
                        "Automatic refresh: {interval}",
                        &[("interval", (&every).into())],
                    )
                }
                (Act::PlaylistOrder, _) => {
                    ui.app.settings.get();
                    let order = crate::wiring::settings::choice(iptv_settings::keys::PLAYLIST_SORT);
                    let method = crate::screens::settings_view::choice_label("sort", order);
                    trf("Playlist order: {method}", &[("method", (&method).into())])
                }
                (Act::GroupOrder, _) => {
                    ui.guide_data();
                    crate::chrome::menu_view::group_order_label(
                        &iptv_presentation::lists::ListId::Group(format!("playlist:{}", id.0)),
                    )
                }
                (Act::RefreshOnStart, Some(p)) => trf(
                    "Refresh on startup: {answer}",
                    &[("answer", yes_no(p.policy.refresh_on_start).into())],
                ),
                (Act::OnDemand, Some(_)) => {
                    ui.app.settings.get();
                    trf(
                        "Movies and series: {answer}",
                        &[("answer", yes_no(on_demand(id)).into())],
                    )
                }
                (Act::Streams, Some(_)) => {
                    ui.app.settings.get();
                    ui.app.library.version.get();
                    words::streams(
                        own_streams(id),
                        crate::wiring::runtime_owner::runtime().reported_streams(id),
                    )
                }
                _ => words::act(act, kind, shown, running()).to_owned(),
            }
        };
        let color = if act == Act::Delete { ERROR } else { WHITE };
        ui_kit::action(ui, size, label, focused, color, move || {
            update(ui, |s| s.act(id, act));
        })
    };
    let panel = if side_panel {
        view()
            .test_id(if contextual {
                "playlist-menu-panel"
            } else {
                "playlist-management-panel"
            })
            .h(percentage(1.0))
            .w(move || size.px(400.0))
            .max_w(percentage(1.0))
            .pt_safe_area()
            .px(move || size.px(12.0))
            .py(move || size.px(16.0))
            .gap(move || size.px(6.0))
            .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
            .on_press(|| {})
    } else {
        card(ui, size)
    };
    panel
        .child(
            ui_kit::title(size, move || playlist().map(|p| p.name).unwrap_or_default())
                .role(AccessibilityRole::Header),
        )
        .child(ui_kit::body(size, move || {
            playlist().map(|p| words::summary(&p)).unwrap_or_default()
        }))
        .child(
            ui_kit::body(size, move || {
                playlist()
                    .map(|p| words::guide_line(&p))
                    .unwrap_or_default()
            })
            .number_of_lines(1),
        )
        .child(
            ui_kit::body(size, move || notice().map_or("", words::notice))
                .text_color(rgb(ERROR))
                .h(move || size.px(if notice().is_some() { 22.0 } else { 0.0 })),
        )
        .child(view().h(move || size.px(6.0)))
        .child({
            let rows = element_ref();

            effect(move || {
                if let Some(Layer::Actions { focus, .. }) = state(ui.screens, Screen::layer)
                    && let Some(at) = acts.with(|items| items.iter().position(|a| *a == focus))
                {
                    rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
                }
            });
            let list = virtual_list(
                move || acts.with(Vec::len),
                move |at| {
                    dynamic(move || {
                        let act = acts.with(|items| items.get(at.index()).copied());
                        untracked(move || act.map_or_else(view, act_row))
                    })
                },
            )
            .test_id("playlist-actions-list")
            .estimated_item_size(list_rows::pitch(size, list_rows::ROW))
            .element_ref(&rows);
            if side_panel {
                list.flex_1()
            } else {
                list.h(move || size.px(360.0))
            }
        })
}

fn confirm_card(ui: Ui, size: Scale, id: SourceId) -> impl IntoElement {
    let copy = computed(move || {
        state(ui.screens, |s| {
            let Some(p) = s.row(id) else {
                return (String::new(), String::new());
            };
            let shown = !ui.app.library.all.get() && ui.app.library.shown.get() == Some(id);
            let next = shown
                .then(|| crate::runtime::data::next_playlist_after_deletion(&crate::runtime(), id))
                .flatten();
            (
                trf("Delete “{name}”?", &[("name", (&p.name).into())]),
                words::delete_consequences(
                    p,
                    next.as_deref(),
                    s.rows().iter().any(|row| row.id != id),
                ),
            )
        })
        .unwrap_or_default()
    });
    let focused = move |delete: bool| {
        move || state(ui.screens, |s| s.layer() == Layer::Confirm { id, delete }).unwrap_or(false)
    };
    card(ui, size)
        .test_id("playlist-delete-confirmation")
        .child(ui_kit::title(size, move || {
            copy.with(|copy| copy.0.clone())
        }))
        .child(ui_kit::body(size, move || copy.with(|copy| copy.1.clone())))
        .child(view().h(move || size.px(8.0)))
        .child(ui_kit::action(
            ui,
            size,
            tr("Keep it"),
            focused(false),
            WHITE,
            move || {
                update(ui, |s| s.confirm(id, false));
            },
        ))
        .child(ui_kit::action(
            ui,
            size,
            tr("Delete"),
            focused(true),
            ERROR,
            move || {
                update(ui, |s| s.confirm(id, true));
            },
        ))
}

fn edit_text(screens: crate::ScreensUi) -> String {
    state(screens, |s| s.editing().map(|e| e.text.clone()))
        .flatten()
        .unwrap_or_default()
}

fn set_edit_text(screens: crate::ScreensUi, text: &str) {
    screens.playlists.update(|s| {
        if let Some(s) = s {
            s.set_text(text);
        }
    });
}

/// The text sheet for a rename, a User-Agent or a provider detail; made
/// when entry starts.
fn edit_sheet(ui: Ui, kind: EditKind, prefilled: bool) -> View {
    let mut masked = false;
    let (label, placeholder, url) = match kind {
        EditKind::Rename => (tr("Playlist name"), tr("How the playlist is listed"), false),
        EditKind::UserAgent => (
            tr("User-Agent"),
            tr("Empty: the app's own; some providers ask for a particular one"),
            false,
        ),
        EditKind::Param(p) => {
            masked = p == details::Param::Password;
            (
                words::param(p),
                words::param_placeholder(p, prefilled),
                p.is_link(),
            )
        }
    };
    entry::text_sheet(
        ui,
        TextSheet {
            label,
            placeholder,
            url,
            masked,
            text: |ui| edit_text(ui.screens),
            set_text: |ui, text| set_edit_text(ui.screens, text),
            commit,
            cancel,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_never_names_playlists_or_links() {
        let text = effect_name(&Effect::Guides(SourceId(3)));
        assert_eq!(text, "guide sources of 3");
        let text = effect_name(&Effect::Rename(SourceId(3), "Private name".into()));
        assert!(!text.contains("Private"));
        let agent = Effect::SetUserAgent(SourceId(3), Some("Private/1".into()));
        assert_eq!(effect_name(&agent), "User-Agent of 3: set true");
        let source = iptv_providers::model::Source {
            name: "Private name".into(),
            config: iptv_providers::model::SourceConfig::M3u(
                iptv_providers::model::M3uSourceConfig {
                    location: iptv_providers::SecretUrl::new(
                        "http://private.example.test/list.m3u?token=secret",
                    ),
                    user_agent: None,
                    referrer: None,
                },
            ),
        };
        let all = [
            (Effect::Add, "add"),
            (Effect::Show(SourceId(1)), "show 1"),
            (Effect::Refresh(SourceId(1)), "refresh 1"),
            (Effect::Delete(SourceId(1)), "delete 1"),
            (Effect::RefreshAll, "refresh all"),
            (Effect::OpenDetails(SourceId(1)), "details of 1"),
            (
                Effect::TryDetails(SourceId(1), Box::new(source)),
                "try new details of 1",
            ),
            (Effect::Close, "close"),
        ];
        for (effect, text) in all {
            assert_eq!(effect_name(&effect), text);
        }
    }
}
