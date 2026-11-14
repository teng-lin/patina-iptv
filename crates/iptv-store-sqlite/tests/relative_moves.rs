mod common;
use common::{XT_PASSWORD, catalog, m3u_source, replace};
use iptv_providers::model::ContentKind;
use iptv_store_sqlite::{ChannelQuery, ChannelScope, MoveTarget, OrderScope, Store};

#[test]
fn live_group_steps_and_ends_do_not_cross_invisible_movie_groups() {
    let mut store = Store::open_in_memory().unwrap();
    let source = store
        .add_source(&m3u_source("Generated mixed groups"))
        .unwrap();
    let catalogue = catalog(6, 3, XT_PASSWORD);
    replace(&mut store, source, &catalogue);
    let all = store.library_groups(source).unwrap();
    let movies: Vec<_> = all
        .iter()
        .enumerate()
        .filter(|(_, g)| g.group.kind == ContentKind::Movie)
        .map(|(i, g)| (i, g.group.id.clone()))
        .collect();
    let live: Vec<_> = all
        .iter()
        .filter(|g| g.group.kind == ContentKind::Live)
        .map(|g| g.group.id.clone())
        .collect();
    store
        .move_group_to(source, &live[0], MoveTarget::Index(usize::MAX))
        .unwrap();
    store
        .move_group_to(source, &live[0], MoveTarget::Step(1))
        .unwrap();
    store
        .move_group_to(source, &live[0], MoveTarget::Step(-1))
        .unwrap();
    let changed = store.library_groups(source).unwrap();
    let changed_live: Vec<_> = changed
        .iter()
        .filter(|g| g.group.kind == ContentKind::Live)
        .map(|g| g.group.id.clone())
        .collect();
    assert_eq!(
        changed_live,
        vec![live[1].clone(), live[0].clone(), live[2].clone()]
    );
    for (index, id) in movies {
        assert_eq!(changed[index].group.id, id);
    }
}

#[test]
fn group_and_custom_channel_steps_use_present_rows_and_clamp_at_ends() {
    for custom in [false, true] {
        let mut store = Store::open_in_memory().unwrap();
        let source = store.add_source(&m3u_source("Generated steps")).unwrap();
        let full = catalog(4, 1, XT_PASSWORD);
        replace(&mut store, source, &full);
        let group = store
            .create_custom_group(source, "Generated custom")
            .unwrap();
        for channel in &full.channels {
            store.add_to_custom_group(group, &channel.id).unwrap();
        }
        let mut partial = catalog(4, 1, XT_PASSWORD);
        partial.channels.remove(1);
        replace(&mut store, source, &partial);
        let key = &full.channels[0].id;
        let scope = OrderScope::Group(full.groups[0].id.clone());
        for target in [
            MoveTarget::Step(1),
            MoveTarget::Step(1),
            MoveTarget::Step(1),
        ] {
            if custom {
                store.move_in_custom_group_to(group, key, target).unwrap();
            } else {
                store.move_channel_to(source, &scope, key, target).unwrap();
            }
        }
        let scope = if custom {
            ChannelScope::Custom(group)
        } else {
            ChannelScope::Group(full.groups[0].id.clone())
        };
        let rows = store
            .load_channels(
                source,
                &ChannelQuery {
                    scope,
                    include_hidden: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(&rows.last().unwrap().channel.id, key);
        assert_eq!(rows.len(), 3);
    }
}
