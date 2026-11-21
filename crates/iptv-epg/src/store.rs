//! Compact programme storage for the guide.
//!
//! Each EPG source becomes an [`EpgSource`]: per EPG channel a sorted,
//! non-overlapping array of 16-byte slots plus an interned string arena.
//! A [`GuideStore`] holds several sources and binds playlist channels to
//! the best one (highest priority with data). Every query is a hash lookup
//! plus a binary search: O(log n) in the channel's programme count.

use std::collections::HashMap;

use crate::extra::{self, Extra, Field, NO_EXTRA};
use crate::intern::{Interner, NONE, Sym};
use crate::matching::{ChannelKey, EpgRef, FeedId};
use crate::settle::{ScheduleRules, Settled};
use crate::time::{DAY, UnixTime};
use crate::xmltv::{XmltvChannel, XmltvItem, XmltvProgram};

/// Storage settings (the reference UX's "store descriptions" and "past days
/// to keep").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreOptions {
    pub store_descriptions: bool,
    /// Programmes that ended more than this many days before `now` are
    /// dropped. `None` keeps everything.
    pub past_days_to_keep: Option<u32>,
}

impl Default for StoreOptions {
    fn default() -> Self {
        StoreOptions {
            store_descriptions: true,
            past_days_to_keep: None,
        }
    }
}

/// What happened to the programmes of one source during ingest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub programs_in: u64,
    pub kept: u64,
    /// EPG channels with at least one programme.
    pub channels_with_programs: u64,
    /// `stop <= start`.
    pub dropped_bad_duration: u64,
    /// No `stop` and no following programme to end it.
    pub dropped_no_stop: u64,
    /// Older than `past_days_to_keep`.
    pub dropped_retention: u64,
    /// Same channel and start as an earlier (longer or equal) programme.
    pub dropped_duplicate: u64,
    /// Programmes shortened to end where the next one starts.
    pub overlaps_trimmed: u64,
}

const NO_STOP: i64 = i64::MIN;

/// One programme in a schedule: 16 bytes, times relative to the schedule
/// base (seconds; a schedule spans at most ~136 years).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot {
    pub(crate) start: u32,
    pub(crate) dur: u32,
    pub(crate) title: Sym,
    /// Offset of the packed extras ([`extra`]), or [`NO_EXTRA`].
    pub(crate) extra: u32,
}

/// One programme's fields as plain values: what [`SourceBuilder::push_data`]
/// takes and [`ProgramRef::data`] returns, so a stored guide round-trips
/// without building XMLTV records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgramData<'a> {
    pub start: UnixTime,
    /// `None`: ends where the next programme starts.
    pub stop: Option<UnixTime>,
    pub title: &'a str,
    pub sub_title: Option<&'a str>,
    pub desc: Option<&'a str>,
    /// First category.
    pub category: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub rating: Option<&'a str>,
    pub catchup_id: Option<&'a str>,
    /// One-based.
    pub season: Option<u16>,
    pub episode: Option<u16>,
}

impl<'a> ProgramData<'a> {
    /// The fields of a parsed programme (first category, rating value,
    /// season and episode from the first readable `<episode-num>`).
    pub fn from_xmltv(p: &'a XmltvProgram) -> Self {
        let (season, episode) = p
            .episode_num
            .iter()
            .find_map(|e| e.season_episode())
            .unwrap_or_default();
        let narrow = |n: Option<u32>| n.map(|n| n.min(u32::from(u16::MAX)) as u16);
        ProgramData {
            start: p.start,
            stop: p.stop,
            title: &p.title,
            sub_title: p.sub_title.as_deref(),
            desc: p.desc.as_deref(),
            category: p.categories.first().map(String::as_str),
            icon: p.icon.as_deref(),
            rating: p.rating.as_ref().map(|r| r.value.as_str()),
            catchup_id: p.catchup_id.as_deref(),
            season: narrow(season),
            episode: narrow(episode),
        }
    }

    /// The fields as a guide built from them gives them back
    /// ([`ProgramRef::data`]): empty texts and zero season or episode
    /// are absent, and the description only when `descriptions`.
    pub fn as_stored(self, descriptions: bool) -> Self {
        let text = |t: Option<&'a str>| t.filter(|t| !t.is_empty());
        let num = |n: Option<u16>| n.filter(|&n| n != 0);
        ProgramData {
            sub_title: text(self.sub_title),
            desc: text(self.desc).filter(|_| descriptions),
            category: text(self.category),
            icon: text(self.icon),
            rating: text(self.rating),
            catchup_id: text(self.catchup_id),
            season: num(self.season),
            episode: num(self.episode),
            ..self
        }
    }
}

#[derive(Default)]
pub(crate) struct Schedule {
    pub(crate) base: i64,
    pub(crate) slots: Vec<Slot>,
}

impl Schedule {
    fn start(&self, s: &Slot) -> i64 {
        self.base + i64::from(s.start)
    }
    fn stop(&self, s: &Slot) -> i64 {
        self.base + i64::from(s.start) + i64::from(s.dur)
    }
}

/// The ingested guide data of one EPG source.
pub struct EpgSource {
    id: FeedId,
    priority: i32,
    strings: Interner,
    /// Packed extras ([`extra`]).
    extras: Vec<u32>,
    schedules: Vec<Schedule>,
    by_epg_id: HashMap<Box<str>, u32>,
    channels: Vec<XmltvChannel>,
    report: IngestReport,
}

impl EpgSource {
    pub fn id(&self) -> FeedId {
        self.id
    }

    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// The source's `<channel>` records, for [`crate::MatchIndex::add_source`].
    pub fn channels(&self) -> &[XmltvChannel] {
        &self.channels
    }

    pub fn report(&self) -> IngestReport {
        self.report
    }

    /// Schedule of one EPG channel id of this source.
    pub fn schedule(&self, epg_id: &str) -> Option<ScheduleRef<'_>> {
        let i = *self.by_epg_id.get(epg_id)?;
        Some(ScheduleRef {
            src: self,
            sched: &self.schedules[i as usize],
        })
    }

    /// Number of programmes held.
    pub fn program_count(&self) -> usize {
        self.schedules.iter().map(|s| s.slots.len()).sum()
    }

    /// Distinct strings held (titles, descriptions, ...).
    pub fn string_count(&self) -> usize {
        self.strings.len()
    }

    /// Rough heap usage in bytes.
    pub fn heap_bytes(&self) -> usize {
        self.strings.heap_bytes()
            + self.extras.capacity() * size_of::<u32>()
            + self
                .schedules
                .iter()
                .map(|s| s.slots.capacity() * size_of::<Slot>())
                .sum::<usize>()
            + self.schedules.capacity() * size_of::<Schedule>()
            + self.by_epg_id.keys().map(|k| k.len() + 24).sum::<usize>()
    }

    /// A source from schedules built elsewhere (`crate::settled_source`).
    pub(crate) fn from_parts(
        id: FeedId,
        priority: i32,
        mut strings: Interner,
        mut extras: Vec<u32>,
        schedules: Vec<Schedule>,
        by_epg_id: HashMap<Box<str>, u32>,
        report: IngestReport,
    ) -> Self {
        strings.freeze();
        extras.shrink_to_fit();
        EpgSource {
            id,
            priority,
            strings,
            extras,
            schedules,
            by_epg_id,
            channels: Vec::new(),
            report,
        }
    }

    fn prune(&mut self, cutoff: i64) {
        for s in &mut self.schedules {
            let cut = s
                .slots
                .partition_point(|x| s.base + i64::from(x.start) + i64::from(x.dur) <= cutoff);
            s.slots.drain(..cut);
        }
    }
}

/// Interns `p`'s title and packs its extras (the description only with
/// `descriptions`): its slot's `title` and `extra`.
pub(crate) fn intern_program(
    strings: &mut Interner,
    extras: &mut Vec<u32>,
    p: &ProgramData<'_>,
    descriptions: bool,
) -> (Sym, u32) {
    let mut e = Extra {
        season: p.season.unwrap_or(0),
        episode: p.episode.unwrap_or(0),
        ..Extra::default()
    };
    let desc = p.desc.filter(|_| descriptions);
    for (field, value) in [
        (Field::SubTitle, p.sub_title),
        (Field::Desc, desc),
        (Field::Category, p.category),
        (Field::Icon, p.icon),
        (Field::Rating, p.rating),
        (Field::CatchupId, p.catchup_id),
    ] {
        e.set(field, value.map_or(NONE, |v| strings.intern(v)));
    }
    let extra = extra::push(extras, &e);
    (strings.intern(p.title), extra)
}

struct RawProgram {
    start: i64,
    stop: i64,
    title: Sym,
    extra: u32,
}

/// Builds an [`EpgSource`] from parser output, one record at a time.
///
/// ```
/// use iptv_epg::*;
/// let mut b = SourceBuilder::new(FeedId(1), 10, StoreOptions::default(), 0);
/// b.push_program(XmltvProgram {
///     channel: "one.example".into(),
///     start: 1000,
///     stop: Some(2000),
///     title: "Show".into(),
///     ..Default::default()
/// });
/// let src = b.finish();
/// let p = src.schedule("one.example").unwrap().at(1500).unwrap();
/// assert_eq!((p.title(), p.start(), p.stop()), ("Show", 1000, 2000));
/// ```
pub struct SourceBuilder {
    id: FeedId,
    priority: i32,
    opts: StoreOptions,
    rules: ScheduleRules,
    strings: Interner,
    extras: Vec<u32>,
    by_epg_id: HashMap<Box<str>, u32>,
    raw: Vec<Vec<RawProgram>>,
    channels: Vec<XmltvChannel>,
    report: IngestReport,
}

impl SourceBuilder {
    /// `now` is only used for retention.
    pub fn new(id: FeedId, priority: i32, opts: StoreOptions, now: UnixTime) -> Self {
        SourceBuilder {
            id,
            priority,
            rules: ScheduleRules::new(opts, now),
            opts,
            strings: Interner::new(),
            extras: Vec::new(),
            by_epg_id: HashMap::new(),
            raw: Vec::new(),
            channels: Vec::new(),
            report: IngestReport::default(),
        }
    }

    pub fn push(&mut self, item: XmltvItem) {
        match item {
            XmltvItem::Channel(c) => self.push_channel(c),
            XmltvItem::Program(p) => self.push_program(p),
        }
    }

    pub fn push_channel(&mut self, c: XmltvChannel) {
        self.channels.push(c);
    }

    pub fn push_program(&mut self, p: XmltvProgram) {
        self.push_data(&p.channel, &ProgramData::from_xmltv(&p));
    }

    /// Adds one programme of EPG channel `channel` from plain values (a
    /// stored guide). Same rules as [`Self::push_program`].
    pub fn push_data(&mut self, channel: &str, p: &ProgramData<'_>) {
        if !self.rules.admit(p.start, p.stop, &mut self.report) {
            return;
        }
        let stop = p.stop.unwrap_or(NO_STOP);
        let (title, extra) = intern_program(
            &mut self.strings,
            &mut self.extras,
            p,
            self.opts.store_descriptions,
        );
        let next = self.raw.len() as u32;
        let ch = match self.by_epg_id.get(channel) {
            Some(&ch) => ch,
            None => {
                self.by_epg_id.insert(channel.into(), next);
                self.raw.push(Vec::new());
                next
            }
        };
        self.raw[ch as usize].push(RawProgram {
            start: p.start,
            stop,
            title,
            extra,
        });
    }

    /// Sorts, resolves overlaps and freezes the source.
    ///
    /// Per channel, deterministically ([`crate::ScheduleRules`]):
    /// programmes are ordered by start; of several with the same start the
    /// longest (then the first in input) is kept; a missing stop becomes
    /// the next programme's start; a programme running into the next one
    /// is cut at the next start.
    pub fn finish(mut self) -> EpgSource {
        let mut schedules = Vec::with_capacity(self.raw.len());
        let r = &mut self.report;
        let mut rows = Vec::new();
        for mut raw in std::mem::take(&mut self.raw) {
            // Stable: equal (start, stop) keep input order. NO_STOP sorts last.
            raw.sort_by(|a, b| a.start.cmp(&b.start).then(b.stop.cmp(&a.stop)));
            let before = raw.len();
            raw.dedup_by(|cur, prev| cur.start == prev.start);
            r.dropped_duplicate += (before - raw.len()) as u64;
            rows.clear();
            rows.extend(
                raw.iter()
                    .map(|p| (p.start, (p.stop != NO_STOP).then_some(p.stop))),
            );
            let base = raw.first().map_or(0, |p| p.start);
            let mut slots = Vec::with_capacity(raw.len());
            self.rules.settle(&rows, r, |i, fate| {
                if let Settled::Keep { stop } = fate {
                    let p = &raw[i];
                    // `settle` checked that both fit.
                    slots.push(Slot {
                        start: (i128::from(p.start) - i128::from(base)) as u32,
                        dur: (i128::from(stop) - i128::from(p.start)) as u32,
                        title: p.title,
                        extra: p.extra,
                    });
                }
            });
            // Re-base so the first kept slot starts at 0.
            let shift = slots.first().map_or(0, |s| s.start);
            for s in &mut slots {
                s.start -= shift;
            }
            schedules.push(Schedule {
                base: base + i64::from(shift),
                slots,
            });
        }
        self.strings.freeze();
        self.extras.shrink_to_fit();
        EpgSource {
            id: self.id,
            priority: self.priority,
            strings: self.strings,
            extras: self.extras,
            schedules,
            by_epg_id: self.by_epg_id,
            channels: self.channels,
            report: self.report,
        }
    }
}

/// A borrowed view of one programme.
#[derive(Clone, Copy)]
pub struct ProgramRef<'a> {
    src: &'a EpgSource,
    sched: &'a Schedule,
    slot: &'a Slot,
}

impl<'a> ProgramRef<'a> {
    pub fn start(&self) -> UnixTime {
        self.sched.start(self.slot)
    }

    pub fn stop(&self) -> UnixTime {
        self.sched.stop(self.slot)
    }

    pub fn title(&self) -> &'a str {
        self.src.strings.get(self.slot.title)
    }

    pub fn source(&self) -> FeedId {
        self.src.id
    }

    fn text(&self, field: Field) -> Option<&'a str> {
        let at = self.slot.extra;
        if at == NO_EXTRA {
            return None;
        }
        self.src
            .strings
            .opt(extra::text(&self.src.extras, at, field))
    }

    pub fn sub_title(&self) -> Option<&'a str> {
        self.text(Field::SubTitle)
    }

    /// `None` also when the source was stored without descriptions.
    pub fn desc(&self) -> Option<&'a str> {
        self.text(Field::Desc)
    }

    /// First category.
    pub fn category(&self) -> Option<&'a str> {
        self.text(Field::Category)
    }

    pub fn icon(&self) -> Option<&'a str> {
        self.text(Field::Icon)
    }

    pub fn rating(&self) -> Option<&'a str> {
        self.text(Field::Rating)
    }

    pub fn catchup_id(&self) -> Option<&'a str> {
        self.text(Field::CatchupId)
    }

    /// One-based season and episode, when known.
    pub fn season_episode(&self) -> (Option<u16>, Option<u16>) {
        let at = self.slot.extra;
        if at == NO_EXTRA {
            return (None, None);
        }
        let (s, e) = extra::episode(&self.src.extras, at);
        let nz = |n: u16| (n != 0).then_some(n);
        (nz(s), nz(e))
    }

    /// Every field as plain values (to store the guide).
    pub fn data(&self) -> ProgramData<'a> {
        let (season, episode) = self.season_episode();
        ProgramData {
            start: self.start(),
            stop: Some(self.stop()),
            title: self.title(),
            sub_title: self.sub_title(),
            desc: self.desc(),
            category: self.category(),
            icon: self.icon(),
            rating: self.rating(),
            catchup_id: self.catchup_id(),
            season,
            episode,
        }
    }

    pub fn is_airing(&self, now: UnixTime) -> bool {
        self.start() <= now && now < self.stop()
    }

    /// Fraction elapsed at `now`, clamped to `0.0..=1.0`.
    pub fn progress(&self, now: UnixTime) -> f32 {
        let dur = (self.stop() - self.start()).max(1) as f64;
        ((now - self.start()) as f64 / dur).clamp(0.0, 1.0) as f32
    }
}

impl std::fmt::Debug for ProgramRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program")
            .field("start", &self.start())
            .field("stop", &self.stop())
            .field("title", &self.title())
            .finish()
    }
}

/// A borrowed view of one channel's schedule.
#[derive(Clone, Copy)]
pub struct ScheduleRef<'a> {
    src: &'a EpgSource,
    sched: &'a Schedule,
}

impl<'a> ScheduleRef<'a> {
    pub fn len(&self) -> usize {
        self.sched.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sched.slots.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<ProgramRef<'a>> {
        let slot = self.sched.slots.get(i)?;
        Some(ProgramRef {
            src: self.src,
            sched: self.sched,
            slot,
        })
    }

    pub fn source(&self) -> FeedId {
        self.src.id
    }

    /// Index of the first programme starting after `t`.
    fn first_after(&self, t: UnixTime) -> usize {
        self.sched
            .slots
            .partition_point(|s| self.sched.start(s) <= t)
    }

    /// Index of the programme airing at `t`.
    pub fn index_at(&self, t: UnixTime) -> Option<usize> {
        let i = self.first_after(t).checked_sub(1)?;
        (self.sched.stop(&self.sched.slots[i]) > t).then_some(i)
    }

    /// The programme airing at `t` (`start <= t < stop`).
    pub fn at(&self, t: UnixTime) -> Option<ProgramRef<'a>> {
        self.get(self.index_at(t)?)
    }

    /// The first programme that starts after `t`.
    pub fn next_after(&self, t: UnixTime) -> Option<ProgramRef<'a>> {
        self.get(self.first_after(t))
    }

    /// Index range of programmes overlapping `[t0, t1)`.
    pub fn range_indices(&self, t0: UnixTime, t1: UnixTime) -> std::ops::Range<usize> {
        let slots = &self.sched.slots;
        // Non-overlapping and sorted by start, so stops are sorted too.
        let lo = slots.partition_point(|s| self.sched.stop(s) <= t0);
        let hi = slots.partition_point(|s| self.sched.start(s) < t1);
        lo..hi.max(lo)
    }

    /// Programmes overlapping `[t0, t1)`, in time order.
    pub fn range(
        self,
        t0: UnixTime,
        t1: UnixTime,
    ) -> impl ExactSizeIterator<Item = ProgramRef<'a>> + 'a {
        self.range_indices(t0, t1)
            .map(move |i| self.get(i).expect("in range"))
    }

    pub fn iter(self) -> impl ExactSizeIterator<Item = ProgramRef<'a>> + 'a {
        (0..self.len()).map(move |i| self.get(i).expect("in range"))
    }

    /// Current and next programme at `now`.
    pub fn now_next(&self, now: UnixTime) -> NowNext<'a> {
        let i = self.first_after(now);
        let current = i
            .checked_sub(1)
            .and_then(|j| self.get(j))
            .filter(|p| p.stop() > now);
        NowNext {
            now: current,
            next: self.get(i),
        }
    }
}

/// What is on now and what comes next on one channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct NowNext<'a> {
    pub now: Option<ProgramRef<'a>>,
    pub next: Option<ProgramRef<'a>>,
}

/// Guide data from several sources, bound to playlist channels.
///
/// Binding: for each playlist channel the candidates from
/// [`crate::MatchResult`] are tried in descending source priority, and the
/// first source with a non-empty schedule for that EPG id wins.
#[derive(Default)]
pub struct GuideStore {
    /// Highest priority first; equal priorities keep insertion order.
    sources: Vec<EpgSource>,
    wanted: HashMap<ChannelKey, Vec<EpgRef>>,
    /// Source index, schedule index, candidate index in `wanted`.
    bound: HashMap<ChannelKey, (u32, u32, u32)>,
}

impl GuideStore {
    pub fn new() -> Self {
        GuideStore::default()
    }

    /// Adds a source, replacing one with the same id, and rebinds.
    pub fn replace_source(&mut self, src: EpgSource) {
        self.sources.retain(|s| s.id != src.id);
        let at = self
            .sources
            .iter()
            .position(|s| s.priority < src.priority)
            .unwrap_or(self.sources.len());
        self.sources.insert(at, src);
        self.rebind();
    }

    pub fn remove_source(&mut self, id: FeedId) {
        self.sources.retain(|s| s.id != id);
        self.rebind();
    }

    pub fn sources(&self) -> &[EpgSource] {
        &self.sources
    }

    pub fn source(&self, id: FeedId) -> Option<&EpgSource> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// Sets the playlist → EPG candidates (usually
    /// [`crate::MatchResult::matches`]) and rebinds.
    pub fn set_matches(&mut self, matches: HashMap<ChannelKey, Vec<EpgRef>>) {
        self.wanted = matches;
        self.rebind();
    }

    /// Drops programmes that ended more than `past_days` before `now`.
    pub fn prune(&mut self, now: UnixTime, past_days: u32) {
        let cutoff = now - i64::from(past_days) * DAY;
        for s in &mut self.sources {
            s.prune(cutoff);
        }
        self.rebind();
    }

    fn rebind(&mut self) {
        self.bound.clear();
        let rank: HashMap<FeedId, usize> = self
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        for (key, cands) in &self.wanted {
            let best = cands
                .iter()
                .enumerate()
                .filter_map(|(ci, c)| {
                    let si = *rank.get(&c.source)?;
                    let sched = *self.sources[si].by_epg_id.get(c.epg_id.as_str())?;
                    let empty = self.sources[si].schedules[sched as usize].slots.is_empty();
                    (!empty).then_some((si, sched, ci))
                })
                .min_by_key(|&(si, _, _)| si);
            if let Some((si, sched, ci)) = best {
                self.bound
                    .insert(key.clone(), (si as u32, sched, ci as u32));
            }
        }
    }

    /// Number of playlist channels that have guide data.
    pub fn bound_count(&self) -> usize {
        self.bound.len()
    }

    /// The source and EPG id a playlist channel's guide comes from.
    pub fn binding(&self, key: &ChannelKey) -> Option<(FeedId, &str)> {
        let &(si, _, cand) = self.bound.get(key)?;
        let c = &self.wanted.get(key)?[cand as usize];
        Some((self.sources[si as usize].id, c.epg_id.as_str()))
    }

    /// Every bound playlist channel with the candidate its guide comes
    /// from (source, EPG id, how it matched). Unordered.
    pub fn bindings(&self) -> impl Iterator<Item = (&ChannelKey, &EpgRef)> {
        self.bound.iter().filter_map(|(key, &(_, _, cand))| {
            Some((key, self.wanted.get(key)?.get(cand as usize)?))
        })
    }

    pub fn schedule(&self, key: &ChannelKey) -> Option<ScheduleRef<'_>> {
        let &(si, sched, _) = self.bound.get(key)?;
        let src = &self.sources[si as usize];
        Some(ScheduleRef {
            src,
            sched: &src.schedules[sched as usize],
        })
    }

    /// Programmes of `key` overlapping `[t0, t1)`; empty when unbound.
    pub fn programs(
        &self,
        key: &ChannelKey,
        t0: UnixTime,
        t1: UnixTime,
    ) -> impl Iterator<Item = ProgramRef<'_>> {
        self.schedule(key)
            .into_iter()
            .flat_map(move |s| s.range(t0, t1))
    }

    pub fn program_at(&self, key: &ChannelKey, t: UnixTime) -> Option<ProgramRef<'_>> {
        self.schedule(key)?.at(t)
    }

    pub fn next_after(&self, key: &ChannelKey, t: UnixTime) -> Option<ProgramRef<'_>> {
        self.schedule(key)?.next_after(t)
    }

    /// Now/next for a batch of channels, in input order.
    pub fn now_next<'k>(
        &self,
        keys: impl IntoIterator<Item = &'k ChannelKey>,
        now: UnixTime,
    ) -> Vec<NowNext<'_>> {
        keys.into_iter()
            .map(|k| {
                self.schedule(k)
                    .map(|s| s.now_next(now))
                    .unwrap_or_default()
            })
            .collect()
    }

    /// When the last programme held ends (any source, bound or not).
    pub fn last_stop(&self) -> Option<UnixTime> {
        self.sources
            .iter()
            .flat_map(|src| &src.schedules)
            .filter_map(|s| s.slots.last().map(|slot| s.stop(slot)))
            .max()
    }

    /// When the first programme held starts (any source, bound or not).
    pub fn first_start(&self) -> Option<UnixTime> {
        self.sources
            .iter()
            .flat_map(|src| &src.schedules)
            .filter_map(|s| s.slots.first().map(|slot| s.start(slot)))
            .min()
    }

    /// Rough heap usage of all sources in bytes.
    pub fn heap_bytes(&self) -> usize {
        self.sources.iter().map(EpgSource::heap_bytes).sum()
    }
}
