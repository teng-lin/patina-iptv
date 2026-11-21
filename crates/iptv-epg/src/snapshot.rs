//! A bound guide as bytes, for hosts without the SQLite store (the web
//! host keeps it in IndexedDB). Like the SQLite store, only the EPG channels
//! some playlist channel is bound to are kept, with their bindings, so a
//! restored [`GuideStore`] answers exactly as the one saved did.
//!
//! The format is persisted: a reader rejects any other [`VERSION`]. All
//! integers are little-endian; a string is a `u32` byte length and UTF-8.
//!
//! ```text
//! magic "PGS" version:u8
//! sources:u32 { feed:u32 priority:i32 channels:u32
//!     { epg_id:str programs:u32
//!         { start:i64 stop:i64 title:str flags:u8 [sub_title desc category
//!           icon rating catchup_id: str, present per flag bit 0..5]
//!           [season:u16 if bit 6] [episode:u16 if bit 7] } } }
//! bindings:u32 { key:str feed:u32 epg_id:str kind:u8 }
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use crate::matching::{ChannelKey, EpgRef, FeedId, MatchKind};
use crate::store::{GuideStore, ProgramData, SourceBuilder, StoreOptions};
use crate::time::UnixTime;
use crate::xmltv::XmltvChannel;

const MAGIC: &[u8; 3] = b"PGS";
/// The snapshot format version.
pub const VERSION: u8 = 1;

/// Why bytes are not a snapshot this version reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// Not a snapshot, or one of another format version.
    Version,
    /// Cut short or malformed at this byte offset.
    Corrupt(usize),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version => f.write_str("not a guide snapshot of this version"),
            Self::Corrupt(at) => write!(f, "guide snapshot corrupt at byte {at}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

fn kind_byte(kind: MatchKind) -> u8 {
    match kind {
        MatchKind::Override => 0,
        MatchKind::TvgId => 1,
        MatchKind::Name => 2,
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("snapshot sections hold fewer than 2^32 items"));
    }
    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len());
        let end = end.ok_or(SnapshotError::Corrupt(self.at))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], SnapshotError> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }
    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, SnapshotError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn i32(&mut self) -> Result<i32, SnapshotError> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    fn i64(&mut self) -> Result<i64, SnapshotError> {
        Ok(i64::from_le_bytes(self.array()?))
    }
    /// A count, bounded by the bytes left so a corrupt count cannot make
    /// the reader allocate or loop beyond the input.
    fn count(&mut self, min_item_bytes: usize) -> Result<usize, SnapshotError> {
        let at = self.at;
        let n = self.u32()? as usize;
        if n.saturating_mul(min_item_bytes) > self.bytes.len() - self.at {
            return Err(SnapshotError::Corrupt(at));
        }
        Ok(n)
    }
    fn str(&mut self) -> Result<&'a str, SnapshotError> {
        let at = self.at;
        let n = self.u32()? as usize;
        std::str::from_utf8(self.take(n)?).map_err(|_| SnapshotError::Corrupt(at))
    }
}

impl GuideStore {
    /// The bound part of the guide as bytes ([`GuideStore::from_snapshot`]).
    /// Deterministic: the same guide gives the same bytes.
    pub fn to_snapshot(&self) -> Vec<u8> {
        let mut bindings: Vec<(&ChannelKey, &EpgRef)> = self.bindings().collect();
        bindings.sort_by(|a, b| a.0.cmp(b.0));
        let mut wanted: BTreeMap<FeedId, BTreeSet<&str>> = BTreeMap::new();
        for (_, r) in &bindings {
            wanted
                .entry(r.source)
                .or_default()
                .insert(r.epg_id.as_str());
        }
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(MAGIC);
        w.u8(VERSION);
        let sources: Vec<_> = self
            .sources()
            .iter()
            .filter(|s| wanted.contains_key(&s.id()))
            .collect();
        w.len(sources.len());
        for src in sources {
            w.u32(src.id().0);
            w.i32(src.priority());
            let ids: Vec<&str> = wanted[&src.id()]
                .iter()
                .copied()
                .filter(|id| src.schedule(id).is_some())
                .collect();
            w.len(ids.len());
            for id in ids {
                let schedule = src.schedule(id).expect("filtered");
                w.str(id);
                w.len(schedule.len());
                for i in 0..schedule.len() {
                    let p = schedule.get(i).expect("in range").data();
                    write_program(&mut w, &p);
                }
            }
        }
        w.len(bindings.len());
        for (key, r) in bindings {
            w.str(key.as_str());
            w.u32(r.source.0);
            w.str(&r.epg_id);
            w.u8(kind_byte(r.kind));
        }
        w.0
    }

    /// The guide [`GuideStore::to_snapshot`] saved. `descriptions` keeps
    /// program descriptions; `now` is only used for retention, which
    /// `past_days_to_keep` applies as on import.
    pub fn from_snapshot(
        bytes: &[u8],
        store: StoreOptions,
        now: UnixTime,
    ) -> Result<GuideStore, SnapshotError> {
        let mut r = Reader { bytes, at: 0 };
        if bytes.len() < 4 || &bytes[..3] != MAGIC || bytes[3] != VERSION {
            return Err(SnapshotError::Version);
        }
        r.at = 4;
        let mut guide = GuideStore::new();
        for _ in 0..r.count(12)? {
            let feed = FeedId(r.u32()?);
            let priority = r.i32()?;
            let mut builder = SourceBuilder::new(feed, priority, store, now);
            for _ in 0..r.count(8)? {
                let id = r.str()?;
                builder.push_channel(XmltvChannel {
                    id: id.to_owned(),
                    display_names: Vec::new(),
                    icon: None,
                });
                for _ in 0..r.count(21)? {
                    let p = read_program(&mut r)?;
                    builder.push_data(id, &p);
                }
            }
            guide.replace_source(builder.finish());
        }
        let mut matches: HashMap<ChannelKey, Vec<EpgRef>> = HashMap::new();
        for _ in 0..r.count(13)? {
            let key = ChannelKey::new(r.str()?);
            let source = FeedId(r.u32()?);
            let epg_id = r.str()?.to_owned();
            let at = r.at;
            let kind = match r.u8()? {
                0 => MatchKind::Override,
                1 => MatchKind::TvgId,
                2 => MatchKind::Name,
                _ => return Err(SnapshotError::Corrupt(at)),
            };
            matches.insert(
                key,
                vec![EpgRef {
                    source,
                    epg_id,
                    kind,
                }],
            );
        }
        if r.at != bytes.len() {
            return Err(SnapshotError::Corrupt(r.at));
        }
        guide.set_matches(matches);
        Ok(guide)
    }
}

const TEXTS: usize = 6;
const SEASON: u8 = 1 << 6;
const EPISODE: u8 = 1 << 7;

fn texts<'a>(p: &ProgramData<'a>) -> [Option<&'a str>; TEXTS] {
    [
        p.sub_title,
        p.desc,
        p.category,
        p.icon,
        p.rating,
        p.catchup_id,
    ]
}

fn write_program(w: &mut Writer, p: &ProgramData<'_>) {
    w.i64(p.start);
    w.i64(p.stop.unwrap_or(p.start));
    w.str(p.title);
    let texts = texts(p);
    let mut flags = 0u8;
    for (bit, t) in texts.iter().enumerate() {
        if t.is_some() {
            flags |= 1 << bit;
        }
    }
    if p.season.is_some() {
        flags |= SEASON;
    }
    if p.episode.is_some() {
        flags |= EPISODE;
    }
    w.u8(flags);
    for t in texts.into_iter().flatten() {
        w.str(t);
    }
    if let Some(s) = p.season {
        w.u16(s);
    }
    if let Some(e) = p.episode {
        w.u16(e);
    }
}

fn read_program<'a>(r: &mut Reader<'a>) -> Result<ProgramData<'a>, SnapshotError> {
    let start = r.i64()?;
    let stop = r.i64()?;
    let title = r.str()?;
    let flags = r.u8()?;
    let mut texts: [Option<&'a str>; TEXTS] = [None; TEXTS];
    for (bit, t) in texts.iter_mut().enumerate() {
        if flags & (1 << bit) != 0 {
            *t = Some(r.str()?);
        }
    }
    let season = if flags & SEASON != 0 {
        Some(r.u16()?)
    } else {
        None
    };
    let episode = if flags & EPISODE != 0 {
        Some(r.u16()?)
    } else {
        None
    };
    let [sub_title, desc, category, icon, rating, catchup_id] = texts;
    Ok(ProgramData {
        start,
        stop: Some(stop),
        title,
        sub_title,
        desc,
        category,
        icon,
        rating,
        catchup_id,
        season,
        episode,
    })
}
