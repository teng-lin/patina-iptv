//! Matching playlist channels to EPG channels.
//!
//! Order per source: manual override, then tvg-id (exact, then
//! case-insensitive), then normalized name (playlist name or tvg-name, then
//! the same with a `UK:`-style tag removed). Each source is matched on its
//! own; candidates are returned highest priority first so the store can
//! pick the best source that has data.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::normalize::{normalize_name, strip_tag_prefix};
use crate::xmltv::XmltvChannel;

/// Stable identity of a playlist channel. Opaque here: the providers crate
/// decides what it contains.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelKey(pub String);

impl ChannelKey {
    pub fn new(key: impl Into<String>) -> Self {
        ChannelKey(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ChannelKey {
    fn from(s: &str) -> Self {
        ChannelKey(s.to_string())
    }
}

impl From<String> for ChannelKey {
    fn from(s: String) -> Self {
        ChannelKey(s)
    }
}

impl fmt::Display for ChannelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of one configured EPG source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeedId(pub u32);

/// What matching needs to know about a playlist channel.
#[derive(Clone, Copy, Debug)]
pub struct PlaylistChannel<'a> {
    pub key: &'a ChannelKey,
    /// `tvg-id` (or the provider's EPG channel id).
    pub tvg_id: Option<&'a str>,
    /// Display name.
    pub name: &'a str,
    /// `tvg-name`, when it differs from the display name.
    pub tvg_name: Option<&'a str>,
}

/// A manual EPG assignment; always wins over automatic matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Override {
    /// Use this EPG channel id, from one source or (`None`) from every
    /// source that has it.
    Epg {
        source: Option<FeedId>,
        epg_id: String,
    },
    /// The user switched the guide off for this channel.
    NoEpg,
}

/// Manual assignments by playlist channel.
pub type Overrides = HashMap<ChannelKey, Override>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchKind {
    Override,
    TvgId,
    Name,
}

/// One EPG channel a playlist channel may take its guide from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpgRef {
    pub source: FeedId,
    pub epg_id: String,
    pub kind: MatchKind,
}

/// Counts for the "EPG matched N of M channels" summary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchReport {
    pub channels: usize,
    pub by_override: usize,
    pub by_tvg_id: usize,
    pub by_name: usize,
    /// Matched by a name that several EPG channels of one source share
    /// (the first such channel in document order was used).
    pub ambiguous_name: usize,
    /// Overrides that name an EPG channel no loaded source has.
    pub override_unresolved: usize,
    /// Guide switched off by [`Override::NoEpg`].
    pub disabled: usize,
    pub unmatched: usize,
    pub unmatched_keys: Vec<ChannelKey>,
}

impl MatchReport {
    pub fn matched(&self) -> usize {
        self.by_override + self.by_tvg_id + self.by_name
    }
}

#[derive(Clone, Debug, Default)]
pub struct MatchResult {
    /// Candidates per playlist channel, highest source priority first.
    /// Channels without any candidate are absent.
    pub matches: HashMap<ChannelKey, Vec<EpgRef>>,
    pub report: MatchReport,
}

#[derive(Clone, Copy)]
struct NameEntry {
    id: u32,
    ambiguous: bool,
}

struct SourceIndex {
    source: FeedId,
    priority: i32,
    ids: Vec<Box<str>>,
    exact: HashMap<Box<str>, u32>,
    caseless: HashMap<String, u32>,
    names: HashMap<String, NameEntry>,
}

impl SourceIndex {
    fn by_id(&self, tvg_id: &str) -> Option<u32> {
        let tvg_id = tvg_id.trim();
        if tvg_id.is_empty() {
            return None;
        }
        self.exact
            .get(tvg_id)
            .or_else(|| self.caseless.get(&tvg_id.to_lowercase()))
            .copied()
    }
}

/// Name keys of one playlist channel, best first.
struct Keys<'a> {
    ch: PlaylistChannel<'a>,
    names: Vec<String>,
}

impl<'a> Keys<'a> {
    fn new(ch: PlaylistChannel<'a>) -> Self {
        let raw = [Some(ch.name), ch.tvg_name];
        let mut names: Vec<String> = Vec::new();
        let stripped = raw.iter().flatten().filter_map(|n| strip_tag_prefix(n));
        for n in raw.iter().flatten().copied().chain(stripped) {
            let k = normalize_name(n);
            if !k.is_empty() && !names.contains(&k) {
                names.push(k);
            }
        }
        Keys { ch, names }
    }
}

/// EPG channels of every source, indexed for matching.
#[derive(Default)]
pub struct MatchIndex {
    /// Highest priority first; equal priorities keep insertion order.
    sources: Vec<SourceIndex>,
}

impl MatchIndex {
    pub fn new() -> Self {
        MatchIndex::default()
    }

    /// Adds (or replaces) the channels of one source. Higher `priority`
    /// wins when several sources match the same playlist channel.
    pub fn add_source<'a>(
        &mut self,
        source: FeedId,
        priority: i32,
        channels: impl IntoIterator<Item = &'a XmltvChannel>,
    ) {
        self.remove_source(source);
        let mut idx = SourceIndex {
            source,
            priority,
            ids: Vec::new(),
            exact: HashMap::new(),
            caseless: HashMap::new(),
            names: HashMap::new(),
        };
        let mut stripped: Vec<(String, u32)> = Vec::new();
        for ch in channels {
            let id = ch.id.trim();
            if id.is_empty() || idx.exact.contains_key(id) {
                continue;
            }
            let n = idx.ids.len() as u32;
            idx.ids.push(id.into());
            idx.exact.insert(id.into(), n);
            idx.caseless.entry(id.to_lowercase()).or_insert(n);
            for name in &ch.display_names {
                insert_name(&mut idx.names, normalize_name(name), n);
                if let Some(s) = strip_tag_prefix(name) {
                    stripped.push((normalize_name(s), n));
                }
            }
        }
        // Tag-stripped names never displace a full name.
        for (key, n) in stripped {
            if !idx.names.contains_key(&key) {
                insert_name(&mut idx.names, key, n);
            }
        }
        let at = self
            .sources
            .iter()
            .position(|s| s.priority < priority)
            .unwrap_or(self.sources.len());
        self.sources.insert(at, idx);
    }

    pub fn remove_source(&mut self, source: FeedId) {
        self.sources.retain(|s| s.source != source);
    }

    /// Whether `source` has a channel with exactly this id.
    pub fn contains(&self, source: FeedId, epg_id: &str) -> bool {
        self.sources
            .iter()
            .any(|s| s.source == source && s.exact.contains_key(epg_id))
    }

    /// Matches playlist channels; `overrides` win over automatic matching.
    pub fn match_channels<'a>(
        &self,
        playlist: impl IntoIterator<Item = PlaylistChannel<'a>>,
        overrides: &Overrides,
    ) -> MatchResult {
        let mut result = MatchResult::default();
        let report = &mut result.report;
        for ch in playlist {
            report.channels += 1;
            let key = ch.key;
            if let Some(o) = overrides.get(key) {
                let (cands, resolved) = self.resolve_override(o);
                match o {
                    Override::NoEpg => report.disabled += 1,
                    _ if resolved => report.by_override += 1,
                    _ => report.override_unresolved += 1,
                }
                if !cands.is_empty() {
                    result.matches.insert(key.clone(), cands);
                }
                continue;
            }
            let keys = Keys::new(ch);
            let mut cands = Vec::new();
            let mut ambiguous_first = false;
            for src in &self.sources {
                if let Some((id, kind, ambiguous)) = Self::auto(src, &keys) {
                    ambiguous_first |= cands.is_empty() && ambiguous;
                    cands.push(EpgRef {
                        source: src.source,
                        epg_id: src.ids[id as usize].to_string(),
                        kind,
                    });
                }
            }
            match cands.first().map(|c| c.kind) {
                Some(MatchKind::TvgId) => report.by_tvg_id += 1,
                Some(_) => report.by_name += 1,
                None => {
                    report.unmatched += 1;
                    report.unmatched_keys.push(key.clone());
                    continue;
                }
            }
            report.ambiguous_name += usize::from(ambiguous_first);
            result.matches.insert(key.clone(), cands);
        }
        result
    }

    fn auto(src: &SourceIndex, keys: &Keys) -> Option<(u32, MatchKind, bool)> {
        if let Some(id) = keys.ch.tvg_id.and_then(|t| src.by_id(t)) {
            return Some((id, MatchKind::TvgId, false));
        }
        keys.names
            .iter()
            .find_map(|k| src.names.get(k))
            .map(|e| (e.id, MatchKind::Name, e.ambiguous))
    }

    fn resolve_override(&self, o: &Override) -> (Vec<EpgRef>, bool) {
        let Override::Epg { source, epg_id } = o else {
            return (Vec::new(), false);
        };
        let make = |source| EpgRef {
            source,
            epg_id: epg_id.clone(),
            kind: MatchKind::Override,
        };
        match source {
            // Kept even when unknown: that source may not be loaded yet.
            Some(s) => (vec![make(*s)], self.contains(*s, epg_id)),
            None => {
                let cands: Vec<_> = self
                    .sources
                    .iter()
                    .filter(|s| s.exact.contains_key(epg_id.as_str()))
                    .map(|s| make(s.source))
                    .collect();
                let resolved = !cands.is_empty();
                (cands, resolved)
            }
        }
    }
}

/// Which EPG channels some playlist channel could match, decided per
/// channel as a feed is read (before its channel list is complete): a
/// superset of the candidates [`MatchIndex::match_channels`] returns for
/// the same playlist and overrides. A streaming import keeps only the
/// programs of these channels.
///
/// A channel is wanted when its id equals a playlist tvg-id (ignoring
/// case), one of its names (or the name without a `UK:`-style tag) equals
/// a playlist channel's name key, or an override names its id. Playlist
/// channels with an override take part only through it.
#[derive(Clone, Debug, Default)]
pub struct MatchFilter {
    /// Trimmed, lower-cased tvg-ids.
    ids: HashSet<String>,
    names: HashSet<String>,
    /// Override targets: EPG id -> the sources named (`None`: any).
    overrides: HashMap<String, Vec<Option<FeedId>>>,
}

impl MatchFilter {
    pub fn new<'a>(
        playlist: impl IntoIterator<Item = PlaylistChannel<'a>>,
        overrides: &Overrides,
    ) -> Self {
        let mut f = MatchFilter::default();
        for ch in playlist {
            if let Some(o) = overrides.get(ch.key) {
                if let Override::Epg { source, epg_id } = o {
                    f.overrides.entry(epg_id.clone()).or_default().push(*source);
                }
                continue;
            }
            if let Some(id) = ch.tvg_id.map(str::trim).filter(|t| !t.is_empty()) {
                f.ids.insert(id.to_lowercase());
            }
            f.names.extend(Keys::new(ch).names);
        }
        f
    }

    /// Whether some playlist channel could match `channel` of `source`
    /// (as first declared: [`MatchIndex`] ignores later declarations of an
    /// id).
    pub fn wants_channel(&self, source: FeedId, channel: &XmltvChannel) -> bool {
        let id = channel.id.trim();
        if id.is_empty() {
            return false;
        }
        let name = |n: &str| {
            let k = normalize_name(n);
            !k.is_empty() && self.names.contains(&k)
        };
        self.wants_id(source, id)
            || self.ids.contains(&id.to_lowercase())
            || channel
                .display_names
                .iter()
                .any(|n| name(n) || strip_tag_prefix(n).is_some_and(name))
    }

    /// Whether an override names EPG channel `epg_id` of `source` (an
    /// override with a source binds to its programs even when the feed
    /// does not declare the channel).
    pub fn wants_id(&self, source: FeedId, epg_id: &str) -> bool {
        !self.overrides.is_empty()
            && self
                .overrides
                .get(epg_id)
                .is_some_and(|s| s.iter().any(|s| s.is_none_or(|s| s == source)))
    }
}

fn insert_name(names: &mut HashMap<String, NameEntry>, key: String, id: u32) {
    if key.is_empty() {
        return;
    }
    names
        .entry(key)
        .and_modify(|e| e.ambiguous |= e.id != id)
        .or_insert(NameEntry {
            id,
            ambiguous: false,
        });
}
