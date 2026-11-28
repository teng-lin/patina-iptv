//! One playlist’s Guide data screen (Playlists → playlist → Guide data,
//! or Settings → TV guide data → Guide sources for the playlist on screen).
//! Sources appear in priority order with their last refresh result and
//! actions to change or retry them. Update and Clear affect this playlist
//! only; Clear confirms its name and the programs that will be removed.
//!
//! OK (or a tap) on a source opens its actions: switch it on or off, move
//! it up or down, shift its times (Left/Right, or − and +), name it, and
//! for a source the user added change its link or remove it (after a
//! confirmation saying its programs go at the next update; the
//! provider's own guides are switched off instead). Every change is saved
//! at once (`data::edit_guides`); the guide on screen is checked again a
//! moment later and downloads what changed.
//!
//! Remote: Up/Down move, OK acts, Left/Right shift, Back closes the open
//! sheet, then the screen. Touch and mouse: every row and action is
//! tappable; a tap beside a sheet closes it. Links are shown with their
//! accounts hidden. State: `state.rs`; wording: `words.rs`.

use crate::chrome::i18n::{tr, trf};
use crate::chrome::input::dismiss_keyboard;

use iptv_presentation::library::epg_sources as state;
mod words;

use crate::runtime::model::SourceId;
use iptv_nav::Action;
use nsh_core::Checked;
use nsh_core::prelude::*;

pub use state::Screen as Open;
use state::{Act, Effect, Item, Key, Layer, Screen};

use crate::chrome::scale::Scale;
use crate::chrome::ui_kit::entry;
use crate::chrome::ui_kit::{self, ERROR, ROW_THREE, TOUCH};
use crate::guide::list_rows::{self, Mark};
use crate::theme::{LIST_TEXT, RADIUS, RESTING_ALPHA};
use crate::wiring::data::stored::{self, GuideStatus};
use crate::{Ui, tint};

const WHITE: u32 = 0xFFFFFF;
const CARD_W: f32 = 660.0;

// --- Actions -------------------------------------------------------------------

/// Opens the screen for `playlist`.
pub fn open(screens: crate::ScreensUi, playlist: SourceId) {
    screens.guide_sources.set(Some(screen(playlist)));
    // Timer failures can occur without publishing a new visible guide.
    // Opening the controls always reads the latest persisted refresh result.
    crate::runtime::data::refresh_guide_status(&crate::wiring::runtime_owner::runtime());
}

/// The screen for `playlist`; a host that cannot import offers no guide
/// to add or download.
fn screen(playlist: SourceId) -> Screen {
    let screen = Screen::new(
        playlist,
        crate::wiring::runtime_owner::runtime().source_lines(playlist),
    );
    if crate::host::IMPORTS {
        screen
    } else {
        screen.without_downloads()
    }
}

pub fn close(ui: crate::ScreensUi) {
    if ui.guide_sources.with_untracked(Option::is_some) {
        ui.guide_sources.set(None);
        if !crate::screens::settings_view::is_open(ui) {
            crate::wiring::runtime_owner::runtime().parental_content_closed();
        }
        dismiss_keyboard();
    }
}

pub fn is_open(screens: crate::ScreensUi) -> bool {
    screens.guide_sources.with(Option::is_some)
}

fn state<R>(screens: crate::ScreensUi, f: impl FnOnce(&Screen) -> R) -> Option<R> {
    screens.guide_sources.with(|s| s.as_ref().map(f))
}

fn update(ui: Ui, f: impl FnOnce(&mut Screen) -> Option<Effect>) {
    let mut effect = None;
    ui.screens.guide_sources.update(|s| {
        if let Some(s) = s {
            effect = f(s).map(|e| (s.playlist(), e));
        }
    });
    if let Some((playlist, effect)) = effect {
        perform(ui, playlist, effect);
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
    let typing = editing(ui.screens);
    update(ui, |s| s.key(key));
    if typing && !editing(ui.screens) {
        dismiss_keyboard();
    }
}

/// The platform's Back: ends text entry, closes a sheet, then the screen.
pub fn back(ui: Ui) -> bool {
    key(ui, Action::Back);
    true
}

fn commit(ui: Ui) {
    update(ui, |s| s.key(Key::Ok));
    if !editing(ui.screens) {
        dismiss_keyboard();
    }
}

fn cancel(ui: Ui) {
    dismiss_keyboard();
    update(ui, |s| {
        s.cancel_edit();
        None
    });
}

/// Carries out `effect` of the screen of `playlist`.
fn perform(ui: Ui, playlist: SourceId, effect: Effect) {
    if matches!(&effect, Effect::Clear | Effect::Edit(_) | Effect::Update)
        && crate::wiring::settings::choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR)
            == "blocked_content_and_settings"
        && !crate::wiring::runtime_owner::runtime().parental_settings_allowed()
    {
        crate::screens::parental_view::protected_settings(ui.screens, move || {
            perform(ui, playlist, effect)
        });
        return;
    }
    match effect {
        Effect::Inspect => crate::library_ui::guide_inspect_view::open(ui, playlist),
        Effect::Edit(edit) => {
            log::info!("iptv guide sources of {}: {}", playlist.0, edit_name(&edit));
            crate::wiring::tasks::spawn_in_app(async move {
                if let Err(e) = crate::wiring::data::edit_guides(
                    &crate::wiring::runtime_owner::runtime(),
                    playlist,
                    &edit,
                )
                .await
                {
                    log::warn!("iptv guide sources: the store refused it: {e}");
                }
            });
        }
        Effect::Update => {
            let started = stored::update_source(
                &crate::wiring::runtime_owner::runtime(),
                playlist,
                move |completed| {
                    if completed {
                        crate::chrome::toast::notify(tr("Guide updated"));
                    }
                },
            );
            if started {
                // Shows tr("Updating…").
                ui.app.library.version.update(|v| *v += 1);
            } else {
                crate::chrome::toast::notify(tr("No guide source is switched on"));
            }
        }
        Effect::Clear => {
            stored::clear_source(
                &crate::wiring::runtime_owner::runtime(),
                playlist,
                move |deleted| {
                    crate::chrome::toast::notify(if deleted {
                        tr("Guide data deleted")
                    } else {
                        tr("The guide data could not be deleted")
                    });
                    ui.app.library.version.update(|v| *v += 1);
                },
            );
        }
        Effect::Close => close(ui.screens),
    }
}

/// For the log: the edit without names or links.
fn edit_name(edit: &crate::wiring::data::guides::Edit) -> String {
    use crate::wiring::data::guides::Edit;
    match edit {
        Edit::Add { name, .. } => format!("add (named {})", name.is_some()),
        Edit::Remove(f) => format!("remove {}", f.0),
        Edit::SetEnabled(f, on) => format!("switch {} {}", f.0, if *on { "on" } else { "off" }),
        Edit::Move(f, step) => format!("move {} by {step}", f.0),
        Edit::Shift(f, m) => format!("shift {} to {m} min", f.0),
        Edit::Rename(f, _) => format!("rename {}", f.0),
        Edit::Relink(f, _) => format!("new link for {}", f.0),
    }
}

/// A guide source's title (its name, else where it comes from).
pub fn source_title(s: &crate::runtime::model::GuideSource) -> String {
    words::title(s)
}

// --- View ----------------------------------------------------------------------

/// A row's or action's mark: the pill while focused with the keys shown.
fn mark(chrome: crate::ChromeUi, focused: bool) -> Mark {
    Mark::of(focused && chrome.keys_shown(), false)
}

fn time_label(at: i64) -> String {
    crate::guide::clock::label(at, crate::guide::clock::wall_unix())
}

fn contents_hidden(ui: Ui) -> bool {
    editing(ui.screens)
        || state(ui.screens, |screen| screen.layer() != Layer::List).unwrap_or(false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    // What is stored changed (an edit, an update, a guide download): reload.
    effect(move || {
        ui.app.library.version.get();
        ui.app.settings.get();
        ui.app.data_version.get();
        let playlist = ui
            .screens
            .guide_sources
            .with_untracked(|s| s.as_ref().map(Screen::playlist));
        if let Some(playlist) = playlist {
            let lines = crate::wiring::runtime_owner::runtime().source_lines(playlist);
            ui.screens.guide_sources.update(|s| {
                if let Some(s) = s {
                    s.set_lines(lines);
                }
            });
        }
    });
    let editing = computed(move || editing(ui.screens));
    let size = Scale::new();
    when(
        move || is_open(ui.screens),
        move || {
            ui_kit::screen(ui, size)
                .test_id("guide-sources-screen")
                .accessibility_elements_hidden(move || {
                    crate::chrome::overlays::Overlay::GuideSources.accessibility_covered(ui)
                })
                .child(header(ui, size).accessibility_elements_hidden(move || contents_hidden(ui)))
                .child(
                    ui_kit::caption(size, move || hint(ui))
                        .accessibility_elements_hidden(move || contents_hidden(ui)),
                )
                .child(when(
                    move || state(ui.screens, |s| s.notice().is_some()).unwrap_or(false),
                    move || {
                        ui_kit::body(size, move || {
                            state(ui.screens, |s| s.notice().map(words::notice))
                                .flatten()
                                .unwrap_or_default()
                        })
                        .accessibility_elements_hidden(move || contents_hidden(ui))
                        .text_color(rgb(ERROR))
                    },
                ))
                .child(list(ui, size))
                .child(sheet(ui, size))
                .child(when(move || editing.get(), move || edit_sheet(ui, size)))
        },
    )
}

fn hint(ui: Ui) -> &'static str {
    let layer = state(ui.screens, Screen::layer).unwrap_or(Layer::List);
    match (ui.keys_shown(), layer) {
        (true, Layer::List) => {
            tr("▲▼ choose   OK actions   Back close   The first source with a channel wins")
        }
        (true, Layer::Actions { .. }) => {
            tr("▲▼ choose   OK do it   ◀▶ shift times   Back close the sheet")
        }
        (true, Layer::Confirm { .. } | Layer::Clear { .. }) => {
            tr("▲▼ choose   OK confirm   Back keep it")
        }
        (false, _) => {
            tr("Tap a source for what you can do with it. The first source with a channel wins")
        }
    }
}

fn header(ui: Ui, size: Scale) -> View {
    let name = move || {
        let playlist = state(ui.screens, Screen::playlist);
        ui.app.library.version.get();
        let name = playlist.and_then(|id| {
            crate::wiring::runtime_owner::runtime()
                .playlists()
                .ok()?
                .into_iter()
                .find(|p| p.id == id)
                .map(|p| p.name)
        });
        name.map_or_else(
            || tr("Guide data").to_owned(),
            |n| trf("Guide data · {playlist}", &[("playlist", (&n).into())]),
        )
    };
    ui_kit::header(ui, size, name, |ui| close(ui.screens))
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
    virtual_list(count, move |at| {
        dynamic(move || {
            let index = at.index();
            untracked(move || row(ui, size, index, focused_index))
        })
    })
    .test_id("guide-data-list")
    .accessibility_elements_hidden(move || contents_hidden(ui))
    .estimated_item_size(list_rows::pitch(size, ROW_THREE))
    .element_ref(&rows)
    .flex_1()
}

/// The tr("Update the guide now") row's status.
fn update_status(playlist: SourceId) -> (String, bool) {
    if crate::wiring::runtime_owner::runtime().is_updating(playlist) {
        return (tr("Updating…").to_owned(), false);
    }
    let state = crate::wiring::runtime_owner::runtime().cached_guide_state(playlist);
    match GuideStatus::from_snapshot(state.as_ref()) {
        GuideStatus::Updated { at, channels } => (
            trf(
                "{count, plural, one {Last update {when} · # channel with a guide} \
                 other {Last update {when} · # channels with a guide}}",
                &[
                    ("when", (&time_label(at)).into()),
                    ("count", channels.into()),
                ],
            ),
            false,
        ),
        GuideStatus::Failed { at, .. } => (
            trf(
                "The last update {when} did not load every source",
                &[("when", (&time_label(at)).into())],
            ),
            true,
        ),
        _ => (tr("Not downloaded yet").to_owned(), false),
    }
}

fn row(ui: Ui, size: Scale, index: usize, focused_index: Memo<Option<usize>>) -> impl IntoElement {
    let item = move || state(ui.screens, |s| s.items().get(index).copied()).flatten();
    let line = move || {
        state(ui.screens, |s| match s.items().get(index) {
            Some(Item::Source(feed)) => s.line(*feed).cloned(),
            _ => None,
        })
        .flatten()
    };
    let playlist = move || state(ui.screens, Screen::playlist);
    let title = move || match (line(), item()) {
        (Some(l), _) => format!("{}.  {}", index + 1, words::title(&l.source)),
        (None, Some(Item::Add)) => tr("+ Add a guide source").to_owned(),
        (None, Some(Item::Update)) => {
            if playlist().is_some_and(|p| update_status(p).1) {
                tr("Retry").to_owned()
            } else {
                tr("Update the guide now").to_owned()
            }
        }
        (None, Some(Item::Clear)) => tr("Delete stored guide data").to_owned(),
        (None, Some(Item::Inspect)) => tr("Inspect guide coverage").to_owned(),
        _ => String::new(),
    };
    let summary = move || match (line(), item()) {
        (Some(l), _) => words::origin(&l.source),
        (None, Some(Item::Add)) => tr("An XMLTV link (.xml or .xml.gz)").to_owned(),
        (None, Some(Item::Update)) => tr("Downloads every source switched on").to_owned(),
        (None, Some(Item::Inspect)) => tr("Compare guide channels and program samples").to_owned(),
        (None, Some(Item::Clear)) => {
            tr("Delete this playlist’s stored programs; keep its channels and guide sources")
                .to_owned()
        }
        _ => String::new(),
    };
    let status = move || match (line(), item(), playlist()) {
        (Some(l), _, _) => words::status(&l, time_label),
        (None, Some(Item::Update), Some(p)) => update_status(p),
        _ => (String::new(), false),
    };
    let switched = move || match line() {
        Some(l) if l.source.enabled => tr("On").to_owned(),
        Some(_) => tr("Off").to_owned(),
        None => String::new(),
    };
    let enabled = move || line().is_none_or(|l| l.source.enabled);
    let mark = move || mark(ui.chrome, focused_index.get() == Some(index));
    let is_source = move || line().is_some();
    let is_on = move || line().is_some_and(|l| l.source.enabled);
    list_rows::row(size, ROW_THREE, mark)
        .role(move || {
            if is_source() {
                AccessibilityRole::Switch
            } else {
                AccessibilityRole::Button
            }
        })
        .accessibility_state(move || AccessibilityState {
            checked: if is_source() {
                Some(if is_on() {
                    Checked::True
                } else {
                    Checked::False
                })
            } else {
                None
            },
            ..Default::default()
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
                .flex_1()
                .child(
                    ui_kit::label(size, title).text_color(move || match (mark(), enabled()) {
                        (Mark::Focused, true) => Mark::Focused.text(),
                        (Mark::Focused, false) => Mark::Focused.disabled(),
                        (_, true) => rgb(LIST_TEXT),
                        (_, false) => tint(WHITE, 0x80),
                    }),
                )
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
            list_rows::second(size, switched, crate::chrome::fonts::SANS).text_color(
                move || match (mark(), enabled()) {
                    (Mark::Focused, _) => Mark::Focused.text(),
                    (_, true) => rgb(ui.app.theme.with(|t| t.accent.light)),
                    (_, false) => ui_kit::secondary(),
                },
            ),
        )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sheet2 {
    Actions,
    Confirm,
    Clear,
}

/// The open sheet over the list; rebuilt only when another one opens.
fn sheet(ui: Ui, size: Scale) -> impl IntoElement {
    let open = computed(move || match state(ui.screens, Screen::layer) {
        Some(Layer::Actions { feed, .. }) => Some((Sheet2::Actions, feed)),
        Some(Layer::Confirm { feed, .. }) => Some((Sheet2::Confirm, feed)),
        Some(Layer::Clear { .. }) => Some((Sheet2::Clear, iptv_epg::FeedId(0))),
        Some(Layer::List) | None => None,
    });
    dynamic(move || {
        let shown = open.get();
        untracked(move || match shown {
            Some((Sheet2::Actions, feed)) => {
                backdrop(ui, actions_card(ui, size, feed).into_element())
            }
            Some((Sheet2::Confirm, feed)) => {
                backdrop(ui, confirm_card(ui, size, feed).into_element())
            }
            Some((Sheet2::Clear, _)) => backdrop(ui, clear_card(ui, size).into_element()),
            None => view(),
        })
    })
}

fn backdrop(ui: Ui, card: AnyElement) -> View {
    view()
        .accessibility_elements_hidden(move || editing(ui.screens))
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

/// A small − or + beside the time shift (touch and mouse).
fn step(
    ui: Ui,
    size: Scale,
    label: &'static str,
    feed: iptv_epg::FeedId,
    dir: i32,
) -> impl IntoElement {
    view()
        .w(move || size.px(TOUCH))
        .h(move || size.px(TOUCH - 4.0))
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .bg(tint(WHITE, RESTING_ALPHA * 2))
        .on_press(move || {
            ui.touched();
            update(ui, |s| s.shift(feed, dir));
        })
        .child(ui_kit::label(size, label))
}

fn actions_card(ui: Ui, size: Scale, feed: iptv_epg::FeedId) -> impl IntoElement {
    let line = move || state(ui.screens, |s| s.line(feed).cloned()).flatten();
    let acts = computed(move || state(ui.screens, |s| s.acts(feed)).unwrap_or_default());
    let scrolling = ui_kit::FocusScroll::new();
    let act_row = move |act: Act| {
        let focused = move || {
            state(ui.screens, |s| {
                s.layer() == Layer::Actions { feed, focus: act }
            })
            .unwrap_or(false)
        };
        let label = move || {
            line()
                .map(|l| words::act(act, &l.source))
                .unwrap_or_default()
        };
        let color = if act == Act::Remove { ERROR } else { WHITE };
        let row = ui_kit::action(ui, size, label, focused, color, move || {
            update(ui, |s| s.act(feed, act));
        });
        let row = if act == Act::Shift {
            row.flex_row()
                .items_center()
                .justify_start()
                .gap(move || size.px(8.0))
                .child(view().flex_1())
                .child(step(ui, size, "−", feed, -1))
                .child(step(ui, size, "+", feed, 1))
        } else {
            row
        };
        scrolling.control(row, focused)
    };
    card(ui, size)
        .test_id("guide-source-actions")
        .max_h(percentage(0.95))
        .h(move || {
            px((window().height.0 * 0.95)
                .min(size.of(600.0) * crate::wiring::settings::text_factor().max(1.0)))
        })
        .child(ui_kit::title(size, move || {
            line().map(|l| words::title(&l.source)).unwrap_or_default()
        }))
        .child(
            ui_kit::body(size, move || {
                line().map(|l| words::origin(&l.source)).unwrap_or_default()
            })
            .number_of_lines(1),
        )
        .child(
            ui_kit::body(size, move || {
                line()
                    .map(|l| words::status(&l, time_label).0)
                    .unwrap_or_default()
            })
            .number_of_lines(1),
        )
        .child(view().h(move || size.px(6.0)))
        .child(
            scrolling
                .view()
                .test_id("guide-source-actions-scroll")
                .flex_1()
                .child(dynamic(move || {
                    view().children(acts.get().into_iter().map(act_row))
                })),
        )
}

fn clear_card(ui: Ui, size: Scale) -> impl IntoElement {
    let name = state(ui.screens, Screen::playlist)
        .and_then(|id| {
            crate::wiring::runtime_owner::runtime()
                .playlists()
                .ok()?
                .into_iter()
                .find(|p| p.id == id)
                .map(|p| p.name)
        })
        .unwrap_or_default();
    let focused = move |confirm| {
        move || state(ui.screens, |s| s.layer() == Layer::Clear { confirm }).unwrap_or(false)
    };
    card(ui, size)
        .child(ui_kit::title(
            size,
            trf("Delete guide data for “{playlist}”?", &[("playlist", (&name).into())]),
        ))
        .child(ui_kit::body(
            size,
            tr("The stored programs from every guide source of this playlist are deleted. Channels, guide sources and other playlists are kept. The guide stays empty until it is downloaded again."),
        ))
        .child(ui_kit::action(
            ui, size, tr("Cancel"), focused(false), WHITE,
            move || update(ui, |s| s.confirm_clear(false)),
        ))
        .child(ui_kit::action(
            ui, size, tr("Delete"), focused(true), ERROR,
            move || update(ui, |s| s.confirm_clear(true)),
        ))
}

fn confirm_card(ui: Ui, size: Scale, feed: iptv_epg::FeedId) -> impl IntoElement {
    let (title, body) = state(ui.screens, |s| {
        s.line(feed).map(|l| {
            (
                trf(
                    "Remove “{name}”?",
                    &[("name", (&words::title(&l.source)).into())],
                ),
                words::remove_consequences(&l.source),
            )
        })
    })
    .flatten()
    .unwrap_or_default();
    let focused = move |remove: bool| {
        move || state(ui.screens, |s| s.layer() == Layer::Confirm { feed, remove }).unwrap_or(false)
    };
    card(ui, size)
        .child(ui_kit::title(size, title))
        .child(ui_kit::body(size, body))
        .child(view().h(move || size.px(8.0)))
        .child(ui_kit::action(
            ui,
            size,
            tr("Keep it"),
            focused(false),
            WHITE,
            move || {
                update(ui, |s| s.confirm(feed, false));
            },
        ))
        .child(ui_kit::action(
            ui,
            size,
            tr("Remove"),
            focused(true),
            ERROR,
            move || {
                update(ui, |s| s.confirm(feed, true));
            },
        ))
}

/// The text sheet for a new source's link, then its name, a name, or a
/// new link. One field for every step: it takes the keys again whenever
/// the step changes (Return lets them go).
fn edit_sheet(ui: Ui, size: Scale) -> impl IntoElement {
    let field = element_ref();
    let kind =
        computed(move || state(ui.screens, |s| s.editing().map(|e| e.kind.clone())).flatten());
    effect(move || {
        if kind.get().is_some() {
            field.focus();
        }
    });
    let words = move || kind.get().map(|k| words::sheet(&k)).unwrap_or(("", ""));
    let url = move || {
        !matches!(
            kind.get(),
            Some(state::EditKind::AddName { .. } | state::EditKind::Rename(_))
        )
    };
    let current = move || {
        state(ui.screens, |s| s.editing().map(|e| e.text.clone()))
            .flatten()
            .unwrap_or_default()
    };
    let (outer, card) = entry::frame(ui, size);
    outer.child(
        card.child(ui_kit::title(size, move || words().0))
            .child(
                entry::sheet_field(ui, size)
                    .element_ref(&field)
                    .value(current)
                    .on_change(move |t: &str| {
                        ui.screens.guide_sources.update(|s| {
                            if let Some(s) = s {
                                s.set_text(t);
                            }
                        });
                    })
                    .on_submit(move |_: &str| commit(ui))
                    .auto_focus(true)
                    .keyboard_type(move || {
                        if url() {
                            KeyboardType::Url
                        } else {
                            KeyboardType::Default
                        }
                    })
                    .auto_capitalize(AutoCapitalize::None)
                    .auto_correct(false)
                    .placeholder(move || words().1.to_owned()),
            )
            .child(entry::sheet_buttons(ui, size, cancel, commit)),
    )
}

#[cfg(test)]
mod tests {
    use iptv_providers::SecretUrl;

    use super::*;
    use crate::wiring::data::guides::{Edit, FIRST_USER_FEED as F};

    #[test]
    fn the_log_never_names_sources_or_links() {
        let link = || SecretUrl::new("http://private.example.test/e.xml?token=secret");
        let named = Edit::Add {
            url: link(),
            name: Some("Private".into()),
        };
        assert_eq!(edit_name(&named), "add (named true)");
        let feed = F.0;
        let all = [
            (Edit::Remove(F), format!("remove {feed}")),
            (Edit::SetEnabled(F, true), format!("switch {feed} on")),
            (Edit::SetEnabled(F, false), format!("switch {feed} off")),
            (Edit::Move(F, -1), format!("move {feed} by -1")),
            (Edit::Shift(F, 90), format!("shift {feed} to 90 min")),
            (
                Edit::Rename(F, Some("Private".into())),
                format!("rename {feed}"),
            ),
            (Edit::Relink(F, link()), format!("new link for {feed}")),
        ];
        for (edit, text) in all {
            assert_eq!(edit_name(&edit), text);
        }
    }
}
