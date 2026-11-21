//! Synthetic XMLTV guides shaped like `tools/make-iptv-fixtures.py`: the
//! same word lists and program lengths, generated in chunks and fed to
//! the parser the way `iptv_import::import_epg` does it. Everything is
//! invented; channel ids use the reserved `.example` domain.
#![allow(dead_code)]

use std::fmt::Write as _;

use iptv_epg::{
    ChannelKey, DAY, FeedId, GuideStore, MatchIndex, Overrides, ParseOptions, PlaylistChannel,
    SourceBuilder, StoreOptions, UnixTime, XmltvParser, format_xmltv_time,
};

const ADJECTIVES: [&str; 20] = [
    "Quiet", "Bright", "Hidden", "Northern", "Golden", "Late", "Early", "Silver", "Wild", "Little",
    "Grand", "Distant", "Open", "Secret", "Rapid", "Gentle", "Midnight", "Summer", "Winter",
    "Coastal",
];
const NOUNS: [&str; 20] = [
    "Harbour", "Orbit", "Kitchen", "Valley", "Workshop", "Signal", "Garden", "Circuit", "Lantern",
    "Meadow", "Atlas", "Engine", "Compass", "Archive", "Studio", "Horizon", "Bridge", "Market",
    "Canyon", "Library",
];
const VERBS: [&str; 8] = [
    "explores", "follows", "revisits", "builds", "tests", "maps", "repairs", "visits",
];
const THINGS: [&str; 10] = [
    "an old lighthouse",
    "a mountain railway",
    "three invented recipes",
    "a tiny robot",
    "the night sky",
    "a forgotten garden",
    "a model city",
    "a river delta",
    "a vintage radio",
    "a desert festival",
];
const LENGTHS_MIN: [i64; 10] = [15, 20, 30, 30, 45, 60, 60, 60, 90, 120];

/// Hour-aligned "now" of the synthetic guides.
pub const NOW: UnixTime = 1_800_000_000;

/// The shape of a synthetic guide.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub channels: usize,
    pub days_back: i64,
    pub days_ahead: i64,
}

impl Shape {
    /// `make-iptv-fixtures.py --channels 1000 --days-back 2 --days-ahead 1`:
    /// about 83 programs per channel, 83k in all.
    pub const THOUSAND: Shape = Shape {
        channels: 1000,
        days_back: 2,
        days_ahead: 1,
    };
    /// The script's defaults: 5000 channels, now-7d to now+2d (1.2M).
    pub const DEFAULT_FIXTURE: Shape = Shape {
        channels: 5000,
        days_back: 7,
        days_ahead: 2,
    };
}

pub fn epg_channel(n: usize) -> String {
    format!("ch{n}.synthetic.example")
}

/// The playlist key of channel `n` (any stable string works).
pub fn key(n: usize) -> ChannelKey {
    ChannelKey(format!("m3u:{n:08x}"))
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Writes the guide piecewise into `emit` (64 KiB chunks). Returns the
/// program count.
pub fn generate(shape: Shape, mut emit: impl FnMut(&[u8])) -> u64 {
    let mut buf = String::with_capacity(80 * 1024);
    let mut flush = |buf: &mut String, force: bool| {
        if force || buf.len() >= 64 * 1024 {
            emit(buf.as_bytes());
            buf.clear();
        }
    };
    buf.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");
    for n in 0..shape.channels {
        let _ = writeln!(
            buf,
            "<channel id=\"{}\"><display-name>Synth {n:04}</display-name></channel>",
            epg_channel(n)
        );
        flush(&mut buf, false);
    }
    let start = (NOW - shape.days_back * DAY) / 3600 * 3600;
    let end = NOW + shape.days_ahead * DAY;
    let mut programs = 0;
    for n in 0..shape.channels {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (n as u64 + 1).wrapping_mul(1_000_003));
        let mut t = start - LENGTHS_MIN[rng.below(10)] * 60 / 2;
        let mut episode = 1 + rng.below(39);
        while t < end {
            let len = LENGTHS_MIN[rng.below(10)] * 60;
            let title = format!("{} {}", ADJECTIVES[rng.below(20)], NOUNS[rng.below(20)]);
            let _ = writeln!(
                buf,
                "<programme start=\"{}\" stop=\"{}\" channel=\"{}\"><title>{title}</title>\
                 <desc>{title} {} {}. Episode {episode}.</desc>\
                 <episode-num system=\"onscreen\">E{episode}</episode-num></programme>",
                format_xmltv_time(t),
                format_xmltv_time(t + len),
                epg_channel(n),
                VERBS[rng.below(8)],
                THINGS[rng.below(10)],
            );
            t += len;
            episode += 1;
            programs += 1;
            flush(&mut buf, false);
        }
    }
    buf.push_str("</tv>\n");
    flush(&mut buf, true);
    programs
}

/// What `import_epg` does for one source: chunked parse into a
/// [`SourceBuilder`], then tvg-id matching of the playlist channels.
pub fn import(shape: Shape, store: StoreOptions) -> (GuideStore, u64) {
    let mut parser = XmltvParser::new(ParseOptions {
        descriptions: store.store_descriptions,
        ..ParseOptions::default()
    });
    let mut builder = SourceBuilder::new(FeedId(1), 0, store, NOW);
    let programs = generate(shape, |chunk| {
        parser.feed(chunk, &mut |item| builder.push(item)).unwrap();
    });
    parser.finish(&mut |item| builder.push(item)).unwrap();
    drop(parser);
    let src = builder.finish();
    let mut index = MatchIndex::new();
    index.add_source(src.id(), src.priority(), src.channels());
    let mut guide = GuideStore::new();
    guide.replace_source(src);
    let keys: Vec<ChannelKey> = (0..shape.channels).map(key).collect();
    let ids: Vec<String> = (0..shape.channels).map(epg_channel).collect();
    let names: Vec<String> = (0..shape.channels)
        .map(|n| format!("Synth {n:04}"))
        .collect();
    let matched = index.match_channels(
        (0..shape.channels).map(|n| PlaylistChannel {
            key: &keys[n],
            tvg_id: Some(&ids[n]),
            name: &names[n],
            tvg_name: None,
        }),
        &Overrides::new(),
    );
    guide.set_matches(matched.matches);
    (guide, programs)
}
