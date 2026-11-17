//! Stored guide data: replace, windows, lazy descriptions, prune, delete.

use crate::common;
use crate::epg_synth;
use crate::staged;

use epg_synth::{NOW, Shape};
use iptv_epg::{
    ChannelKey, EpgRef, GuideStore, MatchKind, ProgrammeData, SourceBuilder, StoreOptions,
    XmltvProgramme,
};
use iptv_import::{EpgSourceConfig, FailureKind, GuideState};
use iptv_providers::SecretUrl;
use iptv_store_sqlite::{EpgFeedMeta, EpgWindow, ImportErrorKind, SourceId, Store};

const H: i64 = 3600;

fn store_with_sources() -> (Store, SourceId, SourceId) {
    let mut store = Store::open_in_memory().unwrap();
    store.set_clock(|| NOW);
    let a = store.add_source(&common::xtream_source("A")).unwrap();
    let b = store.add_source(&common::m3u_source("B")).unwrap();
    (store, a, b)
}

fn config(id: u32, priority: i32) -> EpgSourceConfig {
    EpgSourceConfig::new(
        iptv_epg::FeedId(id),
        SecretUrl::new(format!(
            "http://epg.example.com/{id}.xml?password={}",
            common::XT_PASSWORD
        )),
    )
    .with_priority(priority)
}

fn meta(config: EpgSourceConfig) -> EpgFeedMeta {
    EpgFeedMeta {
        config,
        fetched_at: NOW,
        etag: Some("\"v1\"".into()),
        last_modified: None,
        bytes: 1234,
    }
}

fn prog(channel: &str, start: i64, stop: i64, title: &str, desc: Option<&str>) -> XmltvProgramme {
    XmltvProgramme {
        channel: channel.into(),
        start,
        stop: Some(stop),
        title: title.into(),
        desc: desc.map(str::to_owned),
        ..XmltvProgramme::default()
    }
}

/// Two feeds: "hi" (priority 9) has `x`, "lo" (1) has `x` and `y`.
/// Keys: k1 -> x (hi wins), k2 -> y (lo), k3 -> x as well, k4 -> missing.
fn two_feed_guide(tag: &str) -> GuideStore {
    let mut hi = SourceBuilder::new(iptv_epg::FeedId(1), 9, StoreOptions::default(), NOW);
    let mut lo = SourceBuilder::new(iptv_epg::FeedId(2), 1, StoreOptions::default(), NOW);
    for i in -3..6 {
        let t = NOW + i * H;
        hi.push_programme(prog("x", t, t + H, &format!("{tag} X{i}"), Some("About X")));
        lo.push_programme(prog("x", t, t + H, "lo X", None));
        lo.push_programme(prog("y", t, t + 2 * H, &format!("{tag} Y{i}"), None));
    }
    let mut g = GuideStore::new();
    g.replace_source(hi.finish());
    g.replace_source(lo.finish());
    let cand = |s: u32, id: &str, kind| EpgRef {
        source: iptv_epg::FeedId(s),
        epg_id: id.into(),
        kind,
    };
    g.set_matches(
        [
            (
                "k1",
                vec![
                    cand(1, "x", MatchKind::TvgId),
                    cand(2, "x", MatchKind::TvgId),
                ],
            ),
            (
                "k2",
                vec![cand(1, "y", MatchKind::Name), cand(2, "y", MatchKind::Name)],
            ),
            ("k3", vec![cand(1, "x", MatchKind::Override)]),
            ("k4", vec![cand(1, "nothing", MatchKind::TvgId)]),
        ]
        .into_iter()
        .map(|(k, c)| (ChannelKey::from(k), c))
        .collect(),
    );
    g
}

fn titles(g: &GuideStore, key: &str, from: i64, to: i64) -> Vec<String> {
    g.programmes(&key.into(), from, to)
        .map(|p| p.title().to_owned())
        .collect()
}

#[test]
fn replace_then_load_round_trips_bindings_and_priorities() {
    let (mut store, a, _) = store_with_sources();
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    let counts = staged::replace_epg(&mut store, a, &two_feed_guide("v1"), &feeds, 99).unwrap();
    // x from hi (shared by k1 and k3), y from lo: two EPG channels stored.
    assert_eq!((counts.channels, counts.bindings), (2, 3));
    assert_eq!(counts.programmes, 18);
    assert_eq!(counts.descriptions, 9);

    let g = store.load_epg(a, &EpgWindow::from(NOW, 1)).unwrap();
    assert_eq!(g.bound_count(), 3);
    assert_eq!(titles(&g, "k1", NOW, NOW + H), ["v1 X0"]);
    assert_eq!(titles(&g, "k3", NOW, NOW + H), ["v1 X0"]);
    assert_eq!(titles(&g, "k2", NOW, NOW + H), ["v1 Y0"]);
    assert_eq!(g.binding(&"k3".into()), Some((iptv_epg::FeedId(1), "x")));
    assert!(g.programme_at(&"k4".into(), NOW).is_none());

    let stored = store.epg_guide(a).unwrap().unwrap();
    assert_eq!(stored.replaced_at, Some(NOW));
    assert_eq!(stored.ends_at, Some(NOW + 7 * H));
    assert_eq!(
        stored
            .feeds
            .iter()
            .map(|f| (f.meta.config.id.0, f.channel_count, f.programme_count))
            .collect::<Vec<_>>(),
        [(1, 1, 9), (2, 1, 9)]
    );
    assert_eq!(stored.feeds[0].meta, feeds[0]);
    let state: GuideState = stored.refresh_state();
    assert_eq!(state.match_fingerprint, Some(99));
    assert_eq!(state.bound_channels, 3);
}

#[test]
fn windows_keys_and_descriptions() {
    let (mut store, a, _) = store_with_sources();
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    staged::replace_epg(&mut store, a, &two_feed_guide("v1"), &feeds, 0).unwrap();
    // Overlapping [now - 30 min, now + 90 min): X-1 (ends at now) is out.
    let w = EpgWindow {
        from: NOW - H / 2,
        to: NOW + H + H / 2,
        keys: None,
        descriptions: false,
    };
    let g = store.load_epg(a, &w).unwrap();
    assert_eq!(titles(&g, "k1", 0, i64::MAX), ["v1 X-1", "v1 X0", "v1 X1"]);
    let only = [ChannelKey::from("k2")];
    let g = store
        .load_epg(
            a,
            &EpgWindow {
                keys: Some(&only),
                ..w
            },
        )
        .unwrap();
    assert_eq!(g.bound_count(), 1);
    assert!(g.programme_at(&"k1".into(), NOW).is_none());
    assert_eq!(titles(&g, "k2", 0, i64::MAX), ["v1 Y-1", "v1 Y0", "v1 Y1"]);

    let lean = store.load_epg(a, &w).unwrap();
    assert_eq!(lean.programme_at(&"k1".into(), NOW).unwrap().desc(), None);
    let full = store
        .load_epg(
            a,
            &EpgWindow {
                descriptions: true,
                ..w
            },
        )
        .unwrap();
    assert_eq!(
        full.programme_at(&"k1".into(), NOW).unwrap().desc(),
        Some("About X")
    );
    assert_eq!(
        store
            .epg_description(a, &"k3".into(), NOW)
            .unwrap()
            .as_deref(),
        Some("About X")
    );
    assert_eq!(store.epg_description(a, &"k2".into(), NOW).unwrap(), None);
    assert_eq!(
        store.epg_description(a, &"k1".into(), NOW + 1).unwrap(),
        None
    );
}

#[test]
fn titles_stream_for_search_by_channel_position() {
    let (mut store, a, _) = store_with_sources();
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    staged::replace_epg(&mut store, a, &two_feed_guide("v1"), &feeds, 0).unwrap();
    // k4 has no stored guide; "other" is not bound at all.
    let keys = ["k2", "k3", "k1", "k4", "other"].map(ChannelKey::from);
    let mut seen = Vec::new();
    let read = store
        .for_each_epg_title(a, &keys, NOW - H / 2, NOW + H, |at, start, stop, title| {
            seen.push((at, start, stop, title.to_owned()));
        })
        .unwrap();
    // x (shared by k1 and k3) and y (settled to an hour each) overlapping
    // [now - 30 min, now + 1 h): four rows, x heard at both positions.
    assert_eq!(read, 4);
    seen.sort();
    let row = |at, i: i64, t: &str| (at, NOW + i * H, NOW + (i + 1) * H, format!("v1 {t}{i}"));
    assert_eq!(
        seen,
        [
            row(0, -1, "Y"),
            row(0, 0, "Y"),
            row(1, -1, "X"),
            row(1, 0, "X"),
            row(2, -1, "X"),
            row(2, 0, "X"),
        ]
    );
}

#[test]
fn replace_is_per_source_and_total() {
    let (mut store, a, b) = store_with_sources();
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    staged::replace_epg(&mut store, a, &two_feed_guide("old"), &feeds, 0).unwrap();
    staged::replace_epg(&mut store, b, &two_feed_guide("other"), &feeds, 0).unwrap();
    // A refresh with only the low feed: hi's rows and k3 (hi only) go.
    let counts =
        staged::replace_epg(&mut store, a, &two_feed_guide("new"), &feeds[1..], 1).unwrap();
    assert_eq!(
        counts.skipped_bindings, 2,
        "k1/k3 were bound to the hi feed"
    );
    let g = store.load_epg(a, &EpgWindow::from(NOW, 1)).unwrap();
    assert_eq!(g.bound_count(), 1);
    assert_eq!(titles(&g, "k2", NOW, NOW + 1), ["new Y0"]);
    assert_eq!(store.epg_guide(a).unwrap().unwrap().feeds.len(), 1);
    let other = store.load_epg(b, &EpgWindow::from(NOW, 1)).unwrap();
    assert_eq!(titles(&other, "k1", NOW, NOW + 1), ["other X0"]);

    // An unknown source is an error and writes nothing.
    assert!(
        staged::replace_epg(&mut store, SourceId(77), &two_feed_guide("x"), &feeds, 0).is_err()
    );
}

#[test]
fn failures_are_recorded_and_cleared() {
    let (mut store, a, _) = store_with_sources();
    store
        .record_epg_failure(a, FailureKind::Network.into())
        .unwrap();
    let s = store.epg_guide(a).unwrap().unwrap();
    assert_eq!(
        (s.replaced_at, s.attempted_at, s.last_error),
        (None, Some(NOW), Some(ImportErrorKind::Network))
    );
    assert!(s.refresh_state().last_attempt_failed);
    let feeds = [meta(config(1, 9))];
    staged::replace_epg(&mut store, a, &two_feed_guide("v1"), &feeds, 0).unwrap();
    store
        .record_epg_failure(a, FailureKind::Parse.into())
        .unwrap();
    let s = store.epg_guide(a).unwrap().unwrap();
    assert_eq!(s.last_error, Some(ImportErrorKind::InvalidBody));
    assert_eq!(s.replaced_at, Some(NOW), "stored guide kept");
    assert_eq!(
        store
            .load_epg(a, &EpgWindow::from(NOW, 1))
            .unwrap()
            .bound_count(),
        2
    );
    staged::replace_epg(&mut store, a, &two_feed_guide("v2"), &feeds, 0).unwrap();
    assert_eq!(store.epg_guide(a).unwrap().unwrap().last_error, None);
}

#[test]
fn prune_delete_and_cascade() {
    let (mut store, a, b) = store_with_sources();
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    staged::replace_epg(&mut store, a, &two_feed_guide("a"), &feeds, 0).unwrap();
    staged::replace_epg(&mut store, b, &two_feed_guide("b"), &feeds, 0).unwrap();
    // Ending at or before now: X-3..X-1 and Y-3..Y-1 (y's two-hour
    // programmes were cut at the next start), for both sources.
    assert_eq!(store.prune_epg(NOW).unwrap(), 12);
    let g = store.load_epg(a, &EpgWindow::from(NOW, 30)).unwrap();
    assert_eq!(titles(&g, "k1", 0, i64::MAX).len(), 6);
    assert_eq!(
        store.epg_description(a, &"k1".into(), NOW - H).unwrap(),
        None
    );
    assert!(
        store
            .epg_description(a, &"k1".into(), NOW)
            .unwrap()
            .is_some()
    );

    assert!(store.delete_epg(a).unwrap());
    assert!(!store.delete_epg(a).unwrap());
    assert_eq!(store.epg_guide(a).unwrap(), None);
    assert_eq!(
        store
            .load_epg(a, &EpgWindow::from(NOW, 1))
            .unwrap()
            .bound_count(),
        0
    );
    // Deleting the playlist source deletes its guide.
    assert!(store.delete_source(b).unwrap());
    assert_eq!(store.epg_guide(b).unwrap(), None);
}

/// Start, stop, title, description, season/episode.
type Row = (i64, i64, String, Option<String>, (Option<u16>, Option<u16>));

#[test]
fn every_field_survives_the_store() {
    let (mut store, a, _) = store_with_sources();
    let (guide, _) = epg_synth::import(
        Shape {
            channels: 40,
            days_back: 1,
            days_ahead: 1,
        },
        StoreOptions::default(),
    );
    let feeds = [meta(EpgSourceConfig::new(
        iptv_epg::FeedId(1),
        SecretUrl::new("http://epg.example.com/synthetic.xml.gz"),
    ))];
    let counts = staged::replace_epg(&mut store, a, &guide, &feeds, 0).unwrap();
    assert_eq!(counts.channels, 40);
    let back = store
        .load_epg(
            a,
            &EpgWindow {
                descriptions: true,
                ..EpgWindow::from(NOW, 30)
            },
        )
        .unwrap();
    for n in 0..40 {
        let key = epg_synth::key(n);
        let rows = |g: &GuideStore| -> Vec<Row> {
            g.programmes(&key, i64::MIN, i64::MAX)
                .map(|p| {
                    let d: ProgrammeData<'_> = p.data();
                    (
                        d.start,
                        d.stop.unwrap(),
                        d.title.to_owned(),
                        d.desc.map(str::to_owned),
                        (d.season, d.episode),
                    )
                })
                .collect()
        };
        assert_eq!(rows(&back), rows(&guide), "channel {n}");
    }
    assert!(counts.programmes > 40 * 40);
    // Retention: what the window leaves out.
    let recent = store.load_epg(a, &EpgWindow::from(NOW, 0)).unwrap();
    let first = recent
        .programmes(&epg_synth::key(0), i64::MIN, i64::MAX)
        .next()
        .unwrap();
    assert!(first.stop() > NOW && first.start() <= NOW);
}

#[test]
fn feed_rows_never_show_their_url_in_debug() {
    let (mut store, a, _) = store_with_sources();
    staged::replace_epg(
        &mut store,
        a,
        &two_feed_guide("v1"),
        &[meta(config(1, 9))],
        0,
    )
    .unwrap();
    let shown = format!("{:?}", store.epg_guide(a).unwrap());
    assert!(!shown.contains(common::XT_PASSWORD), "{shown}");
}

#[test]
fn the_first_programme_stored_bounds_the_guide() {
    let (mut store, a, b) = store_with_sources();
    assert_eq!(store.epg_first_start(a).unwrap(), None, "nothing stored");
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    staged::replace_epg(&mut store, a, &two_feed_guide("a"), &feeds, 0).unwrap();
    assert_eq!(store.epg_first_start(a).unwrap(), Some(NOW - 3 * H));
    assert_eq!(store.epg_first_start(b).unwrap(), None, "per source");
    // Pruned days go: the first programme left is now's.
    store.prune_epg(NOW).unwrap();
    assert_eq!(store.epg_first_start(a).unwrap(), Some(NOW));
}

#[test]
fn a_window_read_seeks_to_the_programme_reaching_into_it() {
    let (mut store, a, _) = store_with_sources();
    let mut b = SourceBuilder::new(iptv_epg::FeedId(1), 0, StoreOptions::default(), NOW);
    b.push_programme(prog("x", NOW - 50 * H, NOW - 40 * H, "Old", None));
    b.push_programme(prog("x", NOW - 40 * H, NOW - 30 * H, "Older", None));
    // A day and more on air: it started long before the windows below.
    b.push_programme(prog("x", NOW - 30 * H, NOW + 2 * H, "Long", None));
    b.push_programme(prog("x", NOW + 2 * H, NOW + 3 * H, "After", None));
    let mut g = GuideStore::new();
    g.replace_source(b.finish());
    let cand = EpgRef {
        source: iptv_epg::FeedId(1),
        epg_id: "x".into(),
        kind: MatchKind::TvgId,
    };
    g.set_matches([(ChannelKey::from("k"), vec![cand])].into_iter().collect());
    staged::replace_epg(&mut store, a, &g, &[meta(config(1, 0))], 0).unwrap();
    let read = |from: i64, to: i64| {
        let w = EpgWindow {
            from,
            to,
            keys: None,
            descriptions: false,
        };
        titles(&store.load_epg(a, &w).unwrap(), "k", i64::MIN, i64::MAX)
    };
    assert_eq!(read(NOW - H, i64::MAX), ["Long", "After"]);
    assert_eq!(read(NOW, NOW + H), ["Long"]);
    // From exactly a programme's start, and between two.
    assert_eq!(read(NOW + 2 * H, i64::MAX), ["After"]);
    assert_eq!(read(NOW - 40 * H, NOW - 35 * H), ["Older"]);
    assert_eq!(read(NOW - 45 * H, NOW - 39 * H), ["Old", "Older"]);
    // Before anything stored, and after everything.
    assert_eq!(read(NOW - 60 * H, NOW - 55 * H), Vec::<String>::new());
    assert_eq!(read(NOW + 4 * H, i64::MAX), Vec::<String>::new());
}

#[test]
fn title_iteration_keeps_one_snapshot_across_concurrent_guide_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.db");
    let mut writer = Store::open(&path).unwrap();
    let source = writer
        .add_source(&common::xtream_source("Synthetic"))
        .unwrap();
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    staged::replace_epg(&mut writer, source, &two_feed_guide("old"), &feeds, 0).unwrap();
    let reader = writer.open_reader().unwrap();
    let mut replaced = false;
    let mut observed = Vec::new();
    reader
        .for_each_epg_title(
            source,
            &["k1".into(), "k2".into()],
            i64::MIN,
            i64::MAX,
            |_, _, _, title| {
                observed.push(title.to_owned());
                if !replaced {
                    replaced = true;
                    staged::replace_epg(&mut writer, source, &two_feed_guide("new"), &feeds, 1)
                        .unwrap();
                }
            },
        )
        .unwrap();
    assert!(replaced);
    assert!(observed.iter().any(|title| title.starts_with("old Y")));
    assert!(
        observed.iter().all(|title| title.starts_with("old")),
        "{observed:?}"
    );
}

#[test]
fn nested_epg_snapshots_share_the_caller_and_recover_after_read_errors() {
    let (mut store, source, _) = store_with_sources();
    let feeds = [meta(config(1, 9)), meta(config(2, 1))];
    staged::replace_epg(&mut store, source, &two_feed_guide("old"), &feeds, 0).unwrap();
    let keys = [ChannelKey::from("k1"), ChannelKey::from("k2")];
    store
        .read_snapshot(|outer| {
            outer.vod_read_snapshot(|vod| {
                vod.guide_repair_snapshot(|nested| {
                    let metadata = nested.epg_guide(source)?.unwrap();
                    assert_eq!(metadata.feeds.len(), 2);
                    let guide = nested.load_epg(source, &EpgWindow::from(NOW, 1))?;
                    assert!(!titles(&guide, "k1", NOW, NOW + H).is_empty());
                    let mut titles_seen = 0;
                    nested.for_each_epg_title(source, &keys, NOW, NOW + H, |_, _, _, title| {
                        assert!(title.starts_with("old"));
                        titles_seen += 1;
                    })?;
                    assert!(titles_seen >= 2);
                    // A handled nested error must not roll back the outer
                    // snapshot or leave it unable to continue reading.
                    let failed: iptv_store_sqlite::Result<()> = nested.read_snapshot(|_| {
                        Err(iptv_store_sqlite::StoreError::Unavailable(
                            "synthetic nested read",
                        ))
                    });
                    assert!(failed.is_err());
                    assert_eq!(nested.epg_guide(source)?.unwrap().feeds.len(), 2);
                    Ok(())
                })
            })
        })
        .unwrap();
    let failed: iptv_store_sqlite::Result<()> = store.read_snapshot(|snapshot| {
        snapshot.epg_guide(source)?;
        Err(iptv_store_sqlite::StoreError::Unavailable(
            "synthetic outer read",
        ))
    });
    assert!(failed.is_err());
    assert!(store.epg_guide(source).unwrap().is_some());
    // A subsequent write transaction can start only if both successful and
    // failed owned read snapshots released their transaction guards.
    let catalogue = common::catalog(1, 1, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalogue);
    store
        .set_favourite(source, &catalogue.channels[0].id, true)
        .unwrap();
    assert!(
        store
            .is_favourite(source, &catalogue.channels[0].id)
            .unwrap()
    );
}
