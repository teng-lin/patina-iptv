//! What the guide lists. The groups column holds fixed entries first
//! (Favorites, Recently watched, All channels) and then the playlist's
//! groups; the selected entry is the list the guide shows, as catalog
//! channel indices in display order ([`ChannelList`]).
//!
//! The catalog (the full [`Schedule`]) stays the reference for playback,
//! search and history: the guide works on rows of the list through
//! [`ListView`], a [`Schedule`] whose channel `i` is the list's row `i`.
//! Rows are numbered by their place in the list (1..n), so every list
//! starts at 1.
//!
//! Lists are rebuilt when the selection, the catalog or the favorites
//! change; focus is put back by content identity ([`ListId`], channel
//! keys), never by index.

use std::rc::Rc;

use iptv_core::guide::{GroupName, Listing, Program, Schedule};

/// The groups column's fixed entries, before the playlist's groups.
pub const FAVORITES: usize = 0;
pub const RECENT: usize = 1;
/// The default selection.
pub const ALL: usize = 2;
pub const FIRST_GROUP: usize = 3;

/// An entry of the groups column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Favorites,
    Recent,
    All,
    /// A playlist group, by its index in the catalog.
    Group(usize),
}

impl Entry {
    pub fn at(index: usize) -> Entry {
        match index {
            FAVORITES => Entry::Favorites,
            RECENT => Entry::Recent,
            ALL => Entry::All,
            g => Entry::Group(g - FIRST_GROUP),
        }
    }

    pub fn index(self) -> usize {
        match self {
            Entry::Favorites => FAVORITES,
            Entry::Recent => RECENT,
            Entry::All => ALL,
            Entry::Group(g) => FIRST_GROUP + g,
        }
    }
}

/// How many entries the groups column has for `catalog`.
pub fn entry_count(catalog: &dyn Schedule) -> usize {
    FIRST_GROUP + catalog.group_count()
}

/// The name of an entry in the groups column, before localization.
pub fn entry_name(entry: usize, catalog: &dyn Schedule) -> GroupName {
    match Entry::at(entry) {
        Entry::Favorites => GroupName::Favorites,
        Entry::Recent => GroupName::Recent,
        Entry::All => catalog.all_channels_label(),
        Entry::Group(group) => catalog.group_label(group),
    }
}

/// An entry by content: survives a catalog swap that reorders, adds or
/// drops groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListId {
    Favorites,
    Recent,
    All,
    Group(String),
}

impl ListId {
    pub fn of(entry: usize, catalog: &dyn Schedule) -> ListId {
        match Entry::at(entry) {
            Entry::Favorites => ListId::Favorites,
            Entry::Recent => ListId::Recent,
            Entry::All => ListId::All,
            Entry::Group(g) => ListId::Group(catalog.group_key(g)),
        }
    }

    /// The entry with this identity in `catalog`: "All channels" when its
    /// group is gone.
    pub fn find(&self, catalog: &dyn Schedule) -> usize {
        match self {
            ListId::Favorites => FAVORITES,
            ListId::Recent => RECENT,
            ListId::All => ALL,
            ListId::Group(name) => (0..catalog.group_count())
                .find(|&g| catalog.group_key(g) == *name)
                .map_or(ALL, |g| Entry::Group(g).index()),
        }
    }
}

/// The groups column entry a channel resumed at startup
/// (`general.resume_last_channel`) is shown in, decided once the playlist's
/// catalog and favorites are both installed (the runtime installs the
/// favorites before it shows the catalog, and resumes after):
///
/// - Favorites when that category is offered and the channel is one of
///   this catalog's favorites (`favorites`; a channel favorited in
///   another playlist is not among them).
/// - All channels for a hidden channel: no list has a row for it, so the
///   guide keeps its start and focuses nothing else in its place.
/// - None otherwise: the usual place, the list on screen or else the
///   channel's own group.
pub fn resume_entry(
    channel: usize,
    catalog: &dyn Schedule,
    favorites: &[usize],
    favorites_offered: bool,
) -> Option<usize> {
    if channel >= catalog.channel_count() {
        return None;
    }
    if catalog.channel_hidden(channel) {
        return Some(ALL);
    }
    (favorites_offered && favorites.contains(&channel)).then_some(FAVORITES)
}

/// The channels a list shows: catalog indices in display order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelList {
    entry: usize,
    channels: Vec<usize>,
    first_listing: Option<i32>,
}

impl ChannelList {
    /// Creates a list in the given order.
    pub fn new(entry: usize, channels: Vec<usize>) -> Self {
        ChannelList {
            entry,
            channels,
            first_listing: None,
        }
    }

    /// The list of `entry` on `catalog`. `favorites` and `recent` are the
    /// catalog channels of those lists, in their order.
    pub fn build(
        entry: usize,
        catalog: &dyn Schedule,
        favorites: &[usize],
        recent: &[usize],
    ) -> Self {
        let n = catalog.channel_count();
        let channels = match Entry::at(entry) {
            Entry::Favorites => favorites.iter().copied().filter(|&c| c < n).collect(),
            Entry::Recent => recent.iter().copied().filter(|&c| c < n).collect(),
            Entry::All => catalog.playlist_order(),
            Entry::Group(g) if g < catalog.group_count() => catalog.group_channels(g),
            Entry::Group(_) => Vec::new(),
        };
        let channels: Vec<usize> = channels
            .into_iter()
            .filter(|&c| !catalog.channel_hidden(c))
            .collect();
        let first_listing = channels
            .iter()
            .filter_map(|&channel| catalog.listing_start(channel))
            .min();
        ChannelList {
            entry,
            channels,
            first_listing,
        }
    }

    /// The groups column entry this list belongs to.
    pub fn entry(&self) -> usize {
        self.entry
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    pub fn channels(&self) -> &[usize] {
        &self.channels
    }

    /// The catalog channel on `row`.
    pub fn channel(&self, row: usize) -> Option<usize> {
        self.channels.get(row).copied()
    }

    /// The row showing catalog `channel`.
    pub fn row_of(&self, channel: usize) -> Option<usize> {
        self.channels.iter().position(|&c| c == channel)
    }

    /// The number shown before a row's name: its place in the list.
    pub fn number(row: usize) -> u32 {
        row as u32 + 1
    }

    /// The number catalog `channel` shows wherever it appears (the player
    /// card, its lists): its place in this list, else (a channel the list
    /// does not have) the catalog's own number.
    pub fn number_of(&self, channel: usize, catalog: &dyn Schedule) -> u32 {
        self.row_of(channel)
            .map_or_else(|| catalog.channel_number(channel), ChannelList::number)
    }

    /// Channel up/down from `row` by `step`, wrapping within the list.
    pub fn zap(&self, row: usize, step: isize) -> Option<usize> {
        let len = self.len();
        (len > 0).then(|| (row.min(len - 1) as isize + step).rem_euclid(len as isize) as usize)
    }
}

/// A list as a [`Schedule`]: channel `i` is the list's row `i`, numbered by
/// position; programs, names, keys and streams are the catalog channel's.
/// The focus model and the grid run on it.
#[derive(Clone)]
pub struct ListView {
    catalog: Rc<dyn Schedule>,
    list: Rc<ChannelList>,
}

impl ListView {
    pub fn new(catalog: Rc<dyn Schedule>, list: Rc<ChannelList>) -> Self {
        ListView { catalog, list }
    }

    pub fn catalog(&self) -> &Rc<dyn Schedule> {
        &self.catalog
    }

    pub fn list(&self) -> &ChannelList {
        &self.list
    }

    /// The catalog channel on `row`.
    pub fn channel(&self, row: usize) -> Option<usize> {
        self.list.channel(row)
    }
}

/// An hour-long placeholder around `minute`, for rows a list does not have
/// (an empty list still has a cursor).
fn placeholder(minute: i32) -> Program {
    let start = minute - minute.rem_euclid(60);
    Program {
        start,
        end: start + 60,
    }
}

impl Schedule for ListView {
    fn epoch_unix(&self) -> i64 {
        self.catalog.epoch_unix()
    }
    fn follows_wall_clock(&self) -> bool {
        self.catalog.follows_wall_clock()
    }
    fn browsable(&self, now: i32) -> (i32, i32) {
        let (start, end) = self.catalog.browsable(now);
        let first = self
            .list
            .first_listing
            .map_or(now, |first| first.max(start).min(now));
        (first, end)
    }
    fn listing_start(&self, row: usize) -> Option<i32> {
        self.catalog.listing_start(self.channel(row)?)
    }
    fn channel_count(&self) -> usize {
        self.list.len()
    }
    /// A list is one group.
    fn group_count(&self) -> usize {
        1
    }
    fn group_blocked(&self, group: usize) -> bool {
        group == 0
            && matches!(Entry::at(self.list.entry()), Entry::Group(group) if self.catalog.group_blocked(group))
    }
    fn channel_block(&self, row: usize) -> iptv_core::organization::Block {
        self.channel(row)
            .map_or(iptv_core::organization::Block::None, |channel| {
                self.catalog.channel_block(channel)
            })
    }
    fn group_start(&self, _group: usize) -> usize {
        0
    }
    fn program_at(&self, row: usize, minute: i32) -> Program {
        match self.channel(row) {
            Some(c) => self.catalog.program_at(c, minute),
            None => placeholder(minute),
        }
    }
    fn programs_between(&self, row: usize, from: i32, minutes: i32) -> Vec<Program> {
        self.channel(row).map_or_else(Vec::new, |c| {
            self.catalog.programs_between(c, from, minutes)
        })
    }
    fn catchup_days(&self, row: usize) -> u8 {
        self.channel(row)
            .map_or(0, |c| self.catalog.catchup_days(c))
    }
    fn channel_name(&self, row: usize) -> String {
        self.channel(row)
            .map_or_else(String::new, |c| self.catalog.channel_name(c))
    }
    fn channel_number(&self, row: usize) -> u32 {
        ChannelList::number(row)
    }
    fn channel_logo(&self, row: usize) -> Option<String> {
        self.catalog.channel_logo(self.channel(row)?)
    }
    fn group_label(&self, _group: usize) -> GroupName {
        entry_name(self.list.entry(), &*self.catalog)
    }
    fn listing(&self, row: usize, program: Program) -> Option<Listing> {
        self.catalog.listing(self.channel(row)?, program)
    }
    fn stream_url(&self, row: usize) -> Option<String> {
        self.catalog.stream_url(self.channel(row)?)
    }
    fn catchup_url(&self, row: usize, program: Program, now: i32) -> Option<String> {
        self.catalog.catchup_url(self.channel(row)?, program, now)
    }
    fn channel_key(&self, row: usize) -> Option<String> {
        self.catalog.channel_key(self.channel(row)?)
    }
    fn find_channel(&self, key: &str) -> Option<usize> {
        self.list.row_of(self.catalog.find_channel(key)?)
    }
}

#[cfg(test)]
pub(crate) mod tests;
