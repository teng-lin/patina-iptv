//! Movies and series share one portable catalogue state and the persistent player.
//! The cross-source shelves (Continue watching, Recently added) use the same
//! state, details and playback; only their list is read differently.
use crate::{
    Ui,
    i18n::{tr, trf},
    scale::Scale,
    ui_kit,
};
use iptv_core::vod::{
    Control, Details, DiscoveryPage, DiscoveryQuery, Key, Kind, Operation, Progress, Retried,
    Shelf, Sort,
};
use iptv_nav::Action;
use nsh_base::input;
use nsh_core::prelude::*;
fn edit<R>(ui: Ui, f: impl FnOnce(&mut iptv_core::vod::Screen) -> R) -> R {
    let mut result = None;
    ui.vod.update(|s| result = Some(f(s)));
    result.unwrap()
}

pub fn is_open(ui: Ui) -> bool {
    ui.vod.with(|s| s.open)
}

pub fn open(ui: Ui, kind: Kind) {
    ui.vod.update(|s| {
        s.begin(kind);
        s.query.sort = match crate::settings::get().choice(
            iptv_settings::keys::EnumKey("vod.sort"),
            &iptv_settings::Target::Global,
        ) {
            "year" => Sort::Year,
            "rating" => Sort::Rating,
            "date_added" | "last_modified" => Sort::Newest,
            _ => Sort::Name,
        };
    });
    reload(ui, false);
}

/// Opens a cross-source shelf from the rail: movies and series of every
/// enabled playlist in one list. The guide stays underneath, unchanged.
pub fn open_shelf(ui: Ui, shelf: Shelf) {
    ui.vod.update(|s| {
        s.begin_shelf(shelf);
    });
    reload(ui, false);
}

pub fn close(ui: Ui) {
    if ui.player.vod.with_untracked(Option::is_none) {
        crate::runtime().parental_content_closed();
    }
    // A shelf page or focus lookup still being read is no longer wanted.
    crate::runtime().vod_discovery_cancel();
    ui.vod.update(|s| {
        s.open = false;
        s.generation = s.generation.wrapping_add(1);
    });
}

pub fn back(ui: Ui) -> bool {
    if ui.player.vod.with_untracked(Option::is_none) {
        crate::runtime().parental_content_closed();
    }
    let closed = edit(ui, |s| {
        s.back();
        s.control = Control::Catalogue;
        !s.open
    });
    if closed {
        crate::runtime().vod_discovery_cancel();
    }
    true
}

/// Loads the list: from its top (a shelf: the window around its focused
/// title), or (`append`) the page after the titles shown, which stay with
/// the focus.
fn reload(ui: Ui, append: bool) {
    let (query, shelf, generation, offset, anchor) = edit(ui, |s| {
        let offset = if append {
            s.begin_more()
        } else {
            s.reload();
            0
        };
        // A shelf that kept its focused title (reopened, retried) finds it
        // again by identity, however deep it is.
        let anchor = if append {
            None
        } else {
            s.shelf.and(s.focused.clone())
        };
        (s.query.clone(), s.shelf, s.generation, offset, anchor)
    });
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        // A queued host task may first run after Back or another shelf.
        // Do not start work (or supersede the newer read's token) then.
        if !ui
            .vod
            .with_untracked(|s| s.open && s.generation == generation)
        {
            return;
        }
        // A shelf reads one bounded page across every playlist; a
        // catalogue also refreshes its filter choices.
        let result = match (shelf, anchor) {
            (Some(shelf), Some(key)) => owner
                .vod_discovery_around(shelf, key)
                .await
                .map(|page| (page, None)),
            (Some(shelf), None) => owner
                .vod_discovery(DiscoveryQuery::after(shelf, offset as usize))
                .await
                .map(|page| (page, None)),
            (None, _) => owner.vod_catalogue(query, offset).await.map(
                |(cards, more, sources, categories)| {
                    (
                        DiscoveryPage {
                            cards,
                            more,
                            start: offset,
                        },
                        Some((sources, categories)),
                    )
                },
            ),
        };
        ui.vod.update(|s| {
            if s.generation != generation || !s.open {
                return;
            }
            match result {
                Ok((page, filters)) => {
                    if let Some((sources, categories)) = filters {
                        s.sources = sources;
                        s.categories = categories;
                    }
                    if append {
                        s.publish(generation, page.cards, page.more, true);
                    } else {
                        s.publish_window(generation, page);
                    }
                }
                // The focused title is kept: Retry looks for it again.
                Err(_) => s.fail(
                    crate::i18n::key("On-demand library could not be loaded"),
                    if append {
                        Operation::More { offset }
                    } else {
                        Operation::List
                    },
                ),
            }
        });
    });
}

/// Loads the shelf page before the titles shown (Load previous, or Up on
/// the first title with `step_up`, which then focuses the title above).
fn load_previous(ui: Ui, step_up: bool) {
    let Some((query, start, generation)) = edit(ui, |s| {
        let start = s.start;
        s.begin_previous().map(|query| (query, start, s.generation))
    }) else {
        return;
    };
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        if !ui
            .vod
            .with_untracked(|s| s.open && s.generation == generation && s.start == start)
        {
            return;
        }
        let result = owner.vod_discovery(query).await;
        ui.vod.update(|s| {
            if s.generation != generation || !s.open {
                return;
            }
            match result {
                Ok(page) => {
                    if !s.publish_previous(generation, start, page, step_up) {
                        s.busy = false;
                    }
                }
                Err(_) => s.fail(
                    crate::i18n::key("On-demand library could not be loaded"),
                    Operation::Previous { start, step_up },
                ),
            }
        });
    });
}

/// Retry: runs exactly the operation that failed again. A list page reads
/// that page; details open the same title; playback reads the title's
/// details afresh (parental lock, playlist incarnation and policy checked
/// again) and resolves a fresh provider link for the same episode, from
/// its start if that was asked.
fn retry(ui: Ui) {
    let Some(operation) = ui.vod.with_untracked(|s| s.retry_operation()) else {
        return;
    };
    match operation {
        Operation::List => reload(ui, false),
        Operation::More { .. } => reload(ui, true),
        Operation::Previous { step_up, .. } => load_previous(ui, step_up),
        Operation::Details(key) => retry_details(ui, key),
        Operation::Play {
            item,
            key,
            restart,
            incarnation,
        } => retry_play(ui, item, key, restart, incarnation),
    }
}

/// Opens the title whose details failed. The viewer has not left it, so
/// its parental check is asked again (a PIN if the lock requires one now)
/// without first closing the content as choosing another title does.
fn retry_details(ui: Ui, key: Key) {
    let generation = ui.vod.with_untracked(|s| s.generation);
    crate::parental_view::request(ui, iptv_runtime::vod::target(&key), move || {
        if ui
            .vod
            .with_untracked(|s| s.open && s.generation == generation)
        {
            select_authorized(ui, key);
        }
    });
}

/// Plays `key` of `item` again after playback failed: fresh details first,
/// keeping the chosen episode, then the ordinary authorized playback. The
/// fresh details must come from the same playlist `incarnation` the title
/// was chosen from: a playlist deleted and added again, or edited, under
/// the same id is refused, and its title must be chosen again.
fn retry_play(ui: Ui, item: Key, key: Key, restart: bool, incarnation: String) {
    let generation = ui.vod.with_untracked(|s| s.generation);
    crate::parental_view::request(ui, iptv_runtime::vod::target(&item), move || {
        if !ui
            .vod
            .with_untracked(|s| s.open && s.generation == generation)
        {
            return;
        }
        let generation = edit(ui, |s| s.begin_operation());
        let owner = crate::runtime();
        crate::data::spawn_in_app(async move {
            // The preflight gives an immediate stale-action explanation.
            // The runtime also binds the credentials snapshot to this same
            // incarnation before I/O, so an edit racing this check cannot
            // redirect a stale retry to the replacement provider.
            let result = match owner
                .vod_is_incarnation(item.source, incarnation.clone())
                .await
            {
                Ok(true) => {
                    owner
                        .vod_retry_details(item.clone(), incarnation.clone())
                        .await
                }
                Ok(false) => {
                    ui.vod.update(|s| {
                        if s.generation == generation && s.open {
                            // The stale details close and nothing plays.
                            s.playlist_changed();
                            s.error = Some(
                                crate::i18n::key(
                                    "Playback unavailable; refresh the playlist and try again",
                                )
                                .into(),
                            );
                        }
                    });
                    return;
                }
                Err(error) => Err(error),
            };
            let ready = edit(ui, |s| {
                if s.generation != generation || !s.open {
                    return None;
                }
                s.busy = false;
                match result {
                    Ok(details) => {
                        match s.show_details_keeping(details.clone(), &key, &incarnation) {
                            Retried::Ready => Some(details),
                            // A refresh removed the episode: the fresh details
                            // are shown to choose another. The playlist changed:
                            // the stale details closed, nothing is played, and
                            // the title must be chosen again. Neither offers a
                            // Retry that could not succeed.
                            Retried::EpisodeGone | Retried::PlaylistChanged => {
                                s.error = Some(
                                    crate::i18n::key(
                                        "Playback unavailable; refresh the playlist and try again",
                                    )
                                    .into(),
                                );
                                None
                            }
                        }
                    }
                    Err(_) => {
                        s.fail(
                            crate::i18n::key(
                                "Playback unavailable; refresh the playlist and try again",
                            ),
                            Operation::Play {
                                item: item.clone(),
                                key: key.clone(),
                                restart,
                                incarnation: incarnation.clone(),
                            },
                        );
                        None
                    }
                }
            });
            if let Some(details) = ready {
                play_key(ui, details, key, restart);
            }
        });
    });
}

fn select(ui: Ui, key: Key) {
    let same = ui.player.vod.with_untracked(|p| {
        p.as_ref()
            .is_some_and(|(p, _)| p.parent.as_ref().unwrap_or(&p.key) == &key)
    });
    if !same {
        crate::runtime().parental_content_closed();
    }
    let generation = ui.vod.with_untracked(|s| s.generation);
    crate::parental_view::request(ui, iptv_runtime::vod::target(&key), move || {
        if ui
            .vod
            .with_untracked(|s| s.open && s.generation == generation)
        {
            select_authorized(ui, key);
        }
    });
}

fn select_authorized(ui: Ui, key: Key) {
    let generation = edit(ui, |s| {
        s.focused = Some(key.clone());
        s.begin_operation()
    });
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let result = owner.vod_details(key.clone()).await;
        ui.vod.update(|s| {
            if s.generation != generation || !s.open {
                return;
            }
            s.busy = false;
            match result {
                Ok(details) => s.show_details(details),
                Err(_) => s.fail(
                    crate::i18n::key("Details unavailable; refresh the playlist and try again"),
                    Operation::Details(key),
                ),
            }
        });
    });
}

fn play(ui: Ui, restart: bool) {
    let target = ui.vod.with_untracked(|s| {
        s.details.clone().map(|d| {
            let key = if d.item.key.kind == Kind::Movie {
                Some(d.item.key.clone())
            } else {
                s.episode.clone()
            };
            (d, key)
        })
    });
    if let Some((details, Some(key))) = target {
        play_key(ui, details, key, restart);
    }
}

fn play_key(ui: Ui, details: Details, key: Key, restart: bool) {
    let generation = ui.vod.with_untracked(|s| s.generation);
    let parent = (key.kind == Kind::Episode).then(|| details.item.key.clone());
    let run = move || {
        crate::parental_view::request(ui, iptv_runtime::vod::target(&key), move || {
            if ui.vod.with_untracked(|s| s.generation == generation) {
                play_authorized(ui, details, key, restart);
            }
        })
    };
    if let Some(parent) = parent {
        crate::parental_view::request(ui, iptv_runtime::vod::target(&parent), run);
    } else {
        run();
    }
}

fn play_authorized(ui: Ui, details: Details, key: Key, restart: bool) {
    let generation = edit(ui, |s| s.begin_operation());
    // Retry plays the same title or episode the same way, after reading
    // its details and link afresh.
    let failed = Operation::Play {
        item: details.item.key.clone(),
        key: key.clone(),
        restart,
        incarnation: details.incarnation.clone(),
    };
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let result = owner.vod_play(details, key, restart).await;
        if ui.vod.with_untracked(|s| s.generation != generation) {
            return;
        }
        ui.vod.update(|s| s.busy = false);
        match result {
            Ok((playing, source)) => {
                if !owner.reserve_viewing(0, iptv_library::SourceId(playing.key.source)) {
                    // Once a recording stops, Retry resolves a new link.
                    ui.vod.update(|s| {
                        s.fail(
                            crate::i18n::key(
                                "No provider connection available while recording; stop a recording or check Streams at once",
                            ),
                            failed,
                        )
                    });
                    return;
                }
                flush(ui);
                crate::player::play_vod(ui, playing, source);
                close(ui);
            }
            Err(_) => {
                ui.vod.update(|s| {
                    s.fail(
                        crate::i18n::key(
                            "Playback unavailable; refresh the playlist and try again",
                        ),
                        failed,
                    )
                });
            }
        }
    });
}

fn toggle_block(ui: Ui) {
    if let Some(target) = ui.vod.with_untracked(|s| {
        s.details
            .as_ref()
            .map(|d| iptv_runtime::vod::target(&d.item.key))
    }) {
        let blocked = !crate::runtime().parental_explicitly_blocked(&target);
        crate::parental_view::block(ui, target, blocked);
    }
}

fn cycle_source(ui: Ui) {
    ui.vod.update(|s| {
        let at = s
            .query
            .source
            .and_then(|id| s.sources.iter().position(|v| v.id == id));
        s.query.source = match at {
            None => s.sources.first().map(|v| v.id),
            Some(at) => s.sources.get(at + 1).map(|v| v.id),
        };
        s.query.category = None;
    });
    reload(ui, false);
}

fn cycle_category(ui: Ui) {
    ui.vod.update(|s| {
        let at = s.query.category.as_ref().and_then(|(source, id)| {
            s.categories
                .iter()
                .position(|v| v.source == *source && v.id == *id)
        });
        s.query.category = match at {
            None => s.categories.first().map(|v| (v.source, v.id.clone())),
            Some(at) => s.categories.get(at + 1).map(|v| (v.source, v.id.clone())),
        };
    });
    reload(ui, false);
}

fn activate(ui: Ui) {
    let control = ui.vod.with_untracked(|s| s.control);
    match control {
        Control::Catalogue => {
            if let Some(key) = ui.vod.with_untracked(|s| s.focused.clone()) {
                select(ui, key);
            }
        }
        Control::Source => cycle_source(ui),
        Control::Category => cycle_category(ui),
        Control::Sort => {
            ui.vod.update(|s| s.query.sort = s.query.sort.next());
            reload(ui, false);
        }
        Control::Continue => {
            ui.vod
                .update(|s| s.query.continue_watching = !s.query.continue_watching);
            reload(ui, false);
        }
        Control::Play | Control::Episodes => play(ui, false),
        Control::Restart => play(ui, true),
        Control::Seasons => ui.vod.update(|s| s.cycle_season(1)),
        Control::Next => next(ui),
        Control::Block => toggle_block(ui),
        Control::Retry => retry(ui),
        Control::Search => {}
    }
}

pub fn key(ui: Ui, action: Action) {
    if action == Action::Back {
        back(ui);
        return;
    }
    if ui.vod.with_untracked(|s| s.busy) {
        return;
    }
    let control = ui.vod.with_untracked(|s| s.control);
    match action {
        Action::MoveLeft => ui.vod.update(|s| {
            if control == Control::Seasons {
                s.cycle_season(-1)
            } else {
                s.move_control(-1)
            }
        }),
        Action::MoveRight => ui.vod.update(|s| {
            if control == Control::Seasons {
                s.cycle_season(1)
            } else {
                s.move_control(1)
            }
        }),
        Action::MoveUp => {
            let at_top = ui.vod.with_untracked(|s| {
                control == Control::Catalogue
                    && s.cards
                        .first()
                        .is_some_and(|c| Some(&c.key) == s.focused.as_ref())
            });
            // Up on a shelf window's first title reads the titles above it
            // and steps onto the one just above.
            if at_top && ui.vod.with_untracked(|s| s.has_previous()) {
                load_previous(ui, true);
                return;
            }
            ui.vod.update(|s| match control {
                Control::Catalogue if at_top => s.move_control(-1),
                Control::Catalogue => s.move_focus(-1),
                Control::Episodes => s.move_episode(-1),
                _ => s.move_control(-1),
            })
        }
        Action::MoveDown => {
            ui.vod.update(|s| match control {
                Control::Catalogue => s.move_focus(1),
                Control::Episodes => s.move_episode(1),
                _ => s.move_control(1),
            });
            let more = ui.vod.with_untracked(|s| {
                s.control == Control::Catalogue
                    && s.has_more
                    && s.cards
                        .last()
                        .is_some_and(|c| Some(&c.key) == s.focused.as_ref())
            });
            if more {
                reload(ui, true);
            }
        }
        Action::Select => activate(ui),
        _ => {}
    }
}

pub fn title(ui: Ui) -> Option<String> {
    ui.player
        .vod
        .with(|v| v.as_ref().map(|(p, _)| p.title.clone()))
}

pub fn flush(ui: Ui) {
    crate::on_demand::flush(ui, false);
}

pub fn observe(ui: Ui, position_ms: u64, duration_ms: Option<u64>, ended: bool) {
    let mut write = None;
    let mut finished = None;
    let mut series_complete = None;
    ui.player.vod.update(|value| {
        let Some((p, _)) = value else { return };
        let progress =
            Progress::observed(position_ms, duration_ms.or(p.progress.duration_ms), ended);
        if ended
            || progress.completed != p.progress.completed
            || p.last_saved_ms
                .is_none_or(|ms| progress.position_ms.abs_diff(ms) >= 5_000)
        {
            write = Some((
                p.key.clone(),
                p.parent.clone(),
                progress,
                p.details.incarnation.clone(),
            ));
            p.last_saved_ms = Some(progress.position_ms);
        }
        let just_ended = ended && !p.ended;
        let became_completed = progress.completed && !p.progress.completed;
        p.ended |= ended;
        p.progress = progress;
        if let Some(ep) = p.details.episodes.iter_mut().find(|e| e.key == p.key) {
            ep.progress = progress;
        }
        if p.key.kind == Kind::Movie {
            p.details.item.progress = progress;
        }
        if (became_completed || ended)
            && p.parent.is_some()
            && (p.details.episodes.iter().all(|e| e.progress.completed)
                || p.details.next_episode(&p.key).is_none())
        {
            series_complete = p
                .parent
                .clone()
                .map(|key| (key, p.details.incarnation.clone()));
        }
        if just_ended {
            finished = Some(p.clone());
        }
    });
    if let Some((key, parent, progress, incarnation)) = write {
        crate::runtime().queue_vod_progress(key, parent, progress, incarnation);
    }
    if let Some((parent, incarnation)) = series_complete {
        crate::runtime().queue_vod_progress(
            parent,
            None,
            Progress {
                completed: true,
                ..Default::default()
            },
            incarnation,
        );
    }
    if let Some(p) = finished
        && let Some(next) = p.details.next_episode(&p.key)
    {
        if crate::settings::flag(iptv_settings::keys::BoolKey("vod.autoplay_next_episode")) {
            play_key(ui, p.details.clone(), next.key.clone(), false);
        } else {
            ui.vod.update(|s| {
                s.open = true;
                s.show_details(p.details.clone());
                s.episode = Some(next.key.clone());
                s.season = Some(next.season);
                s.notice = Some(crate::i18n::key("Ready for the next episode").into());
            });
        }
    }
}

fn next(ui: Ui) {
    let target = ui.vod.with_untracked(|s| {
        s.details.as_ref().and_then(|d| {
            s.episode
                .as_ref()
                .and_then(|key| d.next_episode(key))
                .map(|e| (d.clone(), e.key.clone()))
        })
    });
    if let Some((details, key)) = target {
        play_key(ui, details, key, false);
    }
}

fn button(
    ui: Ui,
    size: Scale,
    control: Control,
    label: impl IntoReactive<SharedString>,
    callback: impl Fn() + 'static,
) -> View {
    ui_kit::button(
        ui,
        size,
        label,
        move || ui.vod.with(|s| s.control == control),
        move || {
            ui.vod.update(|s| s.control = control);
            callback();
        },
    )
}

fn toolbar(ui: Ui, size: Scale) -> impl IntoElement {
    let field = element_ref();
    effect(move || {
        if ui.vod.with(|s| s.control == Control::Search) {
            field.focus();
        } else {
            field.blur();
        }
    });
    view()
        .gap(move || size.px(8.0))
        .child(
            ui_kit::field(size, input())
                .element_ref(&field)
                .value(move || ui.vod.with(|s| s.query.text.clone()))
                .placeholder(tr("Search movies and series"))
                .auto_correct(false)
                .auto_capitalize(AutoCapitalize::None)
                .return_key_type(ReturnKeyType::Search)
                .on_change(move |text: &str| {
                    ui.vod.update(|s| s.query.text = text.into());
                    reload(ui, false);
                })
                .on_submit(move |_: &str| ui.vod.update(|s| s.control = Control::Catalogue))
                .on_focus(move || ui.vod.update(|s| s.control = Control::Search))
                .test_id("vod-search"),
        )
        .child(
            view()
                .flex_row()
                .gap(move || size.px(8.0))
                .child(button(
                    ui,
                    size,
                    Control::Source,
                    move || {
                        ui.vod.with(|s| {
                            s.query
                                .source
                                .and_then(|id| s.sources.iter().find(|v| v.id == id))
                                .map(|v| v.name.clone())
                                .unwrap_or_else(|| tr("All playlists").into())
                        })
                    },
                    move || cycle_source(ui),
                ))
                .child(button(
                    ui,
                    size,
                    Control::Category,
                    move || {
                        ui.vod.with(|s| {
                            s.query
                                .category
                                .as_ref()
                                .and_then(|(source, id)| {
                                    s.categories
                                        .iter()
                                        .find(|v| v.source == *source && v.id == *id)
                                })
                                .map(|v| v.name.clone())
                                .unwrap_or_else(|| tr("All categories").into())
                        })
                    },
                    move || cycle_category(ui),
                ))
                .child(button(
                    ui,
                    size,
                    Control::Sort,
                    move || {
                        ui.vod.with(|s| match s.query.sort {
                            Sort::Name => tr("Name"),
                            Sort::Newest => tr("Recently added"),
                            Sort::Year => tr("Year"),
                            Sort::Rating => tr("Rating"),
                        })
                    },
                    move || {
                        ui.vod.update(|s| s.query.sort = s.query.sort.next());
                        reload(ui, false);
                    },
                ))
                .child(
                    button(
                        ui,
                        size,
                        Control::Continue,
                        move || {
                            if ui.vod.with(|s| s.query.continue_watching) {
                                format!("✓ {}", tr("Continue watching"))
                            } else {
                                tr("Continue watching").into()
                            }
                        },
                        move || {
                            ui.vod
                                .update(|s| s.query.continue_watching = !s.query.continue_watching);
                            reload(ui, false);
                        },
                    )
                    // The rail's Continue watching entry shares the words.
                    .test_id("vod-continue"),
                ),
        )
}

/// What a card is, for a shelf that mixes movies and series.
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Movie => tr("Film"),
        Kind::Series | Kind::Episode => tr("TV series"),
    }
}

/// A card's line: its name, then on a mixed shelf whether it is a film or a
/// series, the year, the playlist it comes from (titles shared by playlists
/// stay apart) and its progress.
fn card_label(card: &iptv_core::vod::Card, kind: bool) -> String {
    format!(
        "{}{}{} · {}{}",
        card.name,
        if kind {
            format!(" · {}", kind_name(card.key.kind))
        } else {
            String::new()
        },
        card.year.map(|y| format!(" · {y}")).unwrap_or_default(),
        card.source_name,
        if card.progress.completed {
            format!(" · {}", tr("Watched"))
        } else if card.progress.resume_ms() > 0 {
            trf(
                " · Resume at {minutes} min",
                &[("minutes", (card.progress.position_ms / 60_000).into())],
            )
        } else {
            String::new()
        }
    )
}

fn card_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let card = move || ui.vod.with(|s| s.cards.get(at.index()).cloned());
    let mixed = move || ui.vod.with(|s| s.is_shelf());
    ui_kit::button(
        ui,
        size,
        move || {
            card()
                .as_ref()
                .map(|c| card_label(c, mixed()))
                .unwrap_or_default()
        },
        move || {
            card().is_some_and(|c| {
                ui.vod
                    .with(|s| s.control == Control::Catalogue && s.focused.as_ref() == Some(&c.key))
            })
        },
        move || {
            if let Some(card) = card() {
                select(ui, card.key);
            }
        },
    )
    .test_id("vod-title")
}
fn catalogue(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    effect(move || {
        let at = ui.vod.with(|s| {
            s.focused
                .as_ref()
                .and_then(|key| s.cards.iter().position(|c| &c.key == key))
        });
        if let Some(at) = at {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    let empty = when(
        move || {
            ui.vod
                .with(|s| s.cards.is_empty() && !s.busy && s.error.is_none())
        },
        move || {
            ui_kit::body(size, move || {
                if ui.vod.with(|s| s.shelf == Some(Shelf::ContinueWatching)) {
                    tr("Nothing to continue yet. Titles you start on any playlist appear here.")
                } else {
                    tr(
                        "No titles found. Enable movie and series imports in playlist settings, then refresh your playlist.",
                    )
                }
            })
        },
    );
    // A shelf has no search, filter or sort: its titles come from every
    // playlist in a fixed order, so the toolbar would only add stops.
    let filters = when(
        move || !ui.vod.with(|s| s.is_shelf()),
        move || toolbar(ui, size),
    );
    let titles = virtual_list(
        move || ui.vod.with(|s| s.cards.len()),
        move |at| card_row(ui, size, at),
    )
    .element_ref(&rows)
    .flex_1();
    let more = when(
        move || ui.vod.with(|s| s.has_more),
        move || {
            ui_kit::button(
                ui,
                size,
                tr("Load more"),
                || false,
                move || {
                    if !ui.vod.with_untracked(|s| s.busy) {
                        reload(ui, true);
                    }
                },
            )
        },
    );
    // A shelf reopened deep down starts at its focused title's page; the
    // titles above are a tap or click away (Up on the first title with a
    // remote). The focus stays where it is.
    let previous = when(
        move || ui.vod.with(|s| s.has_previous() && s.details.is_none()),
        move || {
            ui_kit::button(
                ui,
                size,
                tr("Load previous"),
                || false,
                move || {
                    if !ui.vod.with_untracked(|s| s.busy) {
                        load_previous(ui, false);
                    }
                },
            )
            .test_id("vod-previous")
        },
    );
    view()
        .flex_1()
        .gap(move || size.px(8.0))
        .child(filters)
        .child(empty)
        .child(previous)
        .child(titles)
        .child(more)
}
fn episode_label(ep: &iptv_core::vod::Episode) -> String {
    let watched = if ep.progress.completed {
        format!(" · {}", tr("Watched"))
    } else {
        String::new()
    };
    format!("{} · {}{watched}", ep.number, ep.title)
}
fn episode_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let ep = move || {
        ui.vod.with(|s| {
            s.details
                .as_ref()
                .and_then(|d| {
                    d.episodes
                        .iter()
                        .filter(|e| Some(e.season) == s.season)
                        .nth(at.index())
                })
                .cloned()
        })
    };
    ui_kit::button(
        ui,
        size,
        move || ep().as_ref().map(episode_label).unwrap_or_default(),
        move || ep().is_some_and(|e| ui.vod.with(|s| s.episode.as_ref() == Some(&e.key))),
        move || {
            if let Some(ep) = ep() {
                ui.vod.update(|s| {
                    s.episode = Some(ep.key);
                    s.control = Control::Episodes;
                });
                play(ui, false);
            }
        },
    )
    .test_id("vod-episode")
}
fn episodes(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    effect(move || {
        let at = ui.vod.with(|s| {
            s.details.as_ref().and_then(|d| {
                d.episodes
                    .iter()
                    .filter(|e| Some(e.season) == s.season)
                    .position(|e| Some(&e.key) == s.episode.as_ref())
            })
        });
        if let Some(at) = at {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    let season = button(
        ui,
        size,
        Control::Seasons,
        move || {
            format!(
                "{} {}",
                tr("Season"),
                ui.vod.with(|s| s.season.unwrap_or(1))
            )
        },
        move || ui.vod.update(|s| s.cycle_season(1)),
    );
    let empty = when(
        move || {
            ui.vod
                .with(|s| s.details.as_ref().is_some_and(|d| d.episodes.is_empty()))
        },
        move || {
            ui_kit::body(
                size,
                tr("This playlist does not provide playable episodes for this series."),
            )
        },
    );
    let list = virtual_list(
        move || {
            ui.vod.with(|s| {
                s.details
                    .as_ref()
                    .map(|d| {
                        d.episodes
                            .iter()
                            .filter(|e| Some(e.season) == s.season)
                            .count()
                    })
                    .unwrap_or(0)
            })
        },
        move |at| episode_row(ui, size, at),
    )
    .element_ref(&rows)
    .flex_1();
    view()
        .flex_1()
        .gap(move || size.px(8.0))
        .child(season)
        .child(empty)
        .child(list)
}
fn metadata(ui: Ui, size: Scale) -> impl IntoElement {
    let poster = when(
        move || {
            ui.vod.with(|s| {
                s.details
                    .as_ref()
                    .is_some_and(|d| d.item.poster.as_ref().is_some_and(|p| !p.is_empty()))
            })
        },
        move || {
            let source = crate::content_image::source(move || {
                ui.vod
                    .with(|s| s.details.as_ref().and_then(|d| d.item.poster.clone()))
                    .unwrap_or_default()
            });
            image(source)
                .w(move || size.px(100.0))
                .h(move || size.px(150.0))
                .object_fit(ResizeMode::Contain)
        },
    );
    let name = ui_kit::heading(size, move || {
        ui.vod.with(|s| {
            s.details
                .as_ref()
                .map(|d| card_label(&d.item, s.is_shelf()))
                .unwrap_or_default()
        })
    });
    let plot = ui_kit::body(size, move || {
        ui.vod.with(|s| {
            s.details
                .as_ref()
                .and_then(|d| d.item.plot.clone())
                .unwrap_or_default()
        })
    })
    .number_of_lines(4);
    let words = view()
        .flex_1()
        .gap(move || size.px(8.0))
        .child(name)
        .child(plot);
    view()
        .flex_row()
        .gap(move || size.px(20.0))
        .child(poster)
        .child(words)
}
fn play_label(ui: Ui) -> &'static str {
    // A series resumes only its chosen episode: after a finished episode the
    // next one plays from its start.
    if ui.vod.with(|s| s.resumes()) {
        tr("Resume")
    } else {
        tr("Play")
    }
}
fn has_next(ui: Ui) -> bool {
    ui.vod.with(|s| {
        s.details.as_ref().is_some_and(|d| {
            s.episode
                .as_ref()
                .is_some_and(|key| d.next_episode(key).is_some())
        })
    })
}
fn block_label(ui: Ui) -> &'static str {
    let blocked = ui.vod.with(|s| {
        s.details.as_ref().is_some_and(|d| {
            crate::runtime().parental_explicitly_blocked(&iptv_runtime::vod::target(&d.item.key))
        })
    });
    if blocked {
        tr("Unblock content")
    } else {
        tr("Block content")
    }
}
fn details(ui: Ui, size: Scale) -> impl IntoElement {
    let play_button = button(
        ui,
        size,
        Control::Play,
        move || play_label(ui),
        move || play(ui, false),
    );
    let restart = button(
        ui,
        size,
        Control::Restart,
        tr("Play from beginning"),
        move || play(ui, true),
    );
    let next = when(
        move || has_next(ui),
        move || {
            button(ui, size, Control::Next, tr("Next episode"), move || {
                next(ui)
            })
        },
    );
    let actions = view()
        .flex_row()
        .gap(move || size.px(12.0))
        .child(play_button)
        .child(restart)
        .child(next);
    let block = button(
        ui,
        size,
        Control::Block,
        move || block_label(ui),
        move || toggle_block(ui),
    );
    let seasons = when(
        move || {
            ui.vod.with(|s| {
                s.details
                    .as_ref()
                    .is_some_and(|d| d.item.key.kind == Kind::Series)
            })
        },
        move || episodes(ui, size),
    );
    view()
        .flex_1()
        .gap(move || size.px(12.0))
        .child(metadata(ui, size))
        .child(actions)
        .child(block)
        .child(seasons)
}
pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            let header = ui_kit::header(
                ui,
                size,
                move || {
                    ui.vod.with(|s| match s.shelf {
                        Some(Shelf::ContinueWatching) => tr("Continue watching"),
                        Some(Shelf::RecentlyAdded) => tr("Recently added"),
                        None if s.query.kind == Kind::Series => tr("Series"),
                        None => tr("Films"),
                    })
                },
                close,
            );
            let hint = ui_kit::caption(size, move || {
                ui.vod.with(|s| match s.shelf {
                    Some(_) if s.details.is_some() => tr(
                        "Choose a title, then play or resume. Left/Right changes controls; Up/Down browses titles and episodes.",
                    ),
                    Some(Shelf::ContinueWatching) => tr(
                        "Titles you have started on any playlist, most recent first. Choose one to resume it.",
                    ),
                    Some(Shelf::RecentlyAdded) => tr(
                        "Newest titles from every playlist first, by the date each playlist added them; titles without a date follow by name.",
                    ),
                    None => tr(
                        "Choose a title, then play or resume. Left/Right changes controls; Up/Down browses titles and episodes.",
                    ),
                })
            });
            // Shown with a failed operation: OK, a tap or a click runs that
            // same operation again (see `retry`).
            let retry_button = when(
                move || ui.vod.with(|s| s.retry.is_some() && !s.busy),
                move || {
                    button(ui, size, Control::Retry, tr("Retry"), move || retry(ui))
                        .test_id("vod-retry")
                },
            );
            let error = ui_kit::body(size, move || {
                ui.vod.with(|s| {
                    s.error
                        .as_ref()
                        .map(|e| tr(e).to_owned())
                        .unwrap_or_default()
                })
            });
            let notice = ui_kit::caption(size, move || {
                ui.vod.with(|s| {
                    s.notice
                        .as_ref()
                        .map(|e| tr(e).to_owned())
                        .unwrap_or_default()
                })
            });
            let busy = ui_kit::caption(size, move || {
                if ui.vod.with(|s| s.busy) {
                    tr("Loading…")
                } else {
                    ""
                }
            });
            let content = when(
                move || ui.vod.with(|s| s.details.is_some()),
                move || details(ui, size),
            )
            .otherwise(move || catalogue(ui, size));
            ui_kit::screen(ui, size)
                .test_id("vod-screen")
                .accessibility_elements_hidden(move || {
                    crate::overlays::Overlay::Vod.accessibility_covered(ui)
                })
                .child(header)
                .child(hint)
                .child(error)
                .child(retry_button)
                .child(notice)
                .child(busy)
                .child(content)
        },
    )
}
