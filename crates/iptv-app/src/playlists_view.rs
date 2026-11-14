//! The playlists screen: the stored playlists (name, type, channel count,
//! last update or last failure, the one on screen marked), "Refresh all"
//! (with two or more) and "Add playlist". OK (or Menu, or a tap) on a
//! playlist opens its actions: show it, refresh it now, rename it, change
//! its provider details (link, server and login, portal and MAC address:
//! checked by importing them, kept only if that works), its User-Agent,
//! its guide sources (`epg_sources_view`), delete it after a confirmation
//! that states what goes. `state.rs` and `details.rs` own
//! focus and flow; `data::library` does the work.
//!
//! Remote: Up/Down move, OK acts, Menu opens a playlist's actions, Back
//! closes the open sheet, then the screen. Touch and mouse: every row and
//! action is tappable; a tap beside a sheet closes it. Text entry uses the
//! wizard's sheet (`wizard_view::entry`). Wording is iptv-rs's own.

use crate::i18n::tr;

use iptv_library::details;
mod details_card;
use iptv_library::playlists as state;
mod words;

use crate::runtime::model::SourceId;
use iptv_nav::Action;
use nsh_core::prelude::*;

pub use state::Screen as Open;
use state::{Act, EditKind, Effect, Item, Key, Layer, Screen};
pub(crate) use words::masked_link;

use crate::data::{self, Activity, Playlist};
use crate::list_rows::{self, Mark};
use crate::scale::Scale;
use crate::ui_kit::{self, ERROR, ROW_THREE};
use crate::wizard_view::entry::{self, TextSheet};
use crate::{Ui, ViewMode, tint};

const WHITE: u32 = 0xFFFFFF;
const CARD_W: f32 = 640.0;

// --- Actions -------------------------------------------------------------------

/// Opens the screen (the navigation rail's Playlists).
pub fn open(ui: Ui) {
    if is_open(ui) || crate::wizard_view::is_open(ui) {
        return;
    }
    let mut screen = Screen::new(load(), ui.library.shown.get_untracked());
    if ui.library.all.get_untracked() {
        screen.focus_all();
    }
    ui.playlists.set(Some(screen));
    if ui.mode.get_untracked() == ViewMode::Fullscreen {
        ui.mode.set(ViewMode::Guide);
    }
}

pub fn close(ui: Ui) {
    if ui.playlists.with_untracked(Option::is_some) {
        ui.playlists.set(None);
        dismiss_keyboard();
    }
}

pub fn is_open(ui: Ui) -> bool {
    ui.playlists.with(Option::is_some)
}

fn load() -> Vec<Playlist> {
    data::playlists().unwrap_or_else(|e| {
        log::warn!("iptv playlists: the store is unavailable: {e}");
        Vec::new()
    })
}

fn state<R>(ui: Ui, f: impl FnOnce(&Screen) -> R) -> Option<R> {
    ui.playlists.with(|s| s.as_ref().map(f))
}

/// Changes the state and carries out what it answers.
fn update(ui: Ui, f: impl FnOnce(&mut Screen) -> Option<Effect>) {
    let mut effect = None;
    ui.playlists.update(|s| {
        if let Some(s) = s {
            effect = f(s);
        }
    });
    if let Some(effect) = effect {
        perform(ui, effect);
    }
}

fn editing(ui: Ui) -> bool {
    state(ui, |s| s.editing().is_some()).unwrap_or(false)
}

/// A remote action while the screen is open.
pub fn key(ui: Ui, action: Action) {
    let key = match action {
        Action::MoveUp => Key::Up,
        Action::MoveDown => Key::Down,
        Action::Select => Key::Ok,
        Action::Back => Key::Back,
        Action::ShowMenu => Key::Menu,
        _ => return,
    };
    if editing(ui) {
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
    data::spawn_in_app(async move {
        log::info!("iptv playlists: {}", effect_name(&effect));
        let result = match effect {
            Effect::AllPlaylists => {
                close(ui);
                data::switch_all();
                Ok(())
            }
            Effect::Add => {
                crate::wizard_view::open_add(ui);
                Ok(())
            }
            Effect::Show(id) => {
                close(ui);
                data::switch_to(id);
                Ok(())
            }
            Effect::Refresh(id) => {
                data::refresh(id);
                Ok(())
            }
            Effect::Policy(id, policy) => data::set_policy(id, policy).await,
            Effect::CustomGroups(id) => {
                crate::custom_groups::open(ui, id, false);
                Ok(())
            }
            Effect::Organize(id, groups) => {
                crate::organization_view::open(
                    ui,
                    id,
                    if groups {
                        crate::organization_view::Kind::Groups
                    } else {
                        crate::organization_view::Kind::Channels
                    },
                    None,
                );
                Ok(())
            }
            Effect::Rename(id, name) => data::rename(id, &name).await,
            Effect::Guides(id) => {
                crate::epg_sources_view::open(ui, id);
                Ok(())
            }
            Effect::Delete(id) => data::delete(id).await,
            Effect::RefreshAll => {
                data::refresh_all();
                Ok(())
            }
            Effect::OpenDetails(id) => {
                if let Some(source) = data::source_details(id) {
                    update(ui, |s| {
                        s.open_details(source);
                        None
                    });
                }
                Ok(())
            }
            Effect::TryDetails(id, source) => {
                data::try_details(id, *source);
                Ok(())
            }
            Effect::SetUserAgent(id, agent) => {
                data::set_user_agent(id, agent);
                Ok(())
            }
            Effect::Close => {
                close(ui);
                Ok(())
            }
        };
        if let Err(e) = result {
            log::warn!("iptv playlists: the store refused it: {e}");
        }
    });
}

/// For the log: the effect without names or links.
fn effect_name(effect: &Effect) -> String {
    match effect {
        Effect::Add => "add".to_owned(),
        Effect::AllPlaylists => "show all playlists".to_owned(),
        Effect::Show(id) => format!("show {}", id.0),
        Effect::Refresh(id) => format!("refresh {}", id.0),
        Effect::Policy(id, _) => format!("playlist policy {}", id.0),
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

// --- View ----------------------------------------------------------------------

/// A row's or action's mark: the pill while focused with the keys shown.
fn mark(ui: Ui, focused: bool) -> Mark {
    Mark::of(focused && ui.keys_shown(), false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    // What is stored changed (refreshed, renamed, deleted, added): reload.
    effect(move || {
        ui.library.version.get();
        ui.settings.get();
        if ui.playlists.with_untracked(Option::is_some) {
            let rows = load();
            ui.playlists.update(|s| {
                if let Some(s) = s {
                    s.set_rows(rows);
                }
            });
        }
    });
    // New details checked: saved closes the form, refused keeps it.
    effect(move || {
        let ended = ui.library.activity.with(|a| {
            a.iter()
                .filter_map(|(id, act)| match act {
                    Activity::Done(Ok(_)) => Some((*id, true)),
                    Activity::Done(Err(_)) | Activity::Rejected(_) => Some((*id, false)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        untracked(|| {
            ui.playlists.update(|s| {
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
    let entry =
        computed(move || state(ui, |s| s.editing().map(|e| (e.kind, e.prefilled))).flatten());
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            ui_kit::screen(ui, size)
                .child(ui_kit::header(ui, size, move || tr("Playlists"), close))
                .child(ui_kit::caption(size, move || hint(ui)))
                .child(list(ui, size))
                .child(sheet(ui, size))
                .child(dynamic(move || {
                    let shown = entry.get();
                    untracked(move || match shown {
                        Some((kind, prefilled)) => edit_sheet(ui, kind, prefilled),
                        None => view(),
                    })
                }))
        },
    )
}

fn hint(ui: Ui) -> &'static str {
    let layer = state(ui, Screen::layer).unwrap_or(Layer::List);
    match (ui.keys_shown(), layer) {
        (true, Layer::List) => "▲▼ choose   OK actions   Back close",
        (true, Layer::Actions { .. }) => "▲▼ choose   OK do it   Back close the sheet",
        (true, Layer::Confirm { .. }) => "▲▼ choose   OK confirm   Back keep it",
        (true, Layer::Details { .. }) => "▲▼ choose   OK change or check   Back leave unchanged",
        (false, _) => "Tap a playlist for what you can do with it",
    }
}

fn list(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    let count = move || state(ui, |s| s.items().len()).unwrap_or(0);
    let focused_index =
        computed(move || state(ui, |s| s.items().iter().position(|i| *i == s.focus())).flatten());
    effect(move || {
        if let Some(i) = focused_index.get()
            && ui.keys_shown()
        {
            rows.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(count, move |at| row(ui, size, at.index(), focused_index))
        .estimated_item_size(list_rows::pitch(size, ROW_THREE))
        .element_ref(&rows)
        .flex_1()
}

fn row(ui: Ui, size: Scale, index: usize, focused_index: Memo<Option<usize>>) -> impl IntoElement {
    let playlist = move || state(ui, |s| s.rows().get(index).cloned()).flatten();
    let item = move || state(ui, |s| s.items().get(index).copied()).flatten();
    let refreshing = move || {
        ui.library.activity.with(|a| {
            a.values()
                .filter(|act| matches!(act, Activity::Running(_) | Activity::Checking(_)))
                .count()
        })
    };
    let shown = move || {
        playlist().is_some_and(|p| !ui.library.all.get() && ui.library.shown.get() == Some(p.id))
            || (item() == Some(Item::AllPlaylists) && ui.library.all.get())
    };
    let activity = move |id: SourceId| ui.library.activity.with(|a| a.get(&id).cloned());
    let status = move || match (playlist(), item()) {
        (Some(p), _) => words::status(&p, activity(p.id).as_ref(), crate::time::wall_unix()),
        (None, Some(Item::RefreshAll)) => (words::refreshing(refreshing()), false),
        _ => (String::new(), false),
    };
    let title = move || match (playlist(), item()) {
        (Some(p), _) => p.name,
        (None, Some(Item::RefreshAll)) => "Refresh all playlists".to_owned(),
        (None, Some(Item::AllPlaylists)) => "All playlists".to_owned(),
        _ => "+ Add playlist".to_owned(),
    };
    let summary = move || match (playlist(), item()) {
        (Some(p), _) => words::summary(&p),
        (None, Some(Item::RefreshAll)) => {
            "Each one downloads again; the one on screen updates in place".to_owned()
        }
        (None, Some(Item::AllPlaylists)) => "Browse enabled playlists together".to_owned(),
        _ => "M3U link or file, Xtream Codes login, Stalker portal".to_owned(),
    };
    let mark = move || mark(ui, focused_index.get() == Some(index));
    list_rows::row(size, ROW_THREE, mark)
        .test_id(
            if state(ui, |s| s.items().get(index) == Some(&Item::AllPlaylists)).unwrap_or(false) {
                "playlists-all"
            } else {
                "playlist-row"
            },
        )
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
                        rgb(ui.theme.with(|t| {
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
            ui_kit::caption(size, move || if shown() { "On screen" } else { "" }).text_color(
                move || match mark() {
                    Mark::Focused => Mark::Focused.text(),
                    _ => rgb(ui.theme.with(|t| t.accent.light)),
                },
            ),
        )
}

/// The open sheet (actions or delete confirmation) over the list; rebuilt
/// only when another sheet opens.
fn sheet(ui: Ui, size: Scale) -> impl IntoElement {
    // Which sheet, not where its focus is.
    let open = computed(move || match state(ui, Screen::layer) {
        Some(Layer::Actions { id, .. }) => Some((Sheet::Actions, id)),
        Some(Layer::Confirm { id, .. }) => Some((Sheet::Confirm, id)),
        Some(Layer::Details { id }) => Some((Sheet::Details, id)),
        Some(Layer::List) | None => None,
    });
    dynamic(move || {
        let shown = open.get();
        untracked(move || match shown {
            Some((Sheet::Actions, id)) => backdrop(ui, actions_card(ui, size, id).into_element()),
            Some((Sheet::Confirm, id)) => backdrop(ui, confirm_card(ui, size, id).into_element()),
            Some((Sheet::Details, id)) => {
                backdrop(ui, details_card::card(ui, size, id).into_element())
            }
            None => view(),
        })
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sheet {
    Actions,
    Confirm,
    Details,
}

fn backdrop(ui: Ui, card: AnyElement) -> View {
    view()
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
    let playlist = move || state(ui, |s| s.row(id).cloned()).flatten();
    let running = move || {
        ui.library.activity.with(|a| {
            matches!(
                a.get(&id),
                Some(Activity::Running(_) | Activity::Checking(_))
            )
        })
    };
    let acts = state(ui, |s| s.acts(id)).unwrap_or_default();
    let notice = move || state(ui, |s| s.notice()).flatten();
    let act_row = move |act: Act| {
        let focused =
            move || state(ui, |s| s.layer() == Layer::Actions { id, focus: act }).unwrap_or(false);
        let label = move || {
            let shown = ui.library.shown.get() == Some(id);
            let kind = playlist().map(|p| p.kind);
            match (act, playlist()) {
                (Act::Enabled, Some(p)) => format!(
                    "Playlist enabled: {}",
                    if p.policy.enabled { "Yes" } else { "No" }
                ),
                (Act::RefreshInterval, Some(p)) => format!(
                    "Automatic refresh: {}",
                    match p.policy.refresh_seconds {
                        0 => tr("Off").to_owned(),
                        n => format!("every {} hours", n / 3600),
                    }
                ),
                (Act::RefreshOnStart, Some(p)) => format!(
                    "Refresh on startup: {}",
                    if p.policy.refresh_on_start {
                        "Yes"
                    } else {
                        "No"
                    }
                ),
                _ => words::act(act, kind, shown, running()).to_owned(),
            }
        };
        let color = if act == Act::Delete { ERROR } else { WHITE };
        ui_kit::action(ui, size, label, focused, color, move || {
            update(ui, |s| s.act(id, act));
        })
    };
    card(ui, size)
        .child(ui_kit::title(size, move || {
            playlist().map(|p| p.name).unwrap_or_default()
        }))
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
            let count = acts.len();
            let acts = std::rc::Rc::new(acts);
            let rows = element_ref();
            let focused_acts = acts.clone();
            effect(move || {
                if let Some(Layer::Actions { focus, .. }) = state(ui, Screen::layer)
                    && let Some(at) = focused_acts.iter().position(|a| *a == focus)
                {
                    rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
                }
            });
            virtual_list(move || count, move |at| act_row(acts[at.index()]))
                .estimated_item_size(list_rows::pitch(size, list_rows::ROW))
                .element_ref(&rows)
                .h(move || size.px(360.0))
        })
}

fn confirm_card(ui: Ui, size: Scale, id: SourceId) -> impl IntoElement {
    let (title, body) = state(ui, |s| {
        let Some(p) = s.row(id) else {
            return (String::new(), String::new());
        };
        let next = s
            .rows()
            .iter()
            .rev()
            .find(|r| r.id != id)
            .map(|r| r.name.as_str());
        let shown = ui.library.shown.get_untracked() == Some(id);
        (
            format!("Delete “{}”?", p.name),
            words::delete_consequences(p, next, shown),
        )
    })
    .unwrap_or_default();
    let focused = move |delete: bool| {
        move || state(ui, |s| s.layer() == Layer::Confirm { id, delete }).unwrap_or(false)
    };
    card(ui, size)
        .child(ui_kit::title(size, title))
        .child(ui_kit::body(size, body))
        .child(view().h(move || size.px(8.0)))
        .child(ui_kit::action(
            ui,
            size,
            "Keep it",
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

fn edit_text(ui: Ui) -> String {
    state(ui, |s| s.editing().map(|e| e.text.clone()))
        .flatten()
        .unwrap_or_default()
}

fn set_edit_text(ui: Ui, text: &str) {
    ui.playlists.update(|s| {
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
        EditKind::Rename => ("Playlist name", "How the playlist is listed", false),
        EditKind::UserAgent => (
            "User-Agent",
            "Empty: the app's own; some providers ask for a particular one",
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
            text: edit_text,
            set_text: set_edit_text,
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
