//! Times the programme index on a synthetic guide: building it and running
//! 100 as-you-type queries.
//!
//! ```sh
//! cargo run --release -p iptv-search --example program_bench [channels] [days]
//! ```
//!
//! Defaults: 2,000 channels, 7 days of programmes (4 back, 3 ahead), titles
//! made from invented syllables (recurring series plus one-off titles).

use std::collections::HashMap;
use std::time::Instant;

use iptv_epg::{
    ChannelKey, DAY, EpgRef, FeedId, GuideStore, MatchKind, SourceBuilder, StoreOptions,
    XmltvProgram,
};
use iptv_search::{ProgramIndex, ProgramSearchOptions};

const NOW: i64 = 1_900_000_000;
const SYL: [&str; 24] = [
    "ka", "lo", "mi", "ren", "tu", "vash", "ol", "pe", "zan", "qui", "bro", "dal", "fen", "gor",
    "hil", "jus", "mar", "nev", "pra", "sol", "tem", "ury", "wex", "yon",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn word(rng: &mut Rng) -> String {
    let n = 2 + rng.below(3);
    let mut w: String = (0..n).map(|_| SYL[rng.below(24) as usize]).collect();
    if let Some(c) = w.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    w
}

fn title(rng: &mut Rng, words: &[String]) -> String {
    let n = 1 + rng.below(4) as usize;
    (0..n)
        .map(|_| words[rng.below(words.len() as u64) as usize].as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let channels: usize = args.next().map_or(2000, |a| a.parse().expect("channels"));
    let days: i64 = args.next().map_or(7, |a| a.parse().expect("days"));
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let words: Vec<String> = (0..20000).map(|_| word(&mut rng)).collect();
    let series: Vec<String> = (0..6000).map(|_| title(&mut rng, &words)).collect();

    let t = Instant::now();
    let mut b = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), NOW);
    let (from, to) = (NOW - (days - 3).max(0) * DAY, NOW + 3.min(days) * DAY);
    for ch in 0..channels {
        let mut start = from - rng.below(3600) as i64;
        while start < to {
            let len = [1800, 1800, 3600, 3600, 5400, 7200][rng.below(6) as usize];
            let title = if rng.below(10) < 7 {
                series[rng.below(series.len() as u64) as usize].clone()
            } else {
                title(&mut rng, &words)
            };
            b.push_program(XmltvProgram {
                channel: format!("c{ch}.example"),
                start,
                stop: Some(start + len),
                title,
                ..Default::default()
            });
            start += len;
        }
    }
    let keys: Vec<ChannelKey> = (0..channels)
        .map(|c| ChannelKey::new(format!("k{c}")))
        .collect();
    let mut store = GuideStore::new();
    store.replace_source(b.finish());
    let matches: HashMap<ChannelKey, Vec<EpgRef>> = keys
        .iter()
        .enumerate()
        .map(|(c, k)| {
            let r = EpgRef {
                source: FeedId(1),
                epg_id: format!("c{c}.example"),
                kind: MatchKind::TvgId,
            };
            (k.clone(), vec![r])
        })
        .collect();
    store.set_matches(matches);
    let store_time = t.elapsed();

    let t = Instant::now();
    let idx = ProgramIndex::build(&store, &keys);
    let build = t.elapsed();

    // As-you-type prefixes of real words, two-word queries, infixes, misses.
    let mut queries: Vec<String> = Vec::new();
    for i in 0..100 {
        let w = &words[rng.below(words.len() as u64) as usize].to_lowercase();
        let q = match i % 5 {
            0 => w[..1].to_owned(),
            1 => w[..3.min(w.len())].to_owned(),
            2 => format!("{w} {}", &words[i].to_lowercase()[..2]),
            3 => w[1..].to_owned(),
            _ => format!("{w}zz"),
        };
        queries.push(q);
    }
    let catchup = |c: usize| if c.is_multiple_of(2) { 3 } else { 0 };
    let opts = ProgramSearchOptions {
        catchup_days: Some(&catchup),
        ..Default::default()
    };
    let t = Instant::now();
    let mut hits = 0;
    let mut worst = (std::time::Duration::ZERO, String::new());
    for q in &queries {
        let tq = Instant::now();
        hits += idx.search_programs(q, NOW, opts).len();
        let e = tq.elapsed();
        if e > worst.0 {
            worst = (e, q.clone());
        }
    }
    let search = t.elapsed();

    println!("guide: {channels} channels x {days} days");
    println!(
        "index: {} programmes, {} distinct titles, {} words, {:.1} MiB heap (store {:.1} MiB)",
        idx.program_count(),
        idx.title_count(),
        idx.word_count(),
        idx.heap_bytes() as f64 / 1048576.0,
        store.heap_bytes() as f64 / 1048576.0,
    );
    println!("store ingest: {store_time:.2?}");
    println!("index build:  {build:.2?}");
    println!(
        "100 queries:  {search:.2?} total, {:.2?} mean, worst {:.2?} ({:?}), {hits} hits",
        search / 100,
        worst.0,
        worst.1
    );
}
