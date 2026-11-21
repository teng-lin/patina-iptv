//! Portable on-demand library state. All selections use source/content identity.
//!
//! Besides the per-kind catalog ([`Query`]), two cross-source discovery
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
//! a bounded page at a time; the per-kind catalog always starts at its top.
//!
//! An empty catalog says why ([`VodState`], set by the shell from the
//! playlists on screen) and offers one action ([`Screen::empty_action`]).
mod library;
pub use library::{EmptyAction, ImportProblem, SourceVod, VodState};
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
    /// The series' own progress after episode `played` was watched, as
    /// its card reads it from the store: the episode's resume point, or
    /// none once that episode is finished (its next one starts from the
    /// beginning). The series counts as watched when that finished
    /// episode was its last or every episode is finished. None when
    /// `played` is not one of its episodes.
    pub fn series_progress(&self, played: &Key) -> Option<Progress> {
        let episode = self.episodes.iter().find(|e| &e.key == played)?;
        Some(if episode.progress.completed {
            Progress {
                position_ms: 0,
                duration_ms: None,
                completed: self.next_episode(played).is_none()
                    || self.episodes.iter().all(|e| e.progress.completed),
            }
        } else {
            Progress {
                completed: false,
                ..episode.progress
            }
        })
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
    Navigation,
    Search,
    Source,
    Category,
    Sort,
    Continue,
    #[default]
    Catalog,
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
    /// The list: a catalog from its top, a shelf around its focused title.
    List,
    /// A catalog read again from its top through at least `through`
    /// titles (the refresh after Back), so a title focused pages down is
    /// found again.
    Through { through: u32 },
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Destination {
    Live,
    #[default]
    Movies,
    Shows,
    Recordings,
}
/// Where the screen stands with the persistent player: a title played
/// full screen hands the screen to the player, and Back from that title
/// returns to its details over the same player (#147).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Handoff {
    /// Nothing played from this screen, or the viewer left it.
    #[default]
    None,
    /// Closed for the title it played: Back from full screen reopens it.
    Playing,
    /// Reopened from full screen on the played title's details.
    Returned,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub destination: Destination,
    pub control: Control,
    /// Measured poster columns; selection itself always uses a content key.
    pub columns: usize,
    pub categories_open: bool,
    pub search_open: bool,
    pub open: bool,
    /// A cross-source shelf instead of the `query.kind` catalog: the
    /// query's kind, filters and sort do not apply to it.
    pub shelf: Option<Shelf>,
    pub query: Query,
    pub sources: Vec<Source>,
    pub categories: Vec<Category>,
    pub cards: Vec<Card>,
    /// A shelf's position of `cards[0]` in its order: titles before it can
    /// be loaded with Load previous. Always 0 for a catalog.
    pub start: u32,
    /// Next store position after the pages consumed. Refreshes can repeat
    /// titles in a later page, so this must not use deduplicated card count.
    pub end: u32,
    pub focused: Option<Key>,
    /// A Down press waiting for the next poster row to arrive.
    pending_down: Option<Key>,
    pub details: Option<Details>,
    /// Metadata for the focused poster; independent of the opened details page.
    pub preview: Option<Details>,
    pub season: Option<u32>,
    pub episode: Option<Key>,
    pub busy: bool,
    pub has_more: bool,
    pub generation: u64,
    pub error: Option<String>,
    /// What Retry repeats; set with [`Screen::error`] by [`Screen::fail`].
    pub retry: Option<Operation>,
    pub notice: Option<String>,
    /// Why the catalog may be empty; kept across [`Screen::begin`] and
    /// updated by the shell through [`Screen::set_library`].
    pub library: VodState,
    /// Whether the screen waits under the player for Back from the title
    /// it played ([`Screen::hand_to_player`]).
    pub handoff: Handoff,
    /// The playlist the empty screen's action last acted on (`Some(None)`:
    /// adding a playlist).
    offered: Option<Option<i64>>,
    /// Whether the window is in the compact layout class
    /// ([`crate::layout::Class::Compact`]); set by the shell through
    /// [`Screen::set_compact`] and kept across [`Screen::begin`].
    compact: bool,
    /// The remote was sent off the empty screen's action because it moved
    /// to another playlist while a read was pending, or went away: the next
    /// settling of the panes must not put it back on the new action
    /// ([`Screen::settle_panes`]).
    left_action: bool,
    /// Search was closed while a read was pending: the remote waits on
    /// Search and goes to the titles only if the reply has something to
    /// focus ([`Screen::close_search`]).
    search_closed_pending: bool,
}
impl Screen {
    pub fn begin(&mut self, kind: Kind) -> u64 {
        *self = Self {
            open: true,
            destination: if kind == Kind::Series {
                Destination::Shows
            } else {
                Destination::Movies
            },
            categories_open: true,
            control: Control::Category,
            query: Query {
                kind,
                ..Default::default()
            },
            generation: self.generation.wrapping_add(1),
            library: self.library,
            offered: self.library.action().map(EmptyAction::target),
            compact: self.compact,
            ..Default::default()
        };
        self.reload()
    }
    /// Sets whether the window is compact. A compact window shows one pane
    /// at a time ([`Screen::one_pane`]).
    pub fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
        self.settle_panes();
    }
    /// Whether the empty catalog takes the window's width: in a compact
    /// window its message and action are the one pane, the categories give
    /// way (there is nothing to filter), and the remote moves between the
    /// rail and the action.
    pub fn one_pane(&self) -> bool {
        self.compact && self.empty_state().is_some()
    }
    /// Whether the categories column shows.
    pub fn shows_categories(&self) -> bool {
        self.categories_open && !self.one_pane()
    }
    /// Whether the rail (Close and the destinations) shows: beside the
    /// categories, and always beside the one-pane empty catalog, where it
    /// is the pointer's way out.
    pub fn shows_navigation(&self) -> bool {
        self.categories_open || self.one_pane()
    }
    /// Opens the categories, or in one pane goes to the empty catalog's
    /// action, the only thing there is to choose (the rail when it offers
    /// none).
    pub fn open_categories(&mut self) {
        if self.one_pane() {
            if self.empty_action().is_some() {
                self.categories_open = false;
                self.control = Control::Catalog;
            } else {
                self.focus_rail();
            }
        } else {
            self.categories_open = true;
            self.control = Control::Category;
        }
    }
    /// The rail in one pane. The categories are marked open, hidden while
    /// the pane lasts ([`Screen::shows_categories`]), so the rail and the
    /// focus on it stay on screen when it ends (titles arrived, a wider
    /// window, smaller text).
    fn focus_rail(&mut self) {
        self.categories_open = true;
        self.control = Control::Navigation;
    }
    /// Keeps the remote on something shown and never on an action it did
    /// not see appear. Once the empty catalog is the one pane, a remote
    /// left on the hidden categories moves to where the titles would be
    /// (the action), or to the rail when there is no action, or when it was
    /// just sent off another playlist's action: OK never acts on a playlist
    /// the screen did not show when the remote got there. A remote on an
    /// action that went away (an import started) goes to the rail too. In
    /// any window, a remote sent off an action that then reached the
    /// action's place again (Retry after a failed read) is sent off once
    /// more when the read shows the new action. In a wider window a remote
    /// on the action's place when the empty screen offers none (an import
    /// started, or titles only elsewhere) goes to the categories, where OK
    /// and the arrows still do something.
    fn settle_panes(&mut self) {
        let on_categories = matches!(
            self.control,
            Control::Category | Control::Continue | Control::Source
        );
        let on_action = self.control == Control::Catalog
            && !self.is_shelf()
            && self.details.is_none()
            && self.cards.is_empty();
        let action = self.empty_action().is_some();
        let empty = self.empty_state().is_some();
        if self.one_pane() {
            if on_categories {
                if action && !self.left_action {
                    self.categories_open = false;
                    self.control = Control::Catalog;
                } else {
                    self.focus_rail();
                }
                self.left_action = false;
            } else if on_action && (!action || self.left_action) {
                self.focus_rail();
                self.left_action = false;
            }
        } else if on_action && empty && (!action || self.left_action) {
            self.control = Control::Category;
            self.categories_open = true;
            // Sent off the action's place: an action shown later (perhaps
            // another playlist's) is not armed by a move to one pane until
            // the remote enters it on purpose.
            self.left_action = true;
        }
    }
    /// Closes the search field (Back, or its submit key): the remote goes to
    /// the titles, or stays on Search when the empty screen offers nothing
    /// to focus (an import running), in any window. While a read is pending
    /// it waits on Search, and goes to the titles when the reply has titles
    /// or an action and the remote is still there.
    pub fn close_search(&mut self) {
        let pending = self.busy && !self.is_shelf() && self.details.is_none();
        self.close_search_with(pending);
    }
    /// [`Screen::close_search`], told whether a read is pending (Back
    /// cancels it, and the shell reads the list again).
    fn close_search_with(&mut self, pending: bool) {
        self.search_open = false;
        self.search_closed_pending = pending;
        self.control = if pending || self.empty_without_action() {
            Control::Search
        } else {
            // Onto the titles, or an action already on screen: the remote
            // enters it on purpose.
            self.left_action = false;
            Control::Catalog
        };
    }
    /// Opens the search field (OK on Search). A wait left by closing it
    /// during a read ([`Screen::close_search`]) is over: the reply must not
    /// take the remote off the field.
    pub fn open_search(&mut self) {
        self.search_open = true;
        self.search_closed_pending = false;
    }
    /// Enters the titles from the categories (OK on a category). An empty
    /// screen without an action has nothing to enter: the remote stays.
    pub fn enter_catalog(&mut self) {
        if !self.empty_without_action() {
            self.browse_horizontal(1);
        }
    }
    /// Whether the empty screen shows without an action (an import running,
    /// or titles only elsewhere): there is nothing to focus where the titles
    /// would be, so the remote's ways go to the toolbar (Search, Sort) and
    /// the categories or the rail instead.
    fn empty_without_action(&self) -> bool {
        self.empty_state().is_some() && self.empty_action().is_none()
    }
    /// Whether the one pane offers no action: the remote's way from the
    /// rail is then the toolbar (Search, Sort).
    fn one_pane_without_action(&self) -> bool {
        self.one_pane() && self.empty_action().is_none()
    }
    /// Sets why the catalog may be empty. When the empty screen's action
    /// now acts on another playlist than the one it offered, a remote on it
    /// goes back to the categories: OK never acts on a playlist the screen
    /// did not show when the remote got there. When nothing is offered (an
    /// import running) a remote on its place goes to the categories, or in
    /// one pane to the rail.
    pub fn set_library(&mut self, state: VodState) {
        self.library = state;
        let Some(action) = state.action() else {
            self.settle_panes();
            return;
        };
        let target = action.target();
        // The empty catalog only (a load may be running): never a shelf,
        // details or a list with titles.
        let moved = self.offered.is_some_and(|offered| offered != target)
            && !self.is_shelf()
            && self.details.is_none()
            && self.cards.is_empty();
        // On Retry after a failed read: its reload puts the remote where the
        // titles would be, which must not arm the new action unseen.
        if moved && self.control == Control::Retry {
            self.left_action = true;
        }
        if moved && self.control == Control::Catalog {
            if self.one_pane() {
                // A later failed read's Retry must not arm it either.
                self.focus_rail();
                self.left_action = true;
            } else {
                // A read is pending: when it publishes an empty catalog
                // the remote must not land on this new action unseen.
                self.control = Control::Category;
                self.categories_open = true;
                self.left_action = true;
            }
        }
        self.offered = Some(target);
        self.settle_panes();
    }
    /// The catalog's state while it shows no titles, has loaded and has
    /// no error, and no search or filter narrows it: what its empty screen
    /// says. `None` otherwise (a shelf, details, titles, a narrowed list).
    pub fn empty_state(&self) -> Option<VodState> {
        let narrowed = !self.query.text.is_empty()
            || self.query.category.is_some()
            || self.query.source.is_some()
            || self.query.continue_watching;
        (!self.is_shelf()
            && self.details.is_none()
            && self.cards.is_empty()
            && !self.busy
            && self.error.is_none()
            && !narrowed)
            .then_some(self.library)
    }
    /// The one action of the empty catalog, if it offers one. The
    /// remote reaches it where the titles would be ([`Control::Catalog`]).
    pub fn empty_action(&self) -> Option<EmptyAction> {
        self.empty_state().and_then(VodState::action)
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
            compact: self.compact,
            ..Default::default()
        };
        self.reload()
    }
    /// Whether a discovery shelf, rather than a catalog, is shown.
    pub fn is_shelf(&self) -> bool {
        self.shelf.is_some()
    }
    pub fn reload(&mut self) -> u64 {
        if self.search_open {
            // A new search: no earlier close waits on its reply.
            self.search_closed_pending = false;
        }
        self.pending_down = None;
        self.preview = None;
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
    /// the focus stay. `None` for a catalog or at the shelf's top.
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
                Control::Catalog
            };
        }
    }
    /// `operation` failed: `message` is a catalog key, and Retry runs the
    /// same operation again. With nothing else to act on, Retry takes the
    /// focus so OK repeats it; otherwise the focus stays where it was (the
    /// focused title, a loaded page, the Play button).
    pub fn fail(&mut self, message: impl Into<String>, operation: Operation) {
        self.busy = false;
        self.search_closed_pending = false;
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
        self.control = Control::Catalog;
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
            out.extend([Control::Play, Control::Restart]);
            if self
                .episode
                .as_ref()
                .is_some_and(|key| details.next_episode(key).is_some())
            {
                out.push(Control::Next);
            }
            out.extend([Control::Block, Control::Seasons, Control::Episodes]);
        } else if self.is_shelf() {
            if !self.cards.is_empty() {
                out.push(Control::Catalog);
            }
        } else {
            out.extend([
                Control::Search,
                Control::Source,
                Control::Category,
                Control::Sort,
                Control::Continue,
            ]);
            if !self.cards.is_empty() || self.empty_action().is_some() {
                out.push(Control::Catalog);
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
        if append
            && let Some(key) = self.pending_down.take()
            && self.focused.as_ref() == Some(&key)
            && let Some(at) = self.cards.iter().position(|c| c.key == key)
        {
            self.focused = self
                .cards
                .get((at + self.columns.max(1)).min(self.cards.len().saturating_sub(1)))
                .map(|c| c.key.clone());
        }
        self.busy = false;
        self.has_more = more;
        // Search closed while this read was pending: the titles, or an
        // action the remote was not sent off ([`Screen::settle_panes`]).
        if std::mem::take(&mut self.search_closed_pending)
            && !self.search_open
            && self.control == Control::Search
            && (!self.cards.is_empty() || (self.empty_action().is_some() && !self.left_action))
        {
            self.control = Control::Catalog;
        }
        self.settle_panes();
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
    /// A poster grid's horizontal movement never wraps into another row.
    /// Left at its edge reveals categories; Right enters the posters.
    pub fn browse_horizontal(&mut self, step: isize) {
        let before = self.control;
        self.move_horizontal(step);
        self.entered_from(before);
    }
    /// The remote moved onto the action's place on purpose: the action
    /// there is one it now sees, so the "sent off" mark is spent. A move
    /// that leaves the focus where it was keeps it.
    fn entered_from(&mut self, before: Control) {
        if before != self.control && self.control == Control::Catalog {
            self.left_action = false;
        }
    }
    fn move_horizontal(&mut self, step: isize) {
        if self.control == Control::Navigation {
            if step > 0 && self.one_pane_without_action() {
                self.control = Control::Search;
            } else if step > 0 {
                // Right from the rail heads for the action on purpose.
                self.left_action = false;
                self.control = Control::Category;
                self.settle_panes();
            }
            return;
        }
        if matches!(
            self.control,
            Control::Category | Control::Continue | Control::Source
        ) {
            if step > 0 && self.empty_without_action() {
                // Nothing to focus where the titles would be: the toolbar.
                self.control = Control::Search;
            } else if step > 0 {
                self.categories_open = false;
                self.control = Control::Catalog;
            } else {
                self.control = Control::Navigation;
            }
            return;
        }
        if self.control != Control::Catalog {
            let mut controls = vec![Control::Search, Control::Sort];
            if self.retry.is_some() {
                controls.insert(0, Control::Retry);
            }
            let at = controls
                .iter()
                .position(|c| *c == self.control)
                .unwrap_or(0);
            if step < 0 && at == 0 && self.one_pane() {
                self.focus_rail();
                return;
            }
            if step < 0 && at == 0 && self.empty_without_action() {
                // Down has no empty pane to enter: Left is the way back.
                self.categories_open = true;
                self.control = Control::Category;
                return;
            }
            self.control = controls[at.saturating_add_signed(step).min(controls.len() - 1)];
            return;
        }
        let at = self.focused_index().unwrap_or(0);
        let columns = self.columns.max(1);
        if step < 0 && self.one_pane() {
            self.focus_rail();
        } else if step < 0 && at.is_multiple_of(columns) {
            self.categories_open = true;
            self.control = Control::Category;
        } else if step < 0 || at % columns + 1 < columns {
            self.move_focus(step);
        }
    }
    pub fn focused_index(&self) -> Option<usize> {
        self.focused
            .as_ref()
            .and_then(|key| self.cards.iter().position(|c| &c.key == key))
    }
    /// Up/Down on categories previews that category. Returns whether the
    /// shell should fetch the changed query. The category key is source-qualified.
    pub fn browse_vertical(&mut self, step: isize) -> bool {
        let before = self.control;
        let changed = self.move_vertical(step);
        self.entered_from(before);
        changed
    }
    fn move_vertical(&mut self, step: isize) -> bool {
        match self.control {
            Control::Navigation => {
                let destinations = [
                    Destination::Live,
                    Destination::Movies,
                    Destination::Shows,
                    Destination::Recordings,
                ];
                let at = destinations
                    .iter()
                    .position(|d| *d == self.destination)
                    .unwrap_or(1);
                self.destination =
                    destinations[at.saturating_add_signed(step).min(destinations.len() - 1)];
            }
            Control::Source => self.control = Control::Category,
            Control::Category | Control::Continue => {
                let mut groups = vec![(true, None), (false, None)];
                groups.extend(
                    self.categories
                        .iter()
                        .map(|c| (false, Some((c.source, c.id.clone())))),
                );
                let at = groups
                    .iter()
                    .position(|g| *g == (self.query.continue_watching, self.query.category.clone()))
                    .unwrap_or(1);
                if at == 0 && step < 0 {
                    self.control = Control::Source;
                    return false;
                }
                let next = at.saturating_add_signed(step).min(groups.len() - 1);
                if next == at {
                    return false;
                }
                self.query.continue_watching = groups[next].0;
                self.query.category = groups[next].1.clone();
                self.control = Control::Category;
                return true;
            }
            Control::Catalog => {
                let at = self.focused_index().unwrap_or(0);
                let columns = self.columns.max(1);
                if step < 0 && at < columns {
                    self.control = if self.retry.is_some() {
                        Control::Retry
                    } else {
                        Control::Search
                    };
                } else if step > 0 && at + columns >= self.cards.len() && self.has_more {
                    self.pending_down = self.focused.clone();
                } else {
                    self.move_focus(step * columns as isize);
                }
            }
            // Nothing to reach below the toolbar on an empty screen without
            // an action.
            _ if step > 0 && !self.empty_without_action() => self.control = Control::Catalog,
            _ => {}
        }
        false
    }
    /// Remove protected artwork/synopses when a runtime grant expires.
    pub fn redact_metadata(&mut self, denied: &[Key]) {
        if self
            .preview
            .as_ref()
            .is_some_and(|d| denied.contains(&d.item.key))
        {
            self.preview = None;
        }
        for card in &mut self.cards {
            if denied.contains(&card.key) {
                card.poster = None;
                card.plot = None;
            }
        }
    }
    pub fn publish_preview(&mut self, generation: u64, details: Details) -> bool {
        if !self.open
            || self.generation != generation
            || self.details.is_some()
            || self.focused.as_ref() != Some(&details.item.key)
        {
            return false;
        }
        self.preview = Some(details);
        true
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
    /// Horizontal details actions stay on their row; seasons and episodes
    /// have their own horizontal content movement.
    pub fn detail_horizontal(&mut self, step: isize) {
        match self.control {
            Control::Seasons => self.cycle_season(step),
            Control::Episodes => self.move_episode(step),
            _ => {
                let controls: Vec<_> = self
                    .controls()
                    .into_iter()
                    .filter(|c| !matches!(c, Control::Seasons | Control::Episodes))
                    .collect();
                if controls.is_empty() {
                    return;
                }
                let at = controls
                    .iter()
                    .position(|c| *c == self.control)
                    .unwrap_or(0);
                self.control = controls[at.saturating_add_signed(step).min(controls.len() - 1)];
            }
        }
    }
    pub fn detail_vertical(&mut self, step: isize) {
        if !self
            .details
            .as_ref()
            .is_some_and(|d| d.item.key.kind == Kind::Series)
        {
            return;
        }
        self.control = match (self.control, step < 0) {
            (Control::Episodes, true) => Control::Seasons,
            (Control::Episodes, false) => Control::Episodes,
            (Control::Seasons, true) => Control::Play,
            (Control::Seasons, false) => Control::Episodes,
            (control, true) => control,
            (_, false) => Control::Seasons,
        };
    }
    pub fn move_control(&mut self, delta: isize) {
        let controls = self.controls();
        if controls.is_empty() {
            // An empty successful shelf has no arrow-focus target. Keep its
            // neutral content anchor; Back remains available in the shell.
            self.control = Control::Catalog;
            return;
        }
        let at = controls
            .iter()
            .position(|c| *c == self.control)
            .or_else(|| {
                // A control that is gone (Retry after a success) restarts
                // from the content, not from a hidden control.
                controls
                    .iter()
                    .position(|c| matches!(c, Control::Catalog | Control::Play))
            });
        let Some(at) = at else {
            // An empty catalog has no content anchor. Enter its visible
            // toolbar at the edge, without stepping past its first action.
            self.control = if delta < 0 {
                controls[controls.len() - 1]
            } else {
                controls[0]
            };
            return;
        };
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
    /// A title chosen here now plays full screen: the screen closes but
    /// keeps its list, focus and details, for Back from that title
    /// ([`Screen::return_from_player`]).
    pub fn hand_to_player(&mut self) {
        self.open = false;
        self.busy = false;
        self.pending_down = None;
        self.handoff = Handoff::Playing;
        self.generation = self.generation.wrapping_add(1);
    }
    /// Whether Back from `item` (a movie, or the series of an episode)
    /// playing full screen returns to its details here.
    pub fn returns_to(&self, item: &Key) -> bool {
        !self.open
            && self.handoff == Handoff::Playing
            && self.details.as_ref().is_some_and(|d| &d.item.key == item)
    }
    /// Back from `played` (a movie, or an episode of `details`) full
    /// screen: its details reopen, the played episode and its season
    /// chosen and the focus on Play, found by identity in `details` (the
    /// player's, with the progress just watched). The list underneath is
    /// the one the title was chosen from; its card there (a movie's, or
    /// the series' of the episode played) shows the progress just watched
    /// without the list being read again (#177).
    pub fn return_from_player(&mut self, mut details: Details, played: &Key) {
        if details.item.key.kind == Kind::Series
            && let Some(progress) = details.series_progress(played)
        {
            details.item.progress = progress;
        }
        if let Some(card) = self.cards.iter_mut().find(|c| c.key == details.item.key) {
            card.progress = details.item.progress;
        }
        self.open = true;
        self.search_open = false;
        self.clear_failure();
        self.notice = None;
        self.generation = self.generation.wrapping_add(1);
        self.show_details(details);
        if let Some(season) = self.details.as_ref().and_then(|d| {
            d.episodes
                .iter()
                .find(|e| &e.key == played)
                .map(|e| e.season)
        }) {
            self.episode = Some(played.clone());
            self.season = Some(season);
        }
        self.handoff = Handoff::Returned;
    }
    pub fn back(&mut self) {
        // A pending catalog read is canceled here and read again by the
        // shell ([`Screen::reload`]).
        let pending = self.busy && !self.is_shelf() && self.details.is_none();
        self.preview = None;
        self.pending_down = None;
        self.generation = self.generation.wrapping_add(1);
        self.busy = false;
        self.error = None;
        self.retry = None;
        if self.control == Control::Retry {
            self.control = Control::Catalog;
        }
        if !self.is_shelf() && self.details.is_none() {
            if self.search_open {
                self.close_search_with(pending);
                return;
            }
            if !self.categories_open && !self.one_pane() {
                self.categories_open = true;
                self.control = Control::Category;
                return;
            }
            if self.control != Control::Navigation {
                if self.one_pane() {
                    self.focus_rail();
                } else {
                    self.control = Control::Navigation;
                }
                return;
            }
        }
        if self.details.take().is_none() {
            self.open = false;
            self.generation = self.generation.wrapping_add(1);
        }
        self.handoff = Handoff::None;
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
    fn poster_grid_moves_in_rows_then_reveals_categories() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(
            g,
            (0..10).map(|n| card(&n.to_string())).collect(),
            false,
            false,
        );
        s.columns = 4;
        s.browse_horizontal(1);
        assert!(!s.categories_open);
        s.browse_horizontal(1);
        assert_eq!(s.focused_index(), Some(1));
        s.browse_vertical(1);
        assert_eq!(s.focused_index(), Some(5));
        s.browse_vertical(1);
        assert_eq!(s.focused_index(), Some(9));
        s.browse_horizontal(-1);
        s.browse_horizontal(-1);
        assert!(s.categories_open);
        assert_eq!(s.focused_index(), Some(8));
        assert_eq!(s.control, Control::Category);
    }
    #[test]
    fn series_actions_follow_the_visible_row_and_vertical_moves_reach_episodes() {
        let mut s = Screen::default();
        s.show_details(series_details(3));
        for control in [
            Control::Restart,
            Control::Next,
            Control::Block,
            Control::Block,
        ] {
            s.detail_horizontal(1);
            assert_eq!(s.control, control);
        }
        s.detail_vertical(1);
        assert_eq!(s.control, Control::Seasons);
        s.detail_vertical(1);
        assert_eq!(s.control, Control::Episodes);
        let before = s.episode.clone();
        s.detail_horizontal(1);
        assert_ne!(s.episode, before);
        s.detail_vertical(-1);
        assert_eq!(s.control, Control::Seasons);
        s.detail_vertical(-1);
        assert_eq!(s.control, Control::Play);
    }
    #[test]
    fn movie_vertical_keys_keep_the_action_row_and_retry_is_horizontal() {
        let mut s = Screen::default();
        let mut details = series_details(0);
        details.item.key.kind = Kind::Movie;
        s.show_details(details);
        let key = s.details.as_ref().unwrap().item.key.clone();
        s.fail("Playback unavailable", Operation::Details(key));
        s.control = Control::Play;
        for step in [-1, 1] {
            s.detail_vertical(step);
            assert_eq!(s.control, Control::Play);
        }
        s.detail_horizontal(-1);
        assert_eq!(s.control, Control::Retry);
        s.detail_vertical(1);
        assert_eq!(s.control, Control::Retry);
        s.detail_horizontal(1);
        assert_eq!(s.control, Control::Play);
    }
    #[test]
    fn series_retry_shares_the_action_row_above_seasons() {
        let mut s = Screen::default();
        s.show_details(series_details(2));
        let key = s.details.as_ref().unwrap().item.key.clone();
        s.fail("Playback unavailable", Operation::Details(key));
        s.control = Control::Play;
        s.detail_vertical(-1);
        assert_eq!(s.control, Control::Play);
        s.detail_horizontal(-1);
        assert_eq!(s.control, Control::Retry);
        s.detail_vertical(1);
        assert_eq!(s.control, Control::Seasons);
    }
    #[test]
    fn revoked_metadata_is_removed_from_the_preview_and_catalog() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        let mut item = card("a");
        item.plot = Some("Generated protected plot".into());
        item.poster = Some("https://example.test/poster.png".into());
        s.publish(g, vec![item.clone()], false, false);
        s.publish_preview(
            g,
            Details {
                incarnation: "generated".into(),
                item: item.clone(),
                episodes: vec![],
                resume_episode: None,
            },
        );
        s.redact_metadata(&[item.key]);
        assert!(s.preview.is_none());
        assert!(s.cards[0].plot.is_none());
        assert!(s.cards[0].poster.is_none());
    }
    #[test]
    fn preview_metadata_cannot_replace_another_title_or_a_newer_query() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(g, vec![card("a"), card("b")], false, false);
        let details = |id| Details {
            incarnation: "generated".into(),
            item: card(id),
            episodes: vec![],
            resume_episode: None,
        };
        assert!(s.publish_preview(g, details("a")));
        s.move_focus(1);
        assert!(!s.publish_preview(g, details("a")));
        let next = s.reload();
        assert!(s.preview.is_none());
        assert!(!s.publish_preview(g, details("b")));
        s.publish(next, vec![card("b")], false, false);
        assert!(s.publish_preview(next, details("b")));
    }
    #[test]
    fn down_at_a_page_boundary_waits_for_the_same_column_in_the_next_row() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(
            g,
            (0..20).map(|n| card(&n.to_string())).collect(),
            true,
            false,
        );
        s.columns = 5;
        s.focused = Some(card("15").key);
        s.control = Control::Catalog;
        s.browse_vertical(1);
        assert_eq!(s.focused, Some(card("15").key));
        s.begin_more();
        s.publish(
            g,
            (20..25).map(|n| card(&n.to_string())).collect(),
            false,
            true,
        );
        assert_eq!(s.focused, Some(card("20").key));
    }
    #[test]
    fn backing_away_from_a_pending_title_invalidates_its_reply() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(g, vec![card("a")], false, false);
        s.browse_horizontal(1);
        let pending = s.begin_operation();
        s.back();
        assert!(s.open && s.categories_open);
        assert!(!s.busy);
        assert_ne!(s.generation, pending);
        assert!(!s.publish(pending, vec![card("old")], false, false));
        assert_eq!(s.focused, Some(card("a").key));
    }
    #[test]
    fn a_remote_can_return_to_retry_after_a_failed_title() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(g, vec![card("a")], false, false);
        s.browse_horizontal(1);
        s.fail("Generated failure", Operation::Details(card("a").key));
        s.browse_vertical(-1);
        assert_eq!(s.control, Control::Retry);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Search);
        s.browse_horizontal(-1);
        assert_eq!(s.control, Control::Retry);
    }
    #[test]
    fn category_previews_keep_source_qualified_identity() {
        let mut s = Screen::default();
        s.begin(Kind::Series);
        s.categories = [1, 2]
            .into_iter()
            .map(|source| Category {
                source,
                id: "same".into(),
                name: "Generated".into(),
                adult: false,
            })
            .collect();
        assert!(s.browse_vertical(1));
        assert_eq!(s.query.category, Some((1, "same".into())));
        assert!(s.browse_vertical(1));
        assert_eq!(s.query.category, Some((2, "same".into())));
        assert!(!s.browse_vertical(1));
        assert!(s.browse_vertical(-1));
        assert_eq!(s.query.category, Some((1, "same".into())));
    }
    #[test]
    fn back_walks_from_posters_to_categories_to_navigation_before_leaving() {
        let mut s = Screen::default();
        s.begin(Kind::Movie);
        s.browse_horizontal(1);
        s.back();
        assert!(s.open && s.categories_open);
        assert_eq!(s.control, Control::Category);
        s.back();
        assert!(s.open);
        assert_eq!(s.control, Control::Navigation);
        s.back();
        assert!(!s.open);
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
        assert_eq!(s.control, Control::Catalog, "no hidden toolbar to reach");
        s.fail("On-demand library could not be loaded", Operation::List);
        assert!(!s.busy);
        assert_eq!(s.control, Control::Retry, "OK repeats the failed load");
        assert_eq!(s.controls(), vec![Control::Retry]);
        assert_eq!(s.retry_operation(), Some(Operation::List));
        let g2 = s.reload();
        assert!(g2 != g && s.error.is_none() && s.retry.is_none());
        assert_eq!(s.control, Control::Catalog);
        s.publish(g2, vec![keyed(1, Kind::Movie, "a")], false, false);
        assert_eq!(s.controls(), vec![Control::Catalog]);
        // An error over a list keeps the focus on the titles.
        s.fail(
            "Details unavailable; refresh the playlist and try again",
            Operation::Details(keyed(1, Kind::Movie, "a").key),
        );
        assert_eq!(s.control, Control::Catalog);
        s.move_control(-1);
        assert_eq!(s.control, Control::Retry);
        s.move_control(1);
        assert_eq!(
            s.control,
            Control::Catalog,
            "populated cards remain reachable"
        );
        s.move_control(-1);
        s.back();
        assert_eq!(s.control, Control::Catalog);
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
                assert_eq!(screen.control, Control::Catalog);
                assert!(screen.focused.is_none());
            }
            screen.fail("On-demand library could not be loaded", Operation::List);
            assert_eq!(screen.controls(), [Control::Retry]);
            for stale in [Control::Retry, Control::Catalog] {
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
        // Another shelf or a catalog starts from its own top.
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
        s.control = Control::Catalog;
        assert!(s.open && s.details.is_none() && s.is_shelf());
        assert_eq!(s.focused, Some(cards[1].key.clone()));
        assert_eq!(s.controls(), vec![Control::Catalog]);
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
        screen.control = Control::Restart;
        screen.detail_horizontal(1);
        assert_eq!(
            screen.control,
            Control::Block,
            "Right must reach a visible action, not the absent Next button"
        );
        screen.detail_horizontal(-1);
        assert_eq!(screen.control, Control::Restart);
        screen.episode = Some(details.episodes[0].key.clone());
        screen.detail_horizontal(1);
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
        assert_eq!(s.control, Control::Catalog, "the focus stays on the titles");
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
        assert_eq!(s.control, Control::Catalog);
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
        // A catalog never windows.
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
    fn an_empty_catalog_enters_the_visible_toolbar_without_skipping_controls() {
        for kind in [Kind::Movie, Kind::Series] {
            let mut screen = Screen::default();
            let generation = screen.begin(kind);
            screen.publish(generation, Vec::new(), false, false);
            for (delta, expected) in [(-1, Control::Continue), (1, Control::Search)] {
                screen.control = Control::Catalog;
                screen.move_control(delta);
                assert_eq!(screen.control, expected, "{kind:?}, direction {delta}");
            }
            screen.move_control(1);
            assert_eq!(
                screen.control,
                Control::Source,
                "the next step advances normally"
            );
        }
    }
    #[test]
    fn an_empty_unfiltered_catalog_offers_its_one_action_where_titles_would_be() {
        let mut s = Screen::default();
        s.set_library(VodState::Failed {
            source: 7,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        assert_eq!(s.empty_action(), None, "nothing is offered while loading");
        assert!(
            matches!(s.library, VodState::Failed { source: 7, .. }),
            "kept across begin"
        );
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.empty_action(), Some(EmptyAction::Retry(7)));
        assert!(s.controls().contains(&Control::Catalog));
        // A search or filter that finds nothing is not the library's state.
        s.query.text = "generated".into();
        assert_eq!(s.empty_state(), None);
        s.query.text.clear();
        s.query.continue_watching = true;
        assert_eq!(s.empty_action(), None);
        s.query.continue_watching = false;
        // Titles available elsewhere, a running import: no action.
        for state in [VodState::Available, VodState::Importing] {
            s.library = state;
            assert_eq!(s.empty_state(), Some(state));
            assert_eq!(s.empty_action(), None);
            assert!(!s.controls().contains(&Control::Catalog));
        }
        // A failed load shows Retry for the load, not the library's action.
        s.library = VodState::NoEligibleSource;
        s.fail("generated failure", Operation::List);
        assert_eq!(s.empty_action(), None);
        // Shelves keep their own empty text.
        let mut shelf = Screen::default();
        shelf.set_library(VodState::NoEligibleSource);
        let generation = shelf.begin_shelf(Shelf::RecentlyAdded);
        shelf.publish(generation, Vec::new(), false, false);
        assert_eq!(shelf.empty_state(), None);
    }
    #[test]
    fn the_remote_leaves_an_action_that_moved_to_another_playlist() {
        let failed = |source| VodState::Failed {
            source,
            problem: ImportProblem::Network,
        };
        let mut s = Screen::default();
        s.set_library(failed(1));
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::Retry(1)));
        // Retrying it: nothing offered meanwhile, the remote goes to the
        // categories (#146).
        s.set_library(VodState::Importing);
        assert_eq!(s.control, Control::Category);
        // The same playlist, now empty: Right reaches its Refresh.
        s.set_library(VodState::Empty { source: 1 });
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::Refresh(1)));
        // Still offered by the same playlist: the remote stays.
        s.set_library(VodState::Empty { source: 1 });
        assert_eq!(s.control, Control::Catalog);
        // Another playlist's failure takes the screen: the remote leaves.
        s.set_library(failed(2));
        assert_eq!(s.control, Control::Category);
        assert!(s.categories_open);
        s.browse_horizontal(1);
        assert_eq!(s.empty_action(), Some(EmptyAction::Retry(2)));
        // A remote elsewhere is not moved.
        s.control = Control::Search;
        s.set_library(failed(3));
        assert_eq!(s.control, Control::Search);
        // An empty shelf keeps its focus.
        let mut shelf = Screen::default();
        let generation = shelf.begin_shelf(Shelf::RecentlyAdded);
        shelf.publish(generation, Vec::new(), false, false);
        shelf.set_library(failed(1));
        shelf.set_library(failed(2));
        assert_eq!(shelf.control, Control::Catalog);
        assert!(!shelf.categories_open);
    }
    #[test]
    fn a_compact_empty_catalog_is_one_pane_between_the_rail_and_its_action() {
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::NoEligibleSource);
        let generation = s.begin(Kind::Movie);
        // Loading: the categories show as before.
        assert!(!s.one_pane() && s.shows_categories());
        s.publish(generation, Vec::new(), false, false);
        // Loaded and empty: the message and its action take the width,
        // with the remote on the action and the rail beside it.
        assert!(s.one_pane());
        assert!(!s.shows_categories() && s.shows_navigation());
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::AddPlaylist));
        // Left reaches the rail, Right the action again; the categories
        // never come back.
        s.browse_horizontal(-1);
        assert_eq!(s.control, Control::Navigation);
        assert!(!s.shows_categories() && s.shows_navigation());
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        // Choosing this destination again goes to its action.
        s.control = Control::Navigation;
        s.open_categories();
        assert_eq!(s.control, Control::Catalog);
        // Back: the rail, then closed.
        s.back();
        assert!(s.open);
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation());
        s.back();
        assert!(!s.open);
        // Wider again with the remote on the action: the categories one
        // Left away, as in every wider window; compact again moves a
        // remote off them.
        let generation = s.begin(Kind::Series);
        s.publish(generation, Vec::new(), false, false);
        s.set_compact(false);
        assert!(!s.one_pane() && !s.shows_navigation());
        s.browse_horizontal(-1);
        assert_eq!(s.control, Control::Category);
        assert!(s.shows_categories());
        s.set_compact(true);
        assert_eq!(s.control, Control::Catalog);
        assert!(!s.shows_categories());
        // Titles: the categories stay a pane of their own.
        let generation = s.begin(Kind::Movie);
        s.publish(generation, vec![card("a")], false, false);
        assert!(!s.one_pane() && s.shows_categories());
        assert_eq!(s.control, Control::Category);
        // A shelf is never one pane.
        let generation = s.begin_shelf(Shelf::RecentlyAdded);
        s.publish(generation, Vec::new(), false, false);
        assert!(!s.one_pane());
    }
    #[test]
    fn the_rail_stays_on_screen_with_the_remote_when_one_pane_ends() {
        // Left to the rail, then a wider window or smaller text.
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::NoEligibleSource);
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(-1);
        s.set_compact(false);
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation() && s.shows_categories());
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Category);
        // Back to the rail, then wider.
        s.set_compact(true);
        assert_eq!(s.control, Control::Catalog);
        s.back();
        assert_eq!(s.control, Control::Navigation);
        s.set_compact(false);
        assert!(s.shows_navigation());
        // An import running offers no action: the remote waits on the rail,
        // not on an empty pane. It finishes with titles: the rail stays.
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::Importing);
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        assert!(s.one_pane());
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation() && !s.shows_categories());
        s.set_library(VodState::Available);
        let generation = s.reload();
        assert!(!s.one_pane());
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation());
        s.publish(generation, vec![card("a")], false, false);
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation() && s.shows_categories());
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Category);
    }
    #[test]
    fn a_pending_read_never_lands_the_remote_on_another_playlists_action() {
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::Retry(1)));
        // A read is pending when the action moves to playlist 2: the
        // remote leaves, and the empty page does not put it back on 2's
        // action.
        let generation = s.reload();
        s.set_library(VodState::ImportsDisabled { source: 2 });
        assert_eq!(s.control, Control::Category);
        s.publish(generation, Vec::new(), false, false);
        assert!(s.one_pane());
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation());
        // Right from the rail, the action now shown, reaches it.
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
        // Shown: an action that moves again sends it to the rail.
        s.set_library(VodState::Empty { source: 3 });
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation() && !s.shows_categories());
    }
    #[test]
    fn a_vanished_action_sends_the_remote_to_the_rail() {
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::Empty { source: 1 });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Catalog);
        // Refresh now: the import runs, the action goes.
        s.set_library(VodState::Importing);
        assert_eq!(s.empty_action(), None);
        assert_eq!(s.control, Control::Navigation);
        assert!(s.shows_navigation());
        // The same while a read is pending: settled when it arrives.
        s.control = Control::Catalog;
        let generation = s.reload();
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Navigation);
    }
    #[test]
    fn a_failed_read_and_its_retry_never_arm_another_playlists_action() {
        for compact in [true, false] {
            let mut s = Screen::default();
            s.set_compact(compact);
            s.set_library(VodState::Failed {
                source: 1,
                problem: ImportProblem::Network,
            });
            let generation = s.begin(Kind::Movie);
            s.publish(generation, Vec::new(), false, false);
            s.browse_horizontal(1);
            assert_eq!(s.control, Control::Catalog);
            // The action moves to playlist 2 during a read, which fails.
            s.reload();
            s.set_library(VodState::ImportsDisabled { source: 2 });
            s.fail("generated failure", Operation::List);
            assert_eq!(s.control, Control::Retry);
            // Retry puts the remote where the titles would be; the empty
            // page then shows playlist 2's action, which it never saw.
            let generation = s.reload();
            assert_eq!(s.control, Control::Catalog);
            s.publish(generation, Vec::new(), false, false);
            assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
            assert_ne!(s.control, Control::Catalog, "compact: {compact}");
            assert!(s.shows_navigation() || s.shows_categories());
            // Moving there on purpose reaches it.
            s.browse_horizontal(1);
            assert_eq!(s.control, Control::Catalog);
        }
    }
    #[test]
    fn the_remote_reaches_the_toolbar_of_a_compact_import() {
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::Importing);
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Navigation);
        // Right: the toolbar, not the rail again.
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Search);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Sort);
        // Down has no empty pane to enter.
        s.browse_vertical(1);
        assert_eq!(s.control, Control::Sort);
        // Left from the toolbar's first control: the rail.
        s.browse_horizontal(-1);
        s.browse_horizontal(-1);
        assert_eq!(s.control, Control::Navigation);
        // With an action, Right reaches the action as before.
        s.set_library(VodState::NoEligibleSource);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
    }
    #[test]
    fn a_target_change_on_retry_never_arms_the_new_action() {
        for compact in [true, false] {
            let mut s = Screen::default();
            s.set_compact(compact);
            s.set_library(VodState::Failed {
                source: 1,
                problem: ImportProblem::Network,
            });
            let generation = s.begin(Kind::Movie);
            s.publish(generation, Vec::new(), false, false);
            s.browse_horizontal(1);
            // The read fails first; Retry takes the remote.
            s.reload();
            s.fail("generated failure", Operation::List);
            assert_eq!(s.control, Control::Retry);
            // Then the action moves to playlist 2, and its reload empties.
            s.set_library(VodState::ImportsDisabled { source: 2 });
            assert_eq!(s.control, Control::Retry);
            let generation = s.reload();
            s.publish(generation, Vec::new(), false, false);
            assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
            assert_ne!(s.control, Control::Catalog, "compact: {compact}");
        }
    }
    #[test]
    fn ok_on_a_category_enters_a_shown_action_for_good() {
        let mut s = Screen::default();
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        // Wide: the action moves to playlist 2, the remote goes to the
        // categories; OK there enters 2's shown action.
        s.set_library(VodState::ImportsDisabled { source: 2 });
        assert_eq!(s.control, Control::Category);
        s.enter_catalog();
        assert_eq!(s.control, Control::Catalog);
        // Compact afterwards keeps it there.
        s.set_compact(true);
        assert_eq!(s.control, Control::Catalog);
    }
    #[test]
    fn closing_search_in_a_compact_import_keeps_the_remote_on_search() {
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::Importing);
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Search);
        s.search_open = true;
        s.back();
        assert!(s.open && !s.search_open);
        assert_eq!(s.control, Control::Search);
        s.search_open = true;
        s.close_search();
        assert_eq!(s.control, Control::Search);
        // With an action, closing search goes where the titles would be.
        s.set_library(VodState::NoEligibleSource);
        s.search_open = true;
        s.close_search();
        assert_eq!(s.control, Control::Catalog);
    }
    #[test]
    fn a_compact_move_to_the_rail_survives_a_failed_read_and_retry() {
        let mut s = Screen::default();
        s.set_compact(true);
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Catalog);
        // Loaded and shown: the action moves to playlist 2, the remote to
        // the rail. The next read fails; Retry, then an empty page.
        s.set_library(VodState::ImportsDisabled { source: 2 });
        assert_eq!(s.control, Control::Navigation);
        s.reload();
        s.fail("generated failure", Operation::List);
        let generation = s.reload();
        assert_eq!(s.control, Control::Catalog);
        s.publish(generation, Vec::new(), false, false);
        assert_ne!(s.control, Control::Catalog);
        // Right from the rail selects the shown action.
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
    }
    #[test]
    fn a_key_that_leaves_retry_focused_keeps_the_mark() {
        for compact in [true, false] {
            let mut s = Screen::default();
            s.set_compact(compact);
            s.set_library(VodState::Failed {
                source: 1,
                problem: ImportProblem::Network,
            });
            let generation = s.begin(Kind::Movie);
            s.publish(generation, Vec::new(), false, false);
            s.browse_horizontal(1);
            s.reload();
            s.fail("generated failure", Operation::List);
            s.set_library(VodState::ImportsDisabled { source: 2 });
            // Up (and Left) on Retry: the focus stays.
            s.browse_vertical(-1);
            s.browse_horizontal(-1);
            assert_eq!(s.control, Control::Retry);
            let generation = s.reload();
            s.publish(generation, Vec::new(), false, false);
            assert_ne!(s.control, Control::Catalog, "compact: {compact}");
        }
    }
    #[test]
    fn a_wide_refresh_leaves_the_remote_on_a_visible_control() {
        // #146: Refresh now in a wider window, then the import runs.
        let mut s = Screen::default();
        s.set_library(VodState::Empty { source: 1 });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert!(!s.shows_categories());
        s.set_library(VodState::Importing);
        assert_eq!(s.empty_action(), None);
        assert_eq!(s.control, Control::Category);
        assert!(s.shows_categories() && s.shows_navigation());
        // The same when the read showing it arrives later.
        s.control = Control::Catalog;
        s.categories_open = false;
        let generation = s.reload();
        assert_eq!(s.control, Control::Catalog, "loading: left alone");
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Category);
        assert!(s.shows_categories());
        // The import ends empty again: Right reaches the shown action.
        s.set_library(VodState::Empty { source: 1 });
        assert_eq!(s.control, Control::Category);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::Refresh(1)));
        // It ends with titles: the remote stays on the categories shown
        // with them, and Right enters the posters.
        s.set_library(VodState::Available);
        assert_eq!(s.control, Control::Category);
        let generation = s.reload();
        s.publish(generation, vec![card("a")], false, false);
        assert_eq!(s.control, Control::Category);
        assert!(s.shows_categories());
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.focused, Some(card("a").key));
    }
    #[test]
    fn wide_arrows_never_land_on_an_empty_pane_without_an_action() {
        for state in [VodState::Importing, VodState::Available] {
            let mut s = Screen::default();
            s.set_library(state);
            let generation = s.begin(Kind::Series);
            s.publish(generation, Vec::new(), false, false);
            assert_eq!(s.control, Control::Category, "{state:?}");
            // OK on a category has nothing to enter.
            s.enter_catalog();
            assert_eq!(s.control, Control::Category, "{state:?}");
            // Right: the toolbar, not the empty pane.
            s.browse_horizontal(1);
            assert_eq!(s.control, Control::Search, "{state:?}");
            s.browse_horizontal(1);
            assert_eq!(s.control, Control::Sort, "{state:?}");
            // Down has no empty pane to enter.
            s.browse_vertical(1);
            assert_eq!(s.control, Control::Sort, "{state:?}");
            s.browse_horizontal(-1);
            s.browse_vertical(1);
            assert_eq!(s.control, Control::Search, "{state:?}");
            // Closing Search keeps the remote there.
            s.search_open = true;
            s.back();
            assert!(s.open && !s.search_open);
            assert_eq!(s.control, Control::Search, "{state:?}");
            // Left from the toolbar's first control: the categories.
            s.browse_horizontal(-1);
            assert_eq!(s.control, Control::Category, "{state:?}");
            assert!(s.shows_categories());
            // Once an action shows, Right and Down reach it.
            s.set_library(VodState::NoEligibleSource);
            assert_eq!(s.control, Control::Category, "{state:?}");
            s.browse_horizontal(1);
            assert_eq!(s.control, Control::Catalog, "{state:?}");
            s.control = Control::Search;
            s.browse_vertical(1);
            assert_eq!(s.control, Control::Catalog, "{state:?}");
            s.control = Control::Search;
            s.search_open = true;
            s.close_search();
            assert_eq!(s.control, Control::Catalog, "{state:?}");
            s.control = Control::Category;
            s.enter_catalog();
            assert_eq!(s.control, Control::Catalog, "{state:?}");
        }
        // A search or filter that finds nothing keeps its routes.
        let mut s = Screen::default();
        s.set_library(VodState::Importing);
        let generation = s.begin(Kind::Movie);
        s.query.text = "generated".into();
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        // Titles: Right enters them, Left from Search stays.
        let mut s = Screen::default();
        s.set_library(VodState::Importing);
        let generation = s.begin(Kind::Movie);
        s.publish(generation, vec![card("a")], false, false);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        s.control = Control::Search;
        s.browse_horizontal(-1);
        assert_eq!(s.control, Control::Search);
        s.browse_vertical(1);
        assert_eq!(s.control, Control::Catalog);
    }
    #[test]
    fn a_wide_remote_sent_off_an_action_stays_off_through_an_import() {
        // The existing guard: the action moves to playlist 2 while a read
        // is pending; an import then hides it, then 2's action shows.
        let mut s = Screen::default();
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        let generation = s.reload();
        s.set_library(VodState::ImportsDisabled { source: 2 });
        assert_eq!(s.control, Control::Category);
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Category);
        s.set_library(VodState::Importing);
        assert_eq!(s.control, Control::Category);
        s.set_library(VodState::ImportsDisabled { source: 2 });
        assert_eq!(s.control, Control::Category);
        // Right reaches it on purpose.
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
    }
    #[test]
    fn an_import_that_displaces_the_remote_keeps_compact_off_an_unseen_action() {
        // Retry playlist 1 in a wide window; its import hides the action
        // and moves the remote to the categories. It ends with no titles,
        // and playlist 2's "Playlist settings" now shows.
        let mut s = Screen::default();
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        s.set_library(VodState::Importing);
        assert_eq!(s.control, Control::Category);
        s.set_library(VodState::ImportsDisabled { source: 2 });
        let generation = s.reload();
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
        assert_eq!(s.control, Control::Category);
        // Compact: the rail, not playlist 2's action.
        s.set_compact(true);
        assert_eq!(s.control, Control::Navigation);
        // Entering it on purpose arms it, and compact keeps it.
        s.set_compact(false);
        assert_eq!(s.control, Control::Navigation);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Category);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        s.set_compact(true);
        assert_eq!(s.control, Control::Catalog);
    }
    #[test]
    fn closing_search_during_a_pending_read_waits_on_search() {
        for compact in [false, true] {
            // An import running: the search text cleared, then submitted
            // before its read replies.
            let mut s = Screen::default();
            s.set_compact(compact);
            s.set_library(VodState::Importing);
            let generation = s.begin(Kind::Movie);
            s.query.text = "generated".into();
            s.publish(generation, Vec::new(), false, false);
            s.control = Control::Search;
            s.search_open = true;
            s.query.text.clear();
            let generation = s.reload();
            s.close_search();
            assert_eq!(s.control, Control::Search, "compact: {compact}");
            s.publish(generation, Vec::new(), false, false);
            assert_eq!(s.control, Control::Search, "compact: {compact}");
            // The same with Back, which reads the list again.
            s.search_open = true;
            s.reload();
            s.back();
            assert!(s.open && !s.search_open);
            assert_eq!(s.control, Control::Search, "compact: {compact}");
            let generation = s.reload();
            s.publish(generation, Vec::new(), false, false);
            assert_eq!(s.control, Control::Search, "compact: {compact}");
            // A reply with titles: the remote goes to them.
            s.search_open = true;
            let generation = s.reload();
            s.close_search();
            assert_eq!(s.control, Control::Search, "compact: {compact}");
            s.publish(generation, vec![card("a")], false, false);
            assert_eq!(s.control, Control::Catalog, "compact: {compact}");
            // A reply with an action: the remote goes to it.
            s.set_library(VodState::NoEligibleSource);
            s.control = Control::Search;
            s.search_open = true;
            let generation = s.reload();
            s.close_search();
            s.publish(generation, Vec::new(), false, false);
            assert_eq!(s.control, Control::Catalog, "compact: {compact}");
            // Moved elsewhere meanwhile: the reply leaves it there.
            s.control = Control::Search;
            s.search_open = true;
            let generation = s.reload();
            s.close_search();
            s.control = Control::Sort;
            s.publish(generation, Vec::new(), false, false);
            assert_eq!(s.control, Control::Sort, "compact: {compact}");
        }
    }
    #[test]
    fn back_from_search_with_titles_waits_for_the_pending_read() {
        // Titles shown, a search typed: Back while its read is pending
        // cancels it (the shell reads again) and waits on Search.
        let mut s = Screen::default();
        let generation = s.begin(Kind::Movie);
        s.publish(generation, vec![card("a")], false, false);
        s.control = Control::Search;
        s.open_search();
        s.query.text = "generated".into();
        s.reload();
        s.back();
        assert!(s.open && !s.search_open);
        assert_eq!(s.control, Control::Search);
        let generation = s.reload();
        s.publish(generation, vec![card("b")], false, false);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.focused, Some(card("b").key));
    }
    #[test]
    fn a_new_search_is_never_left_by_an_earlier_close() {
        // Two ways back into the field: OK on Search, or typing (a read
        // while it is open).
        for reopen in [true, false] {
            let mut s = Screen::default();
            let generation = s.begin(Kind::Movie);
            s.publish(generation, vec![card("a")], false, false);
            s.control = Control::Search;
            s.open_search();
            s.query.text = "generated".into();
            s.reload();
            s.close_search();
            assert_eq!(s.control, Control::Search);
            // Back into the field before the reply; typing reads again.
            if reopen {
                s.open_search();
            } else {
                s.search_open = true;
            }
            s.query.text = "generated more".into();
            let generation = s.reload();
            s.publish(generation, vec![card("b")], false, false);
            assert!(s.search_open);
            assert_eq!(s.control, Control::Search, "reopen: {reopen}");
            // Even a stale wait never leaves an open field.
            s.search_closed_pending = true;
            let generation = s.reload();
            s.search_open = true;
            s.publish(generation, vec![card("b")], false, false);
            assert_eq!(s.control, Control::Search, "reopen: {reopen}");
        }
    }
    #[test]
    fn closing_search_onto_a_shown_action_enters_it() {
        // The remote is sent off playlist 1's action when it moves to
        // playlist 2, then goes to Search (a pointer) and closes it with
        // playlist 2's action on screen.
        let mut s = Screen::default();
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        s.set_library(VodState::ImportsDisabled { source: 2 });
        assert_eq!(s.control, Control::Category);
        s.control = Control::Search;
        s.open_search();
        s.close_search();
        assert_eq!(s.control, Control::Catalog);
        // Entered on purpose: a resize or the next read keeps it there.
        s.set_compact(true);
        assert_eq!(s.control, Control::Catalog);
        s.set_compact(false);
        let generation = s.reload();
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Catalog);
        assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
        // Sent off again, then Search closed while a read is pending: the
        // reply shows the action, and the remote waits on Search rather
        // than landing there and being sent off.
        s.set_library(VodState::Empty { source: 3 });
        assert_eq!(s.control, Control::Category);
        s.control = Control::Search;
        s.open_search();
        let generation = s.reload();
        s.close_search();
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.empty_action(), Some(EmptyAction::Refresh(3)));
        assert_eq!(s.control, Control::Search);
        // Down from there enters it on purpose.
        s.browse_vertical(1);
        assert_eq!(s.control, Control::Catalog);
        s.set_compact(true);
        assert_eq!(s.control, Control::Catalog);
    }
    #[test]
    fn the_catalog_keeps_its_toolbar() {
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
        assert!(s.controls().contains(&Control::Catalog));
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
    #[test]
    fn back_from_a_played_episode_reopens_its_show_on_that_episode() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Series);
        let mut details = series_details(4);
        details.episodes[2].season = 2;
        details.episodes[3].season = 2;
        s.publish(g, vec![details.item.clone()], false, false);
        s.show_details(details.clone());
        let played = details.episodes[3].key.clone();
        s.hand_to_player();
        assert!(!s.open && s.returns_to(&details.item.key));
        assert!(!s.returns_to(&card("other").key));
        s.return_from_player(details.clone(), &played);
        assert!(s.open && !s.returns_to(&details.item.key));
        assert_eq!(s.episode, Some(played));
        assert_eq!(s.season, Some(2));
        assert_eq!(s.control, Control::Play);
        assert_eq!(s.handoff, Handoff::Returned);
        // A second Back leaves the details for the list, on the same title.
        s.back();
        assert!(s.open && s.details.is_none());
        assert_eq!(s.focused, Some(details.item.key.clone()));
        assert_eq!(s.handoff, Handoff::None);
    }
    #[test]
    fn back_from_a_played_episode_shows_its_series_progress_on_the_list() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Series);
        let mut details = series_details(3);
        s.publish(
            g,
            vec![card("a"), details.item.clone(), card("b")],
            false,
            false,
        );
        s.show_details(details.clone());
        let first = details.episodes[0].key.clone();
        // Part of the first episode watched: the card resumes it.
        s.hand_to_player();
        details.episodes[0].progress = Progress::observed(120_000, Some(600_000), false);
        s.return_from_player(details.clone(), &first);
        assert_eq!(s.cards[1].progress, details.episodes[0].progress);
        assert_eq!(
            s.details.as_ref().map(|d| d.item.progress),
            Some(s.cards[1].progress)
        );
        s.back();
        assert_eq!(s.focused, Some(details.item.key.clone()));
        assert_eq!(s.cards[1].progress.resume_ms(), 120_000);
        // Finished: no resume point, the series not yet watched.
        s.show_details(details.clone());
        s.hand_to_player();
        details.episodes[0].progress = Progress::observed(600_000, Some(600_000), true);
        s.return_from_player(details.clone(), &first);
        assert_eq!(s.cards[1].progress, Progress::default());
        // Its last episode finished: the series is watched.
        let last = details.episodes[2].key.clone();
        s.hand_to_player();
        details.episodes[2].progress = Progress::observed(600_000, Some(600_000), true);
        s.return_from_player(details.clone(), &last);
        assert!(s.cards[1].progress.completed);
        assert_eq!(s.cards[1].progress.resume_ms(), 0);
        // The other cards are untouched.
        assert_eq!(s.cards[0].progress, Progress::default());
        assert_eq!(s.cards[2].progress, Progress::default());
    }
    #[test]
    fn back_from_a_played_movie_shows_its_progress_on_the_list() {
        let mut s = Screen::default();
        let g = s.begin(Kind::Movie);
        s.publish(g, vec![card("a"), card("b")], false, false);
        let mut details = Details {
            incarnation: "generated".into(),
            item: card("b"),
            episodes: vec![],
            resume_episode: None,
        };
        s.show_details(details.clone());
        s.hand_to_player();
        details.item.progress = Progress::observed(120_000, Some(600_000), false);
        s.return_from_player(details.clone(), &details.item.key.clone());
        assert_eq!(s.control, Control::Play);
        assert_eq!(s.focused, Some(card("b").key));
        assert_eq!(s.cards[1].progress, details.item.progress);
        assert!(s.resumes());
        // Choosing anything else ends the handoff.
        s.hand_to_player();
        s.begin(Kind::Series);
        assert!(!s.returns_to(&card("b").key));
    }
}
