//! Movies and series share one portable catalog state and the persistent player.
//! The cross-source shelves (Continue watching, Recently added) use the same
//! state, details and playback; only their list is read differently.
use crate::{
    Ui,
    i18n::{tr, trf},
    scale::Scale,
    ui_kit,
};
use iptv_core::vod::{Details, DiscoveryPage, DiscoveryQuery, Key, Kind, Progress, Shelf, Sort};
use iptv_nav::Action;
use iptv_presentation::vod::{Control, Operation, Retried};
use nsh_base::input;
use nsh_core::prelude::*;
mod browser;
mod empty;
fn edit<R>(ui: Ui, f: impl FnOnce(&mut iptv_presentation::vod::Screen) -> R) -> R {
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
        s.query.sort = sort_of(crate::settings::get().choice(
            iptv_settings::keys::VOD_SORT,
            &iptv_settings::Target::Global,
        ));
    });
    reload(ui, false);
}

/// The catalog's Sort as `vod.sort` keeps it (#282): the order chosen
/// last opens again.
fn sort_of(choice: &str) -> Sort {
    match choice {
        "year" => Sort::Year,
        "rating" => Sort::Rating,
        "date_added" => Sort::Newest,
        _ => Sort::Name,
    }
}

fn choice_of(sort: Sort) -> &'static str {
    match sort {
        Sort::Name => "name",
        Sort::Newest => "date_added",
        Sort::Year => "year",
        Sort::Rating => "rating",
    }
}

/// The catalog's Sort moved on: shown at once and kept for the next time
/// the catalog opens (on this device; a backup carries it).
fn cycle_sort(ui: Ui) {
    let sort = edit(ui, |s| {
        s.cycle_sort();
        s.query.sort
    });
    reload(ui, false);
    if let Err(error) = crate::settings::set(
        iptv_settings::keys::VOD_SORT.as_str(),
        iptv_settings::Value::choice(choice_of(sort)),
    ) {
        log::warn!("iptv vod: the catalog order was not kept: {error}");
    }
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
        crate::runtime_owner::runtime().parental_content_closed();
    }
    // A shelf page or focus lookup still being read is no longer wanted.
    crate::runtime_owner::runtime().vod_discovery_cancel();
    ui.vod.update(|s| {
        s.open = false;
        s.handoff = iptv_presentation::vod::Handoff::None;
        s.generation = s.generation.wrapping_add(1);
    });
}

/// Back from a movie or episode playing full screen: its details reopen
/// over the same player, the played episode chosen (#147). The guide
/// returns underneath, so leaving the details and their list ends where
/// the title was chosen from, the title playing on in the preview. False
/// when the title did not come from this screen.
pub fn return_from_player(ui: Ui) -> bool {
    let Some((details, played, saved)) = ui.player.vod.with_untracked(|p| {
        p.as_ref()
            .map(|(p, _)| (p.details.clone(), p.key.clone(), p.last_saved_ms.is_some()))
    }) else {
        return false;
    };
    if !ui.vod.with_untracked(|s| s.returns_to(&details.item.key)) {
        return false;
    }
    ui.vod
        .update(|s| s.return_from_player(details, &played, saved));
    ui.mode.apply(crate::mode::Action::ReturnToBrowse);
    true
}

/// Whether Back from the title playing full screen returns to its
/// details, and to which library (the visible ‹'s label).
pub fn returns_to(ui: Ui) -> Option<Kind> {
    ui.player.vod.with(|p| {
        p.as_ref().and_then(|(p, _)| {
            let item = &p.details.item.key;
            ui.vod.with(|s| s.returns_to(item)).then_some(item.kind)
        })
    })
}

pub fn back(ui: Ui) -> bool {
    if ui.player.vod.with_untracked(Option::is_none) {
        crate::runtime_owner::runtime().parental_content_closed();
    }
    let (closed, refresh, end) = edit(ui, |s| {
        let pending = s.busy;
        let had_details = s.details.is_some();
        // Details reopened from the player over a list that watching
        // changes (a shelf, or the Continue watching category): the list
        // is read again for the progress and order just changed, its title
        // kept by identity (a shelf reads the window around it). Another
        // category's order does not change; its movie card already shows
        // the progress (`Screen::return_from_player`), and reading it
        // again from its first page could lose a deep title's focus.
        let watched = had_details
            && s.handoff == iptv_presentation::vod::Handoff::Returned
            && (s.is_shelf() || s.query.continue_watching);
        s.back();
        if had_details {
            s.control = Control::Catalog;
        }
        (
            !s.open,
            s.open && (watched || (pending && !had_details && !s.is_shelf())),
            s.end,
        )
    });
    crate::parental_view::enforce(ui);
    if closed {
        crate::runtime_owner::runtime().vod_discovery_cancel();
    } else if refresh {
        load(ui, false, Some(end));
    }
    true
}

/// Loads the list: from its top (a shelf: the window around its focused
/// title), or (`append`) the page after the titles shown, which stay with
/// the focus.
fn reload(ui: Ui, append: bool) {
    load(ui, append, None);
}

/// [`reload`]; with `keep` (the titles it had loaded), a catalog read
/// again from its top reads at least that many, so a title focused pages
/// down keeps the focus by identity (a shelf always reads the window
/// around its focused title). Retry of that read reads as many again.
fn load(ui: Ui, append: bool, keep: Option<u32>) {
    let (query, shelf, generation, offset, anchor, through) = edit(ui, |s| {
        let through = match keep {
            Some(end) if !append && !s.is_shelf() => end,
            _ => 0,
        };
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
        (
            s.query.clone(),
            s.shelf,
            s.generation,
            offset,
            anchor,
            through,
        )
    });
    let owner = crate::runtime_owner::runtime();
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
        // catalog also refreshes its filter choices.
        let result =
            match (shelf, anchor) {
                (Some(shelf), Some(key)) => owner
                    .vod_discovery_around(shelf, key)
                    .await
                    .map(|page| (page, None)),
                (Some(shelf), None) => owner
                    .vod_discovery(DiscoveryQuery::after(shelf, offset as usize))
                    .await
                    .map(|page| (page, None)),
                (None, _) if through > 0 => owner.vod_catalog_through(query, through).await.map(
                    |(cards, more, sources, categories)| {
                        (
                            DiscoveryPage {
                                cards,
                                more,
                                start: 0,
                            },
                            Some((sources, categories)),
                        )
                    },
                ),
                (None, _) => owner.vod_catalog(query, offset).await.map(
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
                    } else if through > 0 {
                        Operation::Through { through }
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
    let owner = crate::runtime_owner::runtime();
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
        Operation::Through { through } => load(ui, false, Some(through)),
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
        let owner = crate::runtime_owner::runtime();
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
        crate::runtime_owner::runtime().parental_content_closed();
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
    let owner = crate::runtime_owner::runtime();
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
    // The viewer's choice supersedes a next episode about to autoplay
    // (`autoplay_next`), also while its PIN is still being checked.
    let generation = edit(ui, |s| {
        s.generation = s.generation.wrapping_add(1);
        s.generation
    });
    let parent = (key.kind == Kind::Episode).then(|| details.item.key.clone());
    let run = move || {
        crate::parental_view::request(ui, iptv_runtime::vod::target(&key), move || {
            if ui.vod.with_untracked(|s| s.generation == generation) {
                play_authorized(ui, details, key, restart, true, None);
            }
        })
    };
    if let Some(parent) = parent {
        crate::parental_view::request(ui, iptv_runtime::vod::target(&parent), run);
    } else {
        run();
    }
}

/// Plays `key` full screen, or (`full_screen` false) in the preview behind
/// the details, which stay open (#187). `after`: the episode that ended,
/// when this autoplays the next one. That needs the viewer still where
/// they watched it, with nothing else playing; else the ended episode
/// stops, the next one up next.
fn play_authorized(
    ui: Ui,
    details: Details,
    key: Key,
    restart: bool,
    full_screen: bool,
    after: Option<Key>,
) {
    let generation = edit(ui, |s| s.begin_operation());
    // Retry plays the same title or episode the same way, after reading
    // its details and link afresh.
    let failed = Operation::Play {
        item: details.item.key.clone(),
        key: key.clone(),
        restart,
        incarnation: details.incarnation.clone(),
    };
    let series = details.item.key.clone();
    let owner = crate::runtime_owner::runtime();
    crate::data::spawn_in_app(async move {
        let result = owner.vod_play(details, key, restart).await;
        // The episode that ended, its next one not played after all.
        let not_played = {
            let after = after.clone();
            move || {
                if let Some(ended) = &after {
                    stop_ended(ui, ended);
                }
            }
        };
        if ui.vod.with_untracked(|s| s.generation != generation) {
            not_played();
            return;
        }
        ui.vod.update(|s| s.busy = false);
        if let Some(ended) = &after {
            let held = ui
                .player
                .vod
                .with_untracked(|p| p.as_ref().is_some_and(|(p, _)| &p.key == ended));
            let there = matches!(
                (where_watched(ui, &series), full_screen),
                (Watched::FullScreen, true) | (Watched::Details, false)
            );
            if !held || !there {
                not_played();
                return;
            }
        }
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
                    not_played();
                    return;
                }
                flush(ui);
                crate::player::play_vod(ui, playing, source, full_screen);
                if full_screen {
                    // The screen waits under the player: Back from the
                    // title returns to its details (`return_from_player`).
                    crate::runtime_owner::runtime().vod_discovery_cancel();
                    ui.vod.update(|s| s.hand_to_player());
                }
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
                not_played();
            }
        }
    });
}

/// Nothing plays any more if the player holds `ended`, ended.
fn stop_ended(ui: Ui, ended: &Key) {
    if ui
        .player
        .vod
        .with_untracked(|p| p.as_ref().is_some_and(|(p, _)| p.ended && &p.key == ended))
    {
        crate::player_keys::stopped(ui);
    }
}

fn toggle_block(ui: Ui) {
    if let Some(target) = ui.vod.with_untracked(|s| {
        s.details
            .as_ref()
            .map(|d| iptv_runtime::vod::target(&d.item.key))
    }) {
        let blocked = !crate::runtime_owner::runtime().parental_explicitly_blocked(&target);
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
        Control::Navigation => browser::activate_navigation(ui),
        Control::Catalog => {
            if let Some(key) = ui.vod.with_untracked(|s| s.focused.clone()) {
                select(ui, key);
            } else {
                empty::run(ui);
            }
        }
        Control::Source => cycle_source(ui),
        Control::Category
            if ui
                .vod
                .with_untracked(|s| !s.is_shelf() && s.details.is_none()) =>
        {
            ui.vod.update(|s| s.enter_catalog());
        }
        Control::Category => cycle_category(ui),
        Control::Sort => cycle_sort(ui),
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
        Control::Search => ui.vod.update(|s| s.open_search()),
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
    if ui
        .vod
        .with_untracked(|s| !s.is_shelf() && s.details.is_none())
    {
        match action {
            Action::MoveLeft | Action::MoveRight => ui
                .vod
                .update(|s| s.browse_horizontal(if action == Action::MoveLeft { -1 } else { 1 })),
            Action::MoveUp | Action::MoveDown => {
                let changed = edit(ui, |s| {
                    s.browse_vertical(if action == Action::MoveUp { -1 } else { 1 })
                });
                if changed {
                    reload(ui, false);
                } else if ui.vod.with_untracked(|s| {
                    s.control == Control::Catalog
                        && s.has_more
                        && s.focused_index()
                            .is_some_and(|at| at + s.columns.max(1) >= s.cards.len())
                }) {
                    reload(ui, true);
                }
            }
            Action::Select => activate(ui),
            _ => {}
        }
        return;
    }
    if ui.vod.with_untracked(|s| s.details.is_some()) {
        match action {
            Action::MoveLeft | Action::MoveRight => ui
                .vod
                .update(|s| s.detail_horizontal(if action == Action::MoveLeft { -1 } else { 1 })),
            Action::MoveUp | Action::MoveDown => ui
                .vod
                .update(|s| s.detail_vertical(if action == Action::MoveUp { -1 } else { 1 })),
            Action::Select => activate(ui),
            _ => {}
        }
        return;
    }
    match action {
        Action::MoveLeft => ui.vod.update(|s| {
            if control == Control::Seasons {
                s.cycle_season(-1)
            } else if control == Control::Episodes {
                s.move_episode(-1)
            } else {
                s.move_control(-1)
            }
        }),
        Action::MoveRight => ui.vod.update(|s| {
            if control == Control::Seasons {
                s.cycle_season(1)
            } else if control == Control::Episodes {
                s.move_episode(1)
            } else {
                s.move_control(1)
            }
        }),
        Action::MoveUp => {
            let at_top = ui.vod.with_untracked(|s| {
                control == Control::Catalog
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
                Control::Catalog if s.cards.is_empty() => s.move_control(-1),
                Control::Catalog if at_top => s.move_control(-1),
                Control::Catalog => s.move_focus(-1),
                Control::Episodes => s.control = Control::Seasons,
                _ => s.move_control(-1),
            })
        }
        Action::MoveDown => {
            ui.vod.update(|s| match control {
                Control::Catalog if s.cards.is_empty() => s.move_control(1),
                Control::Catalog => s.move_focus(1),
                Control::Episodes => s.move_control(1),
                _ => s.move_control(1),
            });
            let more = ui.vod.with_untracked(|s| {
                s.control == Control::Catalog
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
        crate::runtime_owner::runtime().queue_vod_progress(key, parent, progress, incarnation);
    }
    if let Some((parent, incarnation)) = series_complete {
        crate::runtime_owner::runtime().queue_vod_progress(
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
        // The episode's end was saved: its series' card shows it, also
        // when Back from the next episode comes before that one saves.
        let mut details = p.details.clone();
        ui.vod.update(|s| s.watched(&mut details, &p.key, true));
        let autoplay = crate::settings::flag(iptv_settings::keys::VOD_AUTOPLAY_NEXT_EPISODE);
        let (key, season) = (next.key.clone(), next.season);
        // The screen never changes on its own (#187).
        match where_watched(ui, &details.item.key) {
            Watched::FullScreen | Watched::Details if autoplay => {
                autoplay_next(ui, p.key, details, key, season)
            }
            watched => wait_for_next(ui, watched, details, key, season),
        }
    }
}

/// Where the viewer is as the playing episode of `series` ends.
enum Watched {
    /// Its player full screen, nothing covering the video.
    FullScreen,
    /// The series' details, the episode in the preview behind them.
    Details,
    /// A list or shelf, the guide, another title's details, settings…
    Elsewhere,
}

fn where_watched(ui: Ui, series: &Key) -> Watched {
    use crate::overlays::Overlay;
    let details = ui.top_overlay() == Some(Overlay::Vod)
        && ui
            .vod
            .with_untracked(|s| s.details.as_ref().is_some_and(|d| &d.item.key == series));
    if details {
        Watched::Details
    } else if ui.mode.get_untracked() == crate::ViewMode::Fullscreen
        && !ui.overlay_open(Overlay::covers_video)
    {
        Watched::FullScreen
    } else {
        Watched::Elsewhere
    }
}

/// The series' details shown stay, with the progress just watched, and
/// choose the next episode `key` of `season`; `ready` says it waits for
/// Play.
fn select_next(ui: Ui, details: Details, key: &Key, season: u32, ready: bool) {
    ui.vod.update(|s| {
        s.details = Some(details);
        s.episode = Some(key.clone());
        s.season = Some(season);
        s.notice = ready.then(|| crate::i18n::key("Ready for the next episode").into());
    });
}

/// The next episode `key` is not played: full screen, the series'
/// details open on it; on them, playback stops and they choose it; anywhere
/// else playback stops, its series' card and Continue watching resuming it.
fn wait_for_next(ui: Ui, watched: Watched, details: Details, key: Key, season: u32) {
    match watched {
        Watched::FullScreen => ui.vod.update(|s| {
            s.open = true;
            s.show_details(details);
            s.episode = Some(key);
            s.season = Some(season);
            s.notice = Some(crate::i18n::key("Ready for the next episode").into());
        }),
        Watched::Details => {
            crate::player_keys::stopped(ui);
            select_next(ui, details, &key, season, true);
        }
        Watched::Elsewhere => crate::player_keys::stopped(ui),
    }
}

/// The next episode `key` plays on where the viewer watches, full screen
/// or in the preview behind the series' details (chosen there), when it
/// needs no PIN now. A PIN is asked for only when the viewer plays it:
/// else it waits as with autoplay off, as it does when the viewer acted
/// meanwhile. Nothing happens if something else plays by then.
fn autoplay_next(ui: Ui, ended: Key, details: Details, key: Key, season: u32) {
    let series = details.item.key.clone();
    // The viewer acting on the screen meanwhile (`play_key` and Back
    // advance its generation) leaves the next episode waiting.
    let generation = ui.vod.with_untracked(|s| s.generation);
    crate::data::spawn_in_app(async move {
        let owner = crate::runtime_owner::runtime();
        let allowed = owner
            .parental_check(&iptv_runtime::vod::target(&series))
            .await
            .is_ok()
            && owner
                .parental_check(&iptv_runtime::vod::target(&key))
                .await
                .is_ok();
        // Something else plays (Stop included): the viewer's.
        if !ui
            .player
            .vod
            .with_untracked(|p| p.as_ref().is_some_and(|(p, _)| p.key == ended))
        {
            return;
        }
        let allowed = allowed && ui.vod.with_untracked(|s| s.generation) == generation;
        match where_watched(ui, &series) {
            Watched::FullScreen if allowed => {
                play_authorized(ui, details, key, false, true, Some(ended))
            }
            Watched::Details if allowed => {
                select_next(ui, details.clone(), &key, season, false);
                play_authorized(ui, details, key, false, false, Some(ended));
            }
            watched => wait_for_next(ui, watched, details, key, season),
        }
    });
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
    .accessibility_state(move || AccessibilityState {
        selected: ui.vod.with(|s| s.control == control),
        ..Default::default()
    })
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
                    .with(|s| s.control == Control::Catalog && s.focused.as_ref() == Some(&c.key))
            })
        },
        move || {
            if let Some(card) = card() {
                select(ui, card.key);
            }
        },
    )
    .test_id("vod-title")
    .accessibility_state(move || AccessibilityState {
        selected: card().is_some_and(|c| ui.vod.with(|s| s.focused.as_ref() == Some(&c.key))),
        ..Default::default()
    })
}
/// A list opening on its focused row. A list mounted again (Back from
/// details or from the player) runs its scroll-to-focused effect before
/// it is rendered, while its element ref carries nothing, so that first
/// scroll is dropped; the list starts on the row instead (#176).
trait StartingAt {
    fn starting_at(self, at: Option<usize>) -> Self;
}
impl StartingAt for VirtualList {
    fn starting_at(self, at: Option<usize>) -> Self {
        match at {
            Some(at) => self.initial_scroll_index(at),
            None => self,
        }
    }
}
fn shelf_catalog(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    effect(move || {
        if let Some(at) = ui.vod.with(|s| s.focused_index()) {
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
    let titles = virtual_list(
        move || ui.vod.with(|s| s.cards.len()),
        move |at| card_row(ui, size, at),
    )
    .starting_at(ui.vod.with_untracked(|s| s.focused_index()))
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
    .w(move || size.px(300.0))
    .min_h(move || size.px(180.0))
    .mr(move || size.px(12.0))
    .justify_end()
    .test_id("vod-episode")
    .accessibility_state(move || AccessibilityState {
        selected: ep().is_some_and(|e| ui.vod.with(|s| s.episode.as_ref() == Some(&e.key))),
        ..Default::default()
    })
}
fn episodes(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    let chosen = move || {
        ui.vod.with(|s| {
            s.details.as_ref().and_then(|d| {
                d.episodes
                    .iter()
                    .filter(|e| Some(e.season) == s.season)
                    .position(|e| Some(&e.key) == s.episode.as_ref())
            })
        })
    };
    effect(move || {
        if let Some(at) = chosen() {
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
    .starting_at(untracked(chosen))
    .element_ref(&rows)
    .horizontal()
    .estimated_item_size(move || size.px(312.0))
    .flex_1();
    view()
        .flex_1()
        .gap(move || size.px(8.0))
        .child(season)
        .child(empty)
        .child(list)
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
            crate::runtime_owner::runtime()
                .parental_explicitly_blocked(&iptv_runtime::vod::target(&d.item.key))
        })
    });
    if blocked {
        tr("Unblock content")
    } else {
        tr("Block content")
    }
}
fn details(ui: Ui, size: Scale) -> impl IntoElement {
    let page = ui_kit::FocusScroll::new();
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
        .flex_wrap()
        .gap(move || size.px(12.0))
        .child(when(
            move || ui.vod.with(|s| s.retry.is_some() && !s.busy),
            move || {
                button(ui, size, Control::Retry, tr("Retry"), move || retry(ui))
                    .test_id("vod-retry")
            },
        ))
        .child(play_button)
        .child(restart)
        .child(next)
        .child(button(
            ui,
            size,
            Control::Block,
            move || block_label(ui),
            move || toggle_block(ui),
        ));
    let seasons = when(
        move || {
            ui.vod.with(|s| {
                s.details
                    .as_ref()
                    .is_some_and(|d| d.item.key.kind == Kind::Series)
            })
        },
        move || {
            page.control(
                view().h(move || size.px(260.0)).child(episodes(ui, size)),
                move || {
                    ui.vod
                        .with(|s| matches!(s.control, Control::Seasons | Control::Episodes))
                },
            )
        },
    );
    page.view().flex_1().child(
        view()
            .min_h(move || px(window().height.0 * 0.9))
            .gap(move || size.px(12.0))
            .child(browser::hero(ui, size).h(move || px(window().height.0 * 0.75)))
            .child(page.control(actions, move || {
                ui.vod.with(|s| {
                    matches!(
                        s.control,
                        Control::Retry
                            | Control::Play
                            | Control::Restart
                            | Control::Next
                            | Control::Block
                    )
                })
            }))
            .child(seasons),
    )
}
fn status_message(ui: Ui) -> String {
    ui.vod.with(|s| {
        if let Some(e) = &s.error {
            tr(e).to_owned()
        } else if s.busy {
            tr("Loading…").to_owned()
        } else {
            s.notice
                .as_ref()
                .map(|e| tr(e).to_owned())
                .unwrap_or_default()
        }
    })
}
fn detail_screen(ui: Ui, size: Scale) -> View {
    view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .pb_safe_area()
        .px(move || size.px(30.0))
        .pb(move || size.px(24.0))
        .bg(move || rgb(ui.app.theme.with(|t| t.background.window)))
        .test_id("vod-screen")
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::Vod.accessibility_covered(ui)
        })
        .child(details(ui, size))
        .child(
            view()
                .absolute()
                .top(move || size.px(24.0))
                .right(move || size.px(30.0))
                .child(ui_kit::button(
                    ui,
                    size,
                    tr("Close"),
                    || false,
                    move || {
                        back(ui);
                    },
                )),
        )
        .child(ui_kit::body(size, move || status_message(ui)))
}
fn library_screen(ui: Ui, size: Scale) -> View {
    let header = ui_kit::header(
        ui,
        size,
        move || {
            ui.vod.with(|s| match s.shelf {
                Some(Shelf::ContinueWatching) => tr("Continue watching"),
                Some(Shelf::RecentlyAdded) => tr("Recently added"),
                None if s.query.kind == Kind::Series => tr("Shows"),
                None => tr("Movies"),
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
            button(ui, size, Control::Retry, tr("Retry"), move || retry(ui)).test_id("vod-retry")
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
    .otherwise(move || shelf_catalog(ui, size));
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
}
pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            when(
                move || ui.vod.with(|s| !s.is_shelf() && s.details.is_none()),
                move || browser::screen(ui, size),
            )
            .otherwise(move || {
                when(
                    move || ui.vod.with(|s| s.details.is_some()),
                    move || detail_screen(ui, size),
                )
                .otherwise(move || library_screen(ui, size))
            })
        },
    )
}
