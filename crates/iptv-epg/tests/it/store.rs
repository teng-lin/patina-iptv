//! Guide store: normalization of schedules, retention, priorities, queries.

use std::collections::HashMap;

use iptv_epg::*;

const H: i64 = 3600;
const T0: i64 = 1_772_323_200; // 2026-03-01T00:00:00Z

fn prog(channel: &str, start: i64, stop: Option<i64>, title: &str) -> XmltvProgram {
    XmltvProgram {
        channel: channel.into(),
        start,
        stop,
        title: title.into(),
        ..XmltvProgram::default()
    }
}

fn build(
    id: u32,
    priority: i32,
    progs: Vec<XmltvProgram>,
    opts: StoreOptions,
    now: i64,
) -> EpgSource {
    let mut b = SourceBuilder::new(FeedId(id), priority, opts, now);
    for p in progs {
        b.push_program(p);
    }
    b.finish()
}

fn titles(s: ScheduleRef<'_>) -> Vec<(i64, i64, String)> {
    s.iter()
        .map(|p| (p.start() - T0, p.stop() - T0, p.title().to_string()))
        .collect()
}

fn messy() -> Vec<XmltvProgram> {
    vec![
        prog("c", T0 + 2 * H, Some(T0 + 3 * H), "C"),
        prog("c", T0, Some(T0 + H), "A short dup"),
        prog("c", T0, Some(T0 + 2 * H), "A long"),
        prog("c", T0, Some(T0 + 2 * H), "A long, later in input"),
        prog(
            "c",
            T0 + H + H / 2,
            Some(T0 + 2 * H + H / 2),
            "B overlaps C",
        ),
        prog("c", T0 + 4 * H, None, "D no stop"),
        prog("c", T0 + 5 * H, Some(T0 + 5 * H), "zero"),
        prog("c", T0 + 6 * H, Some(T0 + 5 * H), "negative"),
        prog("c", T0 + 5 * H, Some(T0 + 6 * H), "E"),
        prog("c", T0 + 7 * H, None, "F last without stop"),
    ]
}

#[test]
fn schedules_are_sorted_non_overlapping_and_deterministic() {
    let src = build(1, 0, messy(), StoreOptions::default(), T0);
    let got = titles(src.schedule("c").unwrap());
    assert_eq!(
        got,
        vec![
            (0, H + H / 2, "A long".to_string()),
            (H + H / 2, 2 * H, "B overlaps C".to_string()),
            (2 * H, 3 * H, "C".to_string()),
            (4 * H, 5 * H, "D no stop".to_string()),
            (5 * H, 6 * H, "E".to_string()),
        ]
    );
    let r = src.report();
    assert_eq!(r.programs_in, 10);
    assert_eq!(r.kept, 5);
    assert_eq!(r.dropped_bad_duration, 2);
    assert_eq!(r.dropped_duplicate, 2);
    assert_eq!(r.dropped_no_stop, 1);
    assert_eq!(r.overlaps_trimmed, 2);

    // Same result for any input order of distinct programs (equal
    // starts with equal stops keep input order, so drop that tie here).
    let mut input: Vec<_> = messy()
        .into_iter()
        .filter(|p| p.title != "A long, later in input")
        .collect();
    for _ in 0..5 {
        input.rotate_left(3);
        input.reverse();
        let src = build(1, 0, input.clone(), StoreOptions::default(), T0);
        assert_eq!(titles(src.schedule("c").unwrap()), got);
    }
}

#[test]
fn queries_respect_half_open_intervals_and_gaps() {
    let src = build(1, 0, messy(), StoreOptions::default(), T0);
    let s = src.schedule("c").unwrap();
    let title = |p: Option<ProgramRef>| p.map(|p| p.title().to_string());
    assert_eq!(title(s.at(T0 - 1)), None);
    assert_eq!(title(s.at(T0)).as_deref(), Some("A long"));
    assert_eq!(title(s.at(T0 + 2 * H - 1)).as_deref(), Some("B overlaps C"));
    assert_eq!(title(s.at(T0 + 2 * H)).as_deref(), Some("C"));
    // Gap 3h..4h.
    assert_eq!(title(s.at(T0 + 3 * H)), None);
    assert_eq!(
        title(s.next_after(T0 + 3 * H)).as_deref(),
        Some("D no stop")
    );
    assert_eq!(title(s.next_after(T0)).as_deref(), Some("B overlaps C"));
    assert_eq!(title(s.next_after(T0 + 5 * H)), None);
    assert_eq!(title(s.at(T0 + 6 * H)), None);

    let range = |a: i64, b: i64| -> Vec<String> {
        s.range(T0 + a, T0 + b)
            .map(|p| p.title().to_string())
            .collect()
    };
    assert_eq!(range(2 * H, 3 * H), ["C"]);
    assert_eq!(range(2 * H - 1, 3 * H + 1), ["B overlaps C", "C"]);
    assert_eq!(range(3 * H, 4 * H), Vec::<String>::new());
    assert_eq!(range(-10 * H, 100 * H).len(), 5);
    assert_eq!(range(5 * H, 5 * H), Vec::<String>::new());

    let nn = s.now_next(T0 + 3 * H + 1);
    assert!(nn.now.is_none());
    assert_eq!(nn.next.unwrap().title(), "D no stop");
    let nn = s.now_next(T0 + 10);
    assert_eq!(nn.now.unwrap().title(), "A long");
    assert_eq!(nn.next.unwrap().title(), "B overlaps C");
    assert!((nn.now.unwrap().progress(T0 + 10) - 10.0 / 5400.0).abs() < 1e-6);
}

#[test]
fn retention_drops_old_programs_at_ingest_and_prune() {
    let progs: Vec<_> = (0..10)
        .map(|d| {
            prog(
                "c",
                T0 + d * DAY,
                Some(T0 + d * DAY + H),
                &format!("day {d}"),
            )
        })
        .collect();
    let now = T0 + 5 * DAY + H / 2;
    let opts = StoreOptions {
        past_days_to_keep: Some(2),
        ..StoreOptions::default()
    };
    let src = build(1, 0, progs.clone(), opts, now);
    // Kept: ended after now - 2 days = 3d+0.5h, so day 3 (ends 3d+1h) on.
    assert_eq!(src.schedule("c").unwrap().get(0).unwrap().title(), "day 3");
    assert_eq!(src.report().dropped_retention, 3);

    let mut store = GuideStore::new();
    store.replace_source(build(1, 0, progs, StoreOptions::default(), now));
    let key = ChannelKey::from("k");
    store.set_matches(HashMap::from([(
        key.clone(),
        vec![EpgRef {
            source: FeedId(1),
            epg_id: "c".into(),
            kind: MatchKind::TvgId,
        }],
    )]));
    assert_eq!(store.schedule(&key).unwrap().len(), 10);
    store.prune(now, 1);
    let s = store.schedule(&key).unwrap();
    assert_eq!((s.len(), s.get(0).unwrap().title()), (6, "day 4"));
    // Pruning everything unbinds the channel.
    store.prune(now + 100 * DAY, 0);
    assert!(store.schedule(&key).is_none());
}

#[test]
fn descriptions_and_extras() {
    let mut p = prog("c", T0, Some(T0 + H), "With extras");
    p.desc = Some("A long description".into());
    p.sub_title = Some("Pilot".into());
    p.categories = vec!["Drama".into(), "Ignored".into()];
    p.icon = Some("https://img.example.com/x.png".into());
    p.catchup_id = Some("cu-1".into());
    p.rating = Some(Rating {
        system: None,
        value: "16".into(),
    });
    p.episode_num = vec![EpisodeNum {
        system: Some("xmltv_ns".into()),
        value: "0.2.".into(),
    }];
    let plain = prog("c", T0 + H, Some(T0 + 2 * H), "Plain");
    let src = build(
        1,
        0,
        vec![p.clone(), plain.clone()],
        StoreOptions::default(),
        T0,
    );
    let s = src.schedule("c").unwrap();
    let a = s.get(0).unwrap();
    assert_eq!(a.desc(), Some("A long description"));
    assert_eq!(a.sub_title(), Some("Pilot"));
    assert_eq!(a.category(), Some("Drama"));
    assert_eq!(a.icon(), Some("https://img.example.com/x.png"));
    assert_eq!(a.catchup_id(), Some("cu-1"));
    assert_eq!(a.rating(), Some("16"));
    assert_eq!(a.season_episode(), (Some(1), Some(3)));
    let b = s.get(1).unwrap();
    assert_eq!(
        (b.desc(), b.sub_title(), b.season_episode()),
        (None, None, (None, None))
    );

    let no_desc = StoreOptions {
        store_descriptions: false,
        ..StoreOptions::default()
    };
    let src = build(1, 0, vec![p, plain], no_desc, T0);
    let a = src.schedule("c").unwrap().get(0).unwrap();
    assert_eq!(a.desc(), None);
    assert_eq!(a.sub_title(), Some("Pilot"));
}

/// What a stored guide does: read every field back as plain values and
/// rebuild an equal source from them.
#[test]
fn program_data_round_trips() {
    let mut p = prog("c", T0, Some(T0 + H), "Full");
    p.desc = Some("About it".into());
    p.sub_title = Some("Part one".into());
    p.categories = vec!["Drama".into()];
    p.icon = Some("https://img.example.com/y.png".into());
    p.catchup_id = Some("cu-7".into());
    p.rating = Some(Rating {
        system: None,
        value: "12".into(),
    });
    p.episode_num = vec![EpisodeNum {
        system: Some("onscreen".into()),
        value: "S02E05".into(),
    }];
    let plain = prog("c", T0 + H, Some(T0 + 2 * H), "Plain");
    let src = build(1, 0, vec![p, plain], StoreOptions::default(), T0);
    let rows: Vec<ProgramData<'_>> = src
        .schedule("c")
        .unwrap()
        .iter()
        .map(|p| p.data())
        .collect();
    assert_eq!(rows[0].season, Some(2));
    assert_eq!(rows[0].episode, Some(5));
    assert_eq!(rows[0].rating, Some("12"));
    assert_eq!(rows[1].desc, None);

    let mut b = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), T0);
    for r in &rows {
        b.push_data("c", r);
    }
    let again = b.finish();
    let back: Vec<ProgramData<'_>> = again
        .schedule("c")
        .unwrap()
        .iter()
        .map(|p| p.data())
        .collect();
    assert_eq!(back, rows);

    // Loaded without descriptions: everything else stays.
    let mut b = SourceBuilder::new(
        FeedId(1),
        0,
        StoreOptions {
            store_descriptions: false,
            ..StoreOptions::default()
        },
        T0,
    );
    b.push_data("c", &rows[0]);
    let lean = b.finish();
    let first = lean.schedule("c").unwrap().get(0).unwrap().data();
    assert_eq!(first.desc, None);
    assert_eq!(
        ProgramData {
            desc: rows[0].desc,
            ..first
        },
        rows[0]
    );
}

#[test]
fn repeated_titles_are_interned_once() {
    let progs: Vec<_> = (0..1000)
        .map(|i| {
            prog(
                "c",
                T0 + i * H,
                Some(T0 + (i + 1) * H),
                if i % 2 == 0 { "News" } else { "Film" },
            )
        })
        .collect();
    let src = build(1, 0, progs, StoreOptions::default(), T0);
    assert_eq!(src.program_count(), 1000);
    assert_eq!(src.string_count(), 2);
}

fn bind_all(store: &mut GuideStore, keys: &[(&str, &[(u32, &str)])]) {
    let matches = keys
        .iter()
        .map(|(k, cands)| {
            let refs = cands
                .iter()
                .map(|&(s, id)| EpgRef {
                    source: FeedId(s),
                    epg_id: id.into(),
                    kind: MatchKind::TvgId,
                })
                .collect();
            (ChannelKey::from(*k), refs)
        })
        .collect();
    store.set_matches(matches);
}

#[test]
fn higher_priority_source_with_data_wins_per_channel() {
    let opts = StoreOptions::default();
    let low = build(
        1,
        1,
        vec![
            prog("a", T0, Some(T0 + H), "low a"),
            prog("b", T0, Some(T0 + H), "low b"),
        ],
        opts,
        T0,
    );
    let high = build(2, 9, vec![prog("a", T0, Some(T0 + H), "high a")], opts, T0);
    let mut store = GuideStore::new();
    store.replace_source(low);
    store.replace_source(high);
    // Candidates in any order: the store ranks by its own priorities.
    bind_all(
        &mut store,
        &[
            ("ka", &[(1, "a"), (2, "a")]),
            ("kb", &[(1, "b"), (2, "b")]),
            ("kz", &[(7, "z")]),
        ],
    );
    let at = |s: &GuideStore, k: &str| s.program_at(&k.into(), T0).map(|p| p.title().to_string());
    assert_eq!(at(&store, "ka").as_deref(), Some("high a"));
    // The high source has no "b": fall back to the low one.
    assert_eq!(at(&store, "kb").as_deref(), Some("low b"));
    assert_eq!(at(&store, "kz"), None);
    assert_eq!(store.binding(&"ka".into()), Some((FeedId(2), "a")));
    assert_eq!(store.bound_count(), 2);
    let mut bound: Vec<_> = store
        .bindings()
        .map(|(k, c)| (k.as_str().to_owned(), c.source, c.epg_id.clone()))
        .collect();
    bound.sort();
    assert_eq!(
        bound,
        [
            ("ka".to_owned(), FeedId(2), "a".to_owned()),
            ("kb".to_owned(), FeedId(1), "b".to_owned())
        ]
    );

    // Re-ingesting the high source with a lower priority flips the winner.
    store.replace_source(build(
        2,
        0,
        vec![prog("a", T0, Some(T0 + H), "high a")],
        opts,
        T0,
    ));
    assert_eq!(at(&store, "ka").as_deref(), Some("low a"));
    store.remove_source(FeedId(1));
    assert_eq!(at(&store, "ka").as_deref(), Some("high a"));
    assert_eq!(at(&store, "kb"), None);
}

#[test]
fn now_next_batch_keeps_input_order() {
    let opts = StoreOptions::default();
    let src = build(
        1,
        0,
        vec![
            prog("a", T0, Some(T0 + H), "a1"),
            prog("a", T0 + H, Some(T0 + 2 * H), "a2"),
            prog("b", T0 + H, Some(T0 + 2 * H), "b1"),
        ],
        opts,
        T0,
    );
    let mut store = GuideStore::new();
    store.replace_source(src);
    bind_all(&mut store, &[("ka", &[(1, "a")]), ("kb", &[(1, "b")])]);
    let keys: Vec<ChannelKey> = ["kb", "none", "ka"].into_iter().map(Into::into).collect();
    let nn = store.now_next(&keys, T0 + 10);
    fn show<'a>(p: Option<ProgramRef<'a>>) -> Option<&'a str> {
        p.map(|p| p.title())
    }
    assert_eq!(nn.len(), 3);
    assert_eq!((show(nn[0].now), show(nn[0].next)), (None, Some("b1")));
    assert_eq!((show(nn[1].now), show(nn[1].next)), (None, None));
    assert_eq!(
        (show(nn[2].now), show(nn[2].next)),
        (Some("a1"), Some("a2"))
    );
    let window: Vec<_> = store
        .programs(&"ka".into(), T0 + H / 2, T0 + 3 * H)
        .map(|p| p.title())
        .collect();
    assert_eq!(window, ["a1", "a2"]);
    assert_eq!(store.programs(&"none".into(), T0, T0 + H).count(), 0);
}

#[test]
fn parse_match_store_end_to_end() {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><tv>
<channel id="one.example"><display-name>Channel One</display-name></channel>
<channel id="two.example"><display-name>Zwei Kanal</display-name></channel>
<programme channel="one.example" start="{a}" stop="{b}"><title>First</title><desc>Desc</desc></programme>
<programme channel="one.example" start="{b}" stop="{c}"><title>Second</title></programme>
<programme channel="two.example" start="{a}" stop="{c}"><title>Long</title></programme>
</tv>"#,
        a = format_xmltv_time(T0),
        b = format_xmltv_time(T0 + H),
        c = format_xmltv_time(T0 + 2 * H),
    );
    let gz = {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(xml.as_bytes()).unwrap();
        e.finish().unwrap()
    };
    let mut builder = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), T0);
    let mut parser = XmltvParser::new(ParseOptions::default());
    for chunk in gz.chunks(50) {
        parser.feed(chunk, &mut |item| builder.push(item)).unwrap();
    }
    let stats = parser.finish(&mut |item| builder.push(item)).unwrap();
    assert_eq!((stats.channels, stats.programs), (2, 3));
    let src = builder.finish();

    let mut index = MatchIndex::new();
    index.add_source(src.id(), src.priority(), src.channels());
    let keys: Vec<ChannelKey> = ["p-one", "p-two", "p-three"]
        .into_iter()
        .map(Into::into)
        .collect();
    let names = ["CHANNEL ONE HD", "Zwei Kanal", "Nothing"];
    let result = index.match_channels(
        keys.iter().zip(names).map(|(key, name)| PlaylistChannel {
            key,
            tvg_id: None,
            name,
            tvg_name: None,
        }),
        &Overrides::new(),
    );
    assert_eq!((result.report.matched(), result.report.unmatched), (2, 1));

    let mut store = GuideStore::new();
    store.replace_source(src);
    store.set_matches(result.matches);
    let nn = store.now_next(&keys, T0 + 5);
    assert_eq!(nn[0].now.unwrap().title(), "First");
    assert_eq!(nn[0].now.unwrap().desc(), Some("Desc"));
    assert_eq!(nn[0].next.unwrap().title(), "Second");
    assert_eq!(nn[1].now.unwrap().title(), "Long");
    assert!(nn[2].now.is_none());
}
