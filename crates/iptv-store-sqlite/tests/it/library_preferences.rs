use crate::common;
use common::*;
use iptv_store_sqlite::{SourcePolicy, Store};
#[test]
fn policy_and_group_edits_survive_refresh_restart_and_are_source_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let mut s = Store::open(&path).unwrap();
    let src = s.add_source(&m3u_source("One")).unwrap();
    let other = s.add_source(&m3u_source("Two")).unwrap();
    let cat = catalog(8, 2, XT_PASSWORD);
    replace(&mut s, src, &cat);
    replace(&mut s, other, &cat);
    let p = SourcePolicy {
        enabled: false,
        refresh_seconds: 3600,
        refresh_on_start: true,
    };
    s.set_source_policy(src, p).unwrap();
    s.rename_group(src, &group(0).id, Some(" Local name "))
        .unwrap();
    s.hide_group(src, &group(0).id, true).unwrap();
    s.move_group(src, &group(1).id, 0).unwrap();
    replace(&mut s, src, &cat);
    drop(s);
    let mut s = Store::open(&path).unwrap();
    assert_eq!(s.source_policy(src).unwrap(), p);
    assert_eq!(s.source_policy(other).unwrap(), SourcePolicy::default());
    let groups = s.library_groups(src).unwrap();
    assert_eq!(groups[0].group.id, group(1).id);
    assert_eq!(groups[1].name, "Local name");
    assert!(groups[1].hidden);
    assert_eq!(groups[1].group.name, group(0).name);
    assert!(!s.library_groups(other).unwrap()[0].hidden);
    s.rename_group(src, &group(0).id, None).unwrap();
    s.hide_group(src, &group(0).id, false).unwrap();
    s.reset_group_order(src).unwrap();
    let groups = s.library_groups(src).unwrap();
    assert_eq!(groups[0].name, group(0).name);
    assert!(!groups[0].hidden);
    s.delete_source(src).unwrap();
    assert!(s.source_policy(src).is_err());
}

#[test]
fn bulk_visibility_is_source_scoped_and_retains_names_and_favorites() {
    use iptv_store_sqlite::ChannelQuery;
    let mut s = Store::open_in_memory().unwrap();
    let source = s.add_source(&m3u_source("Local")).unwrap();
    let cat = catalog(4, 2, XT_PASSWORD);
    replace(&mut s, source, &cat);
    let id = cat.channels[0].id.clone();
    s.rename_channel(source, &id, Some("Local title")).unwrap();
    s.set_favourite(source, &id, true).unwrap();
    let ids = cat
        .channels
        .iter()
        .map(|c| c.id.as_str().to_owned())
        .collect::<Vec<_>>();
    s.set_library_visibility(source, &ids, false, true).unwrap();
    assert!(
        s.load_channels(source, &ChannelQuery::default())
            .unwrap()
            .is_empty()
    );
    replace(&mut s, source, &cat);
    s.set_library_visibility(source, &ids, false, false)
        .unwrap();
    let rows = s.load_channels(source, &ChannelQuery::default()).unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].name, "Local title");
    assert!(rows[0].favourite);
}

fn live_catalog(channels: usize, groups: usize) -> common::Catalog {
    let mut cat = catalog(channels, groups, XT_PASSWORD);
    cat.groups
        .retain(|g| g.kind == iptv_providers::model::ContentKind::Live);
    cat.vod.clear();
    cat.series.clear();
    cat
}

#[test]
fn group_bulk_edits_keep_order_names_and_other_playlists_isolated() {
    use iptv_providers::GroupId;
    let mut store = Store::open_in_memory().unwrap();
    let source = store.add_source(&m3u_source("A")).unwrap();
    let other = store.add_source(&m3u_source("B")).unwrap();
    let cat = live_catalog(6, 3);
    replace(&mut store, source, &cat);
    replace(&mut store, other, &cat);
    store
        .rename_group(source, &group(1).id, Some("Local group"))
        .unwrap();
    store.move_group(source, &group(1).id, 0).unwrap();
    let ids: Vec<_> = (0..3).map(|i| group(i).id.as_str().to_owned()).collect();
    store
        .set_library_visibility(source, &ids, true, true)
        .unwrap();
    assert!(
        store
            .library_groups(source)
            .unwrap()
            .iter()
            .all(|r| r.hidden)
    );
    assert!(
        store
            .library_groups(other)
            .unwrap()
            .iter()
            .all(|r| !r.hidden)
    );
    store
        .set_library_visibility(source, &ids[1..2], true, false)
        .unwrap();
    let rows = store.library_groups(source).unwrap();
    assert_eq!(rows[0].name, "Local group");
    assert!(!rows[0].hidden);
    assert!(rows[1].hidden);
    store.move_group(source, &group(1).id, usize::MAX).unwrap();
    assert_eq!(
        store.library_groups(source).unwrap()[2].group.id,
        group(1).id
    );
    let before = store.library_groups(source).unwrap();
    store
        .move_group(source, &GroupId::from_stored("missing"), 0)
        .unwrap();
    assert_eq!(store.library_groups(source).unwrap(), before);
    store.reset_group_order(source).unwrap();
    store
        .rename_group(source, &group(1).id, Some("  "))
        .unwrap();
    let rows = store.library_groups(source).unwrap();
    assert_eq!(rows[1].name, group(1).name);
    assert_eq!(rows[0].group.id, group(0).id);
    assert!(rows[0].hidden);
}

#[test]
fn temporarily_absent_groups_regain_overlays_and_empty_bulk_edits_are_noops() {
    use iptv_store_sqlite::ChannelQuery;
    let mut store = Store::open_in_memory().unwrap();
    let source = store.add_source(&m3u_source("A")).unwrap();
    let cat = live_catalog(6, 2);
    replace(&mut store, source, &cat);
    store
        .rename_group(source, &group(1).id, Some("Retained name"))
        .unwrap();
    store.hide_group(source, &group(1).id, true).unwrap();
    store.move_group(source, &group(1).id, 0).unwrap();
    replace(&mut store, source, &live_catalog(3, 1));
    assert_eq!(store.library_groups(source).unwrap().len(), 1);
    replace(&mut store, source, &cat);
    let groups = store.library_groups(source).unwrap();
    assert_eq!(groups[0].group.id, group(1).id);
    assert_eq!(groups[0].name, "Retained name");
    assert!(groups[0].hidden);
    store
        .set_library_visibility(source, &[], true, false)
        .unwrap();
    assert_eq!(store.library_groups(source).unwrap(), groups);
    let ids: Vec<_> = cat
        .channels
        .iter()
        .map(|c| c.id.as_str().to_owned())
        .collect();
    store
        .set_library_visibility(source, &ids, false, true)
        .unwrap();
    store
        .set_library_visibility(source, &ids, false, false)
        .unwrap();
    assert_eq!(
        store
            .load_channels(source, &ChannelQuery::default())
            .unwrap()
            .len(),
        6
    );
    replace(&mut store, source, &live_catalog(0, 0));
    assert!(
        store.orphaned_overlays(Some(source)).unwrap().is_empty(),
        "restoring default visibility leaves no phantom channel customizations"
    );
}

#[test]
fn policies_and_bulk_edits_require_a_source_and_delete_cascades_preferences() {
    use iptv_store_sqlite::SourceId;
    let mut store = Store::open_in_memory().unwrap();
    let missing = SourceId(999);
    assert!(store.source_policy(missing).is_err());
    assert!(
        store
            .set_source_policy(missing, SourcePolicy::default())
            .is_err()
    );
    assert!(
        store
            .set_library_visibility(missing, &["generated".into()], false, true)
            .is_err()
    );
    assert!(
        store
            .rename_group(missing, &group(0).id, Some("Name"))
            .is_err()
    );
    assert!(store.hide_group(missing, &group(0).id, true).is_err());
    let source = store.add_source(&m3u_source("A")).unwrap();
    assert_eq!(
        store.source_policy(source).unwrap(),
        SourcePolicy::default()
    );
    replace(&mut store, source, &catalog(2, 1, XT_PASSWORD));
    store
        .set_source_policy(
            source,
            SourcePolicy {
                enabled: false,
                refresh_seconds: 86400,
                refresh_on_start: false,
            },
        )
        .unwrap();
    store.hide_group(source, &group(0).id, true).unwrap();
    store.delete_source(source).unwrap();
    let fresh = store.add_source(&m3u_source("New")).unwrap();
    replace(&mut store, fresh, &catalog(2, 1, XT_PASSWORD));
    assert_eq!(store.source_policy(fresh).unwrap(), SourcePolicy::default());
    assert!(!store.library_groups(fresh).unwrap()[0].hidden);
}

#[test]
fn deleting_playlist_forgets_its_collapsed_section_before_source_id_reuse() {
    let mut store = Store::open_in_memory().unwrap();
    let first = store.add_source(&m3u_source("First")).unwrap();
    let second = store.add_source(&m3u_source("Second")).unwrap();
    store.set_collapsed_playlists(&[first.0, second.0]).unwrap();
    store.delete_source(second).unwrap();
    assert_eq!(store.collapsed_playlists().unwrap(), [first.0]);
    let replacement = store.add_source(&m3u_source("Replacement")).unwrap();
    assert!(
        !store
            .collapsed_playlists()
            .unwrap()
            .contains(&replacement.0)
    );
}

#[test]
fn guide_order_is_scoped_and_rejects_a_stale_draft() {
    use iptv_core::organization::Sort;
    use iptv_library::OrganizationScope;

    let mut store = Store::open_in_memory().unwrap();
    let source = store.add_source(&m3u_source("Generated")).unwrap();
    let cat = live_catalog(8, 2);
    replace(&mut store, source, &cat);
    let scope = OrganizationScope::GroupChannels(group(0).id);
    let groups = |store: &Store| {
        store
            .guide_groups(source)
            .unwrap()
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>()
    };
    let channels = |store: &Store, scope: &OrganizationScope| {
        store
            .ordered_guide_channels(source, scope)
            .unwrap()
            .into_iter()
            .map(|row| row.channel.id.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    let original = channels(&store, &scope);
    let all = channels(&store, &OrganizationScope::Channels);
    let custom = store.create_custom_group(source, "Alpha").unwrap();
    store
        .set_guide_sort(source, &OrganizationScope::Groups, Sort::Name)
        .unwrap();
    let names = groups(&store);
    assert_eq!(names[0], format!("custom:{}", custom.0));
    let reversed: Vec<_> = names.iter().rev().cloned().collect();
    store
        .save_guide_order(source, &OrganizationScope::Groups, &names, &reversed)
        .unwrap();
    assert_eq!(groups(&store), reversed);

    store
        .rename_channel(
            source,
            &iptv_providers::ContentId::from_stored(&original[1]),
            Some("AAA"),
        )
        .unwrap();
    store.set_guide_sort(source, &scope, Sort::Name).unwrap();
    let sorted = channels(&store, &scope);
    assert_eq!(sorted[0], original[1]);
    assert!(
        store
            .save_guide_order(source, &scope, &original, &original)
            .is_err()
    );
    assert_eq!(store.guide_sort(source, &scope).unwrap(), Sort::Name);
    store
        .save_guide_order(source, &scope, &sorted, &original)
        .unwrap();
    replace(&mut store, source, &cat);
    assert_eq!(store.guide_sort(source, &scope).unwrap(), Sort::Manual);
    assert_eq!(channels(&store, &scope), original);
    assert_eq!(channels(&store, &OrganizationScope::Channels), all);
    assert_eq!(groups(&store), reversed);
}

#[test]
fn guide_blocking_is_inherited_across_membership_and_independent_of_parental_locks() {
    use iptv_core::organization::{Block, BlockKind};
    use iptv_core::parental::Target;

    let mut store = Store::open_in_memory().unwrap();
    let source = store.add_source(&m3u_source("Generated one")).unwrap();
    let another = store.add_source(&m3u_source("Generated two")).unwrap();
    let cat = live_catalog(4, 2);
    replace(&mut store, source, &cat);
    replace(&mut store, another, &cat);
    let channel = &cat.channels[0];
    let parent = &channel.groups[0];
    let id = channel.id.as_str();
    let custom = store
        .create_custom_group(source, "Generated custom group")
        .unwrap();
    store.add_to_custom_group(custom, &channel.id).unwrap();
    let custom_key = format!("custom:{}", custom.0);
    let parental = Target::live(source.0, id);
    store.set_parental_blocked(&parental, true).unwrap();
    store
        .set_guide_block(source, BlockKind::Group, parent.as_str(), true)
        .unwrap();
    store
        .set_guide_block(source, BlockKind::Group, &custom_key, true)
        .unwrap();
    store
        .set_guide_block(source, BlockKind::Channel, id, true)
        .unwrap();
    assert_eq!(
        store.guide_blocks(source).unwrap().channel(id),
        Block::Channel
    );
    store
        .set_guide_block(source, BlockKind::Channel, id, false)
        .unwrap();
    assert_eq!(
        store.guide_blocks(source).unwrap().channel(id),
        Block::Group
    );
    assert!(store.guide_channel_blocked(source, id).unwrap());
    assert!(!store.guide_channel_blocked(another, id).unwrap());
    store
        .set_guide_block(source, BlockKind::Group, parent.as_str(), false)
        .unwrap();
    assert!(store.guide_channel_blocked(source, id).unwrap());
    store
        .set_guide_block(source, BlockKind::Group, &custom_key, false)
        .unwrap();
    assert!(!store.guide_channel_blocked(source, id).unwrap());
    assert!(store.parental_target_blocked(&parental).unwrap());
    assert!(store.parental_credential().unwrap().is_none());

    // Deleting a custom group retires its preferences before SQLite can reuse its id.
    use iptv_core::organization::Sort;
    use iptv_library::OrganizationScope;
    let scope = OrganizationScope::CustomChannels(custom);
    store
        .set_guide_block(source, BlockKind::Group, &custom_key, true)
        .unwrap();
    store.set_guide_sort(source, &scope, Sort::Name).unwrap();
    store
        .hide_group(
            source,
            &iptv_providers::GroupId::from_stored(&custom_key),
            true,
        )
        .unwrap();
    assert!(store.delete_custom_group(custom).unwrap());
    assert_eq!(
        store.create_custom_group(source, "Replacement").unwrap(),
        custom
    );
    store.add_to_custom_group(custom, &channel.id).unwrap();
    assert!(!store.guide_channel_blocked(source, id).unwrap());
    assert_eq!(store.guide_sort(source, &scope).unwrap(), Sort::Manual);
    assert!(
        !store
            .guide_groups(source)
            .unwrap()
            .iter()
            .find(|item| item.id == custom_key)
            .unwrap()
            .hidden
    );
}

#[test]
fn the_guide_orders_channels_by_name_recency_and_accumulated_watch_time() {
    use iptv_core::organization::Sort;
    use iptv_library::OrganizationScope;
    use iptv_providers::model::ContentKind;

    let mut store = Store::open_in_memory().unwrap();
    let source = store.add_source(&m3u_source("Generated")).unwrap();
    let catalog = catalog(4, 1, XT_PASSWORD);
    replace(&mut store, source, &catalog);
    let channels = &catalog.channels;
    for (channel, name) in channels.iter().zip(["A", "B", "C", "D"]) {
        store
            .rename_channel(source, &channel.id, Some(name))
            .unwrap();
    }
    for (index, watched_at, elapsed_ms) in [(0, 200, 5_000), (1, 200, 25_000), (2, 300, 12_000)] {
        let key = &channels[index].id;
        store
            .record_watch(source, key, ContentKind::Live, watched_at, None)
            .unwrap();
        store
            .add_channel_watch_time(source, key, elapsed_ms)
            .unwrap();
    }
    store
        .add_channel_watch_time(source, &channels[0].id, 40_000)
        .unwrap();

    let scope = OrganizationScope::GroupChannels(group(0).id);
    for (sort, expected) in [
        (Sort::Name, ["A", "B", "C", "D"]),
        (Sort::NameDescending, ["D", "C", "B", "A"]),
        (Sort::LastWatched, ["C", "B", "A", "D"]),
        (Sort::OldestWatched, ["A", "B", "C", "D"]),
        (Sort::MostWatched, ["A", "B", "C", "D"]),
        (Sort::LeastWatched, ["C", "B", "A", "D"]),
    ] {
        store.set_guide_sort(source, &scope, sort).unwrap();
        assert_eq!(store.guide_sort(source, &scope).unwrap(), sort);
        let names: Vec<_> = store
            .ordered_guide_channels(source, &scope)
            .unwrap()
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert_eq!(names, expected, "{sort:?}");
    }
    let original: Vec<_> = store
        .ordered_guide_channels(source, &scope)
        .unwrap()
        .into_iter()
        .map(|row| row.channel.id.as_str().to_owned())
        .collect();
    let mut custom = original.clone();
    custom.swap(0, 1);
    // The active playback can change the sorted baseline while editing.
    store
        .add_channel_watch_time(source, &channels[2].id, 60_000)
        .unwrap();
    store
        .set_guide_sort(source, &scope, Sort::MostWatched)
        .unwrap();
    assert!(
        store
            .save_guide_order_from(
                source,
                &scope,
                &original,
                &custom,
                Some(Sort::LeastWatched),
                false
            )
            .is_err()
    );
    store
        .set_guide_sort(source, &scope, Sort::LeastWatched)
        .unwrap();
    store
        .save_guide_order_from(
            source,
            &scope,
            &original,
            &custom,
            Some(Sort::LeastWatched),
            false,
        )
        .unwrap();
    assert_eq!(store.guide_sort(source, &scope).unwrap(), Sort::Manual);
    store
        .set_guide_sort(source, &scope, Sort::LeastWatched)
        .unwrap();
    store.clear_history(Some(source)).unwrap();
    let names: Vec<_> = store
        .ordered_guide_channels(source, &scope)
        .unwrap()
        .into_iter()
        .map(|row| row.name)
        .collect();
    assert_eq!(names, ["A", "B", "C", "D"]);
}
