//! Portable on-demand library state. All selections use source/content identity.
//!
//! Besides the per-kind catalog ([`Query`]), two cross-source discovery
//! shelves ([`Shelf`]) mix movies and series from every enabled playlist.
//! They are read a bounded page at a time ([`DiscoveryQuery`]) in a total,
//! deterministic order, so a page boundary never repeats or skips a title
//! while the library is unchanged:
//!
//! - **Continue watching** lists a movie whose saved progress is
//!   [`iptv_core::vod::Progress::unfinished`]: at least [`iptv_core::vod::CONTINUE_MIN_MS`] in and not
//!   completed. A series contributes its single parent record, which follows
//!   the episode watched last. It is listed while the series is not
//!   completed and either that record is [`iptv_core::vod::CONTINUE_MIN_MS`] in or another
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

use iptv_core::vod::{
    Card, Category, Details, DiscoveryPage, DiscoveryQuery, Key, Kind, Query, Shelf, Sort, Source,
};

mod library;
pub use library::{EmptyAction, ImportProblem, SourceVod, VodState};
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
/// The category pane's filter for a catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogGroup {
    ContinueWatching,
    All,
    Category { source: i64, id: String },
}

/// What the shell should do after Back settles the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackOutcome {
    Closed,
    Reload { through: u32 },
    Stay,
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
/// Rail destinations that can never show anything here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Omitted {
    /// Movies and Shows: every enabled playlist is M3U
    /// ([`crate::browse::live_only`]).
    catalogs: bool,
    /// Recordings: the host never records.
    recordings: bool,
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
    /// (the compact window class); set by the shell through
    /// [`Screen::set_compact`] and kept across [`Screen::begin`].
    compact: bool,
    /// What the rail never offers here, set by the shell through
    /// [`Screen::set_navigation`] and kept across [`Screen::begin`].
    omitted: Omitted,
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
    /// Cycle through the catalog sort choices in display order.
    pub fn cycle_sort(&mut self) {
        self.query.sort = match self.query.sort {
            Sort::Name => Sort::Newest,
            Sort::Newest => Sort::Year,
            Sort::Year => Sort::Rating,
            Sort::Rating => Sort::Name,
        };
    }

    pub fn begin(&mut self, kind: Kind) -> u64 {
        self.begin_sorted(kind, Sort::default())
    }

    /// Open a kind of catalog using the saved catalog order.
    pub fn begin_sorted(&mut self, kind: Kind, sort: Sort) -> u64 {
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
                sort,
                ..Default::default()
            },
            generation: self.generation.wrapping_add(1),
            library: self.library,
            offered: self.library.action().map(EmptyAction::target),
            compact: self.compact,
            omitted: self.omitted,
            ..Default::default()
        };
        self.reload()
    }
    /// Sets which destinations the rail offers: Movies and Shows unless
    /// every enabled playlist is M3U (`catalogs`), Recordings where the
    /// host records (`recordings`). A remote on one that goes moves to
    /// Live TV.
    pub fn set_navigation(&mut self, catalogs: bool, recordings: bool) {
        self.omitted = Omitted {
            catalogs: !catalogs,
            recordings: !recordings,
        };
        if self.control == Control::Navigation && !self.destinations().contains(&self.destination) {
            self.destination = Destination::Live;
        }
    }
    /// The rail's destinations, in order.
    pub fn destinations(&self) -> Vec<Destination> {
        let mut destinations = vec![Destination::Live];
        if !self.omitted.catalogs {
            destinations.extend([Destination::Movies, Destination::Shows]);
        }
        if !self.omitted.recordings {
            destinations.push(Destination::Recordings);
        }
        destinations
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
        self.enter_navigation();
    }
    /// The remote to the rail, on this screen's destination where the rail
    /// offers it, else on Live TV.
    fn enter_navigation(&mut self) {
        self.control = Control::Navigation;
        if !self.destinations().contains(&self.destination) {
            self.destination = Destination::Live;
        }
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
            omitted: self.omitted,
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
    /// Settle a page above the current shelf, including when the window moved first.
    pub fn finish_previous(
        &mut self,
        generation: u64,
        start: u32,
        page: DiscoveryPage,
        step_up: bool,
    ) {
        if self.accepts_reply(generation)
            && !self.publish_previous(generation, start, page, step_up)
        {
            self.busy = false;
        }
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
    /// Apply a catalog's available filter choices with its title page.
    pub fn publish_catalog(
        &mut self,
        generation: u64,
        page: DiscoveryPage,
        sources: Vec<Source>,
        categories: Vec<Category>,
        append: bool,
    ) -> bool {
        if !self.accepts_reply(generation) {
            return false;
        }
        self.sources = sources;
        self.categories = categories;
        if append {
            self.publish(generation, page.cards, page.more, true)
        } else {
            self.publish_window(generation, page)
        }
    }

    /// Whether an open screen still owns this title or catalog reply.
    pub fn accepts_reply(&self, generation: u64) -> bool {
        self.open && self.generation == generation
    }

    /// Keep focus on the title just selected while its details load.
    pub fn begin_details(&mut self, key: Key) -> u64 {
        self.focused = Some(key);
        self.begin_operation()
    }

    /// Settle an operation owned by the current open screen.
    pub fn finish_operation(&mut self, generation: u64) -> bool {
        if !self.accepts_reply(generation) {
            return false;
        }
        self.busy = false;
        true
    }

    /// Starts opening details or playing: a newer generation, so an older
    /// reply is ignored, with any failure shown before cleared.
    pub fn begin_operation(&mut self) -> u64 {
        self.busy = true;
        self.clear_failure();
        self.supersede_operation()
    }

    /// Fence a pending operation before starting a new authorization check.
    pub fn supersede_operation(&mut self) -> u64 {
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
    /// Apply a retried title read, retaining the selected episode if it still exists.
    pub fn accept_retried_details(
        &mut self,
        details: Details,
        title: &Key,
        incarnation: &str,
        unavailable: &str,
    ) -> Option<Details> {
        match self.show_details_keeping(details.clone(), title, incarnation) {
            Retried::Ready => Some(details),
            Retried::EpisodeGone | Retried::PlaylistChanged => {
                self.error = Some(unavailable.into());
                None
            }
        }
    }

    /// Refuse playback from a playlist whose identity changed since selection.
    pub fn reject_replaced_playlist(&mut self, generation: u64, unavailable: &str) {
        if self.accepts_reply(generation) {
            self.playlist_changed();
            self.error = Some(unavailable.into());
        }
    }

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
                self.enter_navigation();
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
                let destinations = self.destinations();
                let at = destinations
                    .iter()
                    .position(|d| *d == self.destination)
                    .unwrap_or(0);
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
    /// Move to the next source, resetting the category from the previous source.
    pub fn cycle_source(&mut self) {
        let at = self
            .query
            .source
            .and_then(|id| self.sources.iter().position(|s| s.id == id));
        self.query.source = match at {
            None => self.sources.first().map(|s| s.id),
            Some(at) => self.sources.get(at + 1).map(|s| s.id),
        };
        self.query.category = None;
    }

    /// Cycle the catalog category filter.
    pub fn cycle_category(&mut self) {
        let at = self.query.category.as_ref().and_then(|(source, id)| {
            self.categories
                .iter()
                .position(|c| c.source == *source && c.id == *id)
        });
        self.query.category = match at {
            None => self.categories.first().map(|c| (c.source, c.id.clone())),
            Some(at) => self
                .categories
                .get(at + 1)
                .map(|c| (c.source, c.id.clone())),
        };
    }

    /// Choose a category pane filter and focus it.
    pub fn choose_catalog_group(&mut self, group: CatalogGroup) {
        self.control = Control::Category;
        self.query.continue_watching = group == CatalogGroup::ContinueWatching;
        self.query.category = match group {
            CatalogGroup::Category { source, id } => Some((source, id)),
            CatalogGroup::ContinueWatching | CatalogGroup::All => None,
        };
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
    /// Close the details page when its title lost authorization.
    pub fn close_blocked_details(&mut self, denied: &Key) {
        if self.details.as_ref().is_some_and(|d| &d.item.key == denied) {
            self.details = None;
            self.episode = None;
            self.control = Control::Catalog;
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
    /// Focus an episode chosen by content identity.
    pub fn select_episode(&mut self, key: Key) {
        self.episode = Some(key);
        self.control = Control::Episodes;
    }

    /// Keep the current details page with the next episode and updated progress.
    pub fn select_next_episode(
        &mut self,
        details: Details,
        key: Key,
        season: u32,
        notice: Option<&str>,
    ) {
        self.details = Some(details);
        self.episode = Some(key);
        self.season = Some(season);
        self.notice = notice.map(str::to_owned);
    }

    /// Reopen details from full-screen playback for a next episode waiting to play.
    pub fn reopen_for_next_episode(
        &mut self,
        details: Details,
        key: Key,
        season: u32,
        notice: &str,
    ) {
        self.open = true;
        self.show_details(details);
        self.episode = Some(key);
        self.season = Some(season);
        self.notice = Some(notice.into());
    }

    /// Dismiss the catalog and invalidate replies from its previous session.
    pub fn close(&mut self) {
        self.open = false;
        self.handoff = Handoff::None;
        self.generation = self.generation.wrapping_add(1);
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
    /// `played` (a movie, or an episode of `details`, the player's) was
    /// watched: its title's card in the list, and `details`' own item,
    /// show the progress just watched, without the list being read again
    /// (#177). `saved` tells whether the player saved progress for
    /// `played`: until it does, the store's series record still describes
    /// the episode watched before, and a series' card stays as it was.
    pub fn watched(&mut self, details: &mut Details, played: &Key, saved: bool) {
        let progress = if details.item.key.kind == Kind::Series {
            saved.then(|| details.series_progress(played)).flatten()
        } else {
            Some(details.item.progress)
        };
        if let Some(progress) = progress {
            details.item.progress = progress;
            if let Some(card) = self.cards.iter_mut().find(|c| c.key == details.item.key) {
                card.progress = progress;
            }
        }
    }
    /// Back from `played` (a movie, or an episode of `details`) full
    /// screen: its details reopen, the played episode and its season
    /// chosen and the focus on Play, found by identity in `details` (the
    /// player's, with the progress just watched). The list underneath is
    /// the one the title was chosen from; its card there (a movie's, or
    /// the series' of the episode played) shows the progress just watched
    /// without the list being read again (#177; see [`Screen::watched`]).
    pub fn return_from_player(&mut self, mut details: Details, played: &Key, saved: bool) {
        self.watched(&mut details, played, saved);
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
    /// Handle Back and report whether the catalog must refresh to keep its focus.
    pub fn back(&mut self) -> BackOutcome {
        let had_details = self.details.is_some();
        let pending_catalog = self.busy && !self.is_shelf() && !had_details;
        // Watched progress can reorder these views. Other lists already have the
        // updated card and keep the page window the viewer was looking at.
        let watched = had_details
            && self.handoff == Handoff::Returned
            && (self.is_shelf() || self.query.continue_watching);
        self.traverse_back(pending_catalog);
        if had_details {
            self.control = Control::Catalog;
        }
        if !self.open {
            BackOutcome::Closed
        } else if watched || pending_catalog {
            BackOutcome::Reload { through: self.end }
        } else {
            BackOutcome::Stay
        }
    }

    fn traverse_back(&mut self, pending_catalog: bool) {
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
                self.close_search_with(pending_catalog);
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
                    self.enter_navigation();
                }
                return;
            }
        }
        if self.details.take().is_none() {
            self.close();
        } else {
            self.handoff = Handoff::None;
        }
    }
}
#[cfg(test)]
mod tests;
