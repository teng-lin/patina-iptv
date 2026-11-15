//! Movies and series share one portable catalogue state and the persistent player.
use crate::{Ui, i18n::tr, scale::Scale, ui_kit};
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
    ui.vod.update(|s| {
        s.open = false;
        s.generation = s.generation.wrapping_add(1);
    });
}
pub fn back(ui: Ui) -> bool {
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
                    s.error = Some("On-demand library could not be loaded".into());
                }
            }
        });
    });
}
fn select(ui: Ui, key: Key) {
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
                    s.error = Some("Details unavailable; refresh the playlist and try again".into())
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
    if !crate::runtime().reserve_viewing(0, iptv_library::SourceId(key.source)) {
        ui.vod.update(|s|s.error=Some("No provider connection available while recording; stop a recording or check Streams at once".into()));
        return;
    }
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
                flush(ui);
                crate::player::play_vod(ui, playing, source);
                close(ui);
            }
            Err(_) => {
                owner.release_viewing(0);
                ui.vod.update(|s| {
                    s.error =
                        Some("Playback unavailable; refresh the playlist and try again".into())
                });
            }
        }
    });
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
    let ms = ui.player.main().position_ms.get_untracked();
    if let Some((p, _)) = ui.player.vod.get_untracked() {
        let owner = crate::runtime();
        crate::data::spawn_in_app(async move {
            let _ = owner
                .vod_save_progress(
                    p.key,
                    p.parent,
                    Progress::observed(ms, p.progress.duration_ms, false),
                )
                .await;
        });
    }
}
pub fn observe(ui: Ui, position_ms: u64, duration_ms: Option<u64>, ended: bool) {
    let mut write = None;
    let mut finished = None;
    ui.player.vod.update(|value| {
        let Some((p, _)) = value else { return };
        let progress =
            Progress::observed(position_ms, duration_ms.or(p.progress.duration_ms), ended);
        if ended
            || progress.completed != p.progress.completed
            || p.last_saved_ms
                .is_none_or(|ms| progress.position_ms.abs_diff(ms) >= 5_000)
        {
            write = Some((p.key.clone(), p.parent.clone(), progress));
            p.last_saved_ms = Some(progress.position_ms);
        }
        if ended && !p.ended {
            finished = Some(p.clone());
            p.ended = true;
        }
        p.progress = progress;
    });
    if let Some((key, parent, progress)) = write {
        let owner = crate::runtime();
        crate::data::spawn_in_app(async move {
            let _ = owner.vod_save_progress(key, parent, progress).await;
        });
    }
    if let Some(p) = finished {
        if let Some(next) = p.details.next_episode(&p.key) {
            if crate::settings::flag(iptv_settings::keys::BoolKey("vod.autoplay_next_episode")) {
                play_key(ui, p.details.clone(), next.key.clone(), false);
            } else {
                ui.vod.update(|s| {
                    s.open = true;
                    s.show_details(p.details.clone());
                    s.episode = Some(next.key.clone());
                    s.season = Some(next.season);
                    s.notice = Some("Ready for the next episode".into());
                });
            }
        }
    }
}
fn next(ui: Ui) {
    let current = ui.player.vod.get_untracked();
    if let Some((p, _)) = current
        && let Some(next) = p.details.next_episode(&p.key)
    {
        play_key(ui, p.details.clone(), next.key.clone(), false);
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
                        tr(ui.vod.with(|s| match s.query.sort {
                            Sort::Name => "Name",
                            Sort::Newest => "Recently added",
                            Sort::Year => "Year",
                            Sort::Rating => "Rating",
                        }))
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
            format!(
                " · {} {} min",
                tr("Resume"),
                card.progress.position_ms / 60_000
            )
        } else {
            String::new()
        }
    )
}
fn catalogue(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    effect(move || {
        if let Some(at) = ui.vod.with(|s| {
            s.focused
                .as_ref()
                .and_then(|key| s.cards.iter().position(|c| &c.key == key))
        }) {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    view().flex_1().gap(move||size.px(8.0)).child(toolbar(ui,size))
    .child(when(move||ui.vod.with(|s|s.cards.is_empty()&&!s.busy),move||ui_kit::body(size,tr("No titles found. Enable movie and series imports in playlist settings, then refresh your playlist."))))
    .child(virtual_list(move||ui.vod.with(|s|s.cards.len()),move|at|{
        let card=move||ui.vod.with(|s|s.cards.get(at.index()).cloned());
        ui_kit::button(ui,size,move||card().as_ref().map(card_label).unwrap_or_default(),move||card().is_some_and(|c|ui.vod.with(|s|s.control==Control::Catalogue&&s.focused.as_ref()==Some(&c.key))),move||if let Some(card)=card(){select(ui,card.key)}).test_id("vod-title")
    }).element_ref(&rows).flex_1())
    .child(when(move||ui.vod.with(|s|s.has_more),move||ui_kit::button(ui,size,tr("Load more"),||false,move||if !ui.vod.with_untracked(|s|s.busy){reload(ui,true);})))
}
fn details(ui: Ui, size: Scale) -> impl IntoElement {
    let actions = view()
        .flex_row()
        .gap(move || size.px(12.0))
        .child(button(
            ui,
            size,
            Control::Play,
            move || {
                ui.vod.with(|s| {
                    s.details
                        .as_ref()
                        .map(|d| {
                            if d.item.progress.resume_ms() > 0
                                || d.episodes
                                    .iter()
                                    .find(|e| Some(&e.key) == s.episode.as_ref())
                                    .is_some_and(|e| e.progress.resume_ms() > 0)
                            {
                                tr("Resume")
                            } else {
                                tr("Play")
                            }
                        })
                        .unwrap_or(tr("Play"))
                })
            },
            move || play(ui, false),
        ))
        .child(button(
            ui,
            size,
            Control::Restart,
            tr("Play from beginning"),
            move || play(ui, true),
        ))
        .child(when(
            move || {
                ui.player.vod.with(|p| {
                    p.as_ref()
                        .is_some_and(|(p, _)| p.details.next_episode(&p.key).is_some())
                })
            },
            move || {
                button(ui, size, Control::Next, tr("Next episode"), move || {
                    next(ui)
                })
            },
        ));
    view().flex_1().gap(move||size.px(12.0))
        .child(ui_kit::heading(size,move||ui.vod.with(|s|s.details.as_ref().map(|d|card_label(&d.item)).unwrap_or_default())))
        .child(ui_kit::body(size,move||ui.vod.with(|s|s.details.as_ref().and_then(|d|d.item.plot.clone()).unwrap_or_default())))
        .child(actions)
        .child(when(move||ui.vod.with(|s|s.details.as_ref().is_some_and(|d|d.item.key.kind==Kind::Series)),move||{
            view().flex_1().gap(move||size.px(8.0))
                .child(button(ui,size,Control::Seasons,move||format!("{} {}",tr("Season"),ui.vod.with(|s|s.season.unwrap_or(1))),move||ui.vod.update(|s|s.cycle_season(1))))
                .child(when(move||ui.vod.with(|s|s.details.as_ref().is_some_and(|d|d.episodes.is_empty())),move||ui_kit::body(size,tr("This playlist does not provide playable episodes for this series."))))
                .child(virtual_list(move||ui.vod.with(|s|s.details.as_ref().map(|d|d.episodes.iter().filter(|e|Some(e.season)==s.season).count()).unwrap_or(0)),move|at|{
                    let ep=move||ui.vod.with(|s|s.details.as_ref().and_then(|d|d.episodes.iter().filter(|e|Some(e.season)==s.season).nth(at.index())).cloned());
                    ui_kit::button(ui,size,move||ep().map(|e|format!("{} · {}{}",e.number,e.title,if e.progress.completed{format!(" · {}",tr("Watched"))}else{String::new()})).unwrap_or_default(),move||ep().is_some_and(|e|ui.vod.with(|s|s.episode.as_ref()==Some(&e.key))),move||if let Some(ep)=ep(){ui.vod.update(|s|{s.episode=Some(ep.key.clone());s.control=Control::Episodes;});play(ui,false);}).test_id("vod-episode")
                }).flex_1())
        }))
}
pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            ui_kit::screen(ui,size).test_id("vod-screen")
    .accessibility_elements_hidden(move||crate::overlays::Overlay::Vod.accessibility_covered(ui))
    .child(ui_kit::header(ui,size,move||if ui.vod.with(|s|s.query.kind==Kind::Series){tr("Series")}else{tr("Films")},close))
    .child(ui_kit::caption(size,tr("Choose a title, then play or resume. Left/Right changes controls; Up/Down browses titles and episodes.")))
    .child(ui_kit::body(size,move||ui.vod.with(|s|s.error.as_ref().map(|e|tr(e).to_owned()).unwrap_or_default())))
    .child(ui_kit::caption(size,move||ui.vod.with(|s|s.notice.as_ref().map(|e|tr(e).to_owned()).unwrap_or_default())))
    .child(ui_kit::caption(size,move||if ui.vod.with(|s|s.busy){tr("Loading…")}else{""}))
    .child(when(move||ui.vod.with(|s|s.details.is_some()),move||details(ui,size)).otherwise(move||catalogue(ui,size)))
        },
    )
}
