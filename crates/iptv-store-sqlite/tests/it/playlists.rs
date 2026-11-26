//! What the playlists screen keeps: the shown source, each source's guide
//! sources, and what deleting a source takes with it.

use crate::common;

use common::*;
use iptv_providers::SecretUrl;
use iptv_store_sqlite::{ChannelQuery, GuideSource, SourceId, Store, StoreError};

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.set_clock(|| 1_800_000_000);
    s
}

fn typed() -> SecretUrl {
    SecretUrl::new("http://guide.example.test/xmltv.php?username=u&password=synthetic-pass-7f3a")
}

#[test]
fn the_shown_source_is_remembered_while_it_exists() {
    let mut s = store();
    assert_eq!(s.active_source().unwrap(), None);
    let a = s.add_source(&m3u_source("A")).unwrap();
    let b = s.add_source(&xtream_source("B")).unwrap();
    s.set_active_source(b).unwrap();
    assert_eq!(s.active_source().unwrap(), Some(b));
    s.set_active_source(a).unwrap();
    assert_eq!(s.active_source().unwrap(), Some(a));
    assert!(matches!(
        s.set_active_source(SourceId(99)),
        Err(StoreError::SourceNotFound(_))
    ));
    // Deleting another source keeps the choice; deleting it forgets it.
    s.delete_source(b).unwrap();
    assert_eq!(s.active_source().unwrap(), Some(a));
    s.delete_source(a).unwrap();
    assert_eq!(s.active_source().unwrap(), None);
    // A new source is not shown until chosen.
    let c = s.add_source(&m3u_source("C")).unwrap();
    assert_eq!(s.active_source().unwrap(), None);
    s.set_active_source(c).unwrap();
    assert_eq!(s.active_source().unwrap(), Some(c));
}

#[test]
fn sources_keep_the_order_they_are_given() {
    let mut s = store();
    let a = s.add_source(&m3u_source("A")).unwrap();
    let b = s.add_source(&xtream_source("B")).unwrap();
    let c = s.add_source(&m3u_source("C")).unwrap();
    let order = |s: &Store| {
        s.sources()
            .unwrap()
            .iter()
            .map(|x| x.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&s), [a, b, c]);
    s.set_source_order(&[c, a, b]).unwrap();
    assert_eq!(order(&s), [c, a, b]);
    // Unknown ids are ignored; sources left out follow in their order.
    s.set_source_order(&[b, SourceId(99)]).unwrap();
    assert_eq!(order(&s), [b, c, a]);
    // A new source goes last.
    let d = s.add_source(&m3u_source("D")).unwrap();
    assert_eq!(order(&s), [b, c, a, d]);
}

fn guides() -> Vec<GuideSource> {
    vec![
        GuideSource {
            feed: iptv_epg::FeedId(1),
            provider_index: None,
            url: typed(),
            name: Some("Typed".into()),
            enabled: true,
            shift_minutes: -120,
        },
        GuideSource {
            feed: iptv_epg::FeedId(2),
            provider_index: Some(0),
            url: SecretUrl::new("http://panel.example.test/xmltv.php?password=p"),
            name: None,
            enabled: false,
            shift_minutes: 0,
        },
    ]
}

#[test]
fn guide_sources_are_kept_in_order_and_replaced_whole() {
    let mut s = store();
    let a = s.add_source(&xtream_source("A")).unwrap();
    // Never arranged: the app arranges them at the next import.
    assert_eq!(s.guide_sources(a).unwrap(), []);
    s.set_guide_sources(a, &guides()).unwrap();
    assert_eq!(s.guide_sources(a).unwrap(), guides());
    // Debug never shows the addresses' credentials.
    assert!(!format!("{:?}", s.guide_sources(a).unwrap()).contains(XT_PASSWORD));
    let mut reordered = guides();
    reordered.reverse();
    reordered[0].enabled = true;
    s.set_guide_sources(a, &reordered).unwrap();
    assert_eq!(s.guide_sources(a).unwrap(), reordered);
    s.set_guide_sources(a, &[]).unwrap();
    assert_eq!(s.guide_sources(a).unwrap(), []);
    assert!(matches!(
        s.guide_sources(SourceId(42)),
        Err(StoreError::SourceNotFound(_))
    ));
    assert!(matches!(
        s.set_guide_sources(SourceId(42), &guides()),
        Err(StoreError::SourceNotFound(_))
    ));
}

#[test]
fn editing_a_source_keeps_its_settings() {
    let mut s = store();
    let a = s.add_source(&xtream_source("A")).unwrap();
    s.set_guide_sources(a, &guides()).unwrap();
    s.rename_source(a, "Renamed").unwrap();
    s.update_source(a, &xtream_source("Again")).unwrap();
    assert_eq!(s.guide_sources(a).unwrap(), guides());
    assert_eq!(s.sources().unwrap()[0].name, "Again");
}

#[test]
fn deleting_a_source_takes_its_data_and_leaves_the_others() {
    let mut s = store();
    let a = s.add_source(&m3u_source("A")).unwrap();
    let b = s.add_source(&m3u_source("B")).unwrap();
    for id in [a, b] {
        replace(&mut s, id, &catalog(4, 2, "x"));
        s.set_guide_sources(id, &guides()).unwrap();
        let first = s.load_channels(id, &ChannelQuery::default()).unwrap()[0]
            .channel
            .id
            .clone();
        s.set_favorite(id, &first, true).unwrap();
    }
    assert!(s.delete_source(a).unwrap());
    assert_eq!(s.source(a).unwrap(), None);
    assert!(s.guide_sources(a).is_err());
    assert!(
        s.load_channels(a, &ChannelQuery::default())
            .unwrap()
            .is_empty()
    );
    let only = s.sources().unwrap();
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].id, b);
    assert_eq!(
        s.load_channels(b, &ChannelQuery::default()).unwrap().len(),
        4
    );
    let favorites = ChannelQuery {
        favorites_only: true,
        ..ChannelQuery::default()
    };
    assert_eq!(s.load_channels(b, &favorites).unwrap().len(), 1);
    assert_eq!(s.guide_sources(b).unwrap(), guides());
}
