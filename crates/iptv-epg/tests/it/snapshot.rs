//! Guide snapshots: the bound guide round-trips through bytes.

use std::collections::HashMap;

use iptv_epg::*;

const H: i64 = 3600;
const T0: i64 = 1_772_323_200; // 2026-03-01T00:00:00Z

fn guide() -> GuideStore {
    let mut high = SourceBuilder::new(FeedId(1), 10, StoreOptions::default(), T0);
    high.push_program(XmltvProgram {
        channel: "one.ex".into(),
        start: T0,
        stop: Some(T0 + H),
        title: "News".into(),
        sub_title: Some("Morning".into()),
        desc: Some("The day so far.".into()),
        categories: vec!["Current affairs".into()],
        episode_num: vec![EpisodeNum {
            system: Some("onscreen".into()),
            value: "S02E03".into(),
        }],
        ..XmltvProgram::default()
    });
    high.push_program(XmltvProgram {
        channel: "one.ex".into(),
        start: T0 + H,
        stop: None,
        title: "Weather".into(),
        ..XmltvProgram::default()
    });
    high.push_program(XmltvProgram {
        channel: "unbound.ex".into(),
        start: T0,
        stop: Some(T0 + H),
        title: "Not kept".into(),
        ..XmltvProgram::default()
    });
    let mut low = SourceBuilder::new(FeedId(2), 0, StoreOptions::default(), T0);
    low.push_program(XmltvProgram {
        channel: "two.ex".into(),
        start: T0,
        stop: Some(T0 + 2 * H),
        title: "Film".into(),
        ..XmltvProgram::default()
    });
    let mut g = GuideStore::new();
    g.replace_source(high.finish());
    g.replace_source(low.finish());
    let r = |source: u32, id: &str, kind| EpgRef {
        source: FeedId(source),
        epg_id: id.into(),
        kind,
    };
    g.set_matches(HashMap::from([
        (
            ChannelKey::new("m3u:a"),
            vec![r(1, "one.ex", MatchKind::TvgId)],
        ),
        (
            ChannelKey::new("m3u:b"),
            vec![r(2, "two.ex", MatchKind::Name)],
        ),
        (
            ChannelKey::new("m3u:c"),
            vec![r(1, "missing.ex", MatchKind::TvgId)],
        ),
    ]));
    g
}

/// Start, stop, title, sub-title, description, season and episode.
type Row = (
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
    (Option<u16>, Option<u16>),
);

fn programs(g: &GuideStore, key: &str) -> Vec<Row> {
    g.schedule(&ChannelKey::new(key))
        .map(|s| {
            s.iter()
                .map(|p| {
                    (
                        p.start(),
                        p.stop(),
                        p.title().to_owned(),
                        p.sub_title().map(str::to_owned),
                        p.desc().map(str::to_owned),
                        p.season_episode(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_bound_guide_round_trips() {
    let g = guide();
    let bytes = g.to_snapshot();
    assert_eq!(bytes, g.to_snapshot(), "deterministic");
    let back = GuideStore::from_snapshot(&bytes, StoreOptions::default(), T0).unwrap();
    assert_eq!(back.bound_count(), g.bound_count());
    for key in ["m3u:a", "m3u:b"] {
        assert_eq!(programs(&back, key), programs(&g, key), "{key}");
    }
    assert_eq!(programs(&back, "m3u:a")[0].5, (Some(2), Some(3)));
    assert_eq!(
        back.binding(&ChannelKey::new("m3u:b")),
        Some((FeedId(2), "two.ex"))
    );
    // Only bound channels are kept.
    assert!(
        back.source(FeedId(1))
            .unwrap()
            .schedule("unbound.ex")
            .is_none()
    );
    assert_eq!(
        back.to_snapshot(),
        bytes,
        "a restored guide saves the same bytes"
    );
}

#[test]
fn descriptions_and_old_programs_follow_the_store_options() {
    let bytes = guide().to_snapshot();
    let opts = StoreOptions {
        store_descriptions: false,
        past_days_to_keep: Some(1),
    };
    let back = GuideStore::from_snapshot(&bytes, opts, T0 + 3 * DAY).unwrap();
    assert!(programs(&back, "m3u:a").is_empty(), "older than a day");
    let back = GuideStore::from_snapshot(
        &bytes,
        StoreOptions {
            store_descriptions: false,
            ..opts
        },
        T0,
    )
    .unwrap();
    assert_eq!(programs(&back, "m3u:a")[0].4, None);
}

#[test]
fn other_versions_and_damaged_bytes_are_rejected() {
    let bytes = guide().to_snapshot();
    let opts = StoreOptions::default();
    assert_eq!(
        GuideStore::from_snapshot(b"", opts, T0).err(),
        Some(SnapshotError::Version)
    );
    let mut other = bytes.clone();
    other[3] = SNAPSHOT_VERSION + 1;
    assert_eq!(
        GuideStore::from_snapshot(&other, opts, T0).err(),
        Some(SnapshotError::Version)
    );
    for cut in [5, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            matches!(
                GuideStore::from_snapshot(&bytes[..cut], opts, T0),
                Err(SnapshotError::Corrupt(_))
            ),
            "{cut}"
        );
    }
    let mut long = bytes.clone();
    long.push(0);
    assert!(matches!(
        GuideStore::from_snapshot(&long, opts, T0),
        Err(SnapshotError::Corrupt(_))
    ));
    // A huge count fails fast instead of allocating.
    let mut huge = bytes;
    huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        GuideStore::from_snapshot(&huge, opts, T0),
        Err(SnapshotError::Corrupt(4))
    ));
}
