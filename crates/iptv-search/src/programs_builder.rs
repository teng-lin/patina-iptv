//! A [`ProgramIndex`] from programs read one at a time (a stored
//! guide's titles streamed from disk) instead of a [`iptv_epg::GuideStore`]
//! held in memory.

use std::collections::HashMap;

use iptv_epg::UnixTime;

use crate::programs::ProgramIndex;

/// Collects programs for a [`ProgramIndex`].
///
/// ```
/// use iptv_search::{ProgramIndexBuilder, ProgramSearchOptions};
/// let mut b = ProgramIndexBuilder::new(2);
/// b.push(1, 1000, 4600, "Evening News");
/// let index = b.finish();
/// let opts = ProgramSearchOptions { include_past_without_catchup: true, ..Default::default() };
/// let hits = index.search_programs("news", 2000, opts);
/// assert_eq!((hits[0].channel, hits[0].start), (1, 1000));
/// ```
#[derive(Default)]
pub struct ProgramIndexBuilder {
    channels: usize,
    identities: Vec<crate::ChannelId>,
    ids: HashMap<Box<str>, u32>,
    /// Title id, channel, start, stop.
    raw: Vec<(u32, u32, i64, i64)>,
}

impl ProgramIndexBuilder {
    /// For `channels` channels (indexes `0..channels`). Initial identities are
    /// anonymous and scoped to this builder/index; supply real content IDs with
    /// [`Self::set_identity`] to retain focus across rebuilt indexes.
    pub fn new(channels: usize) -> Self {
        ProgramIndexBuilder {
            channels,
            identities: (0..channels)
                .map(|_| crate::ChannelId::anonymous())
                .collect(),
            ..Default::default()
        }
    }

    pub fn set_identity(&mut self, channel: usize, identity: crate::ChannelId) {
        if let Some(current) = self.identities.get_mut(channel) {
            *current = identity;
        }
    }

    /// Adds one program; invalid channels, nonpositive or unrepresentable durations are ignored.
    pub fn push(&mut self, channel: usize, start: UnixTime, stop: UnixTime, title: &str) {
        if channel >= self.channels
            || stop <= start
            || u32::try_from(i128::from(stop) - i128::from(start)).is_err()
        {
            return;
        }
        let next = self.ids.len() as u32;
        let id = match self.ids.get(title) {
            Some(&id) => id,
            None => {
                self.ids.insert(title.into(), next);
                next
            }
        };
        self.raw.push((id, channel as u32, start, stop));
    }

    /// Programs added.
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    pub fn finish(self) -> ProgramIndex {
        let mut titles = vec![""; self.ids.len()];
        for (title, &id) in &self.ids {
            titles[id as usize] = title;
        }
        ProgramIndex::assemble(&titles, &self.raw, self.identities)
    }
}

#[cfg(test)]
mod tests {
    use iptv_epg::{
        ChannelKey, EpgRef, FeedId, GuideStore, MatchKind, SourceBuilder, StoreOptions,
    };

    use super::*;
    use crate::ProgramSearchOptions;

    #[test]
    fn compressed_titles_keep_source_incarnations_and_allow_memory_qualification() {
        let first = crate::ChannelId::in_source(1, "generated-first", "shared-key");
        let second = crate::ChannelId::in_source(2, "generated-second", "shared-key");
        let mut builder = ProgramIndexBuilder::new(2);
        builder.set_identity(0, first.clone());
        builder.set_identity(1, second.clone());
        for channel in 0..2 {
            builder.push(channel, 1000, 4600, "Generated bulletin");
        }
        let mut index = builder.finish();
        let hits = index.search_programs("bulletin", 2000, Default::default());
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].identity, first);
        assert_eq!(hits[1].identity, second);
        let recreated = crate::ChannelId::in_source(1, "generated-recreated", "shared-key");
        index.set_identity(0, recreated.clone());
        let qualified = index.search_programs("bulletin", 2000, Default::default());
        assert_eq!(qualified[0].identity, recreated);
        assert_ne!(qualified[0].identity, hits[0].identity);
        assert_eq!(qualified[1].identity, hits[1].identity);
    }

    #[test]
    fn invalid_or_unrepresentable_intervals_are_skipped_without_corrupting_hits() {
        let mut b = ProgramIndexBuilder::new(1);
        b.push(0, i64::MIN, i64::MAX, "Invalid span");
        b.push(0, 0, 10, "Synthetic news");
        b.push(
            0,
            i64::from(u32::MAX) + 1,
            i64::from(u32::MAX) + 2,
            "Outside compact range",
        );
        let index = b.finish();
        assert_eq!(index.program_count(), 1);
        let opts = ProgramSearchOptions {
            include_past_without_catchup: true,
            ..Default::default()
        };
        let hits = index.search_programs("synthetic", 5, opts);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].start, hits[0].end), (0, 10));
        for now in [i64::MIN, i64::MAX] {
            let _ = index.search_programs("synthetic", now, opts);
        }
    }

    #[test]
    fn built_from_rows_it_finds_what_the_store_index_finds() {
        let rows = [
            (0, 1000, 2000, "Morning News"),
            (0, 2000, 3000, "Cooking Hour"),
            (1, 1500, 2500, "News at Noon"),
            (1, 2500, 9000, "Morning News"),
        ];
        let mut b = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), 0);
        let mut built = ProgramIndexBuilder::new(2);
        built.set_identity(0, crate::ChannelId::new(None, "a"));
        built.set_identity(1, crate::ChannelId::new(None, "b"));
        for (ch, start, stop, title) in rows {
            b.push_data(
                &format!("e{ch}"),
                &iptv_epg::ProgramData {
                    start,
                    stop: Some(stop),
                    title,
                    ..Default::default()
                },
            );
            built.push(ch, start, stop, title);
        }
        built.push(7, 0, 10, "beyond the channels");
        assert_eq!(built.len(), 4);
        let mut store = GuideStore::new();
        store.replace_source(b.finish());
        let keys = [ChannelKey::new("a"), ChannelKey::new("b")];
        let matches = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let cand = EpgRef {
                    source: FeedId(1),
                    epg_id: format!("e{i}"),
                    kind: MatchKind::TvgId,
                };
                (k.clone(), vec![cand])
            })
            .collect();
        store.set_matches(matches);
        let from_store = ProgramIndex::build(&store, &keys);
        let built = built.finish();
        assert_eq!(built.program_count(), from_store.program_count());
        assert_eq!(built.title_count(), from_store.title_count());
        let opts = ProgramSearchOptions {
            include_past_without_catchup: true,
            ..Default::default()
        };
        for q in ["news", "morning", "cook", "noon"] {
            assert_eq!(
                built.search_programs(q, 2200, opts),
                from_store.search_programs(q, 2200, opts),
                "{q}"
            );
        }
    }
}
