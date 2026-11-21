//! Channel search: a folded copy of every channel's name (and extra keys
//! such as the channel number), scanned per query.
//!
//! A scan is cheap at playlist sizes (thousands of channels, a few words
//! each) and needs no rebuild when the query grows.

use std::collections::BTreeSet;

use iptv_core::guide::Schedule;
use iptv_epg::strip_tag_prefix;

use crate::text::{MatchQuality, Query, tokens};

/// One channel result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelHit {
    pub identity: crate::ChannelId,
    /// Channel index in the schedule.
    pub channel: usize,
    pub score: MatchQuality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelSearchOptions {
    /// Favourite channels before all others (then by match quality).
    pub favorites_first: bool,
    pub limit: usize,
}

impl Default for ChannelSearchOptions {
    fn default() -> Self {
        ChannelSearchOptions {
            favorites_first: false,
            limit: 100,
        }
    }
}

/// Searchable keys of every channel, by channel index.
#[derive(Clone, Debug, Default)]
pub struct ChannelIndex {
    identities: Vec<crate::ChannelId>,
    /// Per channel, its keys as folded words.
    keys: Vec<Vec<Vec<Box<str>>>>,
}

fn key_words(text: &str) -> Vec<Box<str>> {
    tokens(text)
        .into_iter()
        .map(String::into_boxed_str)
        .collect()
}

impl ChannelIndex {
    /// Indexes the channel names of `schedule`.
    pub fn from_schedule(schedule: &dyn Schedule) -> Self {
        let mut index = Self::from_names((0..schedule.channel_count()).map(|c| {
            if schedule.channel_hidden(c) {
                String::new()
            } else {
                schedule.channel_name(c)
            }
        }));
        for channel in 0..schedule.channel_count() {
            if let Some(key) = schedule.channel_key(channel) {
                index.set_identity(channel, crate::ChannelId::new(None, key));
            }
        }
        index
    }

    /// Indexes names in channel order. A leading country/group tag
    /// (`UK: `, `[DE] `) is also indexed without the tag, so the bare name
    /// can rank as exact or prefix. Anonymous row IDs distinguish duplicate
    /// labels within this index, and are never reused by a rebuilt index.
    /// Use [`Self::set_identity`] for content-stable focus across refreshes.
    pub fn from_names<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> Self {
        let names: Vec<String> = names
            .into_iter()
            .map(|name| name.as_ref().to_owned())
            .collect();
        let identities = names
            .iter()
            .map(|_| crate::ChannelId::anonymous())
            .collect();
        let keys = names
            .into_iter()
            .map(|name| {
                let name = name.as_ref();
                let mut keys = vec![key_words(name)];
                if let Some(bare) = strip_tag_prefix(name) {
                    keys.push(key_words(bare));
                }
                keys
            })
            .collect();
        ChannelIndex { keys, identities }
    }

    /// Associate this indexed row with its owning content identity.
    pub fn set_identity(&mut self, channel: usize, identity: crate::ChannelId) {
        if let Some(current) = self.identities.get_mut(channel) {
            *current = identity;
        }
    }

    /// Adds a searchable key (for example the channel number) to `channel`.
    /// Ignored for channels outside the index.
    pub fn add_key(&mut self, channel: usize, key: &str) {
        if let Some(keys) = self.keys.get_mut(channel) {
            let words = key_words(key);
            if !words.is_empty() {
                keys.push(words);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// How well `channel` matches: the best of its keys.
    pub fn match_channel(&self, query: &Query, channel: usize) -> Option<MatchQuality> {
        self.keys
            .get(channel)?
            .iter()
            .filter_map(|k| query.match_tokens(k))
            .max()
    }

    /// Channels matching `query`: favourites first when asked, then
    /// exact > prefix > word > word-prefix > substring, ties by channel
    /// index; at most `opts.limit`.
    pub fn search_channels(
        &self,
        query: &str,
        favorites: &BTreeSet<usize>,
        opts: ChannelSearchOptions,
    ) -> Vec<ChannelHit> {
        let query = Query::parse(query);
        if query.is_empty() || opts.limit == 0 {
            return Vec::new();
        }
        let mut hits: Vec<_> = (0..self.keys.len())
            .filter_map(|channel| {
                self.match_channel(&query, channel)
                    .map(|score| (channel, score))
            })
            .collect();
        let fav = |channel: usize| opts.favorites_first && favorites.contains(&channel);
        // Rank borrowed positions first: only returned hits clone their owned identity.
        hits.sort_by(|a, b| fav(b.0).cmp(&fav(a.0)).then(b.1.cmp(&a.1)));
        hits.truncate(opts.limit);
        hits.into_iter()
            .map(|(channel, score)| ChannelHit {
                channel,
                identity: self.identities[channel].clone(),
                score,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::MatchQuality::*;
    use super::*;
    use iptv_core::guide::SyntheticSchedule;

    const NAMES: [&str; 8] = [
        "Kestrel Sports",    // 0
        "Kestrel",           // 1
        "Night Kestrel HD",  // 2
        "Kestrelwave Radio", // 3
        "Bluekestrel",       // 4
        "Orchard News",      // 5
        "UK: Kestrel Two",   // 6
        "Kestrel",           // 7 (duplicate name)
    ];

    fn search(q: &str, fav: &[usize], opts: ChannelSearchOptions) -> Vec<(usize, MatchQuality)> {
        ChannelIndex::from_names(NAMES)
            .search_channels(q, &fav.iter().copied().collect(), opts)
            .into_iter()
            .map(|h| (h.channel, h.score))
            .collect()
    }

    #[test]
    fn ranking_exact_prefix_word_substring_then_index() {
        let hits = search("kestrel", &[], ChannelSearchOptions::default());
        assert_eq!(
            hits,
            [
                (1, Exact),
                (7, Exact),
                (0, Prefix),
                (3, Prefix),
                (6, Prefix), // via the name without "UK:"
                (2, Word),
                (4, Substring),
            ]
        );
    }

    #[test]
    fn as_you_type_and_all_tokens() {
        let names = |q| -> Vec<usize> {
            search(q, &[], ChannelSearchOptions::default())
                .into_iter()
                .map(|h| h.0)
                .collect()
        };
        assert_eq!(names("kes sp"), [0]);
        assert_eq!(names("sports kes"), [0]);
        assert_eq!(names("n"), [2, 5], "word starts only, name start first");
        assert_eq!(names("news orchard"), [5]);
        assert!(names("kestrel radio sports").is_empty());
        assert!(names("  ").is_empty());
    }

    #[test]
    fn favorites_first_and_limit() {
        let opts = ChannelSearchOptions {
            favorites_first: true,
            limit: 3,
        };
        assert_eq!(
            search("kestrel", &[4, 2], opts),
            [(2, Word), (4, Substring), (1, Exact)]
        );
        // Not asked: favourites rank like any channel.
        let opts = ChannelSearchOptions { limit: 2, ..opts };
        let plain = ChannelSearchOptions {
            favorites_first: false,
            ..opts
        };
        assert_eq!(search("kestrel", &[4], plain), [(1, Exact), (7, Exact)]);
    }

    #[test]
    fn extra_keys_such_as_channel_numbers() {
        let mut idx = ChannelIndex::from_names(NAMES);
        idx.add_key(5, "105");
        idx.add_key(1, "10");
        idx.add_key(99, "1");
        let hits = idx.search_channels("10", &BTreeSet::new(), ChannelSearchOptions::default());
        assert_eq!(
            hits.iter()
                .map(|h| (h.channel, h.score))
                .collect::<Vec<_>>(),
            [(1, Exact), (5, Prefix)]
        );
    }

    #[test]
    fn from_a_schedule() {
        let idx = ChannelIndex::from_schedule(&SyntheticSchedule);
        assert_eq!(idx.len(), iptv_core::guide::CHANNELS);
        let hits = idx.search_channels(
            "channel 42",
            &BTreeSet::new(),
            ChannelSearchOptions::default(),
        );
        assert_eq!(
            hits[0],
            ChannelHit {
                identity: crate::ChannelId::new(None, SyntheticSchedule.channel_key(41).unwrap()),
                channel: 41,
                score: Exact
            }
        );
        // "Channel 420".. "Channel 4299" follow as prefixes, in index order.
        assert_eq!(hits[1].channel, 419);
        assert!(hits.iter().skip(1).all(|h| h.score == Prefix));
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    use iptv_core::guide::{Program, Schedule, SyntheticSchedule};
    struct VisibleCatalog;
    impl Schedule for VisibleCatalog {
        fn channel_count(&self) -> usize {
            3
        }
        fn group_count(&self) -> usize {
            1
        }
        fn group_start(&self, _: usize) -> usize {
            0
        }
        fn group_name(&self, _: usize) -> String {
            "Local".into()
        }
        fn channel_name(&self, c: usize) -> String {
            ["Hidden news", "Visible news", "Visible sport"][c].into()
        }
        fn channel_hidden(&self, c: usize) -> bool {
            c == 0
        }
        fn catchup_days(&self, _: usize) -> u8 {
            0
        }
        fn program_at(&self, c: usize, t: i32) -> Program {
            SyntheticSchedule.program_at(c, t)
        }
        fn programs_between(&self, c: usize, t: i32, m: i32) -> Vec<Program> {
            SyntheticSchedule.programs_between(c, t, m)
        }
        fn title(&self, c: usize, p: Program) -> String {
            SyntheticSchedule.title(c, p)
        }
    }
    #[test]
    fn hidden_channels_cannot_match_or_consume_the_visible_result_limit() {
        let index = ChannelIndex::from_schedule(&VisibleCatalog);
        let options = ChannelSearchOptions {
            limit: 1,
            favorites_first: true,
        };
        let favorites = [0].into_iter().collect();
        assert!(
            index
                .search_channels("hidden", &favorites, options)
                .is_empty()
        );
        assert_eq!(
            index.search_channels("news", &favorites, options)[0].channel,
            1
        );
        assert_eq!(
            index.len(),
            3,
            "result indices remain catalog/playback indices"
        );
    }
}
