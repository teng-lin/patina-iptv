//! Atomic custom membership and independent ordering, using generated data.
use crate::common;
use common::*;
use iptv_providers::ContentId;
use iptv_store_sqlite::{ChannelQuery, ChannelScope, OrderScope, Store};
fn names(s: &Store, source: iptv_store_sqlite::SourceId, scope: ChannelScope) -> Vec<String> {
    s.load_channels(
        source,
        &ChannelQuery {
            scope,
            include_hidden: true,
            ..Default::default()
        },
    )
    .unwrap()
    .into_iter()
    .map(|r| r.channel.id.as_str().to_owned())
    .collect()
}
#[test]
fn bulk_copy_move_is_atomic_deduplicated_source_scoped_and_keeps_channels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite");
    let mut s = Store::open(&path).unwrap();
    let src = s.add_source(&m3u_source("Generated")).unwrap();
    let other = s.add_source(&m3u_source("Other")).unwrap();
    let cat = catalog(6, 2, XT_PASSWORD);
    replace(&mut s, src, &cat);
    let provider_group_count = s.groups(src, None).unwrap().len();
    let a = s.create_custom_group(src, "A").unwrap();
    let b = s.create_custom_group(src, "B").unwrap();
    let foreign = s.create_custom_group(other, "Foreign").unwrap();
    let id = |i: usize| cat.channels[i].id.clone();
    let ids = |indices: &[usize]| {
        indices
            .iter()
            .map(|&i| id(i).as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        s.transfer_custom_members(src, None, a, &[id(2), id(0), id(2)], false)
            .unwrap(),
        2
    );
    assert_eq!(names(&s, src, ChannelScope::Custom(a)), ids(&[2, 0]));
    s.transfer_custom_members(src, None, b, &[id(0)], false)
        .unwrap();
    assert_eq!(
        s.transfer_custom_members(src, Some(a), b, &[id(2), id(0)], true)
            .unwrap(),
        2
    );
    assert!(names(&s, src, ChannelScope::Custom(a)).is_empty());
    assert_eq!(names(&s, src, ChannelScope::Custom(b)), ids(&[0, 2]));
    // A stale member rolls back the entire selection (including valid first ids).
    assert!(
        s.transfer_custom_members(
            src,
            None,
            a,
            &[id(1), ContentId::from_stored("gone")],
            false
        )
        .is_err()
    );
    assert!(names(&s, src, ChannelScope::Custom(a)).is_empty());
    assert!(
        s.transfer_custom_members(src, Some(b), foreign, &[id(0)], true)
            .is_err()
    );
    assert!(
        s.transfer_custom_members(src, Some(foreign), a, &[], false)
            .is_err()
    );
    assert!(
        s.transfer_custom_members(src, None, a, &[id(0)], true)
            .is_err()
    );
    assert!(
        s.transfer_custom_members(src, Some(a), b, &[id(1)], true)
            .is_err()
    );
    assert_eq!(
        s.transfer_custom_members(src, Some(b), b, &[id(0)], true)
            .unwrap(),
        0
    );
    s.move_in_custom_group(b, &id(2), 0).unwrap();
    s.rename_custom_group(b, "Renamed").unwrap();
    drop(s);
    let mut s = Store::open(&path).unwrap();
    assert_eq!(names(&s, src, ChannelScope::Custom(b)), ids(&[2, 0]));
    replace(&mut s, src, &cat);
    assert_eq!(names(&s, src, ChannelScope::Custom(b)), ids(&[2, 0]));
    s.remove_custom_members(b, &[id(2), id(2)]).unwrap();
    assert_eq!(names(&s, src, ChannelScope::Custom(b)), ids(&[0]));
    assert!(s.delete_custom_group(b).unwrap());
    assert_eq!(names(&s, src, ChannelScope::All).len(), 6);
    assert_eq!(s.groups(src, None).unwrap().len(), provider_group_count);
    assert!(s.transfer_custom_members(src, None, b, &[], false).is_err());
    assert!(s.remove_custom_members(b, &[]).is_err());
}
#[test]
fn favorites_group_and_playlist_orders_are_independent_and_survive_reimport() {
    let mut s = Store::open_in_memory().unwrap();
    let src = s.add_source(&m3u_source("Generated")).unwrap();
    let cat = catalog(6, 2, XT_PASSWORD);
    replace(&mut s, src, &cat);
    let id = |i: usize| cat.channels[i].id.clone();
    let group = cat.groups[0].id.clone();
    for i in [0, 2, 4] {
        s.set_favorite(src, &id(i), true).unwrap();
    }
    s.move_favorite(src, &id(4), 0).unwrap();
    s.move_channel(src, &OrderScope::Group(group.clone()), &id(2), 0)
        .unwrap();
    s.move_channel(src, &OrderScope::All, &id(5), 0).unwrap();
    for _ in 0..2 {
        assert_eq!(
            s.load_channels(
                src,
                &ChannelQuery {
                    favorites_only: true,
                    ..Default::default()
                }
            )
            .unwrap()
            .iter()
            .map(|r| r.channel.id.clone())
            .collect::<Vec<_>>(),
            [id(4), id(0), id(2)]
        );
        assert_eq!(
            s.load_channels(
                src,
                &ChannelQuery {
                    scope: ChannelScope::Group(group.clone()),
                    ..Default::default()
                }
            )
            .unwrap()
            .iter()
            .map(|r| r.channel.id.clone())
            .collect::<Vec<_>>(),
            [id(2), id(0), id(4)]
        );
        assert_eq!(
            s.load_channels(src, &ChannelQuery::default()).unwrap()[0]
                .channel
                .id,
            id(5)
        );
        replace(&mut s, src, &cat);
    }
    s.reset_manual_order(src, &OrderScope::Group(group.clone()))
        .unwrap();
    assert_eq!(
        s.load_channels(
            src,
            &ChannelQuery {
                scope: ChannelScope::Group(group),
                ..Default::default()
            }
        )
        .unwrap()[0]
            .channel
            .id,
        id(0)
    );
}
