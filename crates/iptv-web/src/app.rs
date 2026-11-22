//! Live TV: the import, the guide screen (the playing video's preview at the
//! top left beside the focused program, and below them the rail, the groups
//! column and the guide grid: `grid`), the full-screen player with its play
//! bar and channel zapping, search (`search`) and the Favorites order. The
//! state is the core's (`iptv_core`'s guide focus, browse columns, lists and
//! reorder models over an `iptv_guide` schedule, `iptv_search`'s search
//! screen); the host renders the views this emits and plays what it is told
//! to.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use iptv_core::favorites::{Favorites, KeyIndex, Recent};
use iptv_core::guide::{Pane, Program, Schedule, now_minute};
use iptv_core::lists::{self, ChannelList as Rows, Entry, FAVORITES};
use iptv_core::playback::zap_target;
use iptv_core::reorder::{self, Exit, Reorder};
use iptv_epg::{GuideStore, Overrides, StoreOptions};
use iptv_guide::EpgSchedule;
use iptv_guide::playback::{Clock, PlaybackContext, Resolution};
use iptv_import::{
    CancelToken, Catalog, Decision, EpgSourceConfig, GuideState, ImportOptions, MemoryGuide, Phase,
    Progress, RefreshPolicy, SourceMeta, Trigger, decide, matching_fingerprint, plan_startup,
};
use iptv_providers::model::{M3uSourceConfig, SourceConfig, XtreamCredentials};
use iptv_providers::xtream::StreamFormat;
use iptv_providers::{Secret, SecretUrl};
use iptv_search::{SearchAction, SearchEffect, SearchHistory, SearchState, Tab};
use serde_json::{Value, json};

use crate::exec::{HostFetch, spawn};
use crate::grid::{Grid, GridOutcome};
use crate::host::host;
use crate::search::{self, Indexes};
use crate::store;

/// Entries the groups column shows at once.
pub(crate) const VISIBLE_ENTRIES: usize = 10;
/// The play bar's buttons, left to right.
const BAR: [&str; 3] = ["favorite", "guide", "search"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The guide screen, with the video in the preview at the top left.
    Guide,
    /// Full-screen video.
    Full,
    /// The search screen beside the video.
    Search,
}

/// What a held Left does, decided by where its press (the key before its
/// repeats) acted. The core decides, as the page's view may be a key behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeftHold {
    /// Pressed in the guide grid: the first repeat opens the groups column
    /// (Android TV's Long Left).
    Grid,
    /// The groups were opened by this hold, or the press was in full screen
    /// without the play bar (it opened the guide, and holding it must not
    /// go on through the guide): later repeats do nothing.
    Done,
    /// Anywhere else a repeat is a Left like any other.
    Plain,
}

pub(crate) struct App {
    config: Option<SourceConfig>,
    schedule: Option<Rc<EpgSchedule>>,
    /// The list the guide shows: the groups column's selected entry.
    rows: Rc<Rows>,
    /// The guide screen's focus and grid, once there is a schedule.
    grid: Option<Grid>,
    mode: Mode,
    playing: Option<usize>,
    /// `rows`' row of one catalog channel, the last one asked for (the
    /// playing channel, on every view and zap), so it is not searched for
    /// on every key. Cleared whenever `rows` changes.
    row_cache: Cell<Option<(usize, Option<usize>)>>,
    /// The groups column's selection moved while its key repeats: the
    /// selected entry's list is built when the key settles (`settle_rows`),
    /// not once per step; the guide shows an empty list meanwhile.
    rows_pending: bool,
    /// The playing channel plays live (not a program from its archive).
    playing_live: bool,
    /// The playlist's identity for stored data (no credentials).
    source_id: Option<String>,
    /// The user's lists by channel key, so they survive refreshes.
    favorites: Favorites,
    recent: Recent,
    /// Recent searches, kept with the lists.
    searches: SearchHistory,
    keys: KeyIndex,
    /// The search screen while it is open (also while what it played is
    /// full screen, for Back).
    search: Option<SearchState>,
    /// The search indexes of the current guide.
    indexes: Indexes,
    /// Full screen entered from search: Back returns there.
    back_to_search: bool,
    /// Moving a favorite (Favorites list): the order being edited.
    reorder: Option<Reorder<String>>,
    /// The play bar over full-screen video, shown with `BAR[i]` focused.
    bar: Option<usize>,
    /// What repeats of the Left being held do.
    left_hold: LeftHold,
    /// The `id` of the last `play` event: the host names it when that
    /// stream fails, so a failure of a stream since replaced is ignored.
    play_id: u64,
    /// The search tab shown when the guide was replaced under the results,
    /// shown again once the new program index has results for it.
    search_tab: Option<Tab>,
    /// The import's progress for the status line.
    status: Value,
    guide_loaded: bool,
    /// The current playlist's guide refresh inputs and stored state.
    guide: Option<Rc<GuideInputs>>,
    guide_state: Option<GuideState>,
    /// Bumped to stop a waiting guide loop (a manual refresh replaces it).
    guide_generation: u32,
    /// Cancels the current guide loop's download.
    guide_cancel: CancelToken,
    cancel: CancelToken,
    generation: u32,
    /// The viewer's UTC offset, from the host.
    local_offset_secs: i32,
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::new());
}

pub(crate) fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(&mut a.borrow_mut()))
}

fn wall_secs() -> i64 {
    (host().wall_ms() / 1000.0) as i64
}

/// A notice that informs (not an error).
fn info(text: &str) {
    host().emit(&json!({ "type": "notice", "text": text, "kind": "info" }).to_string());
}

impl App {
    fn new() -> Self {
        App {
            config: None,
            schedule: None,
            rows: Rc::new(Rows::default()),
            grid: None,
            mode: Mode::Guide,
            playing: None,
            row_cache: Cell::new(None),
            rows_pending: false,
            playing_live: false,
            source_id: None,
            favorites: Favorites::default(),
            recent: Recent::default(),
            searches: SearchHistory::default(),
            keys: KeyIndex::default(),
            search: None,
            indexes: Indexes::default(),
            back_to_search: false,
            reorder: None,
            bar: None,
            left_hold: LeftHold::Plain,
            play_id: 0,
            search_tab: None,
            status: json!({ "phase": "idle" }),
            guide_loaded: false,
            guide: None,
            guide_state: None,
            guide_generation: 0,
            guide_cancel: CancelToken::new(),
            cancel: CancelToken::new(),
            generation: 0,
            local_offset_secs: 0,
        }
    }

    fn schedule(&self) -> Option<&dyn Schedule> {
        self.schedule.as_deref().map(|s| s as &dyn Schedule)
    }

    fn build_rows(&self, entry: usize, s: &dyn Schedule) -> Rows {
        let favorites = self.favorites.channels(&self.keys);
        let recent = self.recent.channels(&self.keys);
        Rows::build(entry, s, &favorites, &recent)
    }

    /// The guide shows `rows`, focused on `row` (else where it was).
    fn set_rows(&mut self, rows: Rows, row: Option<usize>) {
        self.rows = Rc::new(rows);
        self.row_cache.set(None);
        self.rows_pending = false;
        if let Some(grid) = &mut self.grid {
            grid.set_list(self.rows.clone(), row);
        }
    }

    /// The row of `rows` showing catalog `channel` (cached for the last
    /// channel asked for).
    fn row_of(&self, channel: usize) -> Option<usize> {
        if let Some((c, row)) = self.row_cache.get()
            && c == channel
        {
            return row;
        }
        let row = self.rows.row_of(channel);
        self.row_cache.set(Some((channel, row)));
        row
    }

    /// The number catalog `channel` shows: [`Rows::number_of`], cached.
    fn number_of(&self, channel: usize, s: &dyn Schedule) -> u32 {
        self.row_of(channel)
            .map_or_else(|| s.channel_number(channel), Rows::number)
    }

    /// The groups column's selected list, built now when its key repeats
    /// left it pending.
    fn settle_rows(&mut self) {
        if !self.rows_pending {
            return;
        }
        let entry = self.grid.as_ref().map_or(lists::ALL, Grid::entry);
        self.show_entry(entry);
    }

    /// Shows the list of groups column `entry`.
    fn show_entry(&mut self, entry: usize) {
        let Some(s) = self.schedule.clone() else {
            return;
        };
        let rows = self.build_rows(entry, &*s);
        self.set_rows(rows, None);
    }

    fn key_of(&self, channel: usize) -> Option<String> {
        self.schedule()?.channel_key(channel)
    }

    fn is_favorite(&self, channel: usize) -> bool {
        self.key_of(channel)
            .is_some_and(|k| self.favorites.contains(&k))
    }

    fn save_lists(&self) {
        if let Some(id) = &self.source_id {
            store::save_lists(id, &self.favorites, &self.recent, &self.searches);
        }
    }

    /// Adds or removes the focused row's channel (the playing one in full
    /// screen or on the preview) from Favorites. A Favorites list on screen
    /// follows at once, so no removed channel stays on it.
    fn toggle_favorite(&mut self) {
        let channel = match (self.mode, self.grid.as_ref().map(Grid::pane)) {
            (Mode::Full, _) | (Mode::Guide, Some(Pane::Preview)) => self.playing,
            (Mode::Guide, Some(Pane::Guide)) => self.grid.as_ref().and_then(Grid::focused_channel),
            _ => None,
        };
        let Some(key) = channel.and_then(|c| self.key_of(c)) else {
            return;
        };
        let on = !self.favorites.contains(&key);
        self.favorites.set(&key, on);
        self.save_lists();
        if self.rows.entry() == FAVORITES
            && let Some(s) = self.schedule.clone()
        {
            // The focus stays on its channel when it is still a favorite,
            // else on the row that took its place (`set_list`); an emptied
            // list hands it to the groups column.
            let focused = self.grid.as_ref().and_then(Grid::focused_channel);
            let rows = self.build_rows(FAVORITES, &*s);
            let row = focused.and_then(|c| rows.row_of(c));
            self.set_rows(rows, row);
        }
        info(if on {
            "Added to Favorites"
        } else {
            "Removed from Favorites"
        });
    }

    /// A new schedule for the same catalog (the guide arrived): the list and
    /// the focused channel stay, found again by catalog channel.
    fn replace_schedule(&mut self, schedule: EpgSchedule) {
        // A move in progress ends with what it has: its rows are rebuilt.
        self.finish_reorder();
        self.keys = KeyIndex::build(&schedule);
        self.schedule = Some(Rc::new(schedule));
        let s = self.schedule.clone().expect("just set");
        let now = now_minute(&*s, wall_secs(), 0);
        let entry = self.grid.as_ref().map_or(lists::ALL, Grid::entry);
        self.rows = Rc::new(self.build_rows(entry, &*s));
        self.row_cache.set(None);
        self.rows_pending = false;
        match &mut self.grid {
            Some(grid) => grid.rebase(s, self.rows.clone(), now),
            None => self.grid = Some(Grid::open(s, self.rows.clone(), now)),
        }
        self.indexes.reset();
        if let Some(state) = &self.search {
            // Program results the new guide no longer lists are dropped;
            // the others stay (with the tab and the focused result) until
            // the query runs again on the new index. Channels run now.
            if state.phase() != iptv_search::Phase::Idle {
                self.search_tab.get_or_insert(state.tab());
                let query = state.query().to_owned();
                self.run_search(&query, true);
            }
            self.start_program_index();
        }
    }

    fn play(&mut self, channel: usize) {
        self.play_from(channel, None);
    }

    /// Plays `channel` live, or `archive` (a program of its archive) from
    /// its start. Returns whether the player was asked to play.
    fn play_from(&mut self, channel: usize, archive: Option<Program>) -> bool {
        let Some(s) = self.schedule.clone() else {
            return false;
        };
        let clock = Clock {
            now: wall_secs(),
            local_offset_secs: self.local_offset_secs,
        };
        match s.resolve(channel, archive, clock) {
            Resolution::Play(source) if !crate::exec::allowed(&source.url, source.public_only) => {
                host().emit(
                    &json!({ "type": "notice", "text": "This channel's address is not allowed (it must be a public HTTP(S) address)." })
                        .to_string(),
                );
                false
            }
            Resolution::Play(source) => {
                self.playing = Some(channel);
                self.playing_live = archive.is_none();
                if let Some(key) = s.channel_key(channel) {
                    self.recent.played(&key);
                    self.save_lists();
                }
                let user_agent = source
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                    .map(|(_, v)| v.clone());
                self.play_id += 1;
                host().emit(
                    &json!({
                        "type": "play",
                        "id": self.play_id,
                        "url": source.url,
                        "userAgent": user_agent,
                        "archive": archive.map(|p| s.unix(p.start)),
                        "channel": self.channel_json(&*s, channel, None),
                    })
                    .to_string(),
                );
                true
            }
            Resolution::StalkerLink(_) | Resolution::Unavailable(_) => {
                host().emit(
                    &json!({ "type": "notice", "text": "This channel cannot be played." })
                        .to_string(),
                );
                false
            }
        }
    }

    /// Channel up/down from the playing channel, within the list shown; a
    /// channel the list does not have (played from search) zaps within its
    /// own group, as on Android TV. The guide's focus follows, so the guide
    /// shows it again on the same row.
    fn zap(&mut self, step: isize) {
        let Some(s) = self.schedule.clone() else {
            return;
        };
        let from = self.playing;
        let to = match (from, from.and_then(|c| self.row_of(c))) {
            (_, Some(row)) => self.rows.zap(row, step).and_then(|r| self.rows.channel(r)),
            (Some(channel), None) => Some(zap_target(&*s, channel, step)),
            (None, None) => {
                let row = self.grid.as_ref().map_or(0, Grid::row);
                self.rows.zap(row, step).and_then(|r| self.rows.channel(r))
            }
        };
        let Some(to) = to.filter(|&to| Some(to) != from) else {
            return;
        };
        self.play(to);
        if let (Some(row), Some(grid)) = (self.row_of(to), &mut self.grid) {
            grid.show_channel(row);
        }
    }

    fn now(&self) -> i32 {
        self.schedule().map_or(0, |s| now_minute(s, wall_secs(), 0))
    }

    /// The local time of day of `unix`, as the page shows times.
    fn clock(&self, unix: i64) -> String {
        let secs = (unix + i64::from(self.local_offset_secs)).rem_euclid(86_400);
        format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
    }

    /// The guide screen on the playing channel's row, at now (a list
    /// without it keeps its focus, in the grid).
    fn guide_on_playing(&mut self) {
        self.close_search();
        self.back_to_search = false;
        self.mode = Mode::Guide;
        let row = self.playing.and_then(|c| self.row_of(c));
        let now = self.now();
        if let Some(grid) = &mut self.grid {
            grid.tick(now);
            match row {
                Some(row) => grid.show_channel(row),
                None => grid.focus_guide(),
            }
        }
    }

    /// A key on the guide screen. `scrolling`: a repeat of Up/Down in the
    /// groups column, whose list is built once the key settles.
    fn guide_key(&mut self, key: &str, scrolling: bool) {
        let now = self.now();
        let entries = self.schedule().map_or(0, lists::entry_count);
        // A key held in the groups column only moves through the groups: it
        // stops at the first one, and a new press goes on to the preview.
        let preview = self.playing.is_some() && !scrolling;
        let Some(grid) = self.grid.as_mut() else {
            return;
        };
        match grid.key(key, now, entries, preview) {
            GridOutcome::Stay => {}
            GridOutcome::Preview if scrolling => {
                let entry = grid.entry();
                self.set_rows(Rows::new(entry, Vec::new()), None);
                self.rows_pending = true;
            }
            GridOutcome::Preview => {
                let entry = grid.entry();
                self.show_entry(entry);
            }
            GridOutcome::Watch { channel, archive } => self.watch(channel, archive),
            GridOutcome::Upcoming(start) => info(&format!("Starts at {}", self.clock(start))),
            GridOutcome::Unavailable => info("No longer available"),
            GridOutcome::Full => {
                if self.playing.is_some() {
                    self.mode = Mode::Full;
                }
            }
            GridOutcome::Search => self.open_search(),
            GridOutcome::Empty if grid.entry() == FAVORITES => {
                info("No favorites yet. Yellow adds the focused channel.");
            }
            GridOutcome::Empty => info("This list has no channels."),
        }
    }

    /// OK on a program: full screen, tuning it unless that channel already
    /// plays live.
    fn watch(&mut self, channel: usize, archive: Option<Program>) {
        let tuned = archive.is_none() && self.playing == Some(channel) && self.playing_live;
        if tuned || self.play_from(channel, archive) {
            self.back_to_search = false;
            self.mode = Mode::Full;
        }
    }

    /// The search screen.
    fn open_search(&mut self) {
        if self.mode == Mode::Search {
            return;
        }
        self.search = Some(SearchState::new(self.searches.clone()));
        self.back_to_search = false;
        self.mode = Mode::Search;
        self.start_program_index();
    }

    /// Ends the search screen, keeping its history.
    fn close_search(&mut self) {
        self.keep_history();
        self.search = None;
        self.search_tab = None;
    }

    /// Stores the search screen's history when it changed (a query
    /// completed, a result played), so leaving the app keeps it.
    fn keep_history(&mut self) {
        if let Some(state) = &self.search
            && state.history() != &self.searches
        {
            self.searches = state.history().clone();
            self.save_lists();
        }
    }

    /// Builds the program index in the background unless it exists.
    fn start_program_index(&mut self) {
        let (Some(schedule), Some(generation)) =
            (self.schedule.clone(), self.indexes.start_build())
        else {
            return;
        };
        spawn(async move {
            let current = move || with_app(|a| a.indexes.current(generation));
            let Some((index, built)) = search::build_programs(schedule, current).await else {
                return;
            };
            host().emit(
                &json!({
                    "type": "metric",
                    "name": "searchIndex",
                    "ms": built.ms.round(),
                    "programs": built.programs,
                    "bytes": built.bytes,
                })
                .to_string(),
            );
            let rerun = with_app(|a| {
                if !a.indexes.finish_build(generation, index) {
                    return None;
                }
                a.search
                    .as_ref()
                    .filter(|s| s.phase() != iptv_search::Phase::Idle)
                    .map(|s| s.query().to_owned())
            });
            if let Some(query) = rerun {
                with_app(|a| {
                    a.run_search(&query, false);
                    a.restore_search_tab();
                });
            }
            emit_view();
        });
    }

    /// Runs `query` and shows its results. `keep_listed`: the guide was
    /// replaced under the results of this query, so until its program index
    /// is built the program results it still lists stay.
    fn run_search(&mut self, query: &str, keep_listed: bool) {
        let Some(s) = self.schedule.clone() else {
            return;
        };
        let channels = self.indexes.channels(&*s);
        let favorites = self.favorites.set_of(&self.keys);
        let (channel_hits, mut program_hits) = search::run(
            &channels,
            self.indexes.programs(),
            query,
            &favorites,
            wall_secs(),
        );
        if keep_listed
            && self.indexes.programs().is_none()
            && let Some(state) = &self.search
        {
            program_hits = search::still_listed(&s, state.programs());
        }
        if let Some(state) = &mut self.search {
            state.set_results(query, channel_hits, program_hits);
        }
    }

    /// The new program index answered: the tab shown before the guide was
    /// replaced shows again when it was left only because its results were
    /// gone, on the result focused there if the new guide still has it
    /// (`SearchState` finds it by its identity).
    fn restore_search_tab(&mut self) {
        let (Some(tab), Some(state)) = (self.search_tab.take(), self.search.as_mut()) else {
            return;
        };
        let on_results = matches!(
            state.focus(),
            iptv_search::Focus::Result(_) | iptv_search::Focus::Tabs
        );
        if state.tab() != tab && on_results {
            state.apply(SearchAction::ShowTab(tab));
        }
    }

    fn search_effect(&mut self, effect: SearchEffect) {
        match effect {
            SearchEffect::None => {}
            SearchEffect::Search(query) => self.run_search(&query, false),
            SearchEffect::Close => {
                self.close_search();
                self.mode = Mode::Guide;
            }
            SearchEffect::Play(id) => {
                let Some(channel) = search::channel_of(&id, &self.keys) else {
                    return;
                };
                // What was searched for is history once a result plays.
                self.keep_history();
                // Programs found are on now or later: their channel plays.
                if self.play_from(channel, None) {
                    // The guide follows, so it shows the channel later.
                    if let (Some(row), Some(grid)) = (self.row_of(channel), &mut self.grid) {
                        grid.show_channel(row);
                    }
                    self.back_to_search = true;
                    self.mode = Mode::Full;
                }
            }
        }
    }

    /// A remote key on the search screen; `done` and `cancel` are the
    /// on-screen keyboard's.
    fn search_key(&mut self, key: &str) {
        // The viewer moved on: no tab is brought back under them.
        self.search_tab = None;
        let Some(state) = self.search.as_mut() else {
            self.mode = Mode::Guide;
            return;
        };
        let on_input = *state.focus() == iptv_search::Focus::Input;
        let idle = state.phase() == iptv_search::Phase::Idle;
        let action = match key {
            "up" => SearchAction::Up,
            "down" => SearchAction::Down,
            "left" => SearchAction::Left,
            "right" => SearchAction::Right,
            "ok" if on_input && !state.keyboard_visible() => SearchAction::ShowKeyboard,
            // Done on an empty query only closes the keyboard.
            "done" if idle => SearchAction::KeyboardHidden,
            "ok" | "done" => SearchAction::Ok,
            // Back first hides the keyboard, as Cancel does.
            "cancel" => SearchAction::KeyboardHidden,
            "back" if state.keyboard_visible() => SearchAction::KeyboardHidden,
            "back" => SearchAction::Back,
            _ => return,
        };
        let effect = state.apply(action);
        self.keep_history();
        self.search_effect(effect);
    }

    /// The query field's text changed.
    fn search_text(&mut self, text: &str) {
        if self.mode != Mode::Search {
            return;
        }
        self.search_tab = None;
        if let Some(state) = self.search.as_mut() {
            let effect = state.apply(SearchAction::Type(text.to_owned()));
            self.search_effect(effect);
        }
    }

    /// Blue: the Favorites list; on a row of it in the grid, moving that
    /// favorite.
    fn favorites_key(&mut self) {
        let entries = self.schedule().map_or(0, lists::entry_count);
        let Some(grid) = self.grid.as_mut() else {
            return;
        };
        match self.mode {
            Mode::Search => {}
            // Moving needs another favorite to move past.
            Mode::Guide
                if grid.entry() == FAVORITES
                    && grid.pane() == Pane::Guide
                    && self.rows.len() > 1 =>
            {
                self.start_reorder();
            }
            Mode::Guide | Mode::Full => {
                grid.select_entry(FAVORITES, entries);
                self.close_search();
                self.back_to_search = false;
                self.mode = Mode::Guide;
                // Built again: favorites may have changed since it showed.
                self.show_entry(FAVORITES);
                let empty = self.rows.is_empty();
                if let Some(grid) = &mut self.grid {
                    if empty {
                        grid.open_groups();
                    } else {
                        grid.focus_guide();
                    }
                }
                if empty {
                    info("No favorites yet. Yellow adds the focused channel.");
                }
            }
        }
    }

    /// Move mode on the Favorites list: the focused favorite is picked up.
    fn start_reorder(&mut self) {
        let keys: Vec<String> = self
            .rows
            .channels()
            .iter()
            .filter_map(|&c| self.key_of(c))
            .collect();
        let focused = self
            .grid
            .as_ref()
            .and_then(Grid::focused_channel)
            .and_then(|c| self.key_of(c));
        self.reorder = Some(Reorder::new(keys, focused.as_ref()).selecting_focused());
    }

    /// A key in move mode: Up/Down move the favorite a place, Left/Right
    /// to the top/bottom; OK, Back (or Blue again) end the mode.
    fn reorder_key(&mut self, key: &str) {
        let Some(order) = self.reorder.as_mut() else {
            return;
        };
        let k = match key {
            "up" => reorder::Key::Up,
            "down" => reorder::Key::Down,
            "left" => reorder::Key::Left,
            "right" => reorder::Key::Right,
            "ok" | "back" | "blue" => {
                self.finish_reorder();
                return;
            }
            _ => return,
        };
        if order.key(k) == reorder::Outcome::Moved {
            let row = order.focus_index().unwrap_or(0);
            let channels = self.keys.channels(order.items());
            let Some(s) = self.schedule.clone() else {
                return;
            };
            // Built as the list is (not `Rows::new`), so it keeps how far
            // back its listings reach: a focus on a past program stays.
            let rows = Rows::build(FAVORITES, &*s, &channels, &[]);
            self.set_rows(rows, Some(row));
        }
    }

    /// Ends move mode: a changed order is stored.
    fn finish_reorder(&mut self) {
        let Some(mut order) = self.reorder.take() else {
            return;
        };
        let focused = order.focused().cloned();
        if let reorder::Outcome::Done(Exit::Saved(keys)) = order.commit() {
            self.favorites.apply_order(&keys);
            self.save_lists();
        }
        let Some(s) = self.schedule.clone() else {
            return;
        };
        let rows = self.build_rows(FAVORITES, &*s);
        let row = focused
            .and_then(|k| self.keys.get(&k))
            .and_then(|c| rows.row_of(c))
            .unwrap_or(0);
        self.set_rows(rows, Some(row));
    }

    /// A key while the play bar shows: Left/Right move between its buttons,
    /// OK presses the focused one, Back hides the bar. Channel keys still
    /// zap, under the bar.
    fn bar_key(&mut self, focus: usize, key: &str) {
        match key {
            "left" => self.bar = Some(focus.saturating_sub(1)),
            "right" => self.bar = Some((focus + 1).min(BAR.len() - 1)),
            "ok" => match BAR[focus] {
                "favorite" => self.toggle_favorite(),
                "guide" => self.guide_on_playing(),
                _ => self.open_search(),
            },
            "back" | "barTimeout" => self.bar = None,
            "up" | "chup" => self.zap(-1),
            "down" | "chdown" => self.zap(1),
            _ => {}
        }
    }

    fn full_key(&mut self, key: &str) {
        if let Some(focus) = self.bar {
            return self.bar_key(focus, key);
        }
        match key {
            "up" | "chup" => self.zap(-1),
            "down" | "chdown" => self.zap(1),
            // OK shows the play bar (the channel, its program and buttons)
            // with Favorite focused, over what plays.
            "ok" if self.playing.is_some() => self.bar = Some(0),
            "left" => self.guide_on_playing(),
            // Back returns to the search that played the channel, else to
            // the guide on the playing channel. It never leaves the app
            // (the Exit key does).
            "back" => self.leave_full(),
            _ => {}
        }
    }

    /// Back from full screen: to the search that played the channel, else
    /// to the guide on the playing channel.
    fn leave_full(&mut self) {
        if self.back_to_search && self.search.is_some() {
            self.back_to_search = false;
            self.mode = Mode::Search;
            let effect = self.search.as_mut().map(SearchState::resume);
            if let Some(effect) = effect {
                self.search_effect(effect);
            }
        } else {
            self.guide_on_playing();
        }
    }

    /// The player could not play the stream of `play` event `id`: nothing
    /// plays, and full screen (black now, its bar over nothing) gives way
    /// to where it was entered from. A stream replaced since (the viewer
    /// zapped on before it failed) is not what plays: its failure is
    /// ignored.
    fn play_failed(&mut self, id: Option<u64>) {
        if id != Some(self.play_id) || self.playing.is_none() {
            return;
        }
        self.playing = None;
        self.playing_live = false;
        self.bar = None;
        if self.mode == Mode::Full {
            self.leave_full();
        }
        if let Some(grid) = &mut self.grid {
            grid.preview_gone();
        }
    }

    /// Where a Left pressed now acts, for its repeats ([`LeftHold`]).
    fn left_hold(&self) -> LeftHold {
        let pane = self.grid.as_ref().map(Grid::pane);
        match self.mode {
            _ if self.schedule.is_none() => LeftHold::Plain,
            Mode::Guide if self.reorder.is_none() && pane == Some(Pane::Guide) => LeftHold::Grid,
            Mode::Full if self.bar.is_none() => LeftHold::Done,
            _ => LeftHold::Plain,
        }
    }

    /// A repeat of the held Left. Returns whether it is still to be
    /// handled as a Left.
    fn left_repeat(&mut self) -> bool {
        match self.left_hold {
            LeftHold::Grid => {
                self.left_hold = LeftHold::Done;
                // Still in the grid (the press may have left it already,
                // from the earliest program).
                if self.left_hold() == LeftHold::Grid
                    && let Some(grid) = &mut self.grid
                {
                    grid.open_groups();
                }
                false
            }
            LeftHold::Done => false,
            // The play bar hid meanwhile: Left would open the guide.
            LeftHold::Plain => !(self.mode == Mode::Full && self.bar.is_none()),
        }
    }

    /// One remote key; `repeat` when the page saw it repeat (held).
    fn key(&mut self, key: &str, repeat: bool) {
        if key == "left" {
            if !repeat {
                self.left_hold = self.left_hold();
            } else if !self.left_repeat() {
                return;
            }
        }
        let scrolling = repeat
            && matches!(key, "up" | "down")
            && self.mode == Mode::Guide
            && self.reorder.is_none()
            && self.grid.as_ref().is_some_and(|g| g.pane() == Pane::Groups);
        if !scrolling {
            self.settle_rows();
        }
        self.dispatch_key(key, scrolling);
        // The play bar belongs to full screen.
        if self.mode != Mode::Full {
            self.bar = None;
        }
    }

    fn dispatch_key(&mut self, key: &str, scrolling: bool) {
        if self.schedule.is_none() {
            return;
        }
        if self.reorder.is_some() {
            return self.reorder_key(key);
        }
        match (key, self.mode) {
            ("favorite", _) => self.toggle_favorite(),
            ("guide", Mode::Guide) => {
                if let Some(grid) = &mut self.grid {
                    grid.focus_guide();
                }
            }
            ("guide", Mode::Full) => self.guide_on_playing(),
            ("guide", Mode::Search) => {
                self.close_search();
                self.mode = Mode::Guide;
            }
            ("search", _) => self.open_search(),
            ("blue", _) => self.favorites_key(),
            (_, Mode::Full) => self.full_key(key),
            (_, Mode::Guide) => self.guide_key(key, scrolling),
            (_, Mode::Search) => self.search_key(key),
        }
    }

    fn program_json(s: &dyn Schedule, channel: usize, minute: i32) -> Option<Value> {
        let p = s.program_at(channel, minute);
        let title = s.title(channel, p);
        if title.is_empty() || title == iptv_guide::NO_INFORMATION || title == iptv_guide::LOADING {
            return None;
        }
        let epoch = s.epoch_unix();
        Some(json!({
            "title": title,
            "start": epoch + i64::from(p.start) * 60,
            "end": epoch + i64::from(p.end) * 60,
        }))
    }

    fn channel_json(&self, s: &dyn Schedule, channel: usize, number: Option<u32>) -> Value {
        let minute = now_minute(s, wall_secs(), 0);
        let now = Self::program_json(s, channel, minute);
        let next = now
            .as_ref()
            .and_then(|_| Self::program_json(s, channel, s.program_at(channel, minute).end));
        json!({
            "id": channel,
            "number": number.unwrap_or_else(|| self.number_of(channel, s)),
            "name": s.channel_name(channel),
            // Playlist artwork is content: only public addresses are loaded.
            "logo": s.channel_logo(channel).filter(|l| crate::exec::allowed(l, true)),
            "now": now,
            "next": next,
            "playing": self.playing == Some(channel),
        })
    }

    fn entry_name(s: &dyn Schedule, entry: usize) -> String {
        match Entry::at(entry) {
            Entry::Favorites => "Favorites".into(),
            Entry::Recent => "Recently watched".into(),
            Entry::All => "All channels".into(),
            Entry::Group(g) => s.group_name(g),
        }
    }

    fn window(focus: usize, len: usize, shown: usize) -> usize {
        focus
            .saturating_sub(shown / 2)
            .min(len.saturating_sub(shown))
    }

    fn view(&mut self) -> Value {
        let now = self.now();
        if let Some(grid) = &mut self.grid {
            grid.tick(now);
        }
        let (Some(s), Some(grid)) = (self.schedule(), self.grid.as_ref()) else {
            return json!({ "type": "view", "ready": false, "status": self.status });
        };
        let logo = |c: usize| s.channel_logo(c).filter(|l| crate::exec::allowed(l, true));
        let playing = self.playing.map(|c| self.channel_json(s, c, None));
        match (self.mode, &self.search) {
            (Mode::Search, Some(state)) => json!({
                "type": "view",
                "ready": true,
                "mode": "search",
                "search": search::json(state, s, self.indexes.preparing(), &logo),
                "playing": playing,
                "guide": self.guide_loaded,
                "status": self.status,
            }),
            (Mode::Full, _) => json!({
                "type": "view",
                "ready": true,
                "mode": "full",
                "bar": self.bar.map(|focus| json!({
                    "focus": BAR[focus],
                    "favorite": self.playing.is_some_and(|c| self.is_favorite(c)),
                })),
                "playing": playing,
                "guide": self.guide_loaded,
                "status": self.status,
            }),
            _ => {
                let screen = grid.json(self.playing, &logo, &|c| self.is_favorite(c));
                // The groups column is sent only while it shows.
                let groups = (screen["columns"] == true).then(|| {
                    let entries = lists::entry_count(s);
                    let entry = grid.entry();
                    let start = Self::window(entry, entries, VISIBLE_ENTRIES);
                    let names: Vec<String> = (start..(start + VISIBLE_ENTRIES).min(entries))
                        .map(|e| Self::entry_name(s, e))
                        .collect();
                    json!({ "start": start, "focus": entry, "total": entries, "items": names })
                });
                json!({
                    "type": "view",
                    "ready": true,
                    "mode": "guide",
                    "title": Self::entry_name(s, grid.entry()),
                    "grid": screen,
                    "groups": groups,
                    "reorder": self.reorder.is_some(),
                    // The list is built once the groups column's key settles.
                    "pending": self.rows_pending,
                    "playing": playing,
                    "guide": self.guide_loaded,
                    "status": self.status,
                })
            }
        }
    }
}

fn emit_view() {
    emit_reply(None);
}

/// The view after a command. The host numbers its `key` and `settle`
/// commands (`seq`): the view each of them emits carries that number, so
/// the host knows the command was handled (views are also emitted for
/// other reasons).
fn emit_reply(seq: Option<u64>) {
    let mut v = with_app(|a| a.view());
    if let Some(seq) = seq {
        v["seq"] = seq.into();
    }
    host().emit(&v.to_string());
}

fn phase_name(p: &Progress) -> &'static str {
    match p.phase {
        Phase::Connecting => "connecting",
        Phase::Authenticating => "authenticating",
        Phase::Account => "account",
        Phase::Playlist => "playlist",
        Phase::Groups(_) => "groups",
        Phase::Items(_) => "channels",
        Phase::Guide(_) => "guide",
        Phase::Matching => "matching",
        _ => "done",
    }
}

/// Progress of the import that `current` says is still the one shown.
fn report(current: &dyn Fn() -> bool, p: &Progress) {
    let status = json!({
        "phase": phase_name(p),
        "items": p.items,
        "bytes": p.bytes,
        "totalBytes": p.total_bytes,
    });
    if !current() {
        return;
    }
    with_app(|a| a.status = status.clone());
    {
        host().emit(&json!({ "type": "progress", "status": status }).to_string());
    }
}

fn playback_context(config: &SourceConfig, catalog: &Catalog) -> Option<PlaybackContext> {
    match (config, &catalog.source) {
        // Raw TS starts and zaps fastest on the TV's native player.
        (SourceConfig::Xtream(c), SourceMeta::Xtream { account, .. }) => {
            PlaybackContext::xtream(c, account.as_ref(), StreamFormat::Ts).ok()
        }
        (SourceConfig::M3u(m), _) => Some(PlaybackContext::m3u(m)),
        _ => None,
    }
}

fn schedule(config: &SourceConfig, catalog: &Catalog, store: GuideStore, now: i64) -> EpgSchedule {
    let s = EpgSchedule::from_catalog(&catalog.groups, &catalog.channels, store, now);
    s.with_playback(playback_context(config, catalog))
}

/// How the TV keeps guides: without descriptions (the list does not show
/// them, and they are most of a guide's size) and one past day.
pub(crate) const GUIDE_STORE: StoreOptions = StoreOptions {
    store_descriptions: false,
    past_days_to_keep: Some(1),
};

/// What the guide refresh reads besides the stored state.
pub(crate) struct GuideInputs {
    config: SourceConfig,
    catalog: Rc<Catalog>,
    source_id: String,
    feeds: Vec<EpgSourceConfig>,
    fingerprint: u64,
}

/// The latest stored program end of any bound channel.
fn ends_at(guide: &GuideStore) -> Option<i64> {
    guide
        .bindings()
        .filter_map(|(key, _)| guide.schedule(key))
        .filter_map(|s| s.get(s.len().checked_sub(1)?).map(|p| p.stop()))
        .max()
}

fn set_status(status: Value) {
    with_app(|a| a.status = status);
    emit_view();
}

async fn import(generation: u32, config: SourceConfig, cancel: CancelToken) {
    let fetch = HostFetch::new();
    let now = wall_secs();
    let current = move || with_app(|a| a.generation == generation);
    let mut progress = move |p: &Progress| report(&current, p);
    let catalog = match iptv_import::import_source(
        &fetch,
        &config,
        &ImportOptions::new(now),
        &mut progress,
        &cancel,
    )
    .await
    {
        Ok(c) => Rc::new(c),
        Err(e) => {
            if current() {
                set_status(json!({ "phase": "failed", "error": e.to_string() }));
            }
            return;
        }
    };
    if !current() {
        return;
    }
    let source_id = store::source_id(&config);
    if !store::make_current(&source_id, &current).await {
        return;
    }
    let lists = store::load_lists(&source_id).await;
    let state = store::load_guide_state(&source_id).await;
    if !current() {
        return;
    }
    // The playlist's guide feeds now; when they cannot be read, the stored
    // ones (so a broken header never discards a working guide).
    let (mut feeds, feeds_error) = match catalog.epg_sources(1) {
        Ok(feeds) => (feeds, None),
        Err(e) => (
            state.as_ref().map(|s| s.feeds.clone()).unwrap_or_default(),
            Some(e.to_string()),
        ),
    };
    for feed in &mut feeds {
        feed.store = GUIDE_STORE;
    }
    let state = state.map(|s| store::with_current_urls(s, &feeds));
    let fingerprint = matching_fingerprint(&catalog.channels, &Overrides::default());
    let policy = RefreshPolicy::default();
    // The stored guide shows at once, before any guide download.
    let plan = plan_startup(&policy, &feeds, fingerprint, state.as_ref(), now);
    let stored = if plan.use_stored {
        store::load_guide(&source_id)
            .await
            .and_then(|bytes| GuideStore::from_snapshot(&bytes, GUIDE_STORE, now).ok())
    } else {
        None
    };
    if !current() {
        return;
    }
    // A state without its guide (cleared storage) is no guide at all.
    let (state, decision) = match (&stored, plan.use_stored) {
        (None, true) => (
            None,
            decide(&policy, Trigger::Startup, &feeds, fingerprint, None, now),
        ),
        _ => (state, plan.decision),
    };
    let has_guide = stored.as_ref().is_some_and(|g| g.bound_count() > 0);
    let first = schedule(&config, &catalog, stored.unwrap_or_default(), now);
    let inputs = Rc::new(GuideInputs {
        config,
        catalog,
        source_id: source_id.clone(),
        feeds,
        fingerprint,
    });
    let (guide_generation, guide_cancel) = with_app(|a| {
        a.source_id = Some(source_id);
        a.favorites = lists.favorites;
        a.recent = lists.recent;
        a.searches = lists.searches;
        a.replace_schedule(first);
        a.guide_loaded = has_guide;
        a.guide = Some(inputs.clone());
        a.guide_state = state;
        a.status = json!({ "phase": "done", "guide": has_guide, "error": feeds_error });
        (a.guide_generation, a.guide_cancel.clone())
    });
    emit_view();
    guide_loop(
        generation,
        guide_generation,
        inputs,
        decision,
        fetch,
        guide_cancel,
    )
    .await;
}

/// Keeps the guide as the refresh policy decides, for as long as this
/// playlist and this guide loop are the current ones.
async fn guide_loop(
    generation: u32,
    guide_generation: u32,
    inputs: Rc<GuideInputs>,
    mut decision: Decision,
    fetch: HostFetch,
    cancel: CancelToken,
) {
    let current =
        move || with_app(|a| a.generation == generation && a.guide_generation == guide_generation);
    let policy = RefreshPolicy::default();
    let mut progress = move |p: &Progress| report(&current, p);
    loop {
        if !current() {
            return;
        }
        let wait_until = match decision {
            Decision::Discard => {
                store::discard_guide(&inputs.source_id);
                with_app(|a| a.guide_state = None);
                set_status(json!({ "phase": "done", "guide": false }));
                return;
            }
            Decision::Keep { next_check } => next_check,
            Decision::Backoff { retry_at } => Some(retry_at),
            Decision::Refresh(_) => {
                let updating = with_app(|a| a.guide_loaded);
                set_status(json!({ "phase": "guide", "updating": updating }));
                let now = wall_secs();
                let mut sink = MemoryGuide::new();
                let result = iptv_import::import_epg(
                    &fetch,
                    &inputs.feeds,
                    &inputs.catalog.channels,
                    &Overrides::default(),
                    now,
                    &mut sink,
                    &mut progress,
                    &cancel,
                )
                .await;
                if !current() {
                    return;
                }
                let previous = with_app(|a| a.guide_state.clone()).unwrap_or_default();
                let guide = result.as_ref().ok().map(|epg| sink.into_guide(epg));
                // An empty result never replaces a guide that works.
                let usable = guide
                    .as_ref()
                    .filter(|g| g.bound_count() > 0 || previous.fetched_at.is_none());
                let state = match usable {
                    Some(guide) => {
                        let state = GuideState {
                            fetched_at: Some(now),
                            attempted_at: Some(now),
                            last_attempt_failed: false,
                            feeds: inputs.feeds.clone(),
                            match_fingerprint: Some(inputs.fingerprint),
                            ends_at: ends_at(guide),
                            bound_channels: guide.bound_count(),
                        };
                        store::save_guide(&inputs.source_id, &guide.to_snapshot(), &state);
                        state
                    }
                    None => {
                        let state = GuideState {
                            attempted_at: Some(now),
                            last_attempt_failed: true,
                            ..previous
                        };
                        store::save_guide_state(&inputs.source_id, &state);
                        state
                    }
                };
                let error = result.as_ref().err().map(ToString::to_string);
                let bound = state.bound_channels;
                if let Some(guide) = guide.filter(|_| !state.last_attempt_failed) {
                    let with_guide = schedule(&inputs.config, &inputs.catalog, guide, now);
                    with_app(|a| {
                        a.replace_schedule(with_guide);
                        a.guide_loaded = bound > 0;
                    });
                }
                with_app(|a| a.guide_state = Some(state));
                let loaded = with_app(|a| a.guide_loaded);
                set_status(
                    json!({ "phase": "done", "guide": loaded, "bound": bound, "error": error }),
                );
                let state = with_app(|a| a.guide_state.clone());
                decision = decide(
                    &policy,
                    Trigger::Timer,
                    &inputs.feeds,
                    inputs.fingerprint,
                    state.as_ref(),
                    wall_secs(),
                );
                continue;
            }
        };
        let Some(at) = wait_until else {
            return; // Only on request ("refreshGuide").
        };
        let wait = (at - wall_secs()).max(1) as u64;
        // A superseded loop stops at the next delivery, not hours later.
        crate::exec::sleep_while(std::time::Duration::from_secs(wait), current).await;
        if !current() {
            return;
        }
        let state = with_app(|a| a.guide_state.clone());
        decision = decide(
            &policy,
            Trigger::Timer,
            &inputs.feeds,
            inputs.fingerprint,
            state.as_ref(),
            wall_secs(),
        );
    }
}

/// "Update the guide now": a manual refresh replaces the current guide loop,
/// canceling a download it has in progress.
fn refresh_guide() {
    let Some((generation, guide_generation, inputs, state, cancel)) = with_app(|a| {
        let inputs = a.guide.clone()?;
        a.guide_cancel.cancel();
        a.guide_cancel = CancelToken::new();
        a.guide_generation += 1;
        Some((
            a.generation,
            a.guide_generation,
            inputs,
            a.guide_state.clone(),
            a.guide_cancel.clone(),
        ))
    }) else {
        return;
    };
    let decision = decide(
        &RefreshPolicy::default(),
        Trigger::Manual,
        &inputs.feeds,
        inputs.fingerprint,
        state.as_ref(),
        wall_secs(),
    );
    spawn(guide_loop(
        generation,
        guide_generation,
        inputs,
        decision,
        HostFetch::new(),
        cancel,
    ));
}

/// The source from the host's settings JSON.
fn source_config(v: &Value) -> Result<SourceConfig, String> {
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
    };
    match text("kind") {
        "xtream" => {
            if text("server").is_empty()
                || text("username").is_empty()
                || text("password").is_empty()
            {
                return Err("Enter the server, username and password.".into());
            }
            Ok(SourceConfig::Xtream(XtreamCredentials {
                server_url: text("server").to_owned(),
                username: Secret::new(text("username")),
                password: Secret::new(text("password")),
                user_agent: None,
            }))
        }
        "m3u" => {
            if text("url").is_empty() {
                return Err("Enter the playlist address.".into());
            }
            Ok(SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(text("url")),
                user_agent: None,
                referrer: None,
            }))
        }
        _ => Err("Choose Xtream or an M3U playlist.".into()),
    }
}

/// One command from the host (JSON). Replies are emitted.
pub(crate) fn command(text: &str) {
    let Ok(cmd) = serde_json::from_str::<Value>(text) else {
        host().emit(&json!({ "type": "notice", "text": "bad command" }).to_string());
        return;
    };
    match cmd.get("cmd").and_then(Value::as_str).unwrap_or("") {
        "source" => match source_config(cmd.get("source").unwrap_or(&Value::Null)) {
            Ok(config) => {
                if let Some(offset) = cmd.get("utcOffsetSecs").and_then(Value::as_i64) {
                    with_app(|a| a.local_offset_secs = offset as i32);
                }
                let (generation, cancel) = with_app(|a| {
                    a.cancel.cancel();
                    a.guide_cancel.cancel();
                    a.cancel = CancelToken::new();
                    a.generation += 1;
                    *a = App {
                        generation: a.generation,
                        // Not reused: a failure of the old playlist's
                        // stream never names a new one's.
                        play_id: a.play_id,
                        cancel: a.cancel.clone(),
                        local_offset_secs: a.local_offset_secs,
                        config: Some(config.clone()),
                        status: json!({ "phase": "connecting" }),
                        ..App::new()
                    };
                    (a.generation, a.cancel.clone())
                });
                emit_view();
                spawn(import(generation, config, cancel));
            }
            // The host shows its setup again with the reason.
            Err(text) => {
                host().emit(&json!({ "type": "sourceRejected", "text": text }).to_string())
            }
        },
        // Every key is answered with a view (also one that changed
        // nothing), which carries the command's `seq`.
        "key" => {
            let key = cmd.get("key").and_then(Value::as_str).unwrap_or("");
            let repeat = cmd.get("repeat").and_then(Value::as_bool).unwrap_or(false);
            // Back with nowhere to go does nothing: only the remote's Exit
            // key leaves the app, and the page handles that key itself.
            with_app(|a| a.key(key, repeat));
            emit_reply(cmd.get("seq").and_then(Value::as_u64));
        }
        // The page's repeating key was released: a list left pending while
        // the groups column scrolled is built now.
        "settle" => {
            with_app(App::settle_rows);
            emit_reply(cmd.get("seq").and_then(Value::as_u64));
        }
        "view" => emit_view(),
        "searchText" => {
            let text = cmd.get("text").and_then(Value::as_str).unwrap_or("");
            with_app(|a| a.search_text(text));
            emit_view();
        }
        "refreshGuide" => refresh_guide(),
        "reload" => {
            let config = with_app(|a| a.config.clone());
            if let Some(c) = config {
                let source = match &c {
                    SourceConfig::Xtream(x) => {
                        json!({ "kind": "xtream", "server": x.server_url, "username": x.username.expose(), "password": x.password.expose() })
                    }
                    SourceConfig::M3u(m) => json!({ "kind": "m3u", "url": m.location.expose() }),
                    SourceConfig::Stalker(_) => Value::Null,
                };
                command(&json!({ "cmd": "source", "source": source }).to_string());
            }
        }
        "playFailed" => {
            let id = cmd.get("id").and_then(Value::as_u64);
            with_app(|a| a.play_failed(id));
            emit_view();
        }
        other => host().emit(
            &json!({ "type": "notice", "text": format!("unknown command {other}") }).to_string(),
        ),
    }
}
