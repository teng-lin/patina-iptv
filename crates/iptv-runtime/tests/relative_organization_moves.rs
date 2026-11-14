#[path = "../../iptv-store-sqlite/tests/common/mod.rs"]
mod common;
mod support;

use iptv_core::organization::{Action, Editor};
use iptv_library::OrganizationScope;
use iptv_runtime::{Ports, Runtime, commands};
use std::rc::Rc;
use support::block_on;

#[test]
fn two_later_presses_from_one_view_snapshot_move_twice() {
    let directory = tempfile::tempdir().unwrap();
    let key = iptv_runtime::model::DatabaseKey::from_bytes([0x68; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(directory.path().join("iptv.sqlite"), &key)
            .unwrap();
    let source = store
        .add_source(&common::m3u_source("Generated moves"))
        .unwrap();
    let catalogue = common::catalog(100_000, 1_000, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalogue);
    drop(store);
    let runtime = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        move |_| Ok(key.clone()),
    )));
    {
        runtime.start_services();
        let items = block_on(commands::organization_items(
            &runtime,
            source,
            OrganizationScope::Channels,
            Vec::new(),
        ))
        .unwrap();
        let item = items[0].clone();
        let editor = Editor::new(items, Some(item.id.clone()));
        let first = commands::edit_organization(
            &runtime,
            source,
            OrganizationScope::Channels,
            editor.clone(),
            item.clone(),
            Action::Later,
        );
        let second = commands::edit_organization(
            &runtime,
            source,
            OrganizationScope::Channels,
            editor,
            item.clone(),
            Action::Later,
        );
        block_on(first).unwrap();
        block_on(second).unwrap();
        let changed = block_on(commands::organization_items(
            &runtime,
            source,
            OrganizationScope::Channels,
            Vec::new(),
        ))
        .unwrap();
        assert_eq!(changed[2].id, item.id);
        // Favorites still expose visibility and name edits, even though their
        // ordering uses a separate write queue.
        block_on(commands::edit_organization(
            &runtime,
            source,
            OrganizationScope::Favorites,
            Editor::new(changed, Some(item.id.clone())),
            item.clone(),
            Action::Visibility,
        ))
        .unwrap();
        let changed = block_on(commands::organization_items(
            &runtime,
            source,
            OrganizationScope::Channels,
            Vec::new(),
        ))
        .unwrap();
        assert!(changed.iter().find(|row| row.id == item.id).unwrap().hidden);
        assert!(runtime.drain_services());
    }
}
