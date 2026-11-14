mod common;

use common::{XT_PASSWORD, catalog, m3u_source, replace};
use iptv_store_sqlite::{ChannelQuery, ChannelScope, Store};

#[test]
fn reset_preserves_the_previous_ordering_of_returning_orphans() {
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let run = |reset: bool| {
            let mut store = Store::open_in_memory().unwrap();
            let source = store.add_source(&m3u_source("Generated")).unwrap();
            let full = catalog(3, 1, XT_PASSWORD);
            replace(&mut store, source, &full);
            let group = store
                .create_custom_group(source, "Generated group")
                .unwrap();
            for index in order {
                store
                    .add_to_custom_group(group, &full.channels[index].id)
                    .unwrap();
            }
            let mut partial = catalog(3, 1, XT_PASSWORD);
            partial.channels.remove(1);
            replace(&mut store, source, &partial);
            if reset {
                store.reset_custom_group_order(group).unwrap();
            } else {
                for (at, index) in [0, 2].into_iter().enumerate() {
                    store
                        .move_in_custom_group(group, &full.channels[index].id, at)
                        .unwrap();
                }
            }
            replace(&mut store, source, &full);
            store
                .load_channels(
                    source,
                    &ChannelQuery {
                        scope: ChannelScope::Custom(group),
                        include_hidden: true,
                        ..Default::default()
                    },
                )
                .unwrap()
                .into_iter()
                .map(|row| row.channel.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(run(true), run(false), "initial order {order:?}");
    }
}

#[test]
fn reset_a_hundred_thousand_members_from_a_thousand_groups() {
    let mut store = Store::open_in_memory().unwrap();
    let source = store
        .add_source(&m3u_source("Generated large catalogue"))
        .unwrap();
    let full = catalog(100_000, 1_000, XT_PASSWORD);
    replace(&mut store, source, &full);
    let group = store
        .create_custom_group(source, "Generated reverse order")
        .unwrap();
    for channel in full.channels.iter().rev() {
        store.add_to_custom_group(group, &channel.id).unwrap();
    }
    store.reset_custom_group_order(group).unwrap();
    let rows = store
        .load_channels(
            source,
            &ChannelQuery {
                scope: ChannelScope::Custom(group),
                include_hidden: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(rows.len(), full.channels.len());
    for (actual, expected) in rows.iter().zip(&full.channels) {
        assert_eq!(actual.channel.id, expected.id);
    }
}
