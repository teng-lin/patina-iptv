//! The guide data chooser's state: a search field, the filter (every guide
//! channel, or only those no channel uses), "Automatic", "No guide data",
//! then the matching guide channels of the stored guides. Focus is kept by
//! identity. Pure: the view feeds it keys, taps and typed text and carries
//! out its [`Effect`]s.

use iptv_epg::FeedId as Feed;
use iptv_library::{GuideChannel, SourceId};

use iptv_library::{Choice, Current};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Search,
    Filter,
    Automatic,
    NoGuide,
    Channel(Feed, String),
}

impl Row {
    fn of(choice: &Choice) -> Self {
        match choice {
            Choice::Automatic => Self::Automatic,
            Choice::NoGuide => Self::NoGuide,
            Choice::Channel { feed, epg_channel } => Self::Channel(*feed, epg_channel.clone()),
        }
    }
}

/// Rows before the guide channels.
pub const FIXED: [Row; 4] = [Row::Search, Row::Filter, Row::Automatic, Row::NoGuide];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Ok,
    Back,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Put the keys into the search field (the keyboard comes up).
    Type,
    Choose(Choice),
    Close,
}

/// What the chooser is for and what the store has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    pub playlist: SourceId,
    /// The channel's key (its stable id).
    pub key: String,
    pub name: String,
    pub current: Current,
    pub channels: Vec<GuideChannel>,
    /// A display name per guide source.
    pub feeds: Vec<(Feed, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    subject: Subject,
    query: String,
    /// Every guide channel; else only those no channel uses.
    all: bool,
    /// Indices into `subject.channels` that match.
    shown: Vec<usize>,
    focus: Row,
    typing: bool,
    /// A choice is being applied.
    busy: bool,
}

/// How many rows Page Up/Down move.
const PAGE: usize = 8;

impl Picker {
    /// Focused on the channel's current choice.
    pub fn new(subject: Subject) -> Self {
        let focus = subject
            .current
            .choice
            .as_ref()
            .map_or(Row::Automatic, Row::of);
        let mut p = Self {
            subject,
            query: String::new(),
            all: true,
            shown: Vec::new(),
            focus,
            typing: false,
            busy: false,
        };
        p.filter();
        if !p.rows_contain(&p.focus.clone()) {
            p.focus = Row::Automatic;
        }
        p
    }

    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn all(&self) -> bool {
        self.all
    }

    pub fn typing(&self) -> bool {
        self.typing
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn focus(&self) -> &Row {
        &self.focus
    }

    /// The guide channels that match the search and filter.
    pub fn matches(&self) -> usize {
        self.shown.len()
    }

    pub fn len(&self) -> usize {
        FIXED.len() + self.shown.len()
    }

    /// Includes the search, filter and automatic-choice rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn row(&self, i: usize) -> Option<Row> {
        match FIXED.get(i) {
            Some(r) => Some(r.clone()),
            None => {
                let c = &self.subject.channels[*self.shown.get(i - FIXED.len())?];
                Some(Row::Channel(c.feed, c.epg_channel.clone()))
            }
        }
    }

    pub fn channel(&self, i: usize) -> Option<&GuideChannel> {
        let at = i.checked_sub(FIXED.len())?;
        self.shown.get(at).map(|&c| &self.subject.channels[c])
    }

    pub fn index_of(&self, row: &Row) -> Option<usize> {
        match row {
            Row::Channel(feed, id) => self
                .shown
                .iter()
                .position(|&c| {
                    let c = &self.subject.channels[c];
                    c.feed == *feed && c.epg_channel == *id
                })
                .map(|i| i + FIXED.len()),
            // Every other row is one of the fixed ones.
            _ => FIXED.iter().position(|r| r == row),
        }
    }

    fn rows_contain(&self, row: &Row) -> bool {
        self.index_of(row).is_some()
    }

    pub fn focus_index(&self) -> usize {
        self.index_of(&self.focus).unwrap_or(0)
    }

    /// The row is the channel's current choice.
    pub fn is_current(&self, row: &Row) -> bool {
        self.subject.current.choice.as_ref().map(Row::of).as_ref() == Some(row)
    }

    pub fn feed_name(&self, feed: Feed) -> &str {
        self.subject
            .feeds
            .iter()
            .find(|(f, _)| *f == feed)
            .map_or("", |(_, n)| n.as_str())
    }

    fn filter(&mut self) {
        let q = self.query.trim().to_lowercase();
        let all = self.all;
        let channels = &self.subject.channels;
        let bound = self.subject.current.bound.clone();
        self.shown = (0..channels.len())
            .filter(|&i| {
                let c = &channels[i];
                let mine = bound
                    .as_ref()
                    .is_some_and(|(f, id, _)| *f == c.feed && *id == c.epg_channel);
                let wanted = all || !c.assigned || mine;
                let found = q.is_empty()
                    || c.epg_channel.to_lowercase().contains(&q)
                    || c.name
                        .as_ref()
                        .is_some_and(|n| n.to_lowercase().contains(&q));
                wanted && found
            })
            .collect();
    }

    /// Typed text: the list shows what matches; focus stays on the field.
    /// (The view hands only a changed text.)
    pub fn set_query(&mut self, text: &str) {
        self.query = text.to_owned();
        self.filter();
        self.focus = Row::Search;
    }

    /// The field took or lost the keys.
    pub fn set_typing(&mut self, typing: bool) {
        self.typing = typing;
        if typing {
            self.focus = Row::Search;
        }
    }

    /// Typing ended with Return: the first match, else "Automatic".
    pub fn submit(&mut self) {
        self.typing = false;
        self.focus = self.row(FIXED.len()).unwrap_or(Row::Automatic);
    }

    pub fn toggle_filter(&mut self) {
        self.all = !self.all;
        self.filter();
        self.focus = Row::Filter;
    }

    pub fn key(&mut self, key: Key) -> Option<Effect> {
        if self.busy {
            return (key == Key::Back).then_some(Effect::Close);
        }
        let at = self.focus_index();
        let last = self.len() - 1;
        let to = match key {
            Key::Up => at.saturating_sub(1),
            Key::Down => (at + 1).min(last),
            Key::PageUp => at.saturating_sub(PAGE),
            Key::PageDown => (at + PAGE).min(last),
            Key::Ok => return self.ok(),
            Key::Back if self.typing => {
                self.typing = false;
                return None;
            }
            Key::Back => return Some(Effect::Close),
        };
        self.typing = false;
        if let Some(row) = self.row(to) {
            self.focus = row;
        }
        None
    }

    fn ok(&mut self) -> Option<Effect> {
        match self.focus.clone() {
            Row::Search => {
                self.typing = true;
                Some(Effect::Type)
            }
            Row::Filter => {
                self.toggle_filter();
                None
            }
            Row::Automatic => self.choose(Choice::Automatic),
            Row::NoGuide => self.choose(Choice::NoGuide),
            Row::Channel(feed, epg_channel) => self.choose(Choice::Channel { feed, epg_channel }),
        }
    }

    fn choose(&mut self, choice: Choice) -> Option<Effect> {
        // The current choice again: nothing to do.
        if self.is_current(&Row::of(&choice)) {
            return Some(Effect::Close);
        }
        self.busy = true;
        Some(Effect::Choose(choice))
    }

    /// A tap on row `i`: focus it and OK.
    pub fn tap(&mut self, i: usize) -> Option<Effect> {
        if self.busy {
            return None;
        }
        self.focus = self.row(i)?;
        self.ok()
    }
}

#[cfg(test)]
mod tests {
    use iptv_epg::MatchKind;

    use super::*;

    const A: Feed = Feed(2);
    const B: Feed = Feed(1001);

    fn channel(feed: Feed, id: &str, name: &str, assigned: bool) -> GuideChannel {
        GuideChannel {
            feed,
            epg_channel: id.into(),
            name: Some(name.into()),
            assigned,
        }
    }

    fn subject(choice: Option<Choice>) -> Subject {
        Subject {
            playlist: SourceId(1),
            key: "c1".into(),
            name: "Synthetic One".into(),
            current: Current {
                choice,
                bound: Some((A, "one.test".into(), MatchKind::TvgId)),
            },
            channels: vec![
                channel(A, "one.test", "Synthetic One", true),
                channel(A, "two.test", "Synthetic Two", true),
                channel(B, "alt-one", "Alt One", false),
                channel(B, "alt-news", "Alt News", false),
            ],
            feeds: vec![(A, "The provider's guide".into()), (B, "Alt".into())],
        }
    }

    #[test]
    fn opens_on_the_current_choice() {
        let p = Picker::new(subject(Some(Choice::Automatic)));
        assert_eq!(p.focus(), &Row::Automatic);
        assert_eq!(p.len(), 8);
        let chosen = Choice::Channel {
            feed: B,
            epg_channel: "alt-news".into(),
        };
        let p = Picker::new(subject(Some(chosen)));
        assert_eq!(p.focus(), &Row::Channel(B, "alt-news".into()));
        assert_eq!(p.focus_index(), 7);
        assert_eq!(p.feed_name(B), "Alt");
    }

    #[test]
    fn typing_and_the_filter_narrow_the_list() {
        let mut p = Picker::new(subject(None));
        p.set_query("ONE");
        assert_eq!(p.matches(), 2);
        assert_eq!(p.focus(), &Row::Search);
        p.submit();
        assert_eq!(p.focus(), &Row::Channel(A, "one.test".into()));
        // Only the unassigned ones, and the channel's own.
        p.set_query("");
        p.toggle_filter();
        assert!(!p.all());
        assert_eq!(p.matches(), 3);
        assert!(p.index_of(&Row::Channel(A, "two.test".into())).is_none());
        p.set_query("nothing like it");
        assert_eq!(p.matches(), 0);
        p.submit();
        assert_eq!(p.focus(), &Row::Automatic);
    }

    #[test]
    fn keys_move_type_and_choose() {
        let mut p = Picker::new(subject(Some(Choice::Automatic)));
        p.key(Key::Up);
        p.key(Key::Up);
        assert_eq!(p.focus(), &Row::Search);
        assert_eq!(p.key(Key::Ok), Some(Effect::Type));
        assert!(p.typing());
        assert_eq!(p.key(Key::Back), None, "Back ends typing first");
        assert!(!p.typing());
        p.key(Key::PageDown);
        assert_eq!(p.focus(), &Row::Channel(B, "alt-news".into()));
        assert_eq!(
            p.key(Key::Ok),
            Some(Effect::Choose(Choice::Channel {
                feed: B,
                epg_channel: "alt-news".into()
            }))
        );
        assert!(p.busy());
        assert_eq!(p.key(Key::Down), None, "busy");
        assert_eq!(p.key(Key::Back), Some(Effect::Close));
    }

    #[test]
    fn the_current_choice_again_changes_nothing() {
        let mut p = Picker::new(subject(Some(Choice::NoGuide)));
        assert_eq!(p.key(Key::Ok), Some(Effect::Close));
        assert!(!p.busy());
        assert_eq!(p.tap(2), Some(Effect::Choose(Choice::Automatic)));
        let mut p = Picker::new(subject(None));
        assert_eq!(p.tap(1), None, "the filter");
        assert!(!p.all());
        assert_eq!(p.tap(0), Some(Effect::Type));
    }
}
