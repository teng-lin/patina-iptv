//! The search screen: a query field, recent searches, and results in two
//! tabs (channels, programmes). State and rules live in
//! `iptv_search::SearchState`; this module builds the indexes, runs the
//! searches (debounced), maps remote keys and taps to search actions and
//! carries out what the state answers (play, show details, close).
//!
//! Choosing a result plays it full screen. With `search.stay_on_search`
//! the screen waits suspended and leaving full screen brings it back with
//! its query and focus (`SearchState::resume`); otherwise it closes.
//! Ranking follows `search.favorites_first` and
//! `search.include_past_without_catchup`; with `search.show_history` off
//! the screen opens without the recent searches (they are still kept).
//! Programmes are searched over every day the store keeps
//! (`programme_index.rs`), not only the guide days held in memory.
//! Wording is iptv-rs's own.
//!
//! Multiview's Search and add opens the screen to pick (`pick.rs`): over
//! the grid, without leaving full screen; a result adds its channel as a
//! new tile and the screen closes, Back closes it without one.

use std::rc::Rc;
use std::time::{Duration, Instant};

use iptv_core::guide::{Airing, Schedule};
use iptv_core::menu::Origin;
use iptv_epg::UnixTime;
use iptv_guide::EpgSchedule;
use iptv_nav::Action;
use iptv_search::{
    ChannelSearchOptions, Focus, Phase, ProgrammeHit, ProgrammeSearchOptions, ResultId,
    SearchAction, SearchEffect, SearchHistory, SearchState, Tab,
};
use iptv_settings::{Settings, Target, keys};
use nsh_base::input;
use nsh_core::prelude::*;

mod pick;
mod programme_index;
mod view;

pub use pick::open_to_pick;
pub use view::layer;

use crate::{Playback, Ui, ViewMode, channel_name, clock};

/// Typing pause before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(200);
const LIMIT: usize = 200;
/// Where builds before the store kept the recent searches, as plaintext.
const LEGACY_HISTORY_FILE: &str = "search-history.json";

/// Whether programme search can run on the current guide data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Programmes {
    /// No EPG behind the schedule (the synthetic guide).
    Unavailable,
    Building,
    Ready,
}

/// The open search screen (`Ui::search`).
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub state: SearchState,
    /// A result is playing full screen; the screen returns when it ends.
    pub suspended: bool,
    pub programmes: Programmes,
    /// Picking a channel for multiview (Search and add).
    pub pick: bool,
}

// --- Glue (pure, unit-tested) ------------------------------------------------

/// What choosing a result does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// Channels and programmes on now: the channel, live.
    Live(usize),
    /// A past programme still in the archive: replay from its start.
    CatchUp {
        channel: usize,
        start: UnixTime,
    },
    /// Upcoming programmes (and past ones without replay): their details.
    Details {
        channel: usize,
        start: UnixTime,
    },
    Nothing,
}

/// The `search.*` settings the screen follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Options {
    favourites_first: bool,
    include_past_without_catchup: bool,
    /// Come back to the results after watching one.
    stay: bool,
    show_history: bool,
}

fn options(s: &Settings) -> Options {
    let at = &Target::Global;
    Options {
        favourites_first: s.bool(keys::SEARCH_FAVORITES_FIRST, at),
        include_past_without_catchup: s.bool(keys::SEARCH_INCLUDE_PAST_WITHOUT_CATCHUP, at),
        stay: s.bool(keys::SEARCH_STAY, at),
        show_history: s.bool(keys::SEARCH_SHOW_HISTORY, at),
    }
}

/// The recent searches to save: with the history hidden the screen
/// started empty, so its queries go on top of the saved ones.
fn to_save(shown: bool, saved: SearchHistory, session: &SearchHistory) -> Vec<String> {
    if shown {
        return session.to_list();
    }
    let mut all = saved;
    for q in session.entries().iter().rev() {
        all.push(q);
    }
    all.to_list()
}

fn outcome(id: ResultId, hit: Option<&ProgrammeHit>) -> Outcome {
    match (id, hit) {
        (ResultId::Channel(c), _) => Outcome::Live(c),
        (ResultId::Programme { .. }, None) => Outcome::Nothing,
        (ResultId::Programme { channel, start }, Some(h)) => match h.airing {
            Airing::Live => Outcome::Live(channel),
            Airing::Ended if h.catchup => Outcome::CatchUp { channel, start },
            Airing::Ended | Airing::Upcoming => Outcome::Details { channel, start },
        },
    }
}

/// The remote's action on the search screen.
fn key_action(state: &SearchState, action: Action) -> Option<SearchAction> {
    Some(match action {
        Action::MoveUp => SearchAction::Up,
        Action::MoveDown => SearchAction::Down,
        Action::MoveLeft => SearchAction::Left,
        Action::MoveRight => SearchAction::Right,
        // OK on the field (keyboard down) brings the keyboard back; with
        // the keyboard up it submits.
        Action::Select
            if *state.focus() == Focus::Input
                && (state.phase() == Phase::Idle || !state.keyboard_visible()) =>
        {
            SearchAction::ShowKeyboard
        }
        Action::Select => SearchAction::Ok,
        Action::Back => SearchAction::Back,
        // Number keys type into the query (channel numbers are searchable).
        Action::Digit(d) => SearchAction::Type(format!("{}{d}", state.query())),
        _ => return None,
    })
}

/// The guide minute (from the schedule's `epoch`) inside a programme
/// starting at `start`: rounded up, so it never falls in the programme before.
fn guide_minute(start: UnixTime, epoch: UnixTime) -> i32 {
    (start - epoch + 59).div_euclid(60) as i32
}

/// The guide minute a time is displayed at (as the guide rounds).
fn display_minute(t: UnixTime, epoch: UnixTime) -> i32 {
    (t - epoch).div_euclid(60) as i32
}

/// The airing and replay marker of a programme result.
fn badge(hit: &ProgrammeHit) -> String {
    let when = match hit.airing {
        Airing::Live => "On now",
        Airing::Upcoming => "Later",
        Airing::Ended => "Earlier",
    };
    match (hit.airing, hit.catchup) {
        (_, true) => format!("{when} · ↺ replay"),
        (Airing::Ended, false) => format!("{when} · no replay"),
        _ => when.to_owned(),
    }
}

fn tab_label(tab: Tab, count: usize, programmes: Programmes) -> String {
    // A full list may have been cut at the limit: not a total.
    let count = if count >= LIMIT {
        format!("{LIMIT}+")
    } else {
        count.to_string()
    };
    match (tab, programmes) {
        (Tab::Channels, _) => format!("Channels · {count}"),
        (Tab::Programs, Programmes::Unavailable) => "Programmes · no guide data".to_owned(),
        (Tab::Programs, Programmes::Building) => "Programmes · …".to_owned(),
        (Tab::Programs, Programmes::Ready) => format!("Programmes · {count}"),
    }
}

// --- Data ---------------------------------------------------------------------

fn epg(schedule: &dyn Schedule) -> Option<&EpgSchedule> {
    schedule.as_any()?.downcast_ref::<EpgSchedule>()
}

/// Unix time of guide minute 0, for converting result times to guide
/// minutes (the same epoch the guide and its clock labels use).
fn guide_epoch() -> UnixTime {
    crate::schedule().epoch_unix()
}

fn load_history() -> SearchHistory {
    let list = crate::runtime().search_history();
    SearchHistory::from_list(list, SearchHistory::DEFAULT_CAPACITY)
}

/// App start: reads the recent searches from the store, after moving those
/// an older build kept in a plaintext file into it (once; the file is
/// then deleted). A screen opened before the answer started empty and is
/// given them unless a search was made meanwhile.
pub(crate) fn load_saved_history(ui: Ui) {
    crate::runtime().clear_loaded_search_history();
    let legacy = crate::data::data_dir().join(LEGACY_HISTORY_FILE);
    let loaded = crate::runtime::commands::load_search_history(legacy);
    crate::data::spawn_in_app(async move {
        let list = match loaded.await {
            Ok(list) => list,
            Err(error) => {
                log::warn!("iptv search: cannot read the recent searches: {error}");
                return;
            }
        };
        if options(&crate::settings::get()).show_history {
            ui.search.update(|screen| {
                if let Some(screen) = screen
                    && screen.state.history().is_empty()
                {
                    *screen.state.history_mut() =
                        SearchHistory::from_list(&list, SearchHistory::DEFAULT_CAPACITY);
                }
            });
        }
        crate::runtime().install_search_history(list);
    });
}

/// Moves the recent searches of a legacy file into an empty store table,
/// then deletes the file. An unreadable file is deleted too: it held only
/// recent searches, and plaintext must not stay behind.
fn save_history(ui: Ui) {
    let Some(session) = ui
        .search
        .with_untracked(|s| s.as_ref().map(|s| s.state.history().clone()))
    else {
        return;
    };
    let shown = options(&crate::settings::get()).show_history;
    let list = to_save(shown, load_history(), &session);
    if crate::runtime().search_history_matches(&list) {
        return;
    }
    crate::runtime().install_search_history(list.clone());
    let saved = crate::runtime::commands::save_search_history(list);
    crate::data::spawn_in_app(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv search: could not save the recent searches: {error}");
        }
    });
}

fn set_programmes(ui: Ui, status: Programmes) {
    ui.search.update(|s| {
        if let Some(s) = s {
            s.programmes = status;
        }
    });
}

/// Builds the indexes for the current guide data when they are stale:
/// channels at once, programmes a moment later (about 0.1 s for a large
/// EPG), so the screen shows first.
fn prepare(ui: Ui) {
    let version = ui.data_version.get_untracked();
    let schedule = crate::schedule();
    crate::runtime().prepare_search_channels(version, &*schedule);
    if epg(&*schedule).is_none()
        && schedule
            .as_any()
            .and_then(|s| s.downcast_ref::<iptv_guide::combined::CombinedSchedule>())
            .is_none()
    {
        set_programmes(ui, Programmes::Unavailable);
        return;
    }
    if crate::runtime().has_programme_index() {
        set_programmes(ui, Programmes::Ready);
        return;
    }
    set_programmes(ui, Programmes::Building);
    // Under the app's owner: a timer dies with its owner, and multiview's
    // Search and add opens the screen from a menu item that goes as it
    // closes. A timer that finds the index built does nothing.
    crate::data::spawn_in_app(async move {
        timer(Duration::from_millis(30)).await;
        let wanted = crate::runtime().begin_programme_index(version);
        let schedule = crate::schedule();
        if !wanted {
            return;
        }
        let t = Instant::now();
        programme_index::build(&*schedule, schedule.channel_count(), move |index| {
            let current = crate::runtime().finish_programme_index(version);
            if !current || ui.search.with_untracked(Option::is_none) {
                return;
            }
            log::info!(
                "iptv search: indexed {} programmes in {} ms",
                index.programme_count(),
                t.elapsed().as_millis()
            );
            crate::runtime().install_programme_index(index);
            set_programmes(ui, Programmes::Ready);
            search_again(ui);
        });
    });
}

/// Runs both searches for `query` now and hands the results to the state.
fn run_search(ui: Ui, query: &str) {
    let favourites = ui.favorites.get_untracked();
    let schedule = crate::schedule();
    let days = |c: usize| schedule.catchup_days(c);
    let visible = |c: usize| !schedule.channel_hidden(c);
    let options = options(&crate::settings::get());
    let mut channels = crate::runtime().search_channels(
        query,
        &favourites,
        ChannelSearchOptions {
            favourites_first: options.favourites_first,
            limit: LIMIT,
        },
    );
    let opts = ProgrammeSearchOptions {
        visible: Some(&visible),
        include_past_without_catchup: options.include_past_without_catchup,
        catchup_days: Some(&days),
        favourites: Some(&favourites),
        favourites_first: options.favourites_first,
        limit: LIMIT,
        ..Default::default()
    };
    let mut programmes = crate::runtime().search_programmes(query, crate::time::wall_unix(), opts);
    channels.retain(|h| !schedule.channel_hidden(h.channel));
    programmes.retain(|h| !schedule.channel_hidden(h.channel));
    ui.search.update(|s| {
        if let Some(s) = s {
            s.state.set_results(query, channels, programmes);
        }
    });
}

/// Searches the current query again (new data, programme index ready).
fn search_again(ui: Ui) {
    let query = ui.search.with_untracked(|s| {
        s.as_ref()
            .filter(|s| s.state.phase() != Phase::Idle)
            .map(|s| s.state.query().to_owned())
    });
    if let Some(q) = query {
        run_search(ui, &q);
    }
}

/// Searches `query` after [`DEBOUNCE`] unless a newer search came. The
/// wait runs under the app's owner, not the caller's: a tap on a recent
/// search hides the recent searches, and a timer owned by the tapped row
/// would go with it (the search never ran).
fn debounce(ui: Ui, query: String) {
    let ticket = crate::runtime().begin_search();
    crate::data::spawn_in_app(async move {
        timer(DEBOUNCE).await;
        if crate::runtime().search_is_current(ticket) {
            run_search(ui, &query);
        }
    });
}

// --- Actions -------------------------------------------------------------------

/// Opens the screen (guide menu, control strip). A screen suspended by
/// playback comes back as it was.
pub fn open(ui: Ui) {
    let suspended = ui
        .search
        .with_untracked(|s| s.as_ref().map(|s| s.suspended));
    match suspended {
        Some(true) => resume(ui),
        Some(false) => {}
        None => {
            ui.search.set(Some(Screen {
                state: SearchState::new(if options(&crate::settings::get()).show_history {
                    load_history()
                } else {
                    SearchHistory::default()
                }),
                suspended: false,
                programmes: Programmes::Building,
                pick: false,
            }));
            prepare(ui);
        }
    }
    if ui.mode.get_untracked() == ViewMode::Fullscreen {
        ui.mode.set(crate::settings::browse_mode());
    }
}

pub fn close(ui: Ui) {
    let picking = ui
        .search
        .with_untracked(|s| s.as_ref().is_some_and(|s| s.pick));
    if picking {
        crate::multiview::list_closed(ui);
    }
    save_history(ui);
    crate::runtime().release_programme_index();
    ui.search.set(None);
    dismiss_keyboard();
}

/// The screen is shown and takes the remote.
pub fn is_open(ui: Ui) -> bool {
    ui.search.with(|s| s.as_ref().is_some_and(|s| !s.suspended))
}

/// A remote action while the screen is shown.
pub fn key(ui: Ui, action: Action) {
    let mapped = ui
        .search
        .with_untracked(|s| s.as_ref().and_then(|s| key_action(&s.state, action)));
    if let Some(a) = mapped {
        apply(ui, a);
    }
}

pub fn apply(ui: Ui, action: SearchAction) {
    let mut effect = SearchEffect::None;
    ui.search.update(|s| {
        if let Some(s) = s {
            effect = s.state.apply(action);
        }
    });
    match effect {
        SearchEffect::None => {}
        SearchEffect::Search(q) => debounce(ui, q),
        SearchEffect::Play(id) => choose(ui, id),
        SearchEffect::Close => close(ui),
    }
}

fn resume(ui: Ui) {
    let mut effect = SearchEffect::None;
    ui.search.update(|s| {
        if let Some(s) = s {
            s.suspended = false;
            effect = s.state.resume();
        }
    });
    prepare(ui);
    if let SearchEffect::Search(q) = effect {
        run_search(ui, &q);
    }
}

fn choose(ui: Ui, id: ResultId) {
    if pick::choose(ui, id) {
        return;
    }
    let hit = ui.search.with_untracked(|s| {
        s.as_ref().and_then(|s| {
            s.state
                .programmes()
                .iter()
                .find(|h| {
                    ResultId::Programme {
                        channel: h.channel,
                        start: h.start,
                    } == id
                })
                .cloned()
        })
    });
    match outcome(id, hit.as_ref()) {
        Outcome::Live(channel) => watch(ui, channel, Playback::Live),
        Outcome::CatchUp { channel, start } => {
            let start = guide_minute(start, guide_epoch());
            watch(ui, channel, Playback::CatchUp { start, offset: 0 });
        }
        Outcome::Details { channel, start } => ui.description.set(Some(Origin {
            channel,
            program_start: guide_minute(start, guide_epoch()),
        })),
        Outcome::Nothing => {}
    }
}

/// Plays full screen; the screen waits until playback leaves full screen
/// (`search.stay_on_search`), else closes.
fn watch(ui: Ui, channel: usize, playback: Playback) {
    save_history(ui);
    let now = ui.focus.with(|f| f.now);
    crate::lists::tap(ui, channel, now);
    ui.play(channel, playback);
    // Full screen first: the resume effect ignores a suspended screen in full screen.
    ui.mode.set(ViewMode::Fullscreen);
    ui.show_playing();
    if !options(&crate::settings::get()).stay {
        close(ui);
        return;
    }
    ui.search.update(|s| {
        if let Some(s) = s {
            s.suspended = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_search::MatchQuality;

    fn hit(airing: Airing, catchup: bool) -> ProgrammeHit {
        ProgrammeHit {
            channel: 3,
            start: 1_000,
            end: 2_800,
            title: "Synthetic Show".into(),
            airing,
            catchup,
            score: MatchQuality::Word,
        }
    }

    const P: ResultId = ResultId::Programme {
        channel: 3,
        start: 1_000,
    };

    #[test]
    fn options_follow_the_search_settings() {
        let mut s = Settings::new();
        let defaults = Options {
            favourites_first: true,
            include_past_without_catchup: false,
            stay: false,
            show_history: true,
        };
        assert_eq!(options(&s), defaults);
        s.set("search.favorites_first", iptv_settings::Value::Bool(false))
            .unwrap();
        s.set(
            "search.include_past_without_catchup",
            iptv_settings::Value::Bool(true),
        )
        .unwrap();
        s.set("search.stay_on_search", iptv_settings::Value::Bool(true))
            .unwrap();
        s.set("search.show_history", iptv_settings::Value::Bool(false))
            .unwrap();
        assert_eq!(
            options(&s),
            Options {
                favourites_first: false,
                include_past_without_catchup: true,
                stay: true,
                show_history: false,
            }
        );
    }

    #[test]
    fn hidden_history_still_keeps_new_searches() {
        let saved = SearchHistory::from_list(["news", "films"], 20);
        let mut session = SearchHistory::default();
        session.push("sport");
        session.push("weather");
        assert_eq!(
            to_save(false, saved.clone(), &session),
            ["weather", "sport", "news", "films"]
        );
        // Shown: the screen started from the saved list, its list is it.
        assert_eq!(to_save(true, saved, &session), ["weather", "sport"]);
    }

    #[test]
    fn choosing_a_result() {
        assert_eq!(outcome(ResultId::Channel(7), None), Outcome::Live(7));
        assert_eq!(outcome(P, Some(&hit(Airing::Live, true))), Outcome::Live(3));
        assert_eq!(
            outcome(P, Some(&hit(Airing::Ended, true))),
            Outcome::CatchUp {
                channel: 3,
                start: 1_000
            }
        );
        let details = Outcome::Details {
            channel: 3,
            start: 1_000,
        };
        assert_eq!(outcome(P, Some(&hit(Airing::Upcoming, false))), details);
        assert_eq!(outcome(P, Some(&hit(Airing::Ended, false))), details);
        assert_eq!(outcome(P, None), Outcome::Nothing);
    }

    #[test]
    fn remote_keys() {
        let mut s = SearchState::new(SearchHistory::default());
        let k = |s: &SearchState, a| key_action(s, a);
        // Idle field: OK (re)opens the keyboard; menu keys do nothing here.
        assert_eq!(k(&s, Action::Select), Some(SearchAction::ShowKeyboard));
        assert_eq!(k(&s, Action::ShowMenu), None);
        assert_eq!(k(&s, Action::MoveLeft), Some(SearchAction::Left));
        assert_eq!(k(&s, Action::Back), Some(SearchAction::Back));
        assert_eq!(
            k(&s, Action::Digit(4)),
            Some(SearchAction::Type("4".into()))
        );
        s.apply(SearchAction::Type("10".into()));
        assert_eq!(
            k(&s, Action::Digit(5)),
            Some(SearchAction::Type("105".into()))
        );
        // Keyboard up with a query: OK submits; keyboard down: OK brings it back.
        assert_eq!(k(&s, Action::Select), Some(SearchAction::Ok));
        s.apply(SearchAction::KeyboardHidden);
        assert_eq!(k(&s, Action::Select), Some(SearchAction::ShowKeyboard));
        // On a result OK is OK.
        s.set_results(
            "10",
            vec![iptv_search::ChannelHit {
                channel: 9,
                score: MatchQuality::Exact,
            }],
            vec![],
        );
        s.apply(SearchAction::Down);
        s.apply(SearchAction::Down);
        assert_eq!(s.focus(), &Focus::Result(ResultId::Channel(9)));
        assert_eq!(k(&s, Action::Select), Some(SearchAction::Ok));
    }

    #[test]
    fn times_convert_to_guide_minutes() {
        let epoch = 1_800_000_000;
        assert_eq!(guide_minute(epoch, epoch), 0);
        assert_eq!(guide_minute(epoch + 90, epoch), 2, "inside the programme");
        assert_eq!(guide_minute(epoch - 3_600, epoch), -60);
        assert_eq!(guide_minute(epoch - 3_590, epoch), -59);
        assert_eq!(display_minute(epoch + 90, epoch), 1);
        assert_eq!(display_minute(epoch - 30, epoch), -1);
    }

    #[test]
    fn labels() {
        assert_eq!(badge(&hit(Airing::Live, false)), "On now");
        assert_eq!(badge(&hit(Airing::Live, true)), "On now · ↺ replay");
        assert_eq!(badge(&hit(Airing::Ended, true)), "Earlier · ↺ replay");
        assert_eq!(badge(&hit(Airing::Ended, false)), "Earlier · no replay");
        assert_eq!(badge(&hit(Airing::Upcoming, false)), "Later");
        assert_eq!(
            tab_label(Tab::Channels, 3, Programmes::Unavailable),
            "Channels · 3"
        );
        assert_eq!(
            tab_label(Tab::Programs, 0, Programmes::Unavailable),
            "Programmes · no guide data"
        );
        assert_eq!(
            tab_label(Tab::Programs, 0, Programmes::Building),
            "Programmes · …"
        );
        assert_eq!(
            tab_label(Tab::Programs, 12, Programmes::Ready),
            "Programmes · 12"
        );
        // A list cut at the limit is not a total.
        assert_eq!(
            tab_label(Tab::Programs, LIMIT, Programmes::Ready),
            format!("Programmes · {LIMIT}+")
        );
    }
}
