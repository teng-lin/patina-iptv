//! Manual order, custom groups, EPG override and watch history.

use crate::common;

use common::*;
use iptv_providers::ContentId;
use iptv_providers::model::ContentKind;
use iptv_store_sqlite::{
    ChannelOrder, ChannelQuery, ChannelRow, ChannelScope, EpgOverride, HISTORY_LIMIT, OrderScope,
    PRUNE_SLACK, SourceId, Store, StoreError,
};

fn setup(n: usize, groups: usize) -> (Store, SourceId, Catalog) {
    let mut s = Store::open_in_memory().unwrap();
    let src = s.add_source(&m3u_source("List")).unwrap();
    let cat = catalog(n, groups, XT_PASSWORD);
    replace(&mut s, src, &cat);
    (s, src, cat)
}

fn ids(rows: &[ChannelRow]) -> Vec<String> {
    rows.iter()
        .map(|r| r.channel.id.as_str().to_owned())
        .collect()
}

fn of(cat: &Catalog, idx: &[usize]) -> Vec<String> {
    idx.iter()
        .map(|&i| cat.channels[i].id.as_str().to_owned())
        .collect()
}

fn query(scope: ChannelScope) -> ChannelQuery {
    ChannelQuery {
        scope,
        include_hidden: true,
        ..Default::default()
    }
}

#[test]
fn manual_order_within_a_group_survives_refreshes() {
    // Group 0 of 3 holds channels 0, 3, 6, 9.
    let (mut s, src, cat) = setup(12, 3);
    let c = |i: usize| cat.channels[i].id.clone();
    let g = group(0).id;
    let scope = OrderScope::Group(g.clone());
    let list = query(ChannelScope::Group(g.clone()));

    assert!(s.move_channel(src, &scope, &c(9), 0).unwrap());
    assert!(s.move_channel(src, &scope, &c(0), 2).unwrap());
    assert!(
        !s.move_channel(src, &scope, &c(1), 0).unwrap(),
        "not in the group"
    );
    assert_eq!(
        ids(&s.load_channels(src, &list).unwrap()),
        of(&cat, &[9, 3, 0, 6])
    );
    // The all-channels list and the provider order are unaffected.
    assert_eq!(
        ids(&s.load_channels(src, &query(ChannelScope::All)).unwrap())[..3],
        of(&cat, &[0, 1, 2])
    );
    let provider = ChannelQuery {
        order: ChannelOrder::Provider,
        ..list.clone()
    };
    assert_eq!(
        ids(&s.load_channels(src, &provider).unwrap()),
        of(&cat, &[0, 3, 6, 9])
    );

    // Refresh: channel 3 disappears, channel 12 is new in the group.
    let mut next = catalog(13, 3, "rotated-pass");
    next.channels.remove(3);
    replace(&mut s, src, &next);
    assert_eq!(
        ids(&s.load_channels(src, &list).unwrap()),
        of(&catalog(13, 3, XT_PASSWORD), &[9, 0, 6, 12])
    );
    // Moving while 3 is gone keeps its slot between 9 and 0.
    assert!(s.move_channel(src, &scope, &c(6), 0).unwrap());
    replace(&mut s, src, &cat);
    assert_eq!(
        ids(&s.load_channels(src, &list).unwrap()),
        of(&cat, &[6, 9, 3, 0])
    );

    // Paging walks the effective order.
    let page = ChannelQuery {
        offset: 1,
        limit: Some(2),
        ..list.clone()
    };
    assert_eq!(
        ids(&s.load_channels(src, &page).unwrap()),
        of(&cat, &[9, 3])
    );

    s.set_manual_order(src, &scope, &[c(0), c(6), c(0)])
        .unwrap();
    assert_eq!(
        ids(&s.load_channels(src, &list).unwrap()),
        of(&cat, &[0, 6, 3, 9])
    );
    s.reset_manual_order(src, &scope).unwrap();
    assert_eq!(
        ids(&s.load_channels(src, &list).unwrap()),
        of(&cat, &[0, 3, 6, 9])
    );
}

#[test]
fn manual_order_of_all_channels_and_hidden_filtering() {
    let (mut s, src, cat) = setup(5, 1);
    let c = |i: usize| cat.channels[i].id.clone();
    s.move_channel(src, &OrderScope::All, &c(4), 1).unwrap();
    s.set_hidden(src, &c(0), true).unwrap();
    let visible = s.load_channels(src, &ChannelQuery::default()).unwrap();
    assert_eq!(ids(&visible), of(&cat, &[4, 1, 2, 3]));
    let all = s.load_channels(src, &query(ChannelScope::All)).unwrap();
    assert_eq!(ids(&all), of(&cat, &[0, 4, 1, 2, 3]));
    // Name order ignores the manual order.
    let by_name = ChannelQuery {
        order: ChannelOrder::Name,
        ..Default::default()
    };
    assert_eq!(
        ids(&s.load_channels(src, &by_name).unwrap()),
        of(&cat, &[1, 2, 3, 4])
    );
}

#[test]
fn custom_groups_crud_membership_and_order() {
    let (mut s, src, cat) = setup(6, 2);
    let c = |i: usize| cat.channels[i].id.clone();
    let evening = s.create_custom_group(src, "Evening").unwrap();
    let kids = s.create_custom_group(src, "Kids").unwrap();
    s.rename_custom_group(kids, "Children").unwrap();
    s.move_custom_group(kids, 0).unwrap();
    let names: Vec<String> = s
        .custom_groups(src)
        .unwrap()
        .into_iter()
        .map(|g| g.name)
        .collect();
    assert_eq!(names, ["Children", "Evening"]);

    for i in [5, 1, 3] {
        assert!(s.add_to_custom_group(evening, &c(i)).unwrap());
    }
    assert!(!s.add_to_custom_group(evening, &c(1)).unwrap());
    let scope = query(ChannelScope::Custom(evening));
    assert_eq!(
        ids(&s.load_channels(src, &scope).unwrap()),
        of(&cat, &[5, 1, 3])
    );
    assert!(s.move_in_custom_group(evening, &c(3), 0).unwrap());
    assert_eq!(
        ids(&s.load_channels(src, &scope).unwrap()),
        of(&cat, &[3, 5, 1])
    );

    s.set_hidden(src, &c(5), true).unwrap();
    s.set_favorite(src, &c(1), true).unwrap();
    let visible = ChannelQuery {
        scope: ChannelScope::Custom(evening),
        ..Default::default()
    };
    assert_eq!(
        ids(&s.load_channels(src, &visible).unwrap()),
        of(&cat, &[3, 1])
    );
    let favs = ChannelQuery {
        favorites_only: true,
        ..visible
    };
    assert_eq!(ids(&s.load_channels(src, &favs).unwrap()), of(&cat, &[1]));

    // A member that leaves the catalog is kept and counted again on return.
    let mut without = catalog(6, 2, XT_PASSWORD);
    without.channels.remove(3);
    replace(&mut s, src, &without);
    let counts = |s: &Store| s.custom_groups(src).unwrap()[1].channel_count;
    assert_eq!(counts(&s), 2);
    assert!(s.orphaned_overlays(Some(src)).unwrap()[0].in_custom_group);
    replace(&mut s, src, &cat);
    assert_eq!(counts(&s), 3);

    assert!(s.remove_from_custom_group(evening, &c(5)).unwrap());
    assert!(!s.remove_from_custom_group(evening, &c(5)).unwrap());
    assert!(s.delete_custom_group(evening).unwrap());
    assert!(s.load_channels(src, &scope).unwrap().is_empty());
    assert!(matches!(
        s.add_to_custom_group(evening, &c(0)),
        Err(StoreError::CustomGroupNotFound(_))
    ));
    assert!(s.rename_custom_group(evening, "x").is_err());
}

#[test]
fn epg_override_survives_refresh_and_clears() {
    let (mut s, src, cat) = setup(3, 1);
    let id = cat.channels[1].id.clone();
    let alt = EpgOverride::Xmltv {
        epg_source: "epg-a".into(),
        epg_channel: "alt.example".into(),
    };
    s.set_epg_override(src, &id, Some(&alt)).unwrap();
    s.set_epg_override(src, &cat.channels[2].id, Some(&EpgOverride::Disabled))
        .unwrap();
    replace(&mut s, src, &catalog(3, 1, "rotated"));
    assert_eq!(
        s.channel(src, &id).unwrap().unwrap().epg_override,
        Some(alt)
    );
    assert_eq!(
        s.channel(src, &cat.channels[2].id)
            .unwrap()
            .unwrap()
            .epg_override,
        Some(EpgOverride::Disabled)
    );
    s.set_epg_override(src, &id, None).unwrap();
    s.set_epg_override(src, &cat.channels[2].id, None).unwrap();
    assert_eq!(s.channel(src, &id).unwrap().unwrap().epg_override, None);
    // Nothing customised is left, so nothing can become an orphan.
    replace(&mut s, src, &catalog(0, 1, XT_PASSWORD));
    assert!(s.orphaned_overlays(Some(src)).unwrap().is_empty());
}

#[test]
fn history_lists_recent_items_once_and_survives_refresh() {
    let (mut s, src, cat) = setup(3, 1);
    let other = s.add_source(&xtream_source("Xt")).unwrap();
    let c = |i: usize| cat.channels[i].id.clone();
    let movie = ContentId::from_stored("xt:movie:9");
    s.record_watch(src, &c(0), ContentKind::Live, 100, None)
        .unwrap();
    s.record_watch(src, &c(1), ContentKind::Live, 200, None)
        .unwrap();
    s.record_watch(src, &movie, ContentKind::Movie, 300, Some(42))
        .unwrap();
    s.record_watch(src, &c(0), ContentKind::Live, 400, None)
        .unwrap();
    s.record_watch(other, &c(2), ContentKind::Live, 350, None)
        .unwrap();

    let recent = s.recent(Some(src), None, 10).unwrap();
    assert_eq!(
        recent
            .iter()
            .map(|h| (h.item_id.as_str().to_owned(), h.watched_at))
            .collect::<Vec<_>>(),
        [
            (c(0).as_str().to_owned(), 400),
            (movie.as_str().to_owned(), 300),
            (c(1).as_str().to_owned(), 200)
        ]
    );
    assert_eq!(recent[1].position_secs, Some(42));
    assert_eq!(
        s.recent(Some(src), Some(ContentKind::Live), 1).unwrap()[0].watched_at,
        400
    );
    let everywhere = s.recent(None, None, 10).unwrap();
    assert_eq!(everywhere.len(), 4);
    assert_eq!(everywhere[1].source_id, other);

    replace(&mut s, src, &catalog(0, 1, XT_PASSWORD));
    assert_eq!(s.recent(Some(src), None, 10).unwrap().len(), 3);
    assert_eq!(s.clear_history(Some(src)).unwrap(), 4);
    assert_eq!(s.recent(None, None, 10).unwrap().len(), 1);
    s.delete_source(other).unwrap();
    assert!(s.recent(None, None, 10).unwrap().is_empty());
    assert!(
        s.record_watch(other, &c(0), ContentKind::Live, 1, None)
            .is_err()
    );
}

#[test]
fn history_is_pruned_per_source() {
    let (mut s, src, cat) = setup(1, 1);
    let other = s.add_source(&xtream_source("Xt")).unwrap();
    let id = cat.channels[0].id.clone();
    let movie = ContentId::from_stored("xt:movie:1");
    // The oldest play, then enough newer ones to cross the prune threshold.
    s.record_watch(src, &movie, ContentKind::Movie, 0, None)
        .unwrap();
    s.record_watch(other, &movie, ContentKind::Movie, 0, None)
        .unwrap();
    let plays = i64::from(HISTORY_LIMIT + PRUNE_SLACK);
    for t in 1..=plays {
        s.record_watch(src, &id, ContentKind::Live, t, None)
            .unwrap();
    }
    let recent = s.recent(Some(src), None, 10).unwrap();
    assert_eq!(recent.len(), 1, "the oldest play was pruned");
    assert_eq!(recent[0].watched_at, plays);
    // Rows kept: exactly the newest HISTORY_LIMIT plays.
    assert_eq!(s.clear_history(Some(src)).unwrap(), HISTORY_LIMIT as usize);
    // Pruning is per source.
    assert_eq!(s.recent(Some(other), None, 10).unwrap().len(), 1);
}

/// Resetting a custom group's order writes provider order in one call, and
/// leaves a member whose channel is gone where the old per-channel moves
/// left it (it shows there again when its channel returns).
#[test]
fn a_custom_group_order_resets_in_one_transaction() {
    let build = || {
        let (mut s, src, cat) = setup(8, 2);
        let group = s.create_custom_group(src, "Mixed").unwrap();
        for i in [6, 2, 7, 0, 4, 1] {
            s.add_to_custom_group(group, &cat.channels[i].id).unwrap();
        }
        s.set_hidden(src, &cat.channels[2].id, true).unwrap();
        let mut without = catalog(8, 2, XT_PASSWORD);
        without.channels.remove(7);
        replace(&mut s, src, &without);
        (s, src, cat, group)
    };
    let present_in_provider_order = |s: &Store, src, group| {
        s.load_channels(
            src,
            &ChannelQuery {
                scope: ChannelScope::Custom(group),
                include_hidden: true,
                order: ChannelOrder::Provider,
                ..Default::default()
            },
        )
        .unwrap()
    };

    let (mut one, src, cat, group) = build();
    one.reset_custom_group_order(group).unwrap();
    assert_eq!(
        ids(&one
            .load_channels(src, &query(ChannelScope::Custom(group)))
            .unwrap()),
        of(&cat, &[0, 1, 2, 4, 6])
    );

    // The previous app code: one move (and transaction) per channel.
    let (mut each, _, _, _) = build();
    for (at, row) in present_in_provider_order(&each, src, group)
        .iter()
        .enumerate()
    {
        each.move_in_custom_group(group, &row.channel.id, at)
            .unwrap();
    }
    replace(&mut one, src, &cat);
    replace(&mut each, src, &cat);
    let order = |s: &Store| {
        ids(&s
            .load_channels(src, &query(ChannelScope::Custom(group)))
            .unwrap())
    };
    assert_eq!(order(&one), order(&each));
    assert_eq!(order(&one).len(), 6, "the returned member is listed again");

    assert!(matches!(
        one.reset_custom_group_order(iptv_store_sqlite::CustomGroupId(999)),
        Err(StoreError::CustomGroupNotFound(999))
    ));
}
