//! Portable on-demand library state. All selections use source/content identity.
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
        if self.completed || self.position_ms < 5_000 {
            0
        } else {
            self.position_ms
        }
    }
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
}
impl Details {
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
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub control: Control,
    pub open: bool,
    pub query: Query,
    pub sources: Vec<Source>,
    pub categories: Vec<Category>,
    pub cards: Vec<Card>,
    pub focused: Option<Key>,
    pub details: Option<Details>,
    pub season: Option<u32>,
    pub episode: Option<Key>,
    pub busy: bool,
    pub has_more: bool,
    pub generation: u64,
    pub error: Option<String>,
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
    pub fn reload(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.busy = true;
        self.error = None;
        self.details = None;
        self.episode = None;
        self.generation
    }
    pub fn publish(&mut self, generation: u64, cards: Vec<Card>, more: bool, append: bool) -> bool {
        if generation != self.generation || !self.open {
            return false;
        }
        if !append {
            self.cards = cards;
        } else {
            for card in cards {
                if !self.cards.iter().any(|c| c.key == card.key) {
                    self.cards.push(card);
                }
            }
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
        self.episode = details
            .episodes
            .iter()
            .find(|e| !e.progress.completed)
            .or_else(|| details.episodes.first())
            .map(|e| e.key.clone());
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
        let controls = if self
            .details
            .as_ref()
            .is_some_and(|d| d.item.key.kind == Kind::Movie)
        {
            &[Control::Play, Control::Restart, Control::Block][..]
        } else if self.details.is_some() {
            &[
                Control::Play,
                Control::Restart,
                Control::Seasons,
                Control::Episodes,
                Control::Next,
                Control::Block,
            ][..]
        } else {
            &[
                Control::Search,
                Control::Source,
                Control::Category,
                Control::Sort,
                Control::Continue,
                Control::Catalogue,
            ][..]
        };
        let at = controls
            .iter()
            .position(|c| *c == self.control)
            .unwrap_or(0);
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
}
