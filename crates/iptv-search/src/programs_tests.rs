use std::collections::{BTreeSet, HashMap};

use iptv_core::guide::Airing::{self, *};
use iptv_epg::{
    ChannelKey, EpgRef, FeedId, GuideStore, MatchKind, SourceBuilder, StoreOptions, XmltvProgram,
};

use super::*;
use crate::text::MatchQuality::{self, *};

const NOW: i64 = 1_900_000_000;
const MIN: i64 = 60;
const HOUR: i64 = 3600;

/// Channel keys a..d; "c" has no guide data.
fn keys() -> Vec<ChannelKey> {
    ["a", "b", "c", "d"].map(ChannelKey::new).to_vec()
}

fn store(programs: &[(&str, i64, i64, &str)]) -> GuideStore {
    let mut b = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), NOW);
    for &(ch, start, stop, title) in programs {
        b.push_program(XmltvProgram {
            channel: format!("{ch}.example"),
            start,
            stop: Some(stop),
            title: title.into(),
            ..Default::default()
        });
    }
    let mut store = GuideStore::new();
    store.replace_source(b.finish());
    let matches: HashMap<ChannelKey, Vec<EpgRef>> = keys()
        .into_iter()
        .map(|k| {
            let epg_id = format!("{k}.example");
            let r = EpgRef {
                source: FeedId(1),
                epg_id,
                kind: MatchKind::TvgId,
            };
            (k, vec![r])
        })
        .collect();
    store.set_matches(matches);
    store
}

fn guide() -> GuideStore {
    store(&[
        (
            "a",
            NOW - 3 * DAY,
            NOW - 3 * DAY + HOUR,
            "Night Owl Stories",
        ),
        ("a", NOW - 3 * HOUR, NOW - 2 * HOUR, "Night Owl Stories"),
        ("a", NOW - 30 * MIN, NOW + 30 * MIN, "Morning Bulletin"),
        ("a", NOW + 2 * HOUR, NOW + 3 * HOUR, "Night Owl Stories"),
        ("b", NOW - HOUR, NOW, "Night Owl Stories"),
        ("b", NOW, NOW + HOUR, "Écho de la Nuit"),
        ("b", NOW + HOUR, NOW + 2 * HOUR, "Owlish Tales"),
        (
            "b",
            NOW + 4 * DAY,
            NOW + 4 * DAY + HOUR,
            "Night Owl Stories",
        ),
        ("d", NOW - 8 * DAY, NOW - 8 * DAY + HOUR, "Nightowl"),
        ("d", NOW - 2 * DAY, NOW - 2 * DAY + HOUR, "Nightowl"),
        ("d", NOW - 10 * MIN, NOW + 10 * MIN, "Late Owl"),
        ("d", NOW + 20 * MIN, NOW + 50 * MIN, "The Night Owl"),
    ])
}

fn index() -> ProgramIndex {
    ProgramIndex::build(&guide(), &keys())
}

/// Archive days per channel: a 2, d 7.
fn catchup(ch: usize) -> u8 {
    [2, 0, 0, 7][ch]
}

fn opts() -> ProgramSearchOptions<'static> {
    ProgramSearchOptions {
        catchup_days: Some(&catchup),
        ..Default::default()
    }
}

type Row = (usize, i64, MatchQuality, Airing, bool);

fn run(idx: &ProgramIndex, q: &str, o: ProgramSearchOptions<'_>) -> Vec<Row> {
    idx.search_programs(q, NOW, o)
        .into_iter()
        .map(|h| (h.channel, h.start - NOW, h.score, h.airing, h.catchup))
        .collect()
}

#[test]
fn ranks_by_quality_then_nearest_to_now() {
    let idx = index();
    assert_eq!(
        run(&idx, "owl", opts()),
        [
            (1, HOUR, Prefix, Upcoming, false),
            (3, -10 * MIN, Word, Live, true),
            (3, 20 * MIN, Word, Upcoming, false),
            (0, 2 * HOUR, Word, Upcoming, false),
            (0, -3 * HOUR, Word, Ended, true),
            (3, -2 * DAY, Substring, Ended, true),
        ]
    );
    // Across word boundaries: exact, then the title starting with it.
    assert_eq!(
        run(&idx, "nightowl", opts()),
        [
            (3, -2 * DAY, Exact, Ended, true),
            (0, 2 * HOUR, Prefix, Upcoming, false),
            (0, -3 * HOUR, Prefix, Ended, true),
        ]
    );
}

#[test]
fn past_programs_need_catch_up_unless_asked() {
    let idx = index();
    let with_past = ProgramSearchOptions {
        include_past_without_catchup: true,
        ..opts()
    };
    assert_eq!(
        run(&idx, "night owl st", with_past),
        [
            (0, 2 * HOUR, Prefix, Upcoming, false),
            (1, -HOUR, Prefix, Ended, false),
            (0, -3 * HOUR, Prefix, Ended, true),
            // Older than channel a's 2-day archive.
            (0, -3 * DAY, Prefix, Ended, false),
        ]
    );
    assert_eq!(
        run(&idx, "night owl st", opts()),
        [
            (0, 2 * HOUR, Prefix, Upcoming, false),
            (0, -3 * HOUR, Prefix, Ended, true),
        ]
    );
    // Without archive information no past programme is playable.
    let none = ProgramSearchOptions::default();
    assert_eq!(
        run(&idx, "night owl st", none),
        [(0, 2 * HOUR, Prefix, Upcoming, false)]
    );
}

#[test]
fn time_window_and_live_catch_up() {
    let idx = index();
    let wide = ProgramSearchOptions {
        look_ahead: 5 * DAY,
        look_back: 9 * DAY,
        include_past_without_catchup: true,
        ..opts()
    };
    let starts: Vec<i64> = run(&idx, "night owl stories", wide)
        .iter()
        .map(|r| r.1)
        .collect();
    assert_eq!(starts, [2 * HOUR, 4 * DAY, -HOUR, -3 * HOUR, -3 * DAY]);
    let hits = run(&idx, "nightowl", wide);
    assert_eq!(
        hits[..2],
        [
            (3, -2 * DAY, Exact, Ended, true),
            (3, -8 * DAY, Exact, Ended, false)
        ]
    );
    // A live programme on a channel with an archive can be restarted; one
    // without cannot. Diacritics fold.
    assert_eq!(
        run(&idx, "morning", opts()),
        [(0, -30 * MIN, Prefix, Live, true)]
    );
    assert_eq!(run(&idx, "ECHO", opts()), [(1, 0, Prefix, Live, false)]);
    assert_eq!(run(&idx, "nuit", opts()), [(1, 0, Word, Live, false)]);
}

#[test]
fn group_by_time_and_by_channel() {
    let idx = index();
    let by = |group_by| ProgramSearchOptions { group_by, ..opts() };
    let rows =
        |o| -> Vec<(usize, i64)> { run(&idx, "owl", o).iter().map(|r| (r.0, r.1)).collect() };
    assert_eq!(
        rows(by(GroupBy::Time)),
        [
            (3, -10 * MIN),
            (3, 20 * MIN),
            (1, HOUR),
            (0, 2 * HOUR),
            (0, -3 * HOUR),
            (3, -2 * DAY),
        ]
    );
    assert_eq!(
        rows(by(GroupBy::Channel)),
        [
            (1, HOUR),
            (3, -2 * DAY),
            (3, -10 * MIN),
            (3, 20 * MIN),
            (0, -3 * HOUR),
            (0, 2 * HOUR),
        ]
    );
}

#[test]
fn favorites_first_and_limit() {
    let idx = index();
    let fav = BTreeSet::from([0]);
    let o = ProgramSearchOptions {
        favorites: Some(&fav),
        favorites_first: true,
        limit: 3,
        ..opts()
    };
    let rows: Vec<(usize, i64)> = run(&idx, "owl", o).iter().map(|r| (r.0, r.1)).collect();
    assert_eq!(rows, [(0, 2 * HOUR), (0, -3 * HOUR), (1, HOUR)]);
    let one = ProgramSearchOptions { limit: 1, ..opts() };
    assert_eq!(run(&idx, "owl", one).len(), 1);
    let zero = ProgramSearchOptions { limit: 0, ..opts() };
    assert!(run(&idx, "owl", zero).is_empty());
}

#[test]
fn hit_fields_and_index_stats() {
    let idx = index();
    let hits = idx.search_programs("late", NOW, opts());
    assert_eq!(
        hits,
        [ProgramHit {
            identity: crate::ChannelId::new(None, "d"),
            channel: 3,
            start: NOW - 10 * MIN,
            end: NOW + 10 * MIN,
            title: "Late Owl".into(),
            airing: Live,
            catchup: true,
            score: Prefix,
        }]
    );
    assert_eq!(idx.program_count(), 12);
    assert_eq!(idx.title_count(), 7);
    assert!(idx.word_count() >= 12);
    assert!(idx.heap_bytes() > 0);
    assert!(idx.search_programs("  ", NOW, opts()).is_empty());
    assert!(idx.search_programs("zebra", NOW, opts()).is_empty());
    let empty = ProgramIndex::build(&GuideStore::new(), &keys());
    assert!(empty.search_programs("owl", NOW, opts()).is_empty());
}

/// Deterministic synthetic titles from invented syllables.
fn synthetic_titles(n: usize) -> Vec<String> {
    const SYL: [&str; 12] = [
        "ka", "lo", "mi", "ren", "tu", "vash", "ol", "pe", "zan", "qui", "bro", "é",
    ];
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    (0..n)
        .map(|_| {
            let words = 1 + next() % 3;
            (0..words)
                .map(|_| {
                    let s = 1 + next() % 3;
                    (0..s)
                        .map(|_| SYL[(next() % 12) as usize])
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join(if next() % 4 == 0 { "-" } else { " " })
        })
        .collect()
}

/// The index returns exactly what checking every programme would.
#[test]
fn agrees_with_a_linear_scan() {
    let titles = synthetic_titles(400);
    let mut progs = Vec::new();
    for (i, t) in titles.iter().enumerate() {
        let ch = ["a", "b", "d"][i % 3];
        let start = NOW - 2 * DAY + (i as i64 / 3) * HOUR;
        progs.push((ch, start, start + HOUR, t.as_str()));
    }
    let store = store(&progs);
    let idx = ProgramIndex::build(&store, &keys());
    let all = ProgramSearchOptions {
        include_past_without_catchup: true,
        look_back: 100 * DAY,
        look_ahead: 100 * DAY,
        limit: usize::MAX,
        ..opts()
    };
    let queries = [
        "ka",
        "k",
        "lo mi",
        "vash",
        "renka",
        "ol pe",
        "é",
        "e",
        "zanqui",
        "bro bro",
        "kalo",
        "tu-ka",
        "ashk",
        "mi ren tu",
        "quibro",
        "olp",
    ];
    let mut matched = 0;
    for qs in queries {
        let q = Query::parse(qs);
        let mut expect: Vec<(usize, i64, MatchQuality)> = Vec::new();
        for (c, key) in keys().iter().enumerate() {
            for p in store.programs(key, i64::MIN, i64::MAX) {
                if let Some(s) = q.match_tokens(&crate::tokens(p.title())) {
                    expect.push((c, p.start(), s));
                }
            }
        }
        let mut got: Vec<_> = idx
            .search_programs(qs, NOW, all)
            .into_iter()
            .map(|h| (h.channel, h.start, h.score))
            .collect();
        expect.sort();
        got.sort();
        assert_eq!(got, expect, "query {qs:?}");
        matched += usize::from(!got.is_empty());
    }
    assert!(matched >= 12, "the queries exercise real matches");
}

#[test]
fn hidden_channels_do_not_consume_the_result_limit() {
    let idx = index();
    let visible = |c| c == 1;
    let opts = ProgramSearchOptions {
        visible: Some(&visible),
        limit: 1,
        include_past_without_catchup: true,
        ..opts()
    };
    let result = run(&idx, "owl", opts);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, 1);
}
