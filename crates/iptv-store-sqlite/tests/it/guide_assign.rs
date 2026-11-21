//! Schema v6 guide data: the channels each stored feed lists, how each
//! feed's last download went, and a channel's guide chosen by hand.

use crate::common;
use crate::staged;

use iptv_epg::{
    ChannelKey, EpgRef, GuideStore, MatchKind, SourceBuilder, StoreOptions, XmltvChannel,
    XmltvProgram,
};
use iptv_import::EpgSourceConfig;
use iptv_providers::{ContentId, SecretUrl};
use iptv_store_sqlite::{
    EpgFeedMeta, EpgOverride, EpgWindow, FeedResult, GuideChannel, ImportErrorKind, SourceId, Store,
};

const NOW: i64 = 1_800_000_000;
const H: i64 = 3600;
const FEED: iptv_epg::FeedId = iptv_epg::FeedId(2);

fn store() -> (Store, SourceId) {
    let mut store = Store::open_in_memory().unwrap();
    store.set_clock(|| NOW);
    let a = store.add_source(&common::xtream_source("A")).unwrap();
    (store, a)
}

/// One feed listing three channels; `news` and `film` have programs,
/// `quiet` none. Playlist channel `k1` is bound to `news`.
fn guide() -> GuideStore {
    let mut b = SourceBuilder::new(FEED, 1, StoreOptions::default(), NOW);
    for (id, name) in [
        ("news.test", "Synthetic News"),
        ("film.test", "Synthetic Films"),
        ("quiet.test", "Quiet One"),
    ] {
        b.push_channel(XmltvChannel {
            id: id.into(),
            display_names: vec![name.into()],
            icon: None,
        });
    }
    for (channel, title) in [("news.test", "Bulletin"), ("film.test", "Feature")] {
        b.push_program(XmltvProgram {
            channel: channel.into(),
            start: NOW,
            stop: Some(NOW + H),
            title: title.into(),
            ..XmltvProgram::default()
        });
    }
    let mut g = GuideStore::new();
    g.replace_source(b.finish());
    let news = EpgRef {
        source: FEED,
        epg_id: "news.test".into(),
        kind: MatchKind::TvgId,
    };
    g.set_matches([(ChannelKey::from("k1"), vec![news])].into_iter().collect());
    g
}

fn feeds() -> Vec<EpgFeedMeta> {
    vec![EpgFeedMeta {
        config: EpgSourceConfig::new(FEED, SecretUrl::new("http://epg.example.test/g.xml")),
        fetched_at: NOW,
        etag: None,
        last_modified: None,
        bytes: 10,
    }]
}

fn title(store: &Store, source: SourceId, key: &str) -> Option<String> {
    let g = store.load_epg(source, &EpgWindow::from(NOW, 1)).unwrap();
    g.program_at(&key.into(), NOW).map(|p| p.title().to_owned())
}

#[test]
fn every_listed_channel_is_kept_with_the_guide() {
    let (mut store, a) = store();
    staged::replace_epg(&mut store, a, &guide(), &feeds(), 1).unwrap();
    let channel = |id: &str, name: &str, assigned| GuideChannel {
        feed: FEED,
        epg_channel: id.into(),
        name: Some(name.into()),
        assigned,
    };
    assert_eq!(
        store.guide_channels(a).unwrap(),
        [
            channel("quiet.test", "Quiet One", false),
            channel("film.test", "Synthetic Films", false),
            channel("news.test", "Synthetic News", true),
        ]
    );
    store.delete_epg(a).unwrap();
    assert_eq!(store.guide_channels(a).unwrap(), []);
}

#[test]
fn each_feed_keeps_how_its_last_download_went() {
    let (mut store, a) = store();
    assert_eq!(store.epg_feed_results(a).unwrap(), []);
    staged::replace_epg(&mut store, a, &guide(), &feeds(), 1).unwrap();
    let other = iptv_epg::FeedId(1001);
    store
        .record_epg_feed_results(a, &[(FEED, None), (other, Some(ImportErrorKind::Network))])
        .unwrap();
    assert_eq!(
        store.epg_feed_results(a).unwrap(),
        [
            FeedResult {
                feed: FEED,
                attempted_at: NOW,
                error: None,
                channel_count: 1,
                program_count: 1,
                listed_count: 3,
            },
            FeedResult {
                feed: other,
                attempted_at: NOW,
                error: Some(ImportErrorKind::Network),
                channel_count: 0,
                program_count: 0,
                listed_count: 0,
            },
        ]
    );
    // A source without a stored guide records its failures too.
    let b = store.add_source(&common::m3u_source("B")).unwrap();
    store
        .record_epg_feed_results(b, &[(FEED, Some(ImportErrorKind::HttpStatus))])
        .unwrap();
    assert_eq!(store.epg_feed_results(b).unwrap().len(), 1);
    assert_eq!(store.epg_guide(b).unwrap().unwrap().replaced_at, None);
    // A new download forgets the old results; clearing forgets them all.
    staged::replace_epg(&mut store, a, &guide(), &feeds(), 1).unwrap();
    assert_eq!(store.epg_feed_results(a).unwrap(), []);
    store.record_epg_feed_results(a, &[(FEED, None)]).unwrap();
    store.delete_epg(a).unwrap();
    assert_eq!(store.epg_feed_results(a).unwrap(), []);
}

#[test]
fn a_chosen_guide_applies_to_the_stored_guide_when_its_programs_are_there() {
    let (mut store, a) = store();
    assert!(
        !store.rebind_epg(a, &"k1".into(), None, 5).unwrap(),
        "no guide"
    );
    staged::replace_epg(&mut store, a, &guide(), &feeds(), 1).unwrap();
    assert_eq!(title(&store, a, "k1").as_deref(), Some("Bulletin"));
    // Another playlist channel onto the stored news channel: at once.
    assert!(
        store
            .rebind_epg(a, &"k2".into(), Some((FEED, "news.test")), 7)
            .unwrap()
    );
    assert_eq!(title(&store, a, "k2").as_deref(), Some("Bulletin"));
    assert_eq!(
        store.epg_binding(a, &"k2".into()).unwrap(),
        Some((FEED, "news.test".into(), MatchKind::Override))
    );
    assert_eq!(
        store.epg_binding(a, &"k1".into()).unwrap(),
        Some((FEED, "news.test".into(), MatchKind::TvgId))
    );
    let stored = store.epg_guide(a).unwrap().unwrap();
    assert_eq!(
        (stored.bound_channels, stored.match_fingerprint),
        (2, Some(7))
    );
    // Films' programs were not stored (nothing was bound to it): needs a
    // download, nothing changes.
    assert!(
        !store
            .rebind_epg(a, &"k1".into(), Some((FEED, "film.test")), 8)
            .unwrap()
    );
    assert_eq!(title(&store, a, "k1").as_deref(), Some("Bulletin"));
    assert_eq!(
        store.epg_guide(a).unwrap().unwrap().match_fingerprint,
        Some(7)
    );
    // No guide data: at once.
    assert!(store.rebind_epg(a, &"k1".into(), None, 9).unwrap());
    assert_eq!(title(&store, a, "k1"), None);
    assert_eq!(store.epg_binding(a, &"k1".into()).unwrap(), None);
    assert_eq!(store.epg_guide(a).unwrap().unwrap().bound_channels, 1);
}

#[test]
fn manual_choices_are_read_back_for_matching() {
    let (mut store, a) = store();
    let (one, two) = (ContentId::from_stored("c1"), ContentId::from_stored("c2"));
    store
        .set_epg_override(a, &one, Some(&EpgOverride::for_feed(FEED, "film.test")))
        .unwrap();
    store
        .set_epg_override(a, &two, Some(&EpgOverride::Disabled))
        .unwrap();
    store.set_favorite(a, &one, true).unwrap();
    let read = store.epg_overrides(a).unwrap();
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].1.feed(), Some(FEED));
    assert_eq!(
        read[0].1.to_override(),
        iptv_epg::Override::Epg {
            source: Some(FEED),
            epg_id: "film.test".into()
        }
    );
    assert_eq!(read[1].1.to_override(), iptv_epg::Override::NoEpg);
    // A key that names no feed matches the id in any feed.
    let any = EpgOverride::Xmltv {
        epg_source: "elsewhere".into(),
        epg_channel: "x".into(),
    };
    assert_eq!(
        any.to_override(),
        iptv_epg::Override::Epg {
            source: None,
            epg_id: "x".into()
        }
    );
    store.set_epg_override(a, &one, None).unwrap();
    assert_eq!(store.epg_overrides(a).unwrap().len(), 1);
}
