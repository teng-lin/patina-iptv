//! Older days of a stored guide, read in on demand.
//!
//! A schedule keeps only a recent window of its stored guide in memory
//! (from [`PagedWindow::loaded_from`] on); the days before it are pages of
//! one day each, counted back from there (page 0 ends where the window
//! starts). A page is asked for (`PageLoader`, which reads it off the UI
//! thread) the first time something needs a time in it, shows as loading
//! until [`crate::EpgSchedule::insert_page`] brings it, and is dropped
//! again by [`crate::EpgSchedule::evict_pages`] (the user is back at now)
//! or when more than [`MAX_PAGES`] are held (the least recently used one).
//! The page of the programme playing from the archive can be pinned, so
//! neither drops it.
//!
//! Days before [`PagedWindow::oldest`] (what the store keeps) are empty
//! without being read.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

use iptv_epg::{ChannelKey, DAY, GuideStore, UnixTime};

/// Older days held at once (two for a guide window across a day's edge,
/// one for the archive playing).
pub const MAX_PAGES: usize = 3;

/// Which part of a stored guide a schedule holds in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagedWindow {
    /// The schedule's store holds every programme airing at or after this
    /// (rounded down to the hour); older ones are paged in.
    pub loaded_from: UnixTime,
    /// The store keeps nothing that ended before this.
    pub oldest: UnixTime,
}

/// One older day to read: programmes overlapping `[from, to)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    pub(crate) owner: u64,
    pub(crate) attempt: u64,
    pub page: u32,
    pub from: UnixTime,
    pub to: UnixTime,
}

/// Reads a page (off the UI thread) and hands it to
/// [`crate::EpgSchedule::insert_page`] later. Called from guide queries:
/// it must not call back into the schedule before it returns.
pub type PageLoader = Box<dyn Fn(PageRequest)>;

enum PageState {
    Loading,
    Failed,
    Loaded(GuideStore),
}

struct Page {
    index: u32,
    /// `None` while it loads.
    request: PageRequest,
    state: PageState,
    used: Cell<u64>,
}

pub(crate) struct Pages {
    owner: u64,
    /// `UnixTime::MIN`: everything is in the schedule's own store.
    loaded_from: UnixTime,
    oldest: UnixTime,
    listing_starts: HashMap<ChannelKey, UnixTime>,
    loader: Option<PageLoader>,
    pages: RefCell<Vec<Page>>,
    clock: Cell<u64>,
    pinned: Cell<Option<u32>>,
    empty: GuideStore,
}

impl Default for Pages {
    fn default() -> Self {
        Pages {
            owner: NEXT_OWNER
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    next.checked_add(1)
                })
                .expect("page owner identity exhausted"),
            loaded_from: UnixTime::MIN,
            oldest: UnixTime::MIN,
            listing_starts: HashMap::new(),
            loader: None,
            pages: RefCell::new(Vec::new()),
            clock: Cell::new(0),
            pinned: Cell::new(None),
            empty: GuideStore::new(),
        }
    }
}

impl Pages {
    pub(crate) fn new(
        window: PagedWindow,
        listing_starts: HashMap<ChannelKey, UnixTime>,
        loader: PageLoader,
    ) -> Self {
        Pages {
            loaded_from: window.loaded_from - window.loaded_from.rem_euclid(3600),
            oldest: window.oldest,
            listing_starts,
            loader: Some(loader),
            ..Pages::default()
        }
    }

    /// Whether `t` is in the schedule's own store.
    pub(crate) fn in_window(&self, t: UnixTime) -> bool {
        t >= self.loaded_from
    }

    pub(crate) fn loaded_from(&self) -> UnixTime {
        self.loaded_from
    }

    /// The oldest time the store keeps (`UnixTime::MIN` without pages).
    pub(crate) fn oldest(&self) -> UnixTime {
        self.oldest
    }

    pub(crate) fn listing_start(&self, key: &ChannelKey) -> Option<UnixTime> {
        self.listing_starts
            .get(key)
            .map(|&start| start.max(self.oldest))
    }

    fn index_of(&self, t: UnixTime) -> u32 {
        u32::try_from((self.loaded_from - 1 - t).div_euclid(DAY)).unwrap_or(u32::MAX)
    }

    fn range(&self, index: u32) -> (UnixTime, UnixTime) {
        let to = self.loaded_from - i64::from(index) * DAY;
        (to - DAY, to)
    }

    fn tick(&self) -> u64 {
        self.clock.set(self.clock.get() + 1);
        self.clock.get()
    }

    /// `f` on the page holding `t` (before the window); `None` while it
    /// loads (asked for now if it was not).
    pub(crate) fn with<R>(&self, t: UnixTime, f: impl FnOnce(&GuideStore) -> R) -> Option<R> {
        let index = self.index_of(t);
        if self.range(index).1 <= self.oldest {
            return Some(f(&self.empty));
        }
        {
            let pages = self.pages.borrow();
            if let Some(page) = pages.iter().find(|p| p.index == index) {
                page.used.set(self.tick());
                return match &page.state {
                    PageState::Loaded(store) => Some(f(store)),
                    _ => None,
                };
            }
        }
        self.request(index);
        None
    }

    fn request(&self, index: u32) {
        let Some(loader) = &self.loader else {
            return;
        };
        let (from, to) = self.range(index);
        let request = PageRequest {
            owner: self.owner,
            attempt: self.tick(),
            page: index,
            from,
            to,
        };
        self.pages.borrow_mut().push(Page {
            index,
            request,
            state: PageState::Loading,
            used: Cell::new(self.tick()),
        });
        loader(request);
    }

    /// A page read for `request`; false (and dropped) when it is no longer
    /// wanted or belongs to another window.
    pub(crate) fn insert(&self, request: PageRequest, store: GuideStore) -> bool {
        if self.loader.is_none()
            || request.owner != self.owner
            || self.range(request.page) != (request.from, request.to)
        {
            return false;
        }
        let mut pages = self.pages.borrow_mut();
        let Some(page) = pages
            .iter_mut()
            .find(|p| p.request == request && matches!(p.state, PageState::Loading))
        else {
            return false;
        };
        page.state = PageState::Loaded(store);
        page.used.set(self.tick());
        while pages
            .iter()
            .filter(|p| matches!(p.state, PageState::Loaded(_)))
            .count()
            > MAX_PAGES
        {
            let victim = pages
                .iter()
                .enumerate()
                .filter(|(_, p)| matches!(p.state, PageState::Loaded(_)) && p.index != request.page)
                .filter(|(_, p)| Some(p.index) != self.pinned.get())
                .min_by_key(|(_, p)| p.used.get())
                .map(|(i, _)| i);
            match victim {
                Some(i) => drop(pages.remove(i)),
                None => break,
            }
        }
        true
    }

    pub(crate) fn fail(&self, request: PageRequest) -> bool {
        if request.owner != self.owner {
            return false;
        }
        let mut pages = self.pages.borrow_mut();
        let Some(page) = pages
            .iter_mut()
            .find(|page| page.request == request && matches!(page.state, PageState::Loading))
        else {
            return false;
        };
        page.state = PageState::Failed;
        true
    }

    pub(crate) fn pending(&self, t: UnixTime) -> Option<PageRequest> {
        if self.in_window(t) {
            return None;
        }
        self.pages
            .borrow()
            .iter()
            .find(|page| page.index == self.index_of(t) && matches!(page.state, PageState::Loading))
            .map(|page| page.request)
    }

    /// Explicit user demand may retry a failed page; render reads never do.
    pub(crate) fn retry_failed(&self, t: UnixTime) {
        if self.in_window(t) {
            return;
        }
        let index = self.index_of(t);
        self.pages
            .borrow_mut()
            .retain(|page| !(page.index == index && matches!(page.state, PageState::Failed)));
    }

    /// Drops every page (loaded or loading) but the pinned one; returns
    /// how many loaded ones went.
    pub(crate) fn evict(&self) -> usize {
        let mut pages = self.pages.borrow_mut();
        let loaded = pages
            .iter()
            .filter(|p| matches!(p.state, PageState::Loaded(_)))
            .count();
        pages.retain(|p| Some(p.index) == self.pinned.get());
        loaded
            - pages
                .iter()
                .filter(|p| matches!(p.state, PageState::Loaded(_)))
                .count()
    }

    /// Keeps the page holding `t` (none: no page is kept).
    pub(crate) fn pin(&self, t: Option<UnixTime>) {
        let page = t.filter(|&t| !self.in_window(t)).map(|t| self.index_of(t));
        self.pinned.set(page);
    }

    /// Pages in memory (not counting those loading).
    pub(crate) fn loaded(&self) -> usize {
        self.pages
            .borrow()
            .iter()
            .filter(|p| matches!(p.state, PageState::Loaded(_)))
            .count()
    }
}
