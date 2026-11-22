//! The search screen's state: query, history, results in two tabs
//! (channels, programs) and remote focus.
//!
//! The view renders this state and sends [`SearchAction`]s; the state
//! answers with a [`SearchEffect`] for the app to run (search, play, close).
//! Searching itself happens outside (debounced, possibly off the UI
//! thread) and comes back through [`SearchState::set_results`], which drops
//! answers to an older query. Focus is kept by result identity, so it
//! survives new results, "play and come back" ([`SearchState::resume`])
//! and tab switches.

use iptv_epg::UnixTime;

use crate::channels::ChannelHit;
use crate::history::SearchHistory;
use crate::programs::ProgramHit;
use crate::text::Query;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No query: the history is shown.
    Idle,
    /// A search for the current query is pending; earlier results (if any)
    /// stay visible.
    Typing,
    Results,
    NoResults,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Channels,
    Programs,
}

impl Tab {
    fn other(self) -> Tab {
        match self {
            Tab::Channels => Tab::Programs,
            Tab::Programs => Tab::Channels,
        }
    }
}

/// Identity of a result, stable across searches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResultId {
    Channel(crate::ChannelId),
    Program {
        channel: crate::ChannelId,
        start: UnixTime,
    },
}

impl ResultId {
    pub fn channel(&self) -> &crate::ChannelId {
        match self {
            ResultId::Channel(c) | ResultId::Program { channel: c, .. } => c,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The query field (and the on-screen keyboard when shown).
    Input,
    /// A history entry, by its text.
    History(String),
    /// The Channels / Programs tab row.
    Tabs,
    Result(ResultId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchAction {
    /// The query text changed (keyboard or voice).
    Type(String),
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    /// Empty the query.
    Clear,
    ShowKeyboard,
    /// The on-screen keyboard went away on its own (the platform closed
    /// it, or the field lost focus): nothing else changes.
    KeyboardHidden,
    /// A tap: focus `target` and do what OK does there. Ignored when the
    /// target is not on screen (an old history entry, a result gone).
    Pick(Focus),
    /// A tap on a tab label: show that tab (when it has results).
    ShowTab(Tab),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchEffect {
    None,
    /// Run both searches for this query, then call `set_results`.
    Search(String),
    /// Play the result. The state stays as it is for the return.
    Play(ResultId),
    /// Leave the search screen.
    Close,
}

/// `PartialEq` so a UI can hold it in a change-detecting signal.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchState {
    query: String,
    phase: Phase,
    tab: Tab,
    focus: Focus,
    keyboard: bool,
    channels: Vec<ChannelHit>,
    programs: Vec<ProgramHit>,
    /// Last focused result per tab.
    remembered: [Option<ResultId>; 2],
    history: SearchHistory,
}

impl SearchState {
    pub fn new(history: SearchHistory) -> Self {
        SearchState {
            query: String::new(),
            phase: Phase::Idle,
            tab: Tab::Channels,
            focus: Focus::Input,
            keyboard: true,
            channels: Vec::new(),
            programs: Vec::new(),
            remembered: [None, None],
            history,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn tab(&self) -> Tab {
        self.tab
    }
    pub fn focus(&self) -> &Focus {
        &self.focus
    }
    pub fn keyboard_visible(&self) -> bool {
        self.keyboard
    }
    pub fn channels(&self) -> &[ChannelHit] {
        &self.channels
    }
    pub fn programs(&self) -> &[ProgramHit] {
        &self.programs
    }
    pub fn history(&self) -> &SearchHistory {
        &self.history
    }
    pub fn history_mut(&mut self) -> &mut SearchHistory {
        &mut self.history
    }

    /// Result count of a tab (for its label).
    pub fn count(&self, tab: Tab) -> usize {
        match tab {
            Tab::Channels => self.channels.len(),
            Tab::Programs => self.programs.len(),
        }
    }

    fn id_at(&self, tab: Tab, i: usize) -> Option<ResultId> {
        match tab {
            Tab::Channels => self
                .channels
                .get(i)
                .map(|h| ResultId::Channel(h.identity.clone())),
            Tab::Programs => self.programs.get(i).map(|h| ResultId::Program {
                channel: h.identity.clone(),
                start: h.start,
            }),
        }
    }

    fn position(&self, tab: Tab, id: &ResultId) -> Option<usize> {
        (0..self.count(tab)).find(|&i| self.id_at(tab, i).as_ref() == Some(id))
    }

    /// Where focus goes in `tab`: the remembered result if still there, a
    /// result on the same channel, else the first.
    fn landing(&self, tab: Tab) -> Option<ResultId> {
        let first = self.id_at(tab, 0)?;
        let Some(want) = self.remembered[tab as usize].as_ref() else {
            return Some(first);
        };
        if self.position(tab, want).is_some() {
            return Some(want.clone());
        }
        (0..self.count(tab))
            .filter_map(|i| self.id_at(tab, i))
            .find(|id| id.channel() == want.channel())
            .or(Some(first))
    }

    fn focus_result(&mut self, id: ResultId) {
        self.remembered[self.tab as usize] = Some(id.clone());
        self.focus = Focus::Result(id);
    }

    fn enter_results(&mut self) {
        match self.landing(self.tab) {
            Some(id) => self.focus_result(id),
            None => self.focus = Focus::Input,
        }
    }

    /// Delivers results for `query`; ignored (returns `false`) when the
    /// query has changed since.
    pub fn set_results(
        &mut self,
        query: &str,
        channels: Vec<ChannelHit>,
        programs: Vec<ProgramHit>,
    ) -> bool {
        if query != self.query || self.phase == Phase::Idle {
            return false;
        }
        self.channels = channels;
        self.programs = programs;
        self.phase = if self.channels.is_empty() && self.programs.is_empty() {
            Phase::NoResults
        } else {
            Phase::Results
        };
        if self.count(self.tab) == 0 && self.count(self.tab.other()) > 0 {
            self.tab = self.tab.other();
        }
        match self.focus.clone() {
            Focus::Result(_) | Focus::Tabs if self.phase == Phase::NoResults => {
                self.focus = Focus::Input
            }
            Focus::Result(_) => self.enter_results(),
            Focus::Input if !self.keyboard => self.enter_results(),
            _ => {}
        }
        true
    }

    /// Back from playback: the keyboard stays hidden, focus and query are
    /// kept, and the results are refreshed (airing states move with time).
    pub fn resume(&mut self) -> SearchEffect {
        self.keyboard = false;
        if self.phase == Phase::Idle {
            SearchEffect::None
        } else {
            self.phase = Phase::Typing;
            SearchEffect::Search(self.query.clone())
        }
    }

    pub fn apply(&mut self, action: SearchAction) -> SearchEffect {
        match action {
            SearchAction::Type(q) => self.set_query(q),
            SearchAction::Clear => {
                self.keyboard = true;
                self.set_query(String::new())
            }
            SearchAction::ShowKeyboard => {
                self.keyboard = true;
                self.focus = Focus::Input;
                SearchEffect::None
            }
            SearchAction::Ok => self.ok(),
            SearchAction::Back => self.back(),
            SearchAction::Up => {
                self.step(-1);
                SearchEffect::None
            }
            SearchAction::Down => {
                self.step(1);
                SearchEffect::None
            }
            SearchAction::Left | SearchAction::Right => {
                self.switch_tab();
                SearchEffect::None
            }
            SearchAction::KeyboardHidden => {
                self.keyboard = false;
                SearchEffect::None
            }
            SearchAction::Pick(target) => self.pick(target),
            SearchAction::ShowTab(tab) => {
                self.show_tab(tab);
                SearchEffect::None
            }
        }
    }

    fn pick(&mut self, target: Focus) -> SearchEffect {
        match target {
            Focus::Input => self.apply(SearchAction::ShowKeyboard),
            Focus::History(entry) if self.phase == Phase::Idle => {
                if !self.history.entries().contains(&entry) {
                    return SearchEffect::None;
                }
                self.focus = Focus::History(entry);
                self.ok()
            }
            Focus::Result(id) => {
                if !self.focus_listed(&id) {
                    return SearchEffect::None;
                }
                self.keyboard = false;
                self.ok()
            }
            Focus::History(_) | Focus::Tabs => SearchEffect::None,
        }
    }

    /// Focuses result `id` on its tab, without acting on it, when the
    /// results list it; returns whether they do. For an app that keeps a
    /// focus across results it replaced itself (a guide rebuilt under them).
    pub fn focus_listed(&mut self, id: &ResultId) -> bool {
        let tab = match id {
            ResultId::Channel(_) => Tab::Channels,
            ResultId::Program { .. } => Tab::Programs,
        };
        if self.phase == Phase::Idle || self.position(tab, id).is_none() {
            return false;
        }
        self.tab = tab;
        self.focus_result(id.clone());
        true
    }

    fn show_tab(&mut self, tab: Tab) {
        if self.phase == Phase::Idle || self.count(tab) == 0 {
            return;
        }
        self.tab = tab;
        if matches!(self.focus, Focus::Result(_)) {
            self.enter_results();
        } else {
            self.focus = Focus::Tabs;
        }
    }

    fn set_query(&mut self, q: String) -> SearchEffect {
        self.query = q;
        self.focus = Focus::Input;
        if Query::parse(&self.query).is_empty() {
            self.phase = Phase::Idle;
            self.channels.clear();
            self.programs.clear();
            self.remembered = [None, None];
            SearchEffect::None
        } else {
            self.phase = Phase::Typing;
            SearchEffect::Search(self.query.clone())
        }
    }

    fn ok(&mut self) -> SearchEffect {
        match self.focus.clone() {
            Focus::Input if self.phase != Phase::Idle => {
                self.keyboard = false;
                self.history.push(&self.query);
                if self.phase == Phase::Results {
                    self.enter_results();
                }
                SearchEffect::None
            }
            Focus::Input => SearchEffect::None,
            Focus::History(entry) => {
                self.keyboard = false;
                self.history.push(&entry);
                self.set_query(entry)
            }
            Focus::Tabs => {
                self.enter_results();
                SearchEffect::None
            }
            Focus::Result(id) => {
                self.history.push(&self.query);
                SearchEffect::Play(id)
            }
        }
    }

    fn back(&mut self) -> SearchEffect {
        if self.keyboard && self.phase != Phase::Idle {
            self.keyboard = false;
        } else if self.focus != Focus::Input {
            self.focus = Focus::Input;
        } else {
            return SearchEffect::Close;
        }
        SearchEffect::None
    }

    fn step(&mut self, dir: isize) {
        let has_results = matches!(self.phase, Phase::Results | Phase::Typing)
            && self.count(self.tab) + self.count(self.tab.other()) > 0;
        match self.focus.clone() {
            Focus::Input if dir > 0 && self.phase == Phase::Idle => {
                if let Some(e) = self.history.entries().first() {
                    self.focus = Focus::History(e.clone());
                }
            }
            Focus::Input if dir > 0 && has_results => self.focus = Focus::Tabs,
            Focus::Input => {}
            Focus::History(e) => {
                let entries = self.history.entries();
                let i = entries.iter().position(|x| *x == e).unwrap_or(0);
                self.focus = match i.checked_add_signed(dir) {
                    None => Focus::Input,
                    Some(j) => Focus::History(entries.get(j).unwrap_or(&e).clone()),
                };
            }
            Focus::Tabs if dir < 0 => self.focus = Focus::Input,
            Focus::Tabs => self.enter_results(),
            Focus::Result(id) => {
                let i = self.position(self.tab, &id).unwrap_or(0);
                match i.checked_add_signed(dir) {
                    None => self.focus = Focus::Tabs,
                    Some(j) => {
                        if let Some(next) = self.id_at(self.tab, j) {
                            self.focus_result(next);
                        }
                    }
                }
            }
        }
    }

    fn switch_tab(&mut self) {
        if !matches!(self.focus, Focus::Tabs | Focus::Result(_)) {
            return;
        }
        if self.count(self.tab.other()) == 0 {
            return;
        }
        self.tab = self.tab.other();
        if matches!(self.focus, Focus::Result(_)) {
            self.enter_results();
        }
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
