//! Compile-checked command contracts catch adapter types even through aliases
//! and inferred call sites. The inventory ensures a new command gets a contract.
#[path = "../../iptv-store-sqlite/tests/common/mod.rs"]
mod common;
mod support;

use iptv_core::{
    membership::Member,
    organization::{Action, Editor, Item},
};
use iptv_library::{ChannelScope, CustomGroup, CustomGroupId, OrganizationScope, SourceId};
use iptv_providers::ContentId;
use iptv_runtime::{Error, Ports, Runtime, commands, model::PlaybackChannel};
use std::{collections::BTreeSet, future::Future};

fn response<T>(_: impl Future<Output = Result<T, Error>>) {}

#[test]
fn every_public_command_returns_its_declared_application_value() {
    let owner = Runtime::new(Ports::new(std::path::PathBuf::new(), |_| {
        panic!("type checking command futures must not open storage")
    }));
    let source = SourceId(1);
    let group = CustomGroupId(1);
    let content = || ContentId::from_stored("generated-channel");
    let scope = || OrganizationScope::Channels;
    let item = || Item {
        id: "generated-channel".into(),
        name: "Generated".into(),
        hidden: false,
    };
    let mut checked = BTreeSet::new();
    macro_rules! check {
        ($name:ident ($($argument:expr),* $(,)?) => $value:ty) => {
            response::<$value>(commands::$name(&owner, $($argument),*));
            assert!(checked.insert(stringify!($name).to_owned()));
        };
    }
    check!(custom_groups(source) => Vec<CustomGroup>);
    check!(members(source, ChannelScope::All) => Vec<Member>);
    check!(group_members(source, ChannelScope::All) => (Vec<CustomGroup>, Vec<Member>));
    check!(save_custom_group(source, None, "Generated".into()) => ());
    check!(remove_custom_members(group, vec![content()]) => ());
    check!(transfer_custom_members(source, None, group, vec![content()], false) => usize);
    check!(delete_custom_group(group) => ());
    check!(hide_channel(source, "generated-channel".into()) => ());
    check!(remember_combined_channel("generated-channel".into()) => ());
    check!(channel(source, content()) => Option<PlaybackChannel>);
    check!(save_search_history(vec!["Generated".into()]) => ());
    check!(organization_items(source, scope(), Vec::new()) => Vec<Item>);
    check!(edit_organization(source, scope(), Editor::new(Vec::new(), None), item(), Action::Later) => ());
    check!(favorite_provider_order(source, vec!["generated-channel".into()]) => Vec<String>);
    check!(rename_organization_item(source, scope(), "generated-channel".into(), "Generated".into()) => ());
    check!(load_search_history(std::path::PathBuf::new()) => Vec<String>);
    check!(combined_resume_key() => Option<String>);

    let source = include_str!("../src/commands.rs");
    let exported: BTreeSet<_> = source
        .lines()
        .filter_map(|line| {
            let function = line
                .trim()
                .strip_prefix("pub fn ")
                .or_else(|| line.trim().strip_prefix("pub async fn "))?;
            Some(function.split(['(', '<']).next().unwrap().to_owned())
        })
        .collect();
    assert_eq!(
        exported, checked,
        "each public command needs a typed result contract"
    );
}

#[test]
fn playback_channel_preserves_provider_data_and_parental_block() {
    let directory = tempfile::tempdir().unwrap();
    let key = iptv_runtime::model::DatabaseKey::from_bytes([0x75; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(directory.path().join("iptv.sqlite"), &key)
            .unwrap();
    let source = store
        .add_source(&common::m3u_source("Generated playback"))
        .unwrap();
    let catalog = common::catalog(2, 1, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalog);
    let channel = catalog.channels[0].clone();
    store.set_blocked(source, &channel.id, true).unwrap();
    drop(store);
    let owner = Runtime::new(Ports::new(directory.path().to_owned(), move |_| {
        Ok(key.clone())
    }));
    owner.start_services();
    let result = support::block_on(commands::channel(&owner, source, channel.id.clone())).unwrap();
    // Exhaustive destructuring also prevents replacing the runtime record with
    // an alias to the adapter's larger, non-exhaustive ChannelRow.
    let PlaybackChannel {
        channel: actual,
        blocked,
    } = result.unwrap();
    assert_eq!(actual, channel);
    assert!(blocked);
    let unblocked = support::block_on(commands::channel(
        &owner,
        source,
        catalog.channels[1].id.clone(),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(unblocked.channel, catalog.channels[1]);
    assert!(!unblocked.blocked);
    let missing = support::block_on(commands::channel(
        &owner,
        source,
        ContentId::from_stored("generated-missing"),
    ))
    .unwrap();
    assert!(missing.is_none());
    assert!(owner.drain_services());
}
