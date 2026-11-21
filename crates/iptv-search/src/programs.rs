//! Programme search over a [`GuideStore`]: an inverted index of folded
//! title words, built once per guide load.
//!
//! # Layout and memory
//!
//! Titles repeat (series, news), so the index works on distinct titles:
//!
//! - every programme: one 12-byte occurrence (channel, start, duration),
//!   grouped by title;
//! - every distinct title: its text, three 4-byte offsets and 4 bytes per
//!   word;
//! - every distinct word: its text (a sorted `Box<str>`, 16 bytes + text),
//!   a 4-byte offset and 4 bytes per title containing it.
//!
//! For the synthetic 2,000 channels × 7 days of the `program_bench`
//! example (312,000 programmes, 84,000 distinct titles, 12,000 words) that
//! is about 9 MiB, close to the store itself; the occurrences (3.6 MiB) and
//! the title text copied from the store (about 2.5 MiB) dominate.
//! [`ProgramIndex::heap_bytes`] reports the actual figure. Times are
//! absolute, so the index stays valid as `now` moves; rebuild it when the
//! store changes.
//!
//! # Query
//!
//! Each query token selects words from the sorted vocabulary (a prefix range
//! for short tokens, a scan for substrings), their titles are intersected in
//! a bitset, and every candidate title is checked with the same matcher as
//! channel search ([`Query::match_words`]). Occurrences of matching titles
//! are filtered by the time window and catch-up, then ranked.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use iptv_core::guide::Airing;
use iptv_epg::{ChannelKey, DAY, GuideStore, UnixTime};

use crate::text::{MatchQuality, Query, tokens};

/// How programme results are ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    /// Match quality, then nearest to now: live, upcoming soonest, then
    /// most recent past.
    #[default]
    Relevance,
    /// Channels in the order of their best result; each channel's
    /// programmes together, in start order.
    Channel,
    /// Live, then upcoming soonest, then most recent past; match quality
    /// only breaks ties.
    Time,
}

#[derive(Clone, Copy)]
pub struct ProgramSearchOptions<'a> {
    /// Visibility is applied before ranking/limiting, preserving catalog indices.
    pub visible: Option<&'a dyn Fn(usize) -> bool>,
    /// Past programmes are dropped unless the channel's archive still has
    /// them; `true` keeps them anyway (shown as without catch-up).
    pub include_past_without_catchup: bool,
    /// The channel's archive window in days (0: none). `None`: no channel
    /// has catch-up.
    pub catchup_days: Option<&'a dyn Fn(usize) -> u8>,
    /// Seconds before and after `now` to search (programmes overlapping
    /// the window count).
    pub look_back: i64,
    pub look_ahead: i64,
    pub group_by: GroupBy,
    /// Results on these channels first (only with `favorites_first`).
    pub favorites: Option<&'a BTreeSet<usize>>,
    pub favorites_first: bool,
    pub limit: usize,
}

impl Default for ProgramSearchOptions<'_> {
    fn default() -> Self {
        ProgramSearchOptions {
            visible: None,
            include_past_without_catchup: false,
            catchup_days: None,
            look_back: 7 * DAY,
            look_ahead: 3 * DAY,
            group_by: GroupBy::Relevance,
            favorites: None,
            favorites_first: false,
            limit: 200,
        }
    }
}

/// One programme result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramHit {
    pub identity: crate::ChannelId,
    pub channel: usize,
    pub start: UnixTime,
    pub end: UnixTime,
    pub title: String,
    /// Relative to the `now` of the search.
    pub airing: Airing,
    /// Playable from the archive (live programmes: restartable).
    pub catchup: bool,
    pub score: MatchQuality,
}

#[derive(Clone, Copy, Debug)]
struct Occ {
    channel: u32,
    /// Seconds after the index base.
    start: u32,
    dur: u32,
}

/// Searchable programme titles of a [`GuideStore`], by channel index.
#[derive(Clone, Debug, Default)]
pub struct ProgramIndex {
    base: i64,
    channels: usize,
    identities: Vec<crate::ChannelId>,
    /// Distinct folded words, sorted.
    vocab: Vec<Box<str>>,
    /// Titles containing each word: `post[post_off[w]..post_off[w + 1]]`.
    post_off: Vec<u32>,
    post: Vec<u32>,
    /// Per distinct title: text, words (vocabulary ids, in order) and
    /// occurrences, each `x[x_off[t]..x_off[t + 1]]`.
    text: String,
    text_off: Vec<u32>,
    word_off: Vec<u32>,
    words: Vec<u32>,
    occ_off: Vec<u32>,
    occ: Vec<Occ>,
}

fn offsets_from_counts(counts: &[u32]) -> Vec<u32> {
    let mut off = Vec::with_capacity(counts.len() + 1);
    let mut sum = 0;
    off.push(0);
    for &c in counts {
        sum += c;
        off.push(sum);
    }
    off
}

impl ProgramIndex {
    /// Indexes every programme the store holds for `channels` (channel
    /// index = position; unbound keys simply have no programmes).
    pub fn build<'k>(
        store: &GuideStore,
        channels: impl IntoIterator<Item = &'k ChannelKey>,
    ) -> Self {
        let mut ids: HashMap<&str, u32> = HashMap::new();
        let mut titles: Vec<&str> = Vec::new();
        let mut raw: Vec<(u32, u32, i64, i64)> = Vec::new();
        let mut identities = Vec::new();
        for (ch, key) in channels.into_iter().enumerate() {
            identities.push(crate::ChannelId::new(None, key.as_str()));
            let Some(sched) = store.schedule(key) else {
                continue;
            };
            for p in sched.iter() {
                let title = p.title();
                let id = *ids.entry(title).or_insert_with(|| {
                    titles.push(title);
                    (titles.len() - 1) as u32
                });
                raw.push((id, ch as u32, p.start(), p.stop()));
            }
        }
        Self::assemble(&titles, &raw, identities)
    }

    /// The index of `raw` (title id, channel, start, stop) with the titles
    /// by id (`ProgramIndexBuilder` collects them from elsewhere).
    pub(crate) fn assemble(
        titles: &[&str],
        raw: &[(u32, u32, i64, i64)],
        identities: Vec<crate::ChannelId>,
    ) -> Self {
        let valid = raw
            .iter()
            .filter(|r| r.3 > r.2 && u32::try_from(i128::from(r.3) - i128::from(r.2)).is_ok());
        let base = valid.clone().map(|r| r.2).min().unwrap_or(0);
        // Omit rows outside the compact range instead of fabricating offsets.
        let raw = valid.filter(|r| u32::try_from(i128::from(r.2) - i128::from(base)).is_ok());

        // Words in first-seen order, then sorted and renumbered.
        let mut word_ids: HashMap<String, u32> = HashMap::new();
        let mut vocab: Vec<String> = Vec::new();
        let mut text = String::new();
        let mut text_off = vec![0];
        let mut word_off = vec![0];
        let mut words = Vec::new();
        for title in titles {
            text.push_str(title);
            text_off.push(text.len() as u32);
            for w in tokens(title) {
                let next = vocab.len() as u32;
                let id = *word_ids.entry(w).or_insert_with_key(|w| {
                    vocab.push(w.clone());
                    next
                });
                words.push(id);
            }
            word_off.push(words.len() as u32);
        }
        drop(word_ids);
        text.shrink_to_fit();
        text_off.shrink_to_fit();
        word_off.shrink_to_fit();
        words.shrink_to_fit();
        let mut order: Vec<u32> = (0..vocab.len() as u32).collect();
        order.sort_unstable_by(|&a, &b| vocab[a as usize].cmp(&vocab[b as usize]));
        let mut remap = vec![0u32; vocab.len()];
        for (new, &old) in order.iter().enumerate() {
            remap[old as usize] = new as u32;
        }
        for w in &mut words {
            *w = remap[*w as usize];
        }
        let vocab: Vec<Box<str>> = order
            .iter()
            .map(|&o| std::mem::take(&mut vocab[o as usize]).into_boxed_str())
            .collect();

        // Postings: titles per word, ascending, each title once.
        let title_words = |t: usize| -> Vec<u32> {
            let mut w = words[word_off[t] as usize..word_off[t + 1] as usize].to_vec();
            w.sort_unstable();
            w.dedup();
            w
        };
        let mut counts = vec![0u32; vocab.len()];
        for t in 0..titles.len() {
            for w in title_words(t) {
                counts[w as usize] += 1;
            }
        }
        let post_off = offsets_from_counts(&counts);
        let mut fill = post_off[..vocab.len()].to_vec();
        let mut post = vec![0u32; *post_off.last().unwrap_or(&0) as usize];
        for t in 0..titles.len() {
            for w in title_words(t) {
                post[fill[w as usize] as usize] = t as u32;
                fill[w as usize] += 1;
            }
        }

        // Occurrences grouped by title (counting sort keeps input order).
        let mut counts = vec![0u32; titles.len()];
        for r in raw.clone() {
            counts[r.0 as usize] += 1;
        }
        let occ_off = offsets_from_counts(&counts);
        let mut fill = occ_off[..titles.len()].to_vec();
        let mut occ = vec![
            Occ {
                channel: 0,
                start: 0,
                dur: 0
            };
            raw.clone().count()
        ];
        for &(t, channel, start, stop) in raw {
            let i = &mut fill[t as usize];
            occ[*i as usize] = Occ {
                channel,
                start: (i128::from(start) - i128::from(base)) as u32,
                dur: (i128::from(stop) - i128::from(start)) as u32,
            };
            *i += 1;
        }

        ProgramIndex {
            base,
            channels: identities.len(),
            identities,
            vocab,
            post_off,
            post,
            text,
            text_off,
            word_off,
            words,
            occ_off,
            occ,
        }
    }

    /// Bind a channel's occurrences to their accepting source identity.
    pub fn set_identity(&mut self, channel: usize, identity: crate::ChannelId) {
        if let Some(current) = self.identities.get_mut(channel) {
            *current = identity;
        }
    }

    /// Channel identities in the index's row order, including rows with no titles.
    pub fn channel_identities(&self) -> &[crate::ChannelId] {
        &self.identities
    }

    /// Programmes indexed.
    pub fn program_count(&self) -> usize {
        self.occ.len()
    }

    /// Distinct titles indexed.
    pub fn title_count(&self) -> usize {
        self.text_off.len().saturating_sub(1)
    }

    /// Distinct folded words indexed.
    pub fn word_count(&self) -> usize {
        self.vocab.len()
    }

    /// Heap usage in bytes.
    pub fn heap_bytes(&self) -> usize {
        let u32s = self.post_off.capacity()
            + self.post.capacity()
            + self.text_off.capacity()
            + self.word_off.capacity()
            + self.words.capacity()
            + self.occ_off.capacity();
        self.identities.capacity() * size_of::<crate::ChannelId>()
            + self
                .identities
                .iter()
                .map(crate::ChannelId::heap_bytes)
                .sum::<usize>()
            + u32s * 4
            + self.occ.capacity() * size_of::<Occ>()
            + self.text.capacity()
            + self.vocab.capacity() * size_of::<Box<str>>()
            + self.vocab.iter().map(|w| w.len()).sum::<usize>()
    }

    fn range(off: &[u32], i: usize) -> std::ops::Range<usize> {
        off[i] as usize..off[i + 1] as usize
    }

    fn title(&self, t: usize) -> &str {
        &self.text[Self::range(&self.text_off, t)]
    }

    fn title_words(&self, t: usize) -> &[u32] {
        &self.words[Self::range(&self.word_off, t)]
    }

    fn postings(&self, w: usize) -> &[u32] {
        &self.post[Self::range(&self.post_off, w)]
    }

    /// Vocabulary ids of the words starting with `p`.
    fn prefix_range(&self, p: &str) -> std::ops::Range<usize> {
        let lo = self.vocab.partition_point(|w| &**w < p);
        let len = self.vocab[lo..].partition_point(|w| w.starts_with(p));
        lo..lo + len
    }

    /// A superset of the titles `q` matches, as a bitset.
    fn candidates(&self, q: &Query) -> Vec<u64> {
        let n = self.title_count().div_ceil(64);
        let set = |bits: &mut [u64], t: u32| bits[t as usize / 64] |= 1 << (t % 64);
        let mut acc: Option<Vec<u64>> = None;
        for (i, tok) in q.tokens().iter().enumerate() {
            let mut bits = vec![0u64; n];
            let mut add = |w: usize| self.postings(w).iter().for_each(|&t| set(&mut bits, t));
            if q.infix(i) {
                (0..self.vocab.len())
                    .filter(|&w| self.vocab[w].contains(tok.as_str()))
                    .for_each(&mut add);
            } else {
                self.prefix_range(tok).for_each(&mut add);
            }
            acc = Some(match acc {
                None => bits,
                Some(mut a) => {
                    a.iter_mut().zip(&bits).for_each(|(a, b)| *a &= b);
                    a
                }
            });
        }
        let mut acc = acc.unwrap_or_else(|| vec![0; n]);
        // Exact / prefix matches across word boundaries ("nightowl" for
        // "Night Owl"): titles whose first word is a prefix of the joined
        // query, or (several tokens) starts with it.
        let j = q.joined();
        let mut add_first = |w: usize| {
            for &t in self.postings(w) {
                if self.title_words(t as usize).first() == Some(&(w as u32)) {
                    set(&mut acc, t);
                }
            }
        };
        let ends = j.char_indices().skip(1).map(|(i, _)| i).chain([j.len()]);
        for end in ends {
            if let Ok(w) = self.vocab.binary_search_by(|v| (**v).cmp(&j[..end])) {
                add_first(w);
            }
        }
        if q.tokens().len() > 1 {
            self.prefix_range(j).for_each(add_first);
        }
        acc
    }

    /// Programmes whose title matches `query` (see [`GroupBy`] for the
    /// order), at most `opts.limit`.
    pub fn search_programs(
        &self,
        query: &str,
        now: UnixTime,
        opts: ProgramSearchOptions<'_>,
    ) -> Vec<ProgramHit> {
        let q = Query::parse(query);
        if q.is_empty() || opts.limit == 0 || self.occ.is_empty() {
            return Vec::new();
        }
        let days: Vec<u8> = match opts.catchup_days {
            Some(f) => (0..self.channels).map(f).collect(),
            None => vec![0; self.channels],
        };
        let (from, to) = (
            now.saturating_sub(opts.look_back),
            now.saturating_add(opts.look_ahead),
        );
        let mut cands = Vec::new();
        for (wi, &bits) in self.candidates(&q).iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                let t = wi * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let words = self.title_words(t);
                let Some(score) = q.match_words(words.len(), |k| &self.vocab[words[k] as usize])
                else {
                    continue;
                };
                for o in &self.occ[Self::range(&self.occ_off, t)] {
                    let start = self.base + i64::from(o.start);
                    let end = start + i64::from(o.dur);
                    if end <= from || start >= to {
                        continue;
                    }
                    let ch = o.channel as usize;
                    if opts.visible.is_some_and(|f| !f(ch)) {
                        continue;
                    }
                    let archive = now.saturating_sub(i64::from(days[ch]) * DAY);
                    let (airing, class, dist) = if end <= now {
                        (Airing::Ended, 2, now.abs_diff(end))
                    } else if start <= now {
                        (Airing::Live, 0, 0)
                    } else {
                        (Airing::Upcoming, 1, start.abs_diff(now))
                    };
                    let catchup = airing != Airing::Upcoming && days[ch] > 0 && start >= archive;
                    if airing == Airing::Ended && !catchup && !opts.include_past_without_catchup {
                        continue;
                    }
                    let fav =
                        opts.favorites_first && opts.favorites.is_some_and(|f| f.contains(&ch));
                    cands.push(Cand {
                        fav,
                        score,
                        class,
                        dist,
                        channel: o.channel,
                        start,
                        end,
                        title: t as u32,
                        airing,
                        catchup,
                    });
                }
            }
        }
        let cands = order(cands, opts.group_by, opts.limit);
        cands
            .into_iter()
            .map(|c| ProgramHit {
                identity: self.identities[c.channel as usize].clone(),
                channel: c.channel as usize,
                start: c.start,
                end: c.end,
                title: self.title(c.title as usize).to_owned(),
                airing: c.airing,
                catchup: c.catchup,
                score: c.score,
            })
            .collect()
    }
}

struct Cand {
    fav: bool,
    score: MatchQuality,
    /// 0 live, 1 upcoming, 2 past.
    class: u8,
    /// Seconds from now (live: 0).
    dist: u64,
    channel: u32,
    start: i64,
    end: i64,
    title: u32,
    airing: Airing,
    catchup: bool,
}

fn relevance(a: &Cand, b: &Cand) -> Ordering {
    (b.fav.cmp(&a.fav))
        .then(b.score.cmp(&a.score))
        .then(a.class.cmp(&b.class))
        .then(a.dist.cmp(&b.dist))
        .then(a.channel.cmp(&b.channel))
        .then(a.start.cmp(&b.start))
}

fn by_time(a: &Cand, b: &Cand) -> Ordering {
    (b.fav.cmp(&a.fav))
        .then(a.class.cmp(&b.class))
        .then(a.dist.cmp(&b.dist))
        .then(b.score.cmp(&a.score))
        .then(a.channel.cmp(&b.channel))
        .then(a.start.cmp(&b.start))
}

/// Sorts and truncates. (channel, start) is unique, so every order is total.
fn order(mut c: Vec<Cand>, group_by: GroupBy, limit: usize) -> Vec<Cand> {
    let cmp = match group_by {
        GroupBy::Time => by_time,
        GroupBy::Relevance | GroupBy::Channel => relevance,
    };
    if group_by == GroupBy::Channel {
        c.sort_unstable_by(cmp);
        let mut rank: HashMap<u32, usize> = HashMap::new();
        for x in &c {
            let next = rank.len();
            rank.entry(x.channel).or_insert(next);
        }
        c.sort_unstable_by_key(|x| (rank[&x.channel], x.start));
    } else {
        if c.len() > limit {
            c.select_nth_unstable_by(limit - 1, cmp);
            c.truncate(limit);
        }
        c.sort_unstable_by(cmp);
    }
    c.truncate(limit);
    c
}

#[cfg(test)]
#[path = "programs_tests.rs"]
mod tests;
