//! Public commands preserve source ownership and committed user edits across restart.
use crate::common;
use crate::support;

use iptv_core::organization::{Action, Editor};
use iptv_library::{OrganizationScope, SourceId};
use iptv_providers::ContentId;
use iptv_runtime::{Ports, Runtime, commands, model::DatabaseKey};
use iptv_store_sqlite::{ChannelScope, Store};
use std::{path::Path, rc::Rc};
use support::block_on;

fn seeded() -> (
    tempfile::TempDir,
    DatabaseKey,
    SourceId,
    SourceId,
    Vec<ContentId>,
) {
    let dir = tempfile::tempdir().unwrap();
    let key = DatabaseKey::from_bytes([0x74; 32]);
    let mut store = Store::open_encrypted(dir.path().join("iptv.sqlite"), &key).unwrap();
    let first = store
        .add_source(&common::m3u_source("Generated first"))
        .unwrap();
    let second = store
        .add_source(&common::m3u_source("Generated second"))
        .unwrap();
    let catalog = common::catalog(4, 2, common::XT_PASSWORD);
    common::replace(&mut store, first, &catalog);
    common::replace(&mut store, second, &catalog);
    let keys = catalog.channels.iter().map(|c| c.id.clone()).collect();
    (dir, key, first, second, keys)
}

fn owner(path: &Path, key: &DatabaseKey) -> Rc<Runtime> {
    let key = key.clone();
    let owner = Rc::new(Runtime::new(Ports::new(path.to_owned(), move |_| {
        Ok(key.clone())
    })));
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    owner
}

#[test]
fn custom_membership_move_rejects_foreign_sources_and_survives_restart() {
    let (dir, key, first, second, keys) = seeded();
    let runtime = owner(dir.path(), &key);
    for (source, name) in [
        (first, "Generated A"),
        (first, "Generated B"),
        (second, "Foreign"),
    ] {
        block_on(commands::save_custom_group(
            &runtime,
            source,
            None,
            name.into(),
        ))
        .unwrap();
    }
    let groups = block_on(commands::custom_groups(&runtime, first)).unwrap();
    let a = groups.iter().find(|g| g.name == "Generated A").unwrap().id;
    let b = groups.iter().find(|g| g.name == "Generated B").unwrap().id;
    let foreign = block_on(commands::custom_groups(&runtime, second)).unwrap()[0].id;
    assert_eq!(
        block_on(commands::transfer_custom_members(
            &runtime,
            first,
            None,
            a,
            keys[..3].to_vec(),
            false,
        ))
        .unwrap(),
        3
    );
    assert!(
        block_on(commands::transfer_custom_members(
            &runtime,
            first,
            Some(a),
            foreign,
            keys[..2].to_vec(),
            true,
        ))
        .is_err()
    );
    assert_eq!(
        block_on(commands::members(&runtime, first, ChannelScope::Custom(a)))
            .unwrap()
            .len(),
        3
    );
    assert!(
        block_on(commands::members(
            &runtime,
            second,
            ChannelScope::Custom(foreign)
        ))
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        block_on(commands::transfer_custom_members(
            &runtime,
            first,
            Some(a),
            b,
            keys[..2].to_vec(),
            true,
        ))
        .unwrap(),
        2
    );
    block_on(commands::remove_custom_members(
        &runtime,
        b,
        vec![keys[0].clone()],
    ))
    .unwrap();
    block_on(commands::save_custom_group(
        &runtime,
        first,
        Some(b),
        "Renamed generated".into(),
    ))
    .unwrap();
    assert!(runtime.drain_services());
    drop(runtime);

    let runtime = owner(dir.path(), &key);
    let (groups, members) = block_on(commands::group_members(
        &runtime,
        first,
        ChannelScope::Custom(b),
    ))
    .unwrap();
    assert!(
        groups
            .iter()
            .any(|g| g.id == b && g.name == "Renamed generated" && g.channel_count == 1)
    );
    assert_eq!(
        members.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [keys[1].as_str()]
    );
    assert_eq!(
        block_on(commands::members(&runtime, first, ChannelScope::Custom(a))).unwrap()[0].id,
        keys[2].as_str()
    );
    block_on(commands::delete_custom_group(&runtime, b)).unwrap();
    assert!(
        block_on(commands::custom_groups(&runtime, first))
            .unwrap()
            .iter()
            .all(|g| g.id != b)
    );
    // Deleting a user group must not delete either provider's channels.
    assert_eq!(
        block_on(commands::members(&runtime, first, ChannelScope::All))
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        block_on(commands::members(&runtime, second, ChannelScope::All))
            .unwrap()
            .len(),
        4
    );
    assert!(runtime.drain_services());
}

#[test]
fn organization_edits_preserve_provider_identity_and_the_other_source() {
    let (dir, key, first, second, keys) = seeded();
    let runtime = owner(dir.path(), &key);
    block_on(commands::rename_organization_item(
        &runtime,
        first,
        OrganizationScope::Channels,
        keys[0].to_string(),
        " Generated alias ".into(),
    ))
    .unwrap();
    block_on(commands::hide_channel(&runtime, first, keys[0].to_string())).unwrap();
    let items = block_on(commands::organization_items(
        &runtime,
        first,
        OrganizationScope::Channels,
        vec![],
    ))
    .unwrap();
    let changed = items.iter().find(|i| i.id == keys[0].as_str()).unwrap();
    assert_eq!(changed.name, "Generated alias");
    assert!(changed.hidden);
    let editor = Editor::new(items.clone(), Some(changed.id.clone()));
    block_on(commands::edit_organization(
        &runtime,
        first,
        OrganizationScope::Channels,
        editor,
        changed.clone(),
        Action::Last,
    ))
    .unwrap();
    assert_eq!(
        block_on(commands::organization_items(
            &runtime,
            first,
            OrganizationScope::Channels,
            vec![]
        ))
        .unwrap()
        .last()
        .unwrap()
        .id,
        keys[0].as_str()
    );
    // Favorites' provider reset is independent of the customized all-channel order.
    assert_eq!(
        block_on(commands::favorite_provider_order(
            &runtime,
            first,
            vec![keys[3].to_string(), keys[0].to_string()],
        ))
        .unwrap(),
        [keys[0].to_string(), keys[3].to_string()]
    );
    assert!(runtime.drain_services());
    drop(runtime);

    let runtime = owner(dir.path(), &key);
    let foreign = block_on(commands::organization_items(
        &runtime,
        second,
        OrganizationScope::Channels,
        vec![],
    ))
    .unwrap();
    assert!(
        foreign
            .iter()
            .all(|i| !i.hidden && i.name != "Generated alias")
    );
    let current = block_on(commands::organization_items(
        &runtime,
        first,
        OrganizationScope::Channels,
        vec![],
    ))
    .unwrap();
    assert_eq!(current.last().unwrap().id, keys[0].as_str());
    assert_eq!(current.last().unwrap().name, "Generated alias");
    let channel = block_on(commands::channel(&runtime, first, keys[0].clone()))
        .unwrap()
        .unwrap();
    assert_eq!(channel.channel.id, keys[0]);
    let edited = current.last().unwrap().clone();
    block_on(commands::edit_organization(
        &runtime,
        first,
        OrganizationScope::Channels,
        Editor::new(current, None),
        edited,
        Action::ResetName,
    ))
    .unwrap();
    assert_ne!(
        block_on(commands::organization_items(
            &runtime,
            first,
            OrganizationScope::Channels,
            vec![]
        ))
        .unwrap()
        .last()
        .unwrap()
        .name,
        "Generated alias"
    );
    assert!(runtime.drain_services());
}

#[test]
fn legacy_history_is_consumed_once_and_cannot_replace_newer_committed_history() {
    let (dir, key, _, _, _) = seeded();
    let legacy = dir.path().join("generated-history.json");
    std::fs::write(&legacy, br#"["Generated old search"]"#).unwrap();
    let runtime = owner(dir.path(), &key);
    assert_eq!(
        block_on(commands::load_search_history(&runtime, legacy.clone())).unwrap(),
        ["Generated old search"]
    );
    assert!(!legacy.exists());
    block_on(commands::save_search_history(
        &runtime,
        vec!["Generated newer search".into()],
    ))
    .unwrap();
    std::fs::write(&legacy, br#"["Generated stale search"]"#).unwrap();
    assert_eq!(
        block_on(commands::load_search_history(&runtime, legacy.clone())).unwrap(),
        ["Generated newer search"]
    );
    assert!(!legacy.exists());
    assert!(runtime.drain_services());
    drop(runtime);
    let runtime = owner(dir.path(), &key);
    std::fs::write(&legacy, b"generated malformed JSON").unwrap();
    assert_eq!(
        block_on(commands::load_search_history(&runtime, legacy.clone())).unwrap(),
        ["Generated newer search"]
    );
    assert!(!legacy.exists());
    assert!(runtime.drain_services());
}

#[test]
fn privacy_clears_remove_durable_watch_and_search_history() {
    let (dir, key, first, _, keys) = seeded();
    {
        let mut store = Store::open_encrypted(dir.path().join("iptv.sqlite"), &key).unwrap();
        store
            .record_watch(
                first,
                &keys[0],
                iptv_providers::model::ContentKind::Live,
                1,
                None,
            )
            .unwrap();
        store
            .set_search_history(&["Generated private query".into()])
            .unwrap();
    }
    let runtime = owner(dir.path(), &key);
    assert!(block_on(runtime.clear_watch_history()));
    block_on(commands::save_search_history(&runtime, Vec::new())).unwrap();
    assert!(runtime.drain_services());
    drop(runtime);
    let store = Store::open_encrypted(dir.path().join("iptv.sqlite"), &key).unwrap();
    assert!(store.recent(None, None, 20).unwrap().is_empty());
    assert!(store.search_history().unwrap().is_empty());
}

#[test]
fn search_history_clear_orders_unpolled_saves_and_consumes_legacy_input() {
    let (dir, key, _, _, _) = seeded();
    let legacy = dir.path().join("generated-history.json");
    std::fs::write(&legacy, br#"["Generated legacy search"]"#).unwrap();
    let runtime = owner(dir.path(), &key);
    let startup = commands::load_search_history(&runtime, legacy.clone());
    let older = commands::save_search_history(&runtime, vec!["Generated older search".into()]);
    let cleared = commands::clear_search_history(&runtime, legacy.clone());
    block_on(cleared).unwrap();
    // Replies may be polled in any order: invocation determines writer order.
    block_on(older).unwrap();
    block_on(startup).unwrap();
    assert!(
        block_on(commands::load_search_history(&runtime, legacy.clone()))
            .unwrap()
            .is_empty()
    );
    assert!(!legacy.exists());

    // Clearing also consumes a legacy file when startup has not loaded it yet.
    std::fs::write(&legacy, br#"["Generated legacy search"]"#).unwrap();
    let cleared = commands::clear_search_history(&runtime, legacy.clone());
    let loaded = commands::load_search_history(&runtime, legacy.clone());
    assert!(block_on(loaded).unwrap().is_empty());
    block_on(cleared).unwrap();
    assert!(!legacy.exists());
    assert!(runtime.drain_services());
    drop(runtime);
    assert!(
        Store::open_encrypted(dir.path().join("iptv.sqlite"), &key)
            .unwrap()
            .search_history()
            .unwrap()
            .is_empty()
    );
}
