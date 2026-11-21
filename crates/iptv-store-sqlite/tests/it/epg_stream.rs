//! The streaming guide import (`import_epg` into an [`EpgStage`], then
//! [`Store::replace_epg`]) against the in-memory import it replaced, and
//! what a failed or canceled download leaves. Synthetic data only.

use crate::common;
use crate::stream_support;

use std::collections::{HashMap, HashSet};

use iptv_epg::{
    ChannelKey, DAY, FeedId, GuideStore, MatchIndex, MatchReport, Override, Overrides,
    ParseOptions, PlaylistChannel, ProgramData, SourceBuilder, StoreOptions, UnixTime, parse_xmltv,
};
use iptv_import::{
    CancelToken, EpgSourceConfig, ImportError, NoProgress, Progress, ProgressSink, import_epg,
    matching_fingerprint,
};
use iptv_providers::SecretUrl;
use iptv_providers::model::Channel;
use iptv_store_sqlite::{EpgFeedMeta, EpgStage, EpgWindow, Store};
use stream_support::{El, Feeds, GenShape, HOUR, Prog, Serve, block_on, prog};

const NOW: UnixTime = 1_800_000_000;

fn config(id: u32, name: &str, priority: i32, store: StoreOptions) -> EpgSourceConfig {
    let mut c = EpgSourceConfig::new(
        FeedId(id),
        SecretUrl::new(format!("{}{name}", stream_support::BASE)),
    )
    .with_priority(priority);
    c.store = store;
    c
}

fn key(c: &Channel) -> ChannelKey {
    ChannelKey::new(c.id.as_str())
}

/// What `import_epg` did before it streamed: every loaded feed parsed
/// whole into a `SourceBuilder`, then matched and bound in a `GuideStore`.
fn in_memory(
    feeds: &[(EpgSourceConfig, Option<&str>)],
    channels: &[Channel],
    overrides: &Overrides,
) -> (GuideStore, MatchReport) {
    let mut guide = GuideStore::new();
    let mut index = MatchIndex::new();
    for (cfg, body) in feeds {
        let Some(body) = body else { continue };
        let opts = ParseOptions {
            time: cfg.time,
            descriptions: cfg.store.store_descriptions,
            ..ParseOptions::default()
        };
        let parsed = parse_xmltv(body.as_bytes(), opts).unwrap();
        let mut b = SourceBuilder::new(cfg.id, cfg.priority, cfg.store, NOW);
        parsed.channels.into_iter().for_each(|c| b.push_channel(c));
        parsed.programs.into_iter().for_each(|p| b.push_program(p));
        let src = b.finish();
        index.add_source(cfg.id, cfg.priority, src.channels());
        guide.replace_source(src);
    }
    let keys: Vec<ChannelKey> = channels.iter().map(key).collect();
    let result = index.match_channels(
        channels.iter().zip(&keys).map(|(c, key)| PlaylistChannel {
            key,
            tvg_id: c.epg_id.as_deref(),
            name: &c.name,
            tvg_name: c.epg_name.as_deref(),
        }),
        overrides,
    );
    guide.set_matches(result.matches);
    (guide, result.report)
}

/// A store with one source, in its own directory (the stage goes there).
fn store() -> (Store, iptv_store_sqlite::SourceId, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("iptv.sqlite")).unwrap();
    let source = store.add_source(&common::m3u_source("Synthetic")).unwrap();
    (store, source, dir)
}

const EVERYTHING: EpgWindow<'static> = EpgWindow {
    from: UnixTime::MIN,
    to: UnixTime::MAX,
    keys: None,
    descriptions: true,
};

fn rows(guide: &GuideStore, key: &ChannelKey) -> Vec<ProgramData<'static>> {
    // Owned copies with 'static strings for comparison.
    let leak = |s: &str| -> &'static str { Box::leak(s.to_owned().into_boxed_str()) };
    let opt = |s: Option<&str>| s.map(leak);
    guide
        .schedule(key)
        .into_iter()
        .flat_map(|s| s.iter())
        .map(|p| {
            let d = p.data();
            ProgramData {
                title: leak(d.title),
                sub_title: opt(d.sub_title),
                desc: opt(d.desc),
                category: opt(d.category),
                icon: opt(d.icon),
                rating: opt(d.rating),
                catchup_id: opt(d.catchup_id),
                ..d
            }
        })
        .collect()
}

/// Feed 1 (priority 1): tvg-ids `a0..a29` with every shape the schedule
/// rules handle, a channel declared after its programs, one never
/// declared, and channels nothing matches.
fn feed_one() -> String {
    let mut els = Vec::new();
    let ids: Vec<String> = (0..30).map(|i| format!("a{i}")).collect();
    let names: Vec<String> = (0..30).map(|i| format!("Alpha {i}")).collect();
    let unrelated: Vec<String> = (0..10).map(|i| format!("u{i}")).collect();
    let unrelated_names: Vec<String> = (0..10).map(|i| format!("Unrelated {i}")).collect();
    let name_refs: Vec<[&str; 1]> = names.iter().map(|n| [n.as_str()]).collect();
    let unrelated_refs: Vec<[&str; 1]> = unrelated_names.iter().map(|n| [n.as_str()]).collect();
    for (id, n) in ids.iter().zip(&name_refs) {
        els.push(El::Channel(id, n));
    }
    for (id, n) in unrelated.iter().zip(&unrelated_refs) {
        els.push(El::Channel(id, n));
    }
    // Three days back to a day ahead: retention (2 days) drops the oldest.
    let from = NOW - 3 * DAY;
    let mut titles = Vec::new();
    for c in 0..30 {
        for h in 0..96 {
            titles.push((c, h, format!("Show {c}-{h}")));
        }
    }
    for (c, h, title) in &titles {
        let start = from + h * HOUR;
        let mut p = prog(&ids[*c], start, Some(start + HOUR), title);
        p.desc = Some("An invented program.");
        if h % 5 == 0 {
            p.sub_title = Some("Part one");
            p.episode = Some("S01E02");
            p.category = Some("Synthetic");
        }
        if h % 7 == 0 {
            // Zero season and episode are unknown, as in memory.
            p.episode = Some("S00E00");
        }
        match (*c, *h) {
            // Missing stops: the next start ends them; the last has none.
            (2, h) if h % 3 == 0 || h == 95 => p.stop = None,
            // Overlaps the next program: cut at its start.
            (3, h) if h % 4 == 0 => p.stop = Some(start + 3 * HOUR),
            // Bad durations.
            (4, 10) => p.stop = Some(start),
            (4, 11) => p.stop = Some(start - HOUR),
            _ => {}
        }
        els.push(El::Program(p));
    }
    // Same start: the longest stays; equal ones: the first; a missing
    // stop is shortest.
    for (title, stop) in [
        ("Short", Some(NOW + HOUR / 2)),
        ("Long", Some(NOW + 2 * HOUR)),
        ("Long too", Some(NOW + 2 * HOUR)),
        ("Open", None),
    ] {
        els.push(El::Program(prog("a1", NOW, stop, title)));
    }
    els.push(El::Program(prog("a5", NOW + HOUR / 4, None, "Open")));
    // Reverse document order.
    for h in (0..24).rev() {
        let start = NOW + 3 * DAY + h * HOUR;
        els.push(El::Program(Prog {
            desc: Some("Late night."),
            ..prog("a6", start, Some(start + HOUR), "Reversed")
        }));
    }
    for h in 0..6 {
        let start = NOW + h * HOUR;
        els.push(El::Program(prog("late", start, Some(start + HOUR), "Late")));
        els.push(El::Program(prog(
            "hidden",
            start,
            Some(start + HOUR),
            "Hidden",
        )));
        els.push(El::Program(prog(
            "u3",
            start,
            Some(start + HOUR),
            "Unrelated",
        )));
    }
    els.push(El::Channel("late", &["Late Channel"]));
    stream_support::xmltv(&els)
}

/// Feed 2 (priority 5): name matches for `Alpha 10..17`, interleaved by
/// time; `b3` declares a channel without programs.
fn feed_two() -> String {
    let ids: Vec<String> = (0..8).map(|i| format!("b{i}")).collect();
    let names: Vec<String> = (0..8).map(|i| format!("ALPHA {} HD", i + 10)).collect();
    let name_refs: Vec<[&str; 1]> = names.iter().map(|n| [n.as_str()]).collect();
    let mut els: Vec<El<'_>> = ids
        .iter()
        .zip(&name_refs)
        .map(|(id, n)| El::Channel(id, n))
        .collect();
    let titles: Vec<String> = (0..48).map(|h| format!("Beta {h}")).collect();
    for (h, title) in titles.iter().enumerate() {
        let start = NOW - DAY + h as i64 * HOUR;
        for (b, id) in ids.iter().enumerate() {
            if b != 3 {
                els.push(El::Program(Prog {
                    desc: Some("From the second feed."),
                    ..prog(id, start, Some(start + HOUR), title)
                }));
            }
        }
    }
    stream_support::xmltv(&els)
}

/// The playlist: tvg-ids (one in other case), names, overrides, nothing.
fn channels() -> Vec<Channel> {
    let ids: Vec<String> = (0..30).map(|i| format!("a{i}")).collect();
    let names: Vec<String> = (0..30).map(|i| format!("Alpha {i}")).collect();
    let mut entries: Vec<stream_support::PlaylistEntry<'_>> = ids
        .iter()
        .zip(&names)
        .map(|(id, n)| (Some(id.as_str()), None, n.as_str()))
        .collect();
    entries.extend([
        (Some("LATE"), None, "Late"),
        (None, None, "Chosen by hand"),
        (None, None, "Chosen in any feed"),
        (Some("a8"), None, "Switched off"),
        (None, Some("Nobody 1"), "Nothing"),
        (None, None, "Nothing either"),
    ]);
    stream_support::playlist(&entries)
}

fn overrides(chans: &[Channel]) -> Overrides {
    let mut o = Overrides::new();
    o.insert(
        key(&chans[31]),
        Override::Epg {
            source: Some(FeedId(1)),
            epg_id: "hidden".into(),
        },
    );
    o.insert(
        key(&chans[32]),
        Override::Epg {
            source: None,
            epg_id: "a7".into(),
        },
    );
    o.insert(key(&chans[33]), Override::NoEpg);
    o
}

fn check_equivalent(store_opts: StoreOptions) {
    let (one, two) = (feed_one(), feed_two());
    let configs = [
        config(1, "one.xml", 1, store_opts),
        config(2, "two.xml", 5, store_opts),
        config(3, "missing.xml", 9, store_opts),
    ];
    let fetch = Feeds::new()
        .serve("one.xml", Serve::Bytes(one.clone().into_bytes()))
        .serve("two.xml", Serve::Bytes(two.clone().into_bytes()));
    let chans = channels();
    let overrides = overrides(&chans);
    let (expected, report) = in_memory(
        &[
            (configs[0].clone(), Some(&one)),
            (configs[1].clone(), Some(&two)),
            (configs[2].clone(), None),
        ],
        &chans,
        &overrides,
    );

    let (mut store, source, dir) = store();
    let mut stage = EpgStage::create_in(dir.path()).unwrap();
    let epg = block_on(import_epg(
        &fetch,
        &configs,
        &chans,
        &overrides,
        NOW,
        &mut stage,
        &mut NoProgress,
        &CancelToken::new(),
    ))
    .unwrap();
    assert_eq!(epg.matches, report);
    assert!(epg.sources[2].result.is_err());
    // Bindings: same channels, same EPG channel, same kind.
    let mut bound: Vec<_> = expected
        .bindings()
        .map(|(k, e)| (k.clone(), e.clone()))
        .collect();
    bound.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(epg.bindings, bound);
    let bound_to = |i: usize| {
        epg.binding(&key(&chans[i]))
            .map(|e| (e.source.0, e.epg_id.as_str()))
    };
    // Every rule shape is on a channel bound to feed 1.
    for i in 1..7 {
        assert_eq!(bound_to(i), Some((1, format!("a{i}").as_str())));
    }
    // Feed 2 by name wins over feed 1 by tvg-id, unless it has nothing.
    assert_eq!(bound_to(10), Some((2, "b0")));
    assert_eq!(bound_to(13), Some((1, "a13")));
    assert_eq!(bound_to(30), Some((1, "late")));
    assert_eq!(bound_to(31), Some((1, "hidden")));
    assert_eq!(bound_to(32), Some((1, "a7")));
    assert_eq!(bound_to(33), None);
    assert_eq!(bound_to(34), None);
    assert_eq!(epg.bound_count(), 33);

    let feeds = EpgFeedMeta::from_import(&epg, &configs, NOW);
    let fp = matching_fingerprint(&chans, &overrides);
    let counts = store
        .replace_epg(source, &mut stage, &epg, &feeds, fp)
        .unwrap();
    drop(stage);
    let loaded = store.load_epg(source, &EVERYTHING).unwrap();
    assert_eq!(loaded.bound_count(), expected.bound_count());
    let mut total = 0;
    for c in &chans {
        let k = key(c);
        assert_eq!(loaded.binding(&k), expected.binding(&k), "{}", c.name);
        let (got, want) = (rows(&loaded, &k), rows(&expected, &k));
        assert_eq!(got, want, "{}", c.name);
        total += want.len();
    }
    assert!(total > 2000, "{total}");

    // What the store says about each feed matches the in-memory guide.
    let stored = store.epg_guide(source).unwrap().unwrap();
    let mut per_feed: HashMap<FeedId, (HashSet<&str>, u64)> = HashMap::new();
    let mut ends_at = None;
    for (_, epg) in expected.bindings() {
        let f = per_feed.entry(epg.source).or_default();
        if f.0.insert(epg.epg_id.as_str()) {
            let sched = expected
                .source(epg.source)
                .unwrap()
                .schedule(&epg.epg_id)
                .unwrap();
            f.1 += sched.len() as u64;
            ends_at = ends_at.max(sched.get(sched.len() - 1).map(|p| p.stop()));
        }
    }
    assert_eq!(stored.bound_channels, expected.bound_count() as u64);
    assert_eq!(stored.ends_at, ends_at);
    assert_eq!(stored.feeds.len(), 2);
    for f in &stored.feeds {
        let (chans, programs) = &per_feed[&f.meta.config.id];
        assert_eq!(f.channel_count, chans.len() as u64);
        assert_eq!(f.program_count, *programs);
    }
    assert_eq!(counts.programs, per_feed.values().map(|f| f.1).sum::<u64>());
    // Every channel the feeds list, bound or not, for choosing by hand
    // (plus feed 1's undeclared channel an override bound).
    let listed = store.guide_channels(source).unwrap();
    let listed_one = listed.iter().filter(|c| c.feed == FeedId(1)).count();
    let listed_two = listed.iter().filter(|c| c.feed == FeedId(2)).count();
    assert_eq!((listed_one, listed_two), (41 + 1, 8));
    assert!(
        listed
            .iter()
            .any(|c| c.epg_channel == "hidden" && c.assigned)
    );
}

#[test]
fn streaming_import_stores_what_the_in_memory_import_did() {
    check_equivalent(StoreOptions {
        store_descriptions: true,
        past_days_to_keep: Some(2),
    });
}

#[test]
fn streaming_import_without_descriptions_or_retention() {
    check_equivalent(StoreOptions {
        store_descriptions: false,
        past_days_to_keep: None,
    });
}

/// Imports `feeds` (served as given) into `source`'s stored guide.
// Mirrors the import boundary so each failure fixture can supply its own sinks.
#[allow(clippy::too_many_arguments)]
fn import_into(
    store: &mut Store,
    source: iptv_store_sqlite::SourceId,
    dir: &std::path::Path,
    fetch: &Feeds,
    configs: &[EpgSourceConfig],
    chans: &[Channel],
    progress: &mut impl ProgressSink,
    cancel: &CancelToken,
) -> Result<(), ImportError> {
    let mut stage = EpgStage::create_in(dir).unwrap();
    let epg = block_on(import_epg(
        fetch,
        configs,
        chans,
        &Overrides::new(),
        NOW,
        &mut stage,
        progress,
        cancel,
    ))?;
    let feeds = EpgFeedMeta::from_import(&epg, configs, NOW);
    store
        .replace_epg(source, &mut stage, &epg, &feeds, 1)
        .unwrap();
    Ok(())
}

fn titles(store: &Store, source: iptv_store_sqlite::SourceId, chans: &[Channel]) -> Vec<String> {
    let g = store.load_epg(source, &EVERYTHING).unwrap();
    chans
        .iter()
        .flat_map(|c| rows(&g, &key(c)))
        .map(|p| p.title.to_owned())
        .collect()
}

fn stage_files(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("epg-stage-"))
        .collect()
}

#[test]
#[cfg_attr(target_family = "wasm", ignore = "needs a file system")]
fn a_canceled_or_failed_download_keeps_the_stored_guide() {
    let shape = |from, days| GenShape {
        channels: 40,
        days,
        from,
    };
    let chans = stream_support::generated_playlist(40);
    let opts = StoreOptions::default();
    let fetch = Feeds::new()
        .serve("v1.xml", Serve::Generated(shape(NOW - DAY, 3)))
        .serve("v2.xml", Serve::Generated(shape(NOW, 6)));
    let (mut store, source, dir) = store();
    import_into(
        &mut store,
        source,
        dir.path(),
        &fetch,
        &[config(1, "v1.xml", 0, opts)],
        &chans,
        &mut NoProgress,
        &CancelToken::new(),
    )
    .unwrap();
    let before = titles(&store, source, &chans);
    let state = store.epg_guide(source).unwrap();
    assert_eq!(before.len(), 40 * 72);

    // Canceled half-way through the download: some programs are
    // already staged.
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let mut staged_some = false;
    let mut progress = |p: &Progress| {
        if p.items > 2000 {
            staged_some = true;
            c.cancel();
        }
    };
    let err = import_into(
        &mut store,
        source,
        dir.path(),
        &fetch,
        &[config(1, "v2.xml", 0, opts)],
        &chans,
        &mut progress,
        &cancel,
    );
    assert_eq!(err, Err(ImportError::Canceled));
    assert!(staged_some);
    assert_eq!(titles(&store, source, &chans), before);
    assert_eq!(store.epg_guide(source).unwrap(), state);
    assert_eq!(stage_files(dir.path()), Vec::<String>::new());

    // A connection dropped mid-way: that feed fails, so does the import.
    let mut g = stream_support::Generated::new(shape(NOW, 6));
    let body = [g.chunk().unwrap(), g.chunk().unwrap()].concat();
    let fetch = Feeds::new().serve("cut.xml", Serve::Truncated(body));
    let err = import_into(
        &mut store,
        source,
        dir.path(),
        &fetch,
        &[config(1, "cut.xml", 0, opts)],
        &chans,
        &mut NoProgress,
        &CancelToken::new(),
    );
    assert!(matches!(err, Err(ImportError::Network { .. })), "{err:?}");
    assert_eq!(titles(&store, source, &chans), before);
    assert_eq!(stage_files(dir.path()), Vec::<String>::new());
}

#[test]
fn a_failed_feed_is_left_out_and_the_others_are_stored() {
    let chans = stream_support::generated_playlist(10);
    let opts = StoreOptions::default();
    let good = GenShape {
        channels: 10,
        days: 1,
        from: NOW,
    };
    // The first feed breaks after channel elements and a few programs.
    let mut broken = stream_support::xmltv(&[El::Channel("gen0.example", &["Generated 0"])]);
    broken.truncate(broken.len() - "</tv>\n".len());
    for h in 0..50 {
        let t = NOW + h * HOUR;
        broken.push_str(&format!(
            "<programme start=\"{}\" stop=\"{}\" channel=\"gen0.example\"><title>Broken</title></programme>\n",
            iptv_epg::format_xmltv_time(t),
            iptv_epg::format_xmltv_time(t + HOUR)
        ));
    }
    let fetch = Feeds::new()
        .serve("broken.xml", Serve::Truncated(broken.into_bytes()))
        .serve("good.xml", Serve::Generated(good));
    let (mut store, source, dir) = store();
    import_into(
        &mut store,
        source,
        dir.path(),
        &fetch,
        &[
            config(1, "broken.xml", 9, opts),
            config(2, "good.xml", 1, opts),
        ],
        &chans,
        &mut NoProgress,
        &CancelToken::new(),
    )
    .unwrap();
    let all = titles(&store, source, &chans);
    assert_eq!(all.len(), 10 * 24);
    assert!(all.iter().all(|t| t.starts_with("Show ")));
    let stored = store.epg_guide(source).unwrap().unwrap();
    assert_eq!(stored.feeds.len(), 1);
    assert_eq!(stored.feeds[0].meta.config.id, FeedId(2));
}

#[test]
fn the_store_stays_writable_while_a_guide_downloads() {
    let chans = stream_support::generated_playlist(20);
    let fetch = Feeds::new().serve(
        "g.xml",
        Serve::Generated(GenShape {
            channels: 20,
            days: 2,
            from: NOW,
        }),
    );
    let (mut store, source, dir) = store();
    let mut other = Store::open_with(
        dir.path().join("iptv.sqlite"),
        &iptv_store_sqlite::OpenOptions {
            busy_timeout: std::time::Duration::from_millis(50),
            ..Default::default()
        },
    )
    .unwrap();
    let mut writes = 0;
    let mut progress = |p: &Progress| {
        if p.items > 0 {
            other
                .rename_source(source, &format!("Renamed {writes}"))
                .unwrap();
            writes += 1;
        }
    };
    import_into(
        &mut store,
        source,
        dir.path(),
        &fetch,
        &[config(1, "g.xml", 0, StoreOptions::default())],
        &chans,
        &mut progress,
        &CancelToken::new(),
    )
    .unwrap();
    assert!(writes > 0);
    assert_eq!(titles(&store, source, &chans).len(), 20 * 48);
}

#[test]
#[cfg_attr(target_family = "wasm", ignore = "needs a file system")]
fn stale_stage_files_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("epg-stage-1-0.sqlite"), b"left over").unwrap();
    std::fs::write(dir.path().join("iptv.sqlite"), b"kept").unwrap();
    let stage = EpgStage::create_in(dir.path()).unwrap();
    assert!(stage.path().exists());
    drop(stage);
    assert_eq!(stage_files(dir.path()), ["epg-stage-1-0.sqlite"]);
    EpgStage::remove_stale(dir.path());
    assert_eq!(stage_files(dir.path()), Vec::<String>::new());
    assert!(dir.path().join("iptv.sqlite").exists());
}
