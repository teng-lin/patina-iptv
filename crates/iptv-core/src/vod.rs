//! Portable on-demand library state. All selections use source/content identity.
//!
//! Besides the per-kind catalogue ([`Query`]), two cross-source discovery
//! shelves ([`Shelf`]) mix movies and series from every enabled playlist.
//! They are read a bounded page at a time ([`DiscoveryQuery`]) in a total,
//! deterministic order, so a page boundary never repeats or skips a title
//! while the library is unchanged:
//!
//! - **Continue watching** lists a movie whose saved progress is
//!   [`Progress::unfinished`]: at least [`CONTINUE_MIN_MS`] in and not
//!   completed. A series contributes its single parent record, which follows
//!   the episode watched last. It is listed while the series is not
//!   completed and either that record is [`CONTINUE_MIN_MS`] in or another
//!   of its episodes was finished or watched that long: a series advanced
//!   to its next episode stays listed even if that episode was stopped at
//!   once, while a series only glanced at does not appear. Once the whole
//!   series is completed, older half-watched episodes cannot bring it back
//!   (only watching one of its episodes again does). A series row resumes
//!   from the start of its next episode when the one watched last was
//!   finished. Newest progress first, then source, kind and content id.
//! - **Recently added** orders by the provider's added (movies) or
//!   last-modified (series) time, newest first. Titles without one follow,
//!   by name, then source, kind and content id. A release year is never
//!   taken for the day a title entered the library.
//!
//! Every failure records the [`Operation`] it interrupted, so Retry runs
//! that operation again (the same page, title, episode or restart) rather
//! than reloading the list.
//!
//! A shelf shows a window of its order, starting at [`Screen::start`].
//! Reopening a shelf finds its focused title again by identity: one bounded
//! read locates the title's current place and returns the page holding it
//! ([`DiscoveryQuery::around`]), however deep it is. Load more extends the
//! window downwards and Load previous (or Up on its first title) upwards,
//! a bounded page at a time; the per-kind catalogue always starts at its top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    #[default]
    Movie,
    Series,
    Episode,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::Episode => "episode",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "movie" => Some(Self::Movie),
            "series" => Some(Self::Series),
            "episode" => Some(Self::Episode),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub source: i64,
    pub kind: Kind,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub key: Key,
    pub name: String,
    pub source_name: String,
    pub categories: Vec<String>,
    pub poster: Option<String>,
    pub plot: Option<String>,
    pub year: Option<u16>,
    pub rating_x10: Option<u16>,
    pub duration_secs: Option<u32>,
    pub progress: Progress,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub completed: bool,
}
impl Progress {
    pub fn observed(position_ms: u64, duration_ms: Option<u64>, ended: bool) -> Self {
        let completed = ended
            || duration_ms
                .is_some_and(|d| d > 0 && position_ms >= d.saturating_sub((d / 20).min(60_000)));
        Self {
            position_ms: duration_ms.map_or(position_ms, |d| position_ms.min(d)),
            duration_ms,
            completed,
        }
    }
    pub fn resume_ms(self) -> u64 {
        if self.unfinished() {
            self.position_ms
        } else {
            0
        }
    }
    /// Whether this saved point belongs in Continue watching: started for at
    /// least [`CONTINUE_MIN_MS`] and not completed (the store applies the
    /// same rule in SQL).
    pub fn unfinished(self) -> bool {
        !self.completed && self.position_ms >= CONTINUE_MIN_MS
    }
}
/// Progress shorter than this resumes from the start and is not listed in
/// Continue watching.
pub const CONTINUE_MIN_MS: u64 = 5_000;
/// A cross-source discovery shelf: movies and series from every enabled
/// playlist in one list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shelf {
    /// Unfinished movies and series, most recently watched first.
    ContinueWatching,
    /// Newest provider additions first; undated titles follow by name.
    RecentlyAdded,
}
/// Titles a discovery page asks for by default.
pub const DISCOVERY_PAGE: u32 = 40;
/// The most titles one discovery page returns, whatever is asked.
pub const DISCOVERY_PAGE_MAX: u32 = 100;
/// One bounded page of a discovery shelf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscoveryQuery {
    pub shelf: Shelf,
    /// The shelf position the page starts at.
    pub offset: u32,
    pub limit: u32,
}
impl DiscoveryQuery {
    /// The shelf's first page.
    pub fn first(shelf: Shelf) -> Self {
        Self {
            shelf,
            offset: 0,
            limit: DISCOVERY_PAGE,
        }
    }
    /// The page starting at shelf position `loaded` (after that many titles).
    pub fn after(shelf: Shelf, loaded: usize) -> Self {
        Self {
            offset: u32::try_from(loaded).unwrap_or(u32::MAX),
            ..Self::first(shelf)
        }
    }
    /// The page size actually read: at least one, at most
    /// [`DISCOVERY_PAGE_MAX`].
    pub fn bounded_limit(self) -> u32 {
        self.limit.clamp(1, DISCOVERY_PAGE_MAX)
    }
    /// The one page holding the title that `rank` titles precede in the
    /// shelf's current order: pages are aligned to [`DISCOVERY_PAGE`], so
    /// the window a reopened shelf shows around its focused title is the
    /// same page Load more and Load previous step through. A title gone
    /// from the shelf (`rank` is `None`) gets the first page. However deep
    /// the title is, this is a single bounded read: no earlier page is
    /// read again to reach it.
    pub fn around(shelf: Shelf, rank: Option<u32>) -> Self {
        let rank = rank.unwrap_or(0);
        Self {
            offset: rank - rank % DISCOVERY_PAGE,
            ..Self::first(shelf)
        }
    }
    /// The page just before a window starting at `start`, or `None` at the
    /// shelf's top.
    pub fn before(shelf: Shelf, start: u32) -> Option<Self> {
        let offset = start.checked_sub(1)? / DISCOVERY_PAGE * DISCOVERY_PAGE;
        Some(Self {
            shelf,
            offset,
            limit: start - offset,
        })
    }
}
/// A discovery page: its titles, where they start in the shelf's order
/// and whether another page follows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryPage {
    pub cards: Vec<Card>,
    pub more: bool,
    /// The shelf position of the first card.
    pub start: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    pub source: i64,
    pub id: String,
    pub name: String,
    pub adult: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: i64,
    pub name: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Newest,
    Year,
    Rating,
}
impl Sort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Newest,
            Self::Newest => Self::Year,
            Self::Year => Self::Rating,
            Self::Rating => Self::Name,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub kind: Kind,
    pub source: Option<i64>,
    pub category: Option<(i64, String)>,
    pub text: String,
    pub sort: Sort,
    pub continue_watching: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Episode {
    pub key: Key,
    pub season: u32,
    pub number: u32,
    pub title: String,
    pub plot: Option<String>,
    pub duration_secs: Option<u32>,
    pub progress: Progress,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Details {
    /// Opaque provider-source incarnation fences queued playback/progress after edits.
    pub incarnation: String,
    pub item: Card,
    pub episodes: Vec<Episode>,
    /// The last episode watched, kept by content identity across app restarts.
    pub resume_episode: Option<Key>,
}
impl Details {
    pub fn preferred_episode(&self) -> Option<&Episode> {
        if let Some(at) = self
            .resume_episode
            .as_ref()
            .and_then(|key| self.episodes.iter().position(|episode| &episode.key == key))
        {
            let current = &self.episodes[at];
            return if current.progress.completed {
                self.episodes[at + 1..]
                    .iter()
                    .find(|episode| !episode.progress.completed)
                    .or(Some(current))
            } else {
                Some(current)
            };
        }
        self.episodes
            .iter()
            .find(|episode| !episode.progress.completed)
            .or_else(|| self.episodes.first())
    }
    pub fn seasons(&self) -> Vec<u32> {
        let mut out: Vec<_> = self.episodes.iter().map(|e| e.season).collect();
        out.sort();
        out.dedup();
        out
    }
    pub fn next_episode(&self, key: &Key) -> Option<&Episode> {
        self.episodes
            .iter()
            .position(|e| &e.key == key)
            .and_then(|i| self.episodes.get(i + 1))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playing {
    pub key: Key,
    pub parent: Option<Key>,
    pub title: String,
    pub progress: Progress,
    pub details: Details,
    pub last_saved_ms: Option<u64>,
    pub ended: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Control {
    Search,
    Source,
    Category,
    Sort,
    Continue,
    #[default]
    Catalogue,
    Play,
    Restart,
    Seasons,
    Episodes,
    Next,
    Block,
    /// Shown with an error: runs the failed operation again.
    Retry,
}
/// An operation that failed, which Retry runs again exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    /// The list: a catalogue from its top, a shelf around its focused title.
    List,
    /// The page starting at shelf position `offset` (the window's start plus
    /// its titles); those already shown stay, and so does the focus.
    More { offset: u32 },
    /// The page before a shelf window starting at `start`; the titles shown
    /// stay, and so does the focus (`step_up`: then it moves to the title
    /// just above, as Up on the first title asked).
    Previous { start: u32, step_up: bool },
    /// Opening this title's details.
    Details(Key),
    /// Playing `key` (the movie, or one episode of the series `item`), from
    /// its start when `restart`, as the playlist was when it was chosen
    /// (`incarnation`, opaque). Retry reads `item`'s details afresh, which
    /// checks the parental lock and the playlist's policy again, and plays
    /// only if the playlist is still that incarnation ([`Retried`]): a
    /// playlist deleted and added again, or edited, under the same id is a
    /// different one, and its title must be chosen again. A fresh provider
    /// link is then resolved for the same `key`.
    Play {
        item: Key,
        key: Key,
        restart: bool,
        incarnation: String,
    },
}
/// What fresh details mean for a retried playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retried {
    /// Same playlist incarnation, and the chosen title or episode is still
    /// there: shown with it chosen, ready to play.
    Ready,
    /// Same playlist, but a refresh removed the episode: the fresh details
    /// are shown to choose another.
    EpisodeGone,
    /// The playlist is not the one the title was chosen from (deleted and
    /// added again, or edited): nothing is shown or played from it.
    PlaylistChanged,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub control: Control,
    pub open: bool,
    /// A cross-source shelf instead of the `query.kind` catalogue: the
    /// query's kind, filters and sort do not apply to it.
    pub shelf: Option<Shelf>,
    pub query: Query,
    pub sources: Vec<Source>,
    pub categories: Vec<Category>,
    pub cards: Vec<Card>,
    /// A shelf's position of `cards[0]` in its order: titles before it can
    /// be loaded with Load previous. Always 0 for a catalogue.
    pub start: u32,
    /// Next store position after the pages consumed. Refreshes can repeat
    /// titles in a later page, so this must not use deduplicated card count.
    pub end: u32,
    pub focused: Option<Key>,
    pub details: Option<Details>,
    pub season: Option<u32>,
    pub episode: Option<Key>,
    pub busy: bool,
    pub has_more: bool,
    pub generation: u64,
    pub error: Option<String>,
    /// What Retry repeats; set with [`Screen::error`] by [`Screen::fail`].
    pub retry: Option<Operation>,
    pub notice: Option<String>,
}
impl Screen {
    pub fn begin(&mut self, kind: Kind) -> u64 {
        *self = Self {
            open: true,
            query: Query {
                kind,
                ..Default::default()
            },
            generation: self.generation.wrapping_add(1),
            ..Default::default()
        };
        self.reload()
    }
    /// Opens a cross-source discovery shelf. Its list has no toolbar: the
    /// titles are the only control until an error offers Retry. Opening the
    /// shelf shown last (back from playback) keeps its focused title, found
    /// again by identity when the new page arrives.
    pub fn begin_shelf(&mut self, shelf: Shelf) -> u64 {
        let focused = if self.shelf == Some(shelf) {
            self.focused.take()
        } else {
            None
        };
        *self = Self {
            focused,
            open: true,
            shelf: Some(shelf),
            generation: self.generation.wrapping_add(1),
            ..Default::default()
        };
        self.reload()
    }
    /// Whether a discovery shelf, rather than a catalogue, is shown.
    pub fn is_shelf(&self) -> bool {
        self.shelf.is_some()
    }
    pub fn reload(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.busy = true;
        self.details = None;
        self.episode = None;
        self.clear_failure();
        self.generation
    }
    /// Starts reading the page after the titles shown: they and the focus
    /// stay. Returns the next store position actually consumed, including
    /// repeated titles removed from a page after a library refresh.
    pub fn begin_more(&mut self) -> u32 {
        self.busy = true;
        self.clear_failure();
        self.end
    }
    /// Whether a shelf window starts below its top, so Load previous (or Up
    /// on its first title) has titles to read.
    pub fn has_previous(&self) -> bool {
        self.is_shelf() && self.start > 0
    }
    /// Starts reading the page before a shelf window; the titles shown and
    /// the focus stay. `None` for a catalogue or at the shelf's top.
    pub fn begin_previous(&mut self) -> Option<DiscoveryQuery> {
        let query = DiscoveryQuery::before(self.shelf?, self.start)?;
        self.busy = true;
        self.clear_failure();
        Some(query)
    }
    /// A page read before the window starting at `start` arrived: prepended,
    /// keeping every title shown (one repeated by a reorder meanwhile stays
    /// where it is) and the focus by identity. With `step_up` (Up on the
    /// first title asked for it) the focus moves to the title just above.
    pub fn publish_previous(
        &mut self,
        generation: u64,
        start: u32,
        page: DiscoveryPage,
        step_up: bool,
    ) -> bool {
        if generation != self.generation || !self.open || start != self.start {
            return false;
        }
        let first = self.cards.first().map(|c| c.key.clone());
        let mut earlier: Vec<_> = page
            .cards
            .into_iter()
            .filter(|card| !self.cards.iter().any(|c| c.key == card.key))
            .collect();
        let added = earlier.len();
        earlier.append(&mut self.cards);
        self.cards = earlier;
        self.start = page.start;
        self.busy = false;
        if step_up && added > 0 && self.focused == first {
            self.move_focus(-1);
        }
        true
    }
    /// A shelf window arrived (a reopened shelf around its focused title):
    /// its titles replace the list from shelf position `page.start`, and
    /// the focus stays on its title by identity, or falls to the first one
    /// when the shelf no longer lists it.
    pub fn publish_window(&mut self, generation: u64, page: DiscoveryPage) -> bool {
        let start = page.start;
        let end = start.saturating_add(u32::try_from(page.cards.len()).unwrap_or(u32::MAX));
        if !self.publish(generation, page.cards, page.more, false) {
            return false;
        }
        self.start = start;
        self.end = end;
        true
    }
    /// Starts opening details or playing: a newer generation, so an older
    /// reply is ignored, with any failure shown before cleared.
    pub fn begin_operation(&mut self) -> u64 {
        self.busy = true;
        self.clear_failure();
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }
    fn clear_failure(&mut self) {
        self.error = None;
        self.retry = None;
        if self.control == Control::Retry {
            self.control = if self.details.is_some() {
                Control::Play
            } else {
                Control::Catalogue
            };
        }
    }
    /// `operation` failed: `message` is a catalogue key, and Retry runs the
    /// same operation again. With nothing else to act on, Retry takes the
    /// focus so OK repeats it; otherwise the focus stays where it was (the
    /// focused title, a loaded page, the Play button).
    pub fn fail(&mut self, message: impl Into<String>, operation: Operation) {
        self.busy = false;
        self.error = Some(message.into());
        self.retry = Some(operation);
        if self.details.is_none() && self.cards.is_empty() {
            self.control = Control::Retry;
        }
    }
    /// Retry: the failed operation, for the shell to run again. Its error
    /// stays shown until that operation starts.
    pub fn retry_operation(&self) -> Option<Operation> {
        self.retry.clone()
    }
    /// Fresh details for a retried playback of `key` chosen from the
    /// playlist `incarnation`. Details of another incarnation (the playlist
    /// deleted and added again, or edited, under the same id) are refused:
    /// the stale details close, so the title must be chosen again from the
    /// list. Otherwise they are shown with `key` (the same episode) still
    /// chosen, or the preferred episode when a refresh removed it.
    pub fn show_details_keeping(
        &mut self,
        details: Details,
        key: &Key,
        incarnation: &str,
    ) -> Retried {
        if details.incarnation != incarnation {
            self.playlist_changed();
            return Retried::PlaylistChanged;
        }
        let kept = if details.item.key == *key {
            Some(None)
        } else {
            details
                .episodes
                .iter()
                .find(|e| &e.key == key)
                .map(|e| Some(e.season))
        };
        let control = self.control;
        self.show_details(details);
        if let Some(season) = kept {
            if let Some(season) = season {
                self.episode = Some(key.clone());
                self.season = Some(season);
            }
            // The control that failed (Play or Restart) keeps the focus.
            if matches!(control, Control::Play | Control::Restart) {
                self.control = control;
            }
            Retried::Ready
        } else {
            Retried::EpisodeGone
        }
    }
    /// A retried playback found its playlist changed (deleted and added
    /// again, or edited, under the same id): the stale details close and
    /// nothing plays; the title stays focused in the list, to be chosen
    /// again from the playlist as it is now.
    pub fn playlist_changed(&mut self) {
        self.busy = false;
        self.details = None;
        self.episode = None;
        self.retry = None;
        self.control = Control::Catalogue;
    }
    /// Whether Play resumes: a movie's own saved point, or the chosen
    /// episode's. A series' own record (the episode watched last) does not
    /// apply when another episode, such as the next one, is chosen.
    pub fn resumes(&self) -> bool {
        self.details.as_ref().is_some_and(|d| {
            if d.item.key.kind == Kind::Movie {
                d.item.progress.resume_ms() > 0
            } else {
                d.episodes
                    .iter()
                    .find(|e| Some(&e.key) == self.episode.as_ref())
                    .is_some_and(|e| e.progress.resume_ms() > 0)
            }
        })
    }
    /// The controls Left/Right moves between now, in screen order. A shelf
    /// shows no search, filter or sort controls, so none are offered.
    pub fn controls(&self) -> Vec<Control> {
        let mut out = Vec::new();
        if self.retry.is_some() {
            out.push(Control::Retry);
        }
        if self
            .details
            .as_ref()
            .is_some_and(|d| d.item.key.kind == Kind::Movie)
        {
            out.extend([Control::Play, Control::Restart, Control::Block]);
        } else if let Some(details) = &self.details {
            out.extend([
                Control::Play,
                Control::Restart,
                Control::Seasons,
                Control::Episodes,
            ]);
            if self
                .episode
                .as_ref()
                .is_some_and(|key| details.next_episode(key).is_some())
            {
                out.push(Control::Next);
            }
            out.push(Control::Block);
        } else if self.is_shelf() {
            if !self.cards.is_empty() {
                out.push(Control::Catalogue);
            }
        } else {
            out.extend([
                Control::Search,
                Control::Source,
                Control::Category,
                Control::Sort,
                Control::Continue,
            ]);
            if !self.cards.is_empty() {
                out.push(Control::Catalogue);
            }
        }
        out
    }
    pub fn publish(&mut self, generation: u64, cards: Vec<Card>, more: bool, append: bool) -> bool {
        if generation != self.generation || !self.open {
            return false;
        }
        let consumed = u32::try_from(cards.len()).unwrap_or(u32::MAX);
        if !append {
            self.cards = cards;
            self.start = 0;
            self.end = consumed;
        } else {
            for card in cards {
                if !self.cards.iter().any(|c| c.key == card.key) {
                    self.cards.push(card);
                }
            }
            self.end = self.end.saturating_add(consumed);
        }
        if !self
            .focused
            .as_ref()
            .is_some_and(|k| self.cards.iter().any(|c| &c.key == k))
        {
            self.focused = self.cards.first().map(|c| c.key.clone());
        }
        self.busy = false;
        self.has_more = more;
        true
    }
    pub fn move_focus(&mut self, delta: isize) {
        let at = self
            .focused
            .as_ref()
            .and_then(|k| self.cards.iter().position(|c| &c.key == k))
            .unwrap_or(0);
        let at =
            (at as isize + delta).clamp(0, self.cards.len().saturating_sub(1) as isize) as usize;
        self.focused = self.cards.get(at).map(|c| c.key.clone());
    }
    pub fn show_details(&mut self, details: Details) {
        self.season = details.seasons().first().copied();
        self.episode = details.preferred_episode().map(|e| e.key.clone());
        self.season = details
            .episodes
            .iter()
            .find(|e| Some(&e.key) == self.episode.as_ref())
            .map(|e| e.season)
            .or(self.season);
        self.control = Control::Play;
        self.focused = Some(details.item.key.clone());
        self.details = Some(details);
        self.busy = false;
    }
    pub fn move_control(&mut self, delta: isize) {
        let controls = self.controls();
        if controls.is_empty() {
            // An empty successful shelf has no arrow-focus target. Keep its
            // neutral content anchor; Back remains available in the shell.
            self.control = Control::Catalogue;
            return;
        }
        let at = controls
            .iter()
            .position(|c| *c == self.control)
            .unwrap_or_else(|| {
                // A control that is gone (Retry after a success) restarts
                // from the content, not from a hidden control.
                controls
                    .iter()
                    .position(|c| matches!(c, Control::Catalogue | Control::Play))
                    .unwrap_or(0)
            });
        self.control =
            controls[(at as isize + delta).clamp(0, controls.len() as isize - 1) as usize];
    }
    pub fn move_episode(&mut self, delta: isize) {
        if let Some(d) = &self.details {
            let eps: Vec<_> = d
                .episodes
                .iter()
                .filter(|e| Some(e.season) == self.season)
                .collect();
            let at = eps
                .iter()
                .position(|e| Some(&e.key) == self.episode.as_ref())
                .unwrap_or(0);
            let at = (at as isize + delta).clamp(0, eps.len().saturating_sub(1) as isize) as usize;
            self.episode = eps.get(at).map(|e| e.key.clone());
        }
    }
    pub fn cycle_season(&mut self, delta: isize) {
        if let Some(d) = &self.details {
            let seasons = d.seasons();
            let at = seasons
                .iter()
                .position(|n| Some(*n) == self.season)
                .unwrap_or(0);
            self.season = seasons
                .get(
                    (at as isize + delta).clamp(0, seasons.len().saturating_sub(1) as isize)
                        as usize,
                )
                .copied();
            self.episode = d
                .episodes
                .iter()
                .find(|e| Some(e.season) == self.season)
                .map(|e| e.key.clone());
        }
    }
    pub fn back(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.busy = false;
        self.error = None;
        self.retry = None;
        if self.control == Control::Retry {
            self.control = Control::Catalogue;
        }
        if self.details.take().is_none() {
            self.open = false;
            self.generation = self.generation.wrapping_add(1);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn card(id: &str) -> Card {
        Card {
            key: Key {
                source: 1,
                kind: Kind::Movie,
                id: id.into(),
            },
            name: id.into(),
            source_name: String::new(),
            categories: vec![],
            poster: None,
            plot: None,
            year: None,
            rating_x10: None,
            duration_secs: None,
            progress: Progress::default(),
        }
    }
    #[test]
    fn refresh_restores_content_identity_and_rejects_old_results() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(g, vec![card("a"), card("b")], false, false);
        s.move_focus(1);
        let g = s.reload();
        assert!(!s.publish(g - 1, vec![], false, false));
        s.publish(g, vec![card("b"), card("a")], false, false);
        assert_eq!(s.focused, Some(card("b").key));
    }
    #[test]
    fn completed_media_does_not_resume_near_its_ending() {
        assert_eq!(
            Progress::observed(99_000, Some(100_000), false).resume_ms(),
            0
        );
        assert_eq!(
            Progress::observed(30_000, Some(100_000), false).resume_ms(),
            30_000
        );
    }
    #[test]
    fn independent_sources_do_not_share_identity() {
        let a = card("same");
        let mut b = a.clone();
        b.key.source = 2;
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(g, vec![a, b], false, false);
        assert_eq!(s.cards.len(), 2);
    }
    fn keyed(source: i64, kind: Kind, id: &str) -> Card {
        let mut c = card(id);
        c.key.source = source;
        c.key.kind = kind;
        c
    }
    #[test]
    fn continue_watching_needs_five_seconds_and_no_completion() {
        assert!(!Progress::observed(4_999, Some(100_000), false).unfinished());
        assert!(Progress::observed(CONTINUE_MIN_MS, Some(100_000), false).unfinished());
        assert!(!Progress::observed(99_000, Some(100_000), false).unfinished());
        assert!(!Progress::observed(30_000, Some(100_000), true).unfinished());
        assert!(
            Progress::observed(30_000, None, false).unfinished(),
            "a live-length unknown title still resumes"
        );
    }
    #[test]
    fn discovery_pages_are_bounded_and_start_after_what_is_loaded() {
        let first = DiscoveryQuery::first(Shelf::RecentlyAdded);
        assert_eq!((first.offset, first.bounded_limit()), (0, DISCOVERY_PAGE));
        let next = DiscoveryQuery::after(Shelf::RecentlyAdded, 40);
        assert_eq!(next.offset, 40);
        let huge = DiscoveryQuery {
            limit: u32::MAX,
            ..first
        };
        assert_eq!(huge.bounded_limit(), DISCOVERY_PAGE_MAX);
        let none = DiscoveryQuery { limit: 0, ..first };
        assert_eq!(none.bounded_limit(), 1);
        assert_eq!(
            DiscoveryQuery::after(Shelf::ContinueWatching, usize::MAX).offset,
            u32::MAX
        );
    }
    #[test]
    fn a_shelf_mixes_kinds_and_sources_as_separate_identities() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::ContinueWatching);
        assert!(s.is_shelf() && s.open && s.busy);
        let cards = vec![
            keyed(1, Kind::Movie, "same"),
            keyed(1, Kind::Series, "same"),
            keyed(2, Kind::Movie, "same"),
            keyed(2, Kind::Series, "same"),
        ];
        assert!(s.publish(g, cards.clone(), true, false));
        assert_eq!(s.cards.len(), 4, "kind and source keep duplicates apart");
        s.move_focus(2);
        assert_eq!(s.focused, Some(cards[2].key.clone()));
        // A refresh reorders the shelf (newest progress first): the focus
        // follows the title, not its row.
        let g = s.reload();
        let mut reordered = cards.clone();
        reordered.reverse();
        s.publish(g, reordered, false, false);
        assert_eq!(s.focused, Some(cards[2].key.clone()));
        // The next page repeats nothing already shown.
        s.publish(
            g,
            vec![keyed(2, Kind::Movie, "same"), keyed(3, Kind::Movie, "same")],
            false,
            true,
        );
        assert_eq!(s.cards.len(), 5);
    }
    #[test]
    fn a_shelf_offers_only_its_titles_until_an_error_offers_retry() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::RecentlyAdded);
        assert!(s.controls().is_empty());
        s.move_control(-1);
        assert_eq!(s.control, Control::Catalogue, "no hidden toolbar to reach");
        s.fail("On-demand library could not be loaded", Operation::List);
        assert!(!s.busy);
        assert_eq!(s.control, Control::Retry, "OK repeats the failed load");
        assert_eq!(s.controls(), vec![Control::Retry]);
        assert_eq!(s.retry_operation(), Some(Operation::List));
        let g2 = s.reload();
        assert!(g2 != g && s.error.is_none() && s.retry.is_none());
        assert_eq!(s.control, Control::Catalogue);
        s.publish(g2, vec![keyed(1, Kind::Movie, "a")], false, false);
        assert_eq!(s.controls(), vec![Control::Catalogue]);
        // An error over a list keeps the focus on the titles.
        s.fail(
            "Details unavailable; refresh the playlist and try again",
            Operation::Details(keyed(1, Kind::Movie, "a").key),
        );
        assert_eq!(s.control, Control::Catalogue);
        s.move_control(-1);
        assert_eq!(s.control, Control::Retry);
        s.move_control(1);
        assert_eq!(
            s.control,
            Control::Catalogue,
            "populated cards remain reachable"
        );
        s.move_control(-1);
        s.back();
        assert_eq!(s.control, Control::Catalogue);
        assert!(!s.open, "Back from the list closes the shelf");
    }
    #[test]
    fn empty_shelf_navigation_stays_on_retry_and_empty_success_has_no_targets() {
        for shelf in [Shelf::ContinueWatching, Shelf::RecentlyAdded] {
            let mut screen = Screen::default();
            let generation = screen.begin_shelf(shelf);
            assert!(screen.publish(generation, Vec::new(), false, false));
            assert!(screen.controls().is_empty());
            for delta in [-1, 1] {
                screen.move_control(delta);
                assert_eq!(screen.control, Control::Catalogue);
                assert!(screen.focused.is_none());
            }
            screen.fail("On-demand library could not be loaded", Operation::List);
            assert_eq!(screen.controls(), [Control::Retry]);
            for stale in [Control::Retry, Control::Catalogue] {
                for delta in [-1, 1] {
                    screen.control = stale;
                    screen.move_control(delta);
                    assert_eq!(
                        screen.control,
                        Control::Retry,
                        "empty errors offer only visible Retry"
                    );
                }
            }
            screen.back();
            assert!(!screen.open, "empty shelves still dismiss with Back");
        }
    }

    #[test]
    fn reopening_a_shelf_returns_to_its_title_by_identity() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::ContinueWatching);
        let cards = vec![keyed(1, Kind::Movie, "a"), keyed(2, Kind::Series, "b")];
        s.publish(g, cards.clone(), false, false);
        s.move_focus(1);
        // Playback closed the shelf; the title moved to the top meanwhile.
        s.open = false;
        let g = s.begin_shelf(Shelf::ContinueWatching);
        s.publish(g, vec![cards[1].clone(), cards[0].clone()], false, false);
        assert_eq!(s.focused, Some(cards[1].key.clone()));
        // A finished title has left: the focus falls to the first one.
        let g = s.begin_shelf(Shelf::ContinueWatching);
        s.publish(g, vec![cards[0].clone()], false, false);
        assert_eq!(s.focused, Some(cards[0].key.clone()));
        // Another shelf or a catalogue starts from its own top.
        let g = s.begin_shelf(Shelf::RecentlyAdded);
        assert_eq!(s.focused, None);
        s.publish(g, cards.clone(), false, false);
        s.move_focus(1);
        s.begin(Kind::Movie);
        assert_eq!(s.focused, None);
    }
    #[test]
    fn back_from_details_returns_to_the_same_shelf_title() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::ContinueWatching);
        let cards = vec![keyed(1, Kind::Movie, "a"), keyed(2, Kind::Series, "b")];
        s.publish(g, cards.clone(), false, false);
        s.show_details(Details {
            incarnation: "generated".into(),
            item: cards[1].clone(),
            episodes: vec![],
            resume_episode: None,
        });
        assert_eq!(s.control, Control::Play);
        assert!(s.controls().contains(&Control::Seasons));
        s.back();
        s.control = Control::Catalogue;
        assert!(s.open && s.details.is_none() && s.is_shelf());
        assert_eq!(s.focused, Some(cards[1].key.clone()));
        assert_eq!(s.controls(), vec![Control::Catalogue]);
    }
    fn numbered(count: usize) -> Vec<Card> {
        (0..count)
            .map(|i| {
                keyed(
                    1 + (i % 2) as i64,
                    Kind::Movie,
                    &format!("generated-{i:03}"),
                )
            })
            .collect()
    }
    fn series_details(episodes: usize) -> Details {
        let item = keyed(1, Kind::Series, "generated-series");
        Details {
            incarnation: "generated".into(),
            episodes: (1..=episodes as u32)
                .map(|number| Episode {
                    key: Key {
                        source: 1,
                        kind: Kind::Episode,
                        id: format!("generated-series:{number}"),
                    },
                    season: 1,
                    number,
                    title: format!("Generated episode {number}"),
                    plot: None,
                    duration_secs: Some(120),
                    progress: Progress::default(),
                })
                .collect(),
            item,
            resume_episode: None,
        }
    }
    #[test]
    fn retry_after_a_details_failure_opens_the_same_title_again() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::ContinueWatching);
        let cards = numbered(3);
        s.publish(g, cards.clone(), false, false);
        s.move_focus(2);
        // Choosing a title: its details fail before anything opened.
        s.begin_operation();
        s.fail(
            "Details unavailable; refresh the playlist and try again",
            Operation::Details(cards[2].key.clone()),
        );
        assert!(s.details.is_none() && !s.busy);
        assert_eq!(
            s.retry_operation(),
            Some(Operation::Details(cards[2].key.clone())),
            "Retry opens the title again, not the list's first page"
        );
        assert_eq!(s.focused, Some(cards[2].key.clone()));
        assert_eq!(s.cards, cards, "the list is not reloaded");
        assert!(s.controls().contains(&Control::Retry));
        // Retrying starts the same operation: the error goes, and only a
        // newer reply is accepted.
        let before = s.generation;
        let again = s.begin_operation();
        assert!(again != before && s.error.is_none() && s.retry.is_none());
        s.show_details(series_details(2));
        assert_eq!(s.control, Control::Play);
    }
    #[test]
    fn the_last_episode_skips_the_unmounted_next_action() {
        let mut screen = Screen::default();
        screen.begin_shelf(Shelf::ContinueWatching);
        let details = series_details(2);
        screen.show_details(details.clone());
        screen.episode = Some(details.episodes[1].key.clone());
        screen.control = Control::Episodes;
        screen.move_control(1);
        assert_eq!(
            screen.control,
            Control::Block,
            "Right must reach a visible action, not the absent Next button"
        );
        screen.move_control(-1);
        assert_eq!(screen.control, Control::Episodes);
        screen.episode = Some(details.episodes[0].key.clone());
        screen.move_control(1);
        assert_eq!(
            screen.control,
            Control::Next,
            "Next remains reachable when usable"
        );
    }
    #[test]
    fn load_more_advances_after_a_refresh_repeats_a_whole_page() {
        let shelf = Shelf::RecentlyAdded;
        let original = numbered(120);
        let mut screen = Screen::default();
        let generation = screen.begin_shelf(shelf);
        screen.publish_window(generation, read(&original, DiscoveryQuery::first(shelf)));
        screen.move_focus(39);
        let focused = screen.focused.clone();
        // Forty new additions move the already shown titles onto the next
        // page. Deduplication must not make Load more request it forever.
        let mut refreshed: Vec<_> = (0..40)
            .map(|n| card(&format!("generated-new-{n:03}")))
            .collect();
        refreshed.extend(original.clone());
        for expected_offset in [40, 80, 120] {
            let offset = screen.begin_more();
            assert_eq!(offset, expected_offset);
            let page = read(&refreshed, DiscoveryQuery::after(shelf, offset as usize));
            assert!(screen.publish(generation, page.cards, page.more, true));
            assert_eq!(screen.focused, focused);
            if offset == 40 {
                assert_eq!(screen.cards.len(), 40, "the repeated page is deduplicated");
                assert!(screen.has_more);
            }
        }
        assert_eq!(screen.cards, original);
        assert!(!screen.has_more, "all remaining titles were reached");
    }
    #[test]
    fn retry_after_a_later_page_fails_keeps_the_pages_and_focus() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::RecentlyAdded);
        let all = numbered(100);
        s.publish(g, all[..40].to_vec(), true, false);
        s.move_focus(39);
        let focused = s.focused.clone();
        let offset = s.begin_more();
        assert_eq!(offset, 40);
        assert!(s.busy);
        s.fail(
            "On-demand library could not be loaded",
            Operation::More { offset },
        );
        assert_eq!(s.cards.len(), 40, "loaded pages stay");
        assert_eq!(s.focused, focused);
        assert_eq!(
            s.control,
            Control::Catalogue,
            "the focus stays on the titles"
        );
        assert!(s.has_more);
        assert_eq!(s.retry_operation(), Some(Operation::More { offset: 40 }));
        // Retry reads the same next page and appends it.
        assert_eq!(s.begin_more(), 40);
        assert!(s.error.is_none());
        assert!(s.publish(g, all[40..80].to_vec(), true, true));
        assert_eq!(s.cards.len(), 80);
        assert_eq!(s.focused, focused);
    }
    #[test]
    fn retry_after_playback_fails_plays_the_same_episode_and_restart() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::ContinueWatching);
        let mut details = series_details(4);
        s.publish(g, vec![details.item.clone()], false, false);
        s.show_details(details.clone());
        assert_eq!(s.episode, Some(details.episodes[0].key.clone()));
        // Down twice chooses the third episode; Restart plays it.
        s.move_episode(2);
        let third = details.episodes[2].key.clone();
        assert_eq!(s.episode, Some(third.clone()));
        s.control = Control::Restart;
        s.begin_operation();
        let failed = Operation::Play {
            item: details.item.key.clone(),
            key: third.clone(),
            restart: true,
            incarnation: details.incarnation.clone(),
        };
        s.fail(
            "Playback unavailable; refresh the playlist and try again",
            failed.clone(),
        );
        assert_eq!(s.retry_operation(), Some(failed));
        assert_eq!(s.episode, Some(third.clone()), "the chosen episode stays");
        assert_eq!(s.control, Control::Restart);
        assert!(s.controls().starts_with(&[Control::Retry, Control::Play]));
        // Retry reads fresh details first (their own preference is another
        // episode) and still plays the third episode from its start.
        s.begin_operation();
        details.resume_episode = Some(details.episodes[1].key.clone());
        details.episodes[1].progress = Progress::observed(30_000, Some(120_000), false);
        let chosen = details.incarnation.clone();
        assert_eq!(
            s.show_details_keeping(details.clone(), &third, &chosen),
            Retried::Ready
        );
        assert_eq!(s.episode, Some(third.clone()));
        assert_eq!(s.control, Control::Restart);
        // A refresh that removed the episode cannot play it.
        details.episodes.remove(2);
        assert_eq!(
            s.show_details_keeping(details.clone(), &third, &chosen),
            Retried::EpisodeGone
        );
        assert_eq!(s.episode, Some(details.episodes[1].key.clone()));
        // A movie keeps its own identity.
        let movie = Details {
            item: keyed(2, Kind::Movie, "generated-film"),
            episodes: vec![],
            ..details
        };
        let key = movie.item.key.clone();
        assert_eq!(s.show_details_keeping(movie, &key, &chosen), Retried::Ready);
        assert_eq!(s.episode, None);
    }
    #[test]
    fn retried_playback_never_rebinds_to_a_recreated_or_edited_playlist() {
        let mut s = Screen::default();
        let g = s.begin_shelf(Shelf::ContinueWatching);
        let details = series_details(3);
        s.publish(g, vec![details.item.clone()], false, false);
        s.show_details(details.clone());
        let second = details.episodes[1].key.clone();
        s.episode = Some(second.clone());
        s.begin_operation();
        s.fail(
            "Playback unavailable; refresh the playlist and try again",
            Operation::Play {
                item: details.item.key.clone(),
                key: second.clone(),
                restart: false,
                incarnation: details.incarnation.clone(),
            },
        );
        let Some(Operation::Play { incarnation, .. }) = s.retry_operation() else {
            panic!("playback is retried as playback");
        };
        // Meanwhile the playlist was deleted and added again (or its
        // credentials edited) under the same id: the same keys, a new
        // incarnation. Its fresh details are not this title.
        s.begin_operation();
        let recreated = Details {
            incarnation: "generated-recreated".into(),
            ..details.clone()
        };
        assert_eq!(
            s.show_details_keeping(recreated, &second, &incarnation),
            Retried::PlaylistChanged
        );
        assert!(
            s.details.is_none(),
            "nothing from the new playlist is shown"
        );
        assert_eq!(s.episode, None);
        assert_eq!(s.control, Control::Catalogue);
        assert_eq!(
            s.focused,
            Some(details.item.key.clone()),
            "the title stays focused, to be chosen again"
        );
    }
    /// A generated shelf order and the page a [`DiscoveryQuery`] reads from
    /// it, as the store would (the store tests check the store itself).
    fn read(order: &[Card], query: DiscoveryQuery) -> DiscoveryPage {
        let start = (query.offset as usize).min(order.len());
        let end = (start + query.bounded_limit() as usize).min(order.len());
        DiscoveryPage {
            cards: order[start..end].to_vec(),
            more: end < order.len(),
            start: query.offset,
        }
    }
    #[test]
    fn reopening_a_shelf_shows_the_window_around_a_deep_title() {
        let shelf = Shelf::RecentlyAdded;
        let order = numbered(6_000);
        let target = order[5_000].key.clone();
        let mut s = Screen::default();
        let g = s.begin_shelf(shelf);
        s.publish(
            g,
            read(&order, DiscoveryQuery::first(shelf)).cards,
            true,
            false,
        );
        s.focused = Some(target.clone());
        // Playback or Back closed the shelf; reopening it keeps the title
        // and reads the one bounded page holding its current place.
        s.open = false;
        let g = s.begin_shelf(shelf);
        assert_eq!(s.focused, Some(target.clone()));
        let window = DiscoveryQuery::around(shelf, Some(5_000));
        assert_eq!(
            (window.offset, window.bounded_limit()),
            (5_000, DISCOVERY_PAGE)
        );
        assert!(s.publish_window(g, read(&order, window)));
        assert_eq!(s.focused, Some(target.clone()), "found by key, not row");
        assert_eq!(s.start, 5_000);
        assert_eq!(
            s.cards.len(),
            DISCOVERY_PAGE as usize,
            "no earlier page re-read"
        );
        assert!(s.has_previous());
        // Load more continues below the window, not after 40 titles.
        let offset = s.begin_more();
        assert_eq!(offset, 5_040);
        let below = read(&order, DiscoveryQuery::after(shelf, offset as usize));
        assert_eq!(below.cards[0].key, order[5_040].key);
        assert!(s.publish(g, below.cards, below.more, true));
        assert_eq!(s.start, 5_000);
        // Up on the first title reads the page above and steps onto it.
        s.focused = Some(order[5_000].key.clone());
        let above = s.begin_previous().unwrap();
        assert_eq!((above.offset, above.bounded_limit()), (4_960, 40));
        assert!(s.publish_previous(g, 5_000, read(&order, above), true));
        assert_eq!(s.start, 4_960);
        assert_eq!(s.cards[0].key, order[4_960].key);
        assert_eq!(s.focused, Some(order[4_999].key.clone()));
        // A failed previous page keeps the window; Retry reads it again.
        let above = s.begin_previous().unwrap();
        s.fail(
            "On-demand library could not be loaded",
            Operation::Previous {
                start: 4_960,
                step_up: false,
            },
        );
        assert_eq!((s.start, s.cards.len()), (4_960, 120));
        assert_eq!(s.focused, Some(order[4_999].key.clone()));
        assert_eq!(s.begin_previous(), Some(above));
        assert!(s.publish_previous(g, 4_960, read(&order, above), false));
        assert_eq!(s.start, 4_920);
        assert_eq!(
            s.focused,
            Some(order[4_999].key.clone()),
            "a button keeps focus"
        );
        // The shelf's top has nothing before it.
        let g = s.begin_shelf(shelf);
        assert!(s.publish_window(g, read(&order, DiscoveryQuery::around(shelf, Some(3)))));
        assert_eq!(s.start, 0);
        assert!(!s.has_previous() && s.begin_previous().is_none());
    }
    #[test]
    fn a_reordered_or_deleted_deep_title_is_handled_by_identity() {
        let shelf = Shelf::ContinueWatching;
        let order = numbered(3_000);
        let target = order[2_500].key.clone();
        let mut s = Screen::default();
        let g = s.begin_shelf(shelf);
        s.publish(g, order[..40].to_vec(), true, false);
        s.focused = Some(target.clone());
        s.open = false;
        // Watched again meanwhile: the title moved from 2,501st to 2,123rd.
        let mut reordered = order.clone();
        let moved = reordered.remove(2_500);
        reordered.insert(2_122, moved);
        let g = s.begin_shelf(shelf);
        let window = DiscoveryQuery::around(shelf, Some(2_122));
        assert_eq!(window.offset, 2_120);
        assert!(s.publish_window(g, read(&reordered, window)));
        assert_eq!(s.focused, Some(target.clone()));
        assert_eq!(s.start, 2_120);
        // A failed window read keeps the focused title for Retry.
        s.open = false;
        let g = s.begin_shelf(shelf);
        s.fail("On-demand library could not be loaded", Operation::List);
        assert_eq!(s.focused, Some(target.clone()), "the anchor is not lost");
        assert_eq!(s.retry_operation(), Some(Operation::List));
        // Finished or deleted meanwhile: the shelf no longer lists it, so
        // the first page is shown with the focus on its first title.
        let mut gone = reordered.clone();
        gone.remove(2_122);
        let g2 = s.reload();
        assert!(g2 != g);
        assert_eq!(s.focused, Some(target.clone()));
        let first = DiscoveryQuery::around(shelf, None);
        assert_eq!(first, DiscoveryQuery::first(shelf));
        assert!(s.publish_window(g2, read(&gone, first)));
        assert_eq!(s.start, 0);
        assert_eq!(s.focused, Some(gone[0].key.clone()));
        // A catalogue never windows.
        let g = s.begin(Kind::Movie);
        s.publish(g, numbered(3), false, false);
        assert_eq!(s.start, 0);
        assert!(!s.has_previous() && s.begin_previous().is_none());
    }
    #[test]
    fn play_resumes_the_chosen_episode_not_the_series_record() {
        let mut details = series_details(3);
        // The second episode was finished last; the series record still
        // holds its end point, but the third episode starts from zero.
        details.item.progress = Progress {
            position_ms: 119_000,
            duration_ms: Some(120_000),
            completed: false,
        };
        details.resume_episode = Some(details.episodes[1].key.clone());
        details.episodes[1].progress = Progress::observed(120_000, Some(120_000), true);
        details.episodes[2].progress = Progress::observed(2_000, Some(120_000), false);
        let mut s = Screen::default();
        s.show_details(details.clone());
        assert_eq!(s.episode, Some(details.episodes[2].key.clone()));
        assert!(!s.resumes(), "the next episode plays from its start");
        details.episodes[2].progress = Progress::observed(45_000, Some(120_000), false);
        s.show_details(details);
        assert!(s.resumes());
        let mut film = keyed(2, Kind::Movie, "generated-film");
        film.progress = Progress::observed(30_000, Some(120_000), false);
        s.show_details(Details {
            incarnation: "generated".into(),
            item: film,
            episodes: vec![],
            resume_episode: None,
        });
        assert!(s.resumes());
    }
    #[test]
    fn the_catalogue_keeps_its_toolbar() {
        let mut s = Screen::default();
        let generation = s.begin(Kind::Series);
        assert!(!s.is_shelf());
        assert_eq!(
            s.controls(),
            vec![
                Control::Search,
                Control::Source,
                Control::Category,
                Control::Sort,
                Control::Continue,
            ]
        );
        s.publish(
            generation,
            vec![keyed(1, Kind::Series, "generated")],
            false,
            false,
        );
        assert!(s.controls().contains(&Control::Catalogue));
    }
    #[test]
    fn series_resume_keeps_the_last_watched_episode_and_advances_after_completion() {
        let mut item = card("generated-series");
        item.key.kind = Kind::Series;
        let episodes: Vec<_> = (1..=6)
            .map(|number| Episode {
                key: Key {
                    source: 1,
                    kind: Kind::Episode,
                    id: format!("episode-{number}"),
                },
                season: if number < 5 { 1 } else { 2 },
                number,
                title: format!("Generated episode {number}"),
                plot: None,
                duration_secs: Some(120),
                progress: Progress::default(),
            })
            .collect();
        let fifth = episodes[4].key.clone();
        let sixth = episodes[5].key.clone();
        let mut details = Details {
            incarnation: "generated".into(),
            item,
            episodes,
            resume_episode: Some(fifth.clone()),
        };
        details.episodes[4].progress = Progress::observed(37_000, Some(120_000), false);
        let mut screen = Screen::default();
        screen.show_details(details.clone());
        assert_eq!(screen.episode, Some(fifth.clone()));
        assert_eq!(screen.season, Some(2));
        details.episodes[4].progress.completed = true;
        screen.show_details(details.clone());
        assert_eq!(screen.episode, Some(sixth.clone()));
        details.resume_episode = Some(sixth.clone());
        details.episodes[5].progress.completed = true;
        screen.show_details(details.clone());
        assert_eq!(
            screen.episode,
            Some(sixth),
            "a completed tail never jumps back to unrelated episode one"
        );
        details.resume_episode = None;
        screen.show_details(details.clone());
        assert_eq!(screen.episode, Some(details.episodes[0].key.clone()));
    }
}
