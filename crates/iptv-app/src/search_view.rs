//! The search screen: a query field, recent searches, and results in two
//! tabs (channels, programs). State and rules live in
//! `iptv_presentation::search::SearchState`; this module builds the indexes, runs the
//! searches (debounced), maps remote keys and taps to search actions and
//! carries out what the state answers (play, show details, close).
//!
//! Choosing a result plays it full screen; the screen waits suspended and
//! leaving full screen (Back) brings it back with its query and focus
//! (`SearchState::resume`). Favorite channels rank first. The results'
//! filter row decides whether finished programs without replay are
//! listed. These replaced `search.stay_on_search`, `search.favorites_first`
//! and `search.include_past_without_catchup` (#282), whose stored values
//! are ignored. With `search.show_history` off the screen opens without
//! the recent searches (they are still kept).
//! Programs are searched over every day the store keeps
//! (`program_index.rs`), not only the guide days held in memory.
//! Wording is iptv-rs's own.
//!
//! Multiview's Search and add opens the screen to pick (`pick.rs`): over
//! the grid, without leaving full screen; a result adds its channel as a
//! new tile and the screen closes, Back closes it without one.

use crate::i18n::{tr, trf};
use nsh_core::time::Instant;
use std::time::Duration;

use iptv_core::guide::{Airing, Schedule};
use iptv_epg::UnixTime;
use iptv_guide::EpgSchedule;
use iptv_nav::Action;
use iptv_presentation::menu::Origin;
use iptv_presentation::search::{
    Focus, Phase, ResultId, ResultPolicy, ResultSelection as Outcome, SearchAction, SearchEffect,
    SearchState, Tab, select_result,
};
use iptv_search::{ChannelSearchOptions, ProgramHit, ProgramSearchOptions, SearchHistory};
use iptv_settings::{Settings, Target, keys};
use nsh_base::input;
use nsh_core::prelude::*;

mod pick;
mod program_index;
mod view;

pub use pick::{open_to_pick, picking};
pub use view::layer;

use crate::{Playback, Ui, ViewMode, channel_name, clock};

/// Typing pause before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(200);
const LIMIT: usize = 200;
/// Where builds before the store kept the recent searches, as plaintext.
const LEGACY_HISTORY_FILE: &str = "search-history.json";

/// Whether program search can run on the current guide data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramSearch {
    /// No readable EPG behind the current schedule.
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
    pub programs: ProgramSearch,
    /// Picking a channel for multiview (Search and add).
    pub pick: bool,
}

// --- Glue (pure, unit-tested) ------------------------------------------------

/// The `search.*` settings the screen follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Options {
    show_history: bool,
}

fn options(s: &Settings) -> Options {
    Options {
        show_history: s.bool(keys::SEARCH_SHOW_HISTORY, &Target::Global),
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

/// Both normal playback and multiview consume the current, visible semantic row.
fn resolve_result_channel(
    owner: &crate::runtime::Runtime,
    identity: &iptv_search::ChannelId,
    schedule: &dyn Schedule,
) -> Option<usize> {
    let channel = owner.resolve_search_channel(identity, schedule)?;
    (!schedule.channel_hidden(channel)).then_some(channel)
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

/// The guide minute (from the schedule's `epoch`) inside a program
/// starting at `start`: rounded up, so it never falls in the program before.
fn guide_minute(start: UnixTime, epoch: UnixTime) -> i32 {
    (start - epoch + 59).div_euclid(60) as i32
}

/// The guide minute a time is displayed at (as the guide rounds).
fn display_minute(t: UnixTime, epoch: UnixTime) -> i32 {
    (t - epoch).div_euclid(60) as i32
}

/// The airing and replay marker of a program result.
fn badge(hit: &ProgramHit) -> String {
    let when = match hit.airing {
        Airing::Live => tr("On now"),
        Airing::Upcoming => tr("Later"),
        Airing::Ended => tr("Earlier"),
    };
    match (hit.airing, hit.catchup) {
        (_, true) => trf("{when} · ↺ replay", &[("when", when.into())]),
        (Airing::Ended, false) => trf("{when} · no replay", &[("when", when.into())]),
        _ => when.to_owned(),
    }
}

fn tab_label(tab: Tab, count: usize, programs: ProgramSearch) -> String {
    // A full list may have been cut at the limit: not a total.
    let count = if count >= LIMIT {
        format!("{LIMIT}+")
    } else {
        count.to_string()
    };
    match (tab, programs) {
        (Tab::Channels, _) => trf("Channels · {count}", &[("count", (&count).into())]),
        (Tab::Programs, ProgramSearch::Unavailable) => tr("Programs · no guide data").to_owned(),
        (Tab::Programs, ProgramSearch::Building) => tr("Programs · …").to_owned(),
        (Tab::Programs, ProgramSearch::Ready) => {
            trf("Programs · {count}", &[("count", (&count).into())])
        }
    }
}

// --- Data ---------------------------------------------------------------------

fn epg(schedule: &dyn Schedule) -> Option<&EpgSchedule> {
    schedule.as_any()?.downcast_ref::<EpgSchedule>()
}

/// Unix time of guide minute 0, for converting result times to guide
/// minutes (the same epoch the guide and its clock labels use).
fn guide_epoch() -> UnixTime {
    crate::runtime_owner::schedule().epoch_unix()
}

fn load_history() -> SearchHistory {
    let list = crate::runtime_owner::runtime().search_history();
    SearchHistory::from_list(list, SearchHistory::DEFAULT_CAPACITY)
}

/// App start: reads the recent searches from the store, after moving those
/// an older build kept in a plaintext file into it (once; the file is
/// then deleted). A screen opened before the answer started empty and is
/// given them unless a search was made meanwhile.
pub(crate) fn load_saved_history(ui: Ui) {
    crate::data::spawn_in_app(refresh_saved_history(ui));
}

pub(crate) fn refresh_saved_history(ui: Ui) -> impl std::future::Future<Output = ()> {
    let revision = ui.search_history_revision.get_untracked();
    crate::runtime_owner::runtime().clear_loaded_search_history();
    let legacy = crate::data::data_dir().join(LEGACY_HISTORY_FILE);
    let loaded =
        crate::runtime::commands::load_search_history(&crate::runtime_owner::runtime(), legacy);
    async move {
        let list = match loaded.await {
            Ok(list) => list,
            Err(error) => {
                log::warn!("iptv search: cannot read the recent searches: {error}");
                return;
            }
        };
        if ui.search_history_revision.get_untracked() != revision {
            return;
        }
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
        crate::runtime_owner::runtime().install_search_history(list);
    }
}

/// Clears saved searches immediately and invalidates pending startup loads.
pub(crate) fn clear_history(ui: Ui) {
    ui.search_history_revision.update(|revision| *revision += 1);
    let saved = crate::runtime::commands::clear_search_history(
        &crate::runtime_owner::runtime(),
        crate::data::data_dir().join(LEGACY_HISTORY_FILE),
    );
    crate::runtime_owner::runtime().install_search_history(Vec::new());
    ui.search.update(|screen| {
        if let Some(screen) = screen {
            *screen.state.history_mut() = SearchHistory::default();
        }
    });
    crate::data::spawn_in_app(async move {
        if saved.await.is_err() {
            crate::settings_view::notify(tr(
                "Recent channel changes could not be saved; try again",
            ));
        }
    });
}

fn save_history(ui: Ui) {
    let owner = crate::runtime_owner::runtime();
    // An initial or recovery read can fail independently of later writes.
    // Keep the saved entries if the screen could not load them.
    if !owner.search_history_loaded() {
        return;
    }
    let Some(session) = ui
        .search
        .with_untracked(|s| s.as_ref().map(|s| s.state.history().clone()))
    else {
        return;
    };
    let shown = options(&crate::settings::get()).show_history;
    let list = to_save(shown, load_history(), &session);
    if owner.search_history_matches(&list) {
        return;
    }
    owner.install_search_history(list.clone());
    let saved = crate::runtime::commands::save_search_history(&owner, list);
    crate::data::spawn_in_app(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv search: could not save the recent searches: {error}");
        }
    });
}

fn set_programs(ui: Ui, status: ProgramSearch) {
    ui.search.update(|s| {
        if let Some(s) = s {
            s.programs = status;
        }
    });
}

/// Builds the indexes for the current guide data when they are stale:
/// channels at once, programs a moment later (about 0.1 s for a large
/// EPG), so the screen shows first.
fn prepare(ui: Ui) {
    let version = ui.app.data_version.get_untracked();
    let schedule = crate::runtime_owner::schedule();
    crate::runtime_owner::runtime().prepare_search_channels(version, &*schedule);
    if epg(&*schedule).is_none()
        && schedule
            .as_any()
            .and_then(|s| s.downcast_ref::<iptv_guide::combined::CombinedSchedule>())
            .is_none()
    {
        set_programs(ui, ProgramSearch::Unavailable);
        return;
    }
    if crate::runtime_owner::runtime().has_program_index() {
        set_programs(ui, ProgramSearch::Ready);
        return;
    }
    set_programs(ui, ProgramSearch::Building);
    // Under the app's owner: a timer dies with its owner, and multiview's
    // Search and add opens the screen from a menu item that goes as it
    // closes. A timer that finds the index built does nothing.
    crate::data::spawn_in_app(async move {
        timer(Duration::from_millis(30)).await;
        let wanted = crate::runtime_owner::runtime().begin_program_index(version);
        let schedule = crate::runtime_owner::schedule();
        if !wanted {
            return;
        }
        let t = Instant::now();
        program_index::build(&*schedule, schedule.channel_count(), move |index| {
            let current = crate::runtime_owner::runtime().finish_program_index(version);
            if !current || ui.search.with_untracked(Option::is_none) {
                return;
            }
            let index = match index {
                Ok(index) => index,
                Err(error) => {
                    log::warn!("iptv search: guide titles: {error}");
                    set_programs(ui, ProgramSearch::Unavailable);
                    search_again(ui);
                    return;
                }
            };
            log::info!(
                "iptv search: indexed {} programs in {} ms",
                index.program_count(),
                t.elapsed().as_millis()
            );
            crate::runtime_owner::runtime().install_program_index(index);
            set_programs(ui, ProgramSearch::Ready);
            search_again(ui);
        });
    });
}

/// Runs both searches for `query` now and hands the results to the state.
fn run_search(ui: Ui, query: &str) {
    let favorites = ui.app.favorites.get_untracked();
    let schedule = crate::runtime_owner::schedule();
    let days = |c: usize| schedule.catchup_days(c);
    let visible = |c: usize| !schedule.channel_hidden(c);
    let past_without_replay = ui
        .search
        .with_untracked(|s| s.as_ref().is_some_and(|s| s.state.past_without_replay()));
    let mut channels = crate::runtime_owner::runtime().search_channels(
        query,
        &favorites,
        ChannelSearchOptions {
            favorites_first: true,
            limit: LIMIT,
        },
    );
    let opts = ProgramSearchOptions {
        visible: Some(&visible),
        include_past_without_catchup: past_without_replay,
        catchup_days: Some(&days),
        // As far back as a guide keeps finished programs (a catch-up
        // archive of up to 14 days, #282); pruning bounds what is there.
        look_back: i64::from(iptv_runtime::stored::stored_past_days()) * 86_400,
        favorites: Some(&favorites),
        favorites_first: true,
        limit: LIMIT,
        ..Default::default()
    };
    let mut programs = crate::runtime_owner::runtime().search_programs(
        query,
        crate::clock::wall_unix(),
        &*schedule,
        opts,
    );
    channels.retain_mut(|hit| {
        let Some(channel) =
            resolve_result_channel(&crate::runtime_owner::runtime(), &hit.identity, &*schedule)
        else {
            return false;
        };
        hit.channel = channel;
        true
    });
    programs.retain_mut(|hit| {
        let Some(channel) =
            resolve_result_channel(&crate::runtime_owner::runtime(), &hit.identity, &*schedule)
        else {
            return false;
        };
        hit.channel = channel;
        true
    });
    ui.search.update(|s| {
        if let Some(s) = s {
            s.state.set_results(query, channels, programs);
        }
    });
}

/// Searches the current query again (new data, program index ready).
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
    let ticket = crate::runtime_owner::runtime().begin_search();
    crate::data::spawn_in_app(async move {
        timer(DEBOUNCE).await;
        if crate::runtime_owner::runtime().search_is_current(ticket) {
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
                })
                .with_filter(),
                suspended: false,
                programs: ProgramSearch::Building,
                pick: false,
            }));
            prepare(ui);
        }
    }
    if ui.mode.get_untracked() == ViewMode::Fullscreen {
        ui.mode.apply(crate::mode::Action::ReturnToBrowse);
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
    crate::runtime_owner::runtime().release_program_index();
    ui.search.set(None);
    crate::input::dismiss_keyboard();
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
    if pick::choose(ui, &id) {
        return;
    }
    let hit = ui.search.with_untracked(|s| {
        s.as_ref().and_then(|s| {
            s.state
                .programs()
                .iter()
                .find(|h| {
                    ResultId::Program {
                        channel: h.identity.clone(),
                        start: h.start,
                    } == id
                })
                .cloned()
        })
    });
    let schedule = crate::runtime_owner::schedule();
    let channel =
        resolve_result_channel(&crate::runtime_owner::runtime(), id.channel(), &*schedule);
    match select_result(&id, hit.as_ref(), channel, ResultPolicy::Guide) {
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

/// Plays full screen; the screen waits until playback leaves full screen.
fn watch(ui: Ui, channel: usize, playback: Playback) {
    save_history(ui);
    let now = ui.focus.with(|f| f.now);
    crate::lists::tap(ui, channel, now);
    ui.play(channel, playback);
    // Full screen first: the resume effect ignores a suspended screen in full screen.
    ui.mode.apply(crate::mode::Action::EnterFullscreen);
    ui.show_playing();
    ui.search.update(|s| {
        if let Some(s) = s {
            s.suspended = true;
        }
    });
}

#[cfg(test)]
fn outcome(id: &ResultId, hit: Option<&ProgramHit>, channel: Option<usize>) -> Outcome {
    select_result(id, hit, channel, ResultPolicy::Guide)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_search::MatchQuality;

    fn hit(airing: Airing, catchup: bool) -> ProgramHit {
        ProgramHit {
            identity: iptv_search::ChannelId::new(None, "synthetic-3"),
            channel: 3,
            start: 1_000,
            end: 2_800,
            title: "Synthetic Show".into(),
            airing,
            catchup,
            score: MatchQuality::Word,
        }
    }

    fn p() -> ResultId {
        ResultId::Program {
            channel: iptv_search::ChannelId::new(None, "synthetic-3"),
            start: 1_000,
        }
    }

    #[test]
    fn options_follow_the_search_settings() {
        let mut s = Settings::new();
        assert_eq!(options(&s), Options { show_history: true });
        // The retired search settings change nothing (#282).
        for key in [
            "search.favorites_first",
            "search.include_past_without_catchup",
            "search.stay_on_search",
        ] {
            let stored = !s.bool(
                iptv_settings::keys::BoolKey::try_new(key).unwrap(),
                &Target::Global,
            );
            s.set(key, iptv_settings::Value::Bool(stored)).unwrap();
        }
        assert_eq!(options(&s), Options { show_history: true });
        s.set("search.show_history", iptv_settings::Value::Bool(false))
            .unwrap();
        assert_eq!(
            options(&s),
            Options {
                show_history: false
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
        assert_eq!(
            outcome(
                &ResultId::Channel(iptv_search::ChannelId::new(None, "synthetic-7")),
                None,
                Some(7)
            ),
            Outcome::Live(7)
        );
        assert_eq!(
            outcome(&p(), Some(&hit(Airing::Live, true)), Some(3)),
            Outcome::Live(3)
        );
        assert_eq!(
            outcome(&p(), Some(&hit(Airing::Ended, true)), Some(3)),
            Outcome::CatchUp {
                channel: 3,
                start: 1_000
            }
        );
        let details = Outcome::Details {
            channel: 3,
            start: 1_000,
        };
        assert_eq!(
            outcome(&p(), Some(&hit(Airing::Upcoming, false)), Some(3)),
            details
        );
        assert_eq!(
            outcome(&p(), Some(&hit(Airing::Ended, false)), Some(3)),
            details
        );
        assert_eq!(outcome(&p(), None, Some(3)), Outcome::Nothing);
        assert_eq!(
            outcome(&p(), Some(&hit(Airing::Live, true)), None),
            Outcome::Nothing
        );
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
                identity: iptv_search::ChannelId::new(None, "synthetic-9"),
                channel: 9,
                score: MatchQuality::Exact,
            }],
            vec![],
        );
        s.apply(SearchAction::Down);
        s.apply(SearchAction::Down);
        assert_eq!(
            s.focus(),
            &Focus::Result(ResultId::Channel(iptv_search::ChannelId::new(
                None,
                "synthetic-9"
            )))
        );
        assert_eq!(k(&s, Action::Select), Some(SearchAction::Ok));
    }

    #[test]
    fn times_convert_to_guide_minutes() {
        let epoch = 1_800_000_000;
        assert_eq!(guide_minute(epoch, epoch), 0);
        assert_eq!(guide_minute(epoch + 90, epoch), 2, "inside the program");
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
            tab_label(Tab::Channels, 3, ProgramSearch::Unavailable),
            "Channels · 3"
        );
        assert_eq!(
            tab_label(Tab::Programs, 0, ProgramSearch::Unavailable),
            "Programs · no guide data"
        );
        assert_eq!(
            tab_label(Tab::Programs, 0, ProgramSearch::Building),
            "Programs · …"
        );
        assert_eq!(
            tab_label(Tab::Programs, 12, ProgramSearch::Ready),
            "Programs · 12"
        );
        // A list cut at the limit is not a total.
        assert_eq!(
            tab_label(Tab::Programs, LIMIT, ProgramSearch::Ready),
            format!("Programs · {LIMIT}+")
        );
    }
}
