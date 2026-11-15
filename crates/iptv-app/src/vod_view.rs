//! Movies and series share one portable catalogue state and the persistent player.
use crate::{
    Ui,
    i18n::{tr, trf},
    scale::Scale,
    ui_kit,
};
use iptv_core::vod::{Control, Details, Key, Kind, Progress, Sort};
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

pub fn close(ui: Ui) {
    if ui.player.vod.with_untracked(Option::is_none) {
        crate::runtime().parental_content_closed();
    }
    ui.vod.update(|s| {
        s.open = false;
        s.generation = s.generation.wrapping_add(1);
    });
}

pub fn back(ui: Ui) -> bool {
    if ui.player.vod.with_untracked(Option::is_none) {
        crate::runtime().parental_content_closed();
    }
    ui.vod.update(|s| {
        s.back();
        s.control = Control::Catalogue;
    });
    true
}

fn reload(ui: Ui, append: bool) {
    let (query, generation, offset) = edit(ui, |s| {
        if !append {
            s.reload();
        } else {
            s.busy = true;
        }
        (
            s.query.clone(),
            s.generation,
            if append { s.cards.len() as u32 } else { 0 },
        )
    });
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let result = owner.vod_catalogue(query, offset).await;
        ui.vod.update(|s| {
            if s.generation != generation || !s.open {
                return;
            }
            match result {
                Ok((cards, more, sources, categories)) => {
                    s.sources = sources;
                    s.categories = categories;
                    s.publish(generation, cards, more, append);
                }
                Err(_) => {
                    s.busy = false;
                    s.error =
                        Some(crate::i18n::key("On-demand library could not be loaded").into());
                }
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
        s.busy = true;
        s.error = None;
        s.generation = s.generation.wrapping_add(1);
        s.generation
    });
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let result = owner.vod_details(key).await;
        ui.vod.update(|s| {
            if s.generation != generation || !s.open {
                return;
            }
            s.busy = false;
            match result {
                Ok(details) => s.show_details(details),
                Err(_) => {
                    s.error = Some(
                        crate::i18n::key("Details unavailable; refresh the playlist and try again")
                            .into(),
                    )
                }
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
    let generation = edit(ui, |s| {
        s.busy = true;
        s.error = None;
        s.generation = s.generation.wrapping_add(1);
        s.generation
    });
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
                    ui.vod.update(|s|s.error=Some(crate::i18n::key("No provider connection available while recording; stop a recording or check Streams at once").into()));
                    return;
                }
                flush(ui);
                crate::player::play_vod(ui, playing, source);
                close(ui);
            }
            Err(_) => {
                ui.vod.update(|s| {
                    s.error = Some(
                        crate::i18n::key(
                            "Playback unavailable; refresh the playlist and try again",
                        )
                        .into(),
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
        Action::MoveUp => ui.vod.update(|s| match control {
            Control::Catalogue
                if s.cards
                    .first()
                    .is_some_and(|c| Some(&c.key) == s.focused.as_ref()) =>
            {
                s.move_control(-1)
            }
            Control::Catalogue => s.move_focus(-1),
            Control::Episodes => s.move_episode(-1),
            _ => s.move_control(-1),
        }),
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
        if became_completed
            && p.parent.is_some()
            && p.details.episodes.iter().all(|e| e.progress.completed)
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
) -> impl IntoElement {
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
                .child(button(
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
                )),
        )
}

fn card_label(card: &iptv_core::vod::Card) -> String {
    format!(
        "{}{} · {}{}",
        card.name,
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
    ui_kit::button(
        ui,
        size,
        move || card().as_ref().map(card_label).unwrap_or_default(),
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
        move || ui.vod.with(|s| s.cards.is_empty() && !s.busy),
        move || {
            ui_kit::body(
                size,
                tr(
                    "No titles found. Enable movie and series imports in playlist settings, then refresh your playlist.",
                ),
            )
        },
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
    view()
        .flex_1()
        .gap(move || size.px(8.0))
        .child(toolbar(ui, size))
        .child(empty)
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
                .map(|d| card_label(&d.item))
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
    let resume = ui.vod.with(|s| {
        s.details.as_ref().is_some_and(|d| {
            d.item.progress.resume_ms() > 0
                || d.episodes
                    .iter()
                    .find(|e| Some(&e.key) == s.episode.as_ref())
                    .is_some_and(|e| e.progress.resume_ms() > 0)
        })
    });
    if resume { tr("Resume") } else { tr("Play") }
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
                    if ui.vod.with(|s| s.query.kind == Kind::Series) {
                        tr("Series")
                    } else {
                        tr("Films")
                    }
                },
                close,
            );
            let hint = ui_kit::caption(
                size,
                tr(
                    "Choose a title, then play or resume. Left/Right changes controls; Up/Down browses titles and episodes.",
                ),
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
                .child(notice)
                .child(busy)
                .child(content)
        },
    )
}
