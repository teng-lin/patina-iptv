//! On-demand catalog identities, discovery queries and playback progress.

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
pub use crate::vod_state::{
    Control, Destination, EmptyAction, Handoff, ImportProblem, Operation, Retried, Screen,
    SourceVod, VodState,
};

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
}
