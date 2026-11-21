//! User overlays: survive refreshes, merge at read time, orphans return.

use crate::common;

use common::*;
use iptv_providers::ContentId;
use iptv_providers::model::StreamLocator;
use iptv_store_sqlite::{ChannelOrder, ChannelQuery, ChannelRow, ChannelScope, SourceId, Store};

fn setup(n: usize, groups: usize) -> (Store, SourceId, Catalog) {
    let mut s = Store::open_in_memory().unwrap();
    let src = s.add_source(&m3u_source("List")).unwrap();
    let cat = catalog(n, groups, XT_PASSWORD);
    replace(&mut s, src, &cat);
    (s, src, cat)
}

fn ids(rows: &[ChannelRow]) -> Vec<&str> {
    rows.iter().map(|r| r.channel.id.as_str()).collect()
}

fn names(rows: &[ChannelRow]) -> Vec<&str> {
    rows.iter().map(|r| r.name.as_str()).collect()
}

fn favorites() -> ChannelQuery {
    ChannelQuery {
        favorites_only: true,
        ..Default::default()
    }
}

fn everything() -> ChannelQuery {
    ChannelQuery {
        include_hidden: true,
        ..Default::default()
    }
}

#[test]
fn overlays_survive_a_refresh_that_rotates_url_credentials() {
    let (mut s, src, cat) = setup(10, 2);
    let c = |i: usize| cat.channels[i].id.clone();
    s.set_favorite(src, &c(4), true).unwrap();
    s.set_favorite(src, &c(1), true).unwrap();
    s.rename_channel(src, &c(2), Some("  My Channel  "))
        .unwrap();
    s.renumber_channel(src, &c(3), Some(500)).unwrap();
    s.set_hidden(src, &c(5), true).unwrap();
    s.set_blocked(src, &c(6), true).unwrap();

    // The provider rotated the password embedded in every stream URL; the
    // stable ids do not depend on it.
    let rotated = catalog(10, 2, "rotated-pass-0b9e");
    assert_eq!(rotated.channels[2].id, cat.channels[2].id);
    assert_ne!(rotated.channels[2].stream, cat.channels[2].stream);
    replace(&mut s, src, &rotated);

    let all = s.load_channels(src, &everything()).unwrap();
    let row = |i: usize| all.iter().find(|r| r.channel.id == c(i)).unwrap();
    assert_eq!(row(2).name, "My Channel");
    assert_eq!(row(2).channel.name, "Channel 00002");
    assert_eq!(row(3).number, Some(500));
    assert_eq!(row(3).channel.number, Some(4));
    assert!(row(5).hidden);
    assert!(row(6).blocked);
    match &row(2).channel.stream {
        StreamLocator::Url { url, .. } => assert!(url.contains("rotated-pass-0b9e")),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        ids(&s.load_channels(src, &favorites()).unwrap()),
        [c(4).as_str(), c(1).as_str()]
    );
    assert!(s.orphaned_overlays(None).unwrap().is_empty());
}

#[test]
fn orphaned_overlays_are_kept_reported_and_reapplied() {
    let (mut s, src, cat) = setup(6, 1);
    let gone = cat.channels[2].id.clone();
    s.set_favorite(src, &cat.channels[0].id, true).unwrap();
    s.set_favorite(src, &gone, true).unwrap();
    s.set_favorite(src, &cat.channels[4].id, true).unwrap();
    s.rename_channel(src, &gone, Some("Kept Name")).unwrap();

    let mut without = catalog(6, 1, XT_PASSWORD);
    without.channels.remove(2);
    replace(&mut s, src, &without);

    let orphans = s.orphaned_overlays(Some(src)).unwrap();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].channel_id, gone);
    assert!(orphans[0].favorite && orphans[0].customized);
    assert_eq!(s.load_channels(src, &favorites()).unwrap().len(), 2);
    assert_eq!(s.channel_count(src, &favorites()).unwrap(), 2);

    // Reordering while it is gone keeps its slot relative to neighbours.
    s.move_favorite(src, &cat.channels[4].id, 0).unwrap();

    replace(&mut s, src, &cat);
    assert!(s.orphaned_overlays(Some(src)).unwrap().is_empty());
    let back = s.channel(src, &gone).unwrap().unwrap();
    assert_eq!(back.name, "Kept Name");
    assert_eq!(
        ids(&s.load_channels(src, &favorites()).unwrap()),
        [
            cat.channels[4].id.as_str(),
            cat.channels[0].id.as_str(),
            gone.as_str()
        ]
    );

    replace(&mut s, src, &without);
    assert_eq!(s.purge_orphaned_overlays(src).unwrap(), 2);
    replace(&mut s, src, &cat);
    let back = s.channel(src, &gone).unwrap().unwrap();
    assert_eq!(back.name, "Channel 00002");
    assert!(!back.favorite);
}

#[test]
fn favorites_toggle_and_reorder() {
    let (mut s, src, cat) = setup(5, 1);
    let c = |i: usize| cat.channels[i].id.clone();
    assert!(s.toggle_favorite(src, &c(3)).unwrap());
    assert!(s.toggle_favorite(src, &c(0)).unwrap());
    assert!(s.toggle_favorite(src, &c(2)).unwrap());
    s.set_favorite(src, &c(3), true).unwrap(); // idempotent, keeps its place
    assert_eq!(
        ids(&s.load_channels(src, &favorites()).unwrap()),
        [c(3).as_str(), c(0).as_str(), c(2).as_str()]
    );
    assert!(s.move_favorite(src, &c(2), 0).unwrap());
    assert!(s.move_favorite(src, &c(3), 9).unwrap());
    assert!(!s.move_favorite(src, &c(4), 0).unwrap());
    assert_eq!(
        ids(&s.load_channels(src, &favorites()).unwrap()),
        [c(2).as_str(), c(0).as_str(), c(3).as_str()]
    );
    assert!(!s.toggle_favorite(src, &c(0)).unwrap());
    assert!(!s.is_favorite(src, &c(0)).unwrap());
    assert_eq!(s.channel_count(src, &favorites()).unwrap(), 2);
}

#[test]
fn filters_orders_and_paging() {
    let (mut s, src, cat) = setup(12, 3);
    let c = |i: usize| cat.channels[i].id.clone();
    s.set_hidden(src, &c(0), true).unwrap();
    s.set_blocked(src, &c(1), true).unwrap();
    s.rename_channel(src, &c(11), Some("aaa first")).unwrap();
    s.renumber_channel(src, &c(10), Some(0)).unwrap();

    let visible = s.load_channels(src, &ChannelQuery::default()).unwrap();
    assert_eq!(visible.len(), 11);
    assert!(!ids(&visible).contains(&c(0).as_str()));
    assert!(visible.iter().any(|r| r.blocked));
    let unblocked = ChannelQuery {
        exclude_blocked: true,
        ..Default::default()
    };
    assert_eq!(s.channel_count(src, &unblocked).unwrap(), 10);
    assert_eq!(s.channel_count(src, &everything()).unwrap(), 12);

    let by_name = s
        .load_channels(
            src,
            &ChannelQuery {
                order: ChannelOrder::Name,
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(names(&by_name), ["aaa first", "Channel 00001"]);
    let by_number = s
        .load_channels(
            src,
            &ChannelQuery {
                order: ChannelOrder::Number,
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(by_number[0].number, Some(0));
    assert_eq!(by_number[1].number, Some(2));

    // Group + favourites + paging compose; pages tile the full list.
    let g = ChannelScope::Group(group(1).id);
    let in_group = ChannelQuery {
        scope: g.clone(),
        ..Default::default()
    };
    let full = s.load_channels(src, &in_group).unwrap();
    assert_eq!(
        ids(&full),
        [c(1).as_str(), c(4).as_str(), c(7).as_str(), c(10).as_str()]
    );
    let mut paged = Vec::new();
    for page in 0..3 {
        paged.extend(
            s.load_channels(
                src,
                &ChannelQuery {
                    offset: page * 2,
                    limit: Some(2),
                    ..in_group.clone()
                },
            )
            .unwrap(),
        );
    }
    assert_eq!(paged, full);
    s.set_favorite(src, &c(7), true).unwrap();
    s.set_favorite(src, &c(2), true).unwrap();
    let fav_in_group = ChannelQuery {
        scope: g,
        favorites_only: true,
        ..Default::default()
    };
    assert_eq!(
        ids(&s.load_channels(src, &fav_in_group).unwrap()),
        [c(7).as_str()]
    );
}

#[test]
fn restoring_defaults_removes_the_overlay_row() {
    let (mut s, src, cat) = setup(3, 1);
    let id = cat.channels[1].id.clone();
    s.rename_channel(src, &id, Some("Temp")).unwrap();
    s.set_hidden(src, &id, true).unwrap();
    s.rename_channel(src, &id, None).unwrap();
    s.set_hidden(src, &id, false).unwrap();
    let row = s.channel(src, &id).unwrap().unwrap();
    assert_eq!(
        (row.name.as_str(), row.custom_name.as_deref()),
        ("Channel 00001", None)
    );
    // Nothing left to orphan once the channel disappears.
    replace(&mut s, src, &catalog(1, 1, XT_PASSWORD));
    assert!(s.orphaned_overlays(Some(src)).unwrap().is_empty());
    // A blank rename restores too.
    replace(&mut s, src, &cat);
    s.rename_channel(src, &id, Some("   ")).unwrap();
    assert_eq!(s.channel(src, &id).unwrap().unwrap().custom_name, None);
}

#[test]
fn search_uses_custom_and_provider_names() {
    let (mut s, src, cat) = setup(4, 1);
    let id = cat.channels[3].id.clone();
    s.rename_channel(src, &id, Some("Ocean Live")).unwrap();
    for text in ["ocean", "channel 00003"] {
        let hits = s.search_channels(Some(src), text, 10).unwrap();
        assert_eq!(hits.len(), 1, "{text}");
        assert_eq!(hits[0].channel.id, id);
        assert_eq!(hits[0].name, "Ocean Live");
    }
    s.set_hidden(src, &id, true).unwrap();
    assert!(
        s.search_channels(Some(src), "ocean", 10)
            .unwrap()
            .is_empty()
    );
    // The index follows the next refresh too.
    replace(&mut s, src, &cat);
    s.set_hidden(src, &id, false).unwrap();
    assert_eq!(s.search_channels(Some(src), "ocean", 10).unwrap().len(), 1);
}

#[test]
fn overlays_need_an_existing_source_but_not_a_present_channel() {
    let (mut s, src, _) = setup(1, 1);
    let future = ContentId::from_stored("m3u:not-imported-yet");
    s.set_favorite(src, &future, true).unwrap();
    assert_eq!(s.orphaned_overlays(Some(src)).unwrap().len(), 1);
    assert!(s.set_favorite(SourceId(42), &future, true).is_err());
    assert!(s.set_hidden(SourceId(42), &future, true).is_err());
}
