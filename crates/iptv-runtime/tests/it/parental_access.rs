//! Synthetic catalogue exercises direct-ID gates independently of shell menus.
use crate::common;
use crate::support;
use iptv_core::parental::{ContentKind, Pin, Target};
use iptv_runtime::{Ports, Runtime, commands};
use iptv_settings::Value;
use std::rc::Rc;

#[test]
fn channel_category_movie_settings_and_restart_enforce_authority() {
    let directory = tempfile::tempdir().unwrap();
    let key = iptv_runtime::model::DatabaseKey::from_bytes([0x79; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(directory.path().join("iptv.sqlite"), &key)
            .unwrap();
    let source = store
        .add_source(&common::m3u_source("Generated parental fixture"))
        .unwrap();
    let other = store
        .add_source(&common::m3u_source("Generated separate playlist"))
        .unwrap();
    let mut catalog = common::catalog(2, 2, common::XT_PASSWORD);
    catalog.groups[0].adult = true;
    catalog.groups.last_mut().unwrap().adult = true;
    catalog.vod[0].is_series = true;
    common::replace(&mut store, source, &catalog);
    common::replace(&mut store, other, &catalog);
    let custom = store
        .create_custom_group(source, "Generated protected collection")
        .unwrap();
    let destination = store
        .create_custom_group(source, "Generated destination")
        .unwrap();
    store
        .add_to_custom_group(custom, &catalog.channels[1].id)
        .unwrap();
    let custom_target = Target {
        source: source.0,
        kind: ContentKind::Group,
        id: format!("custom:{}", custom.0),
    };
    assert!(
        store
            .parental_ancestors(&Target::live(source.0, catalog.channels[1].id.as_str()))
            .unwrap()
            .contains(&custom_target)
    );
    assert!(
        store.parental_memberships().unwrap()
            [&Target::live(source.0, catalog.channels[1].id.as_str())]
            .contains(&custom_target)
    );
    drop(store);
    let owner = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x79; 32])),
    )));
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    support::block_on(owner.parental_refresh()).unwrap();
    let recovery =
        support::block_on(owner.parental_change_pin(None, Pin::parse("2468").unwrap())).unwrap();
    assert_eq!(recovery.len(), 32);
    let disabled_future = commands::channel(&owner, source, catalog.channels[0].id.clone());
    owner
        .set_setting("parental.live_enabled", Value::Bool(true))
        .unwrap();
    owner
        .set_setting("parental.vod_enabled", Value::Bool(true))
        .unwrap();
    owner
        .set_setting(
            "parental.require_pin_for",
            Value::Enum("blocked_content_and_settings".into()),
        )
        .unwrap();
    let channel = Target::live(source.0, catalog.channels[0].id.as_str());
    let public = Target::live(source.0, catalog.channels[1].id.as_str());
    let movie = Target {
        source: source.0,
        kind: ContentKind::Movie,
        id: catalog.vod[0].id.as_str().into(),
    };
    support::block_on(owner.parental_set_blocked(movie.clone(), true)).unwrap();
    owner.parental_lock();
    assert!(
        support::block_on(disabled_future).unwrap().is_none(),
        "a command made with protection disabled cannot outlive a protection change"
    );
    assert!(support::block_on(owner.parental_check(&public)).is_ok());
    assert!(support::block_on(owner.parental_check(&channel)).is_err());
    assert!(support::block_on(owner.parental_check(&movie)).is_err());
    let legacy_series = Target {
        kind: ContentKind::Series,
        ..movie.clone()
    };
    assert!(
        support::block_on(owner.parental_check(&legacy_series)).is_err(),
        "Stalker series stored in vod inherit their adult category"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let request = iptv_core::recording::Request {
        source: source.0,
        channel: channel.id.clone(),
        channel_name: "Generated channel".into(),
        title: "Generated program".into(),
        start: now + 3600,
        end: now + 7200,
    };
    assert!(
        support::block_on(owner.schedule_recording(request)).is_err(),
        "direct recording admission must not bypass a locked channel"
    );
    assert!(
        support::block_on(commands::channel(
            &owner,
            source,
            catalog.channels[0].id.clone()
        ))
        .unwrap()
        .is_none()
    );
    assert!(
        owner
            .set_setting("parental.live_enabled", Value::Bool(false))
            .is_err()
    );
    assert!(
        owner
            .set_setting("general.clock", Value::Enum("24h".into()))
            .is_err()
    );
    support::block_on(owner.parental_authorize(channel.clone(), Pin::parse("2468").unwrap()))
        .unwrap();
    assert!(support::block_on(owner.parental_check(&channel)).is_ok());
    assert!(
        support::block_on(commands::channel(
            &owner,
            source,
            catalog.channels[0].id.clone()
        ))
        .unwrap()
        .is_some()
    );
    let pending = commands::channel(&owner, source, catalog.channels[0].id.clone());
    owner.parental_lock();
    assert!(
        support::block_on(pending).unwrap().is_none(),
        "locking revokes a previously constructed command future"
    );
    assert!(!owner.parental_allowed(&Target::live(other.0, catalog.channels[0].id.as_str())));
    assert!(!owner.parental_settings_allowed());
    owner.parental_unblocked_channel_played();
    assert!(support::block_on(owner.parental_check(&channel)).is_err());
    support::block_on(owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()))
        .unwrap();
    assert!(owner.parental_settings_allowed());
    let group = Target {
        source: source.0,
        kind: ContentKind::Group,
        id: catalog.groups[0].id.as_str().into(),
    };
    support::block_on(owner.parental_set_blocked(group.clone(), false)).unwrap();
    owner.parental_lock();
    assert!(support::block_on(owner.parental_check(&channel)).is_ok());
    support::block_on(owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()))
        .unwrap();
    support::block_on(owner.parental_set_blocked(group, true)).unwrap();
    support::block_on(owner.parental_set_blocked(custom_target.clone(), true)).unwrap();
    owner.parental_lock();
    let public_id = catalog.channels[1].id.clone();
    assert!(support::block_on(owner.parental_check(&public)).is_err());
    assert!(
        support::block_on(commands::remove_custom_members(
            &owner,
            custom,
            vec![public_id.clone()]
        ))
        .is_err()
    );
    assert!(support::block_on(commands::delete_custom_group(&owner, custom)).is_err());
    assert!(
        support::block_on(commands::transfer_custom_members(
            &owner,
            source,
            Some(custom),
            destination,
            vec![public_id.clone()],
            true
        ))
        .is_err()
    );
    support::block_on(owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()))
        .unwrap();
    let stale_delete = commands::delete_custom_group(&owner, custom);
    owner.parental_lock();
    assert!(support::block_on(stale_delete).is_err());
    support::block_on(owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()))
        .unwrap();
    support::block_on(commands::remove_custom_members(
        &owner,
        custom,
        vec![public_id],
    ))
    .unwrap();
    assert!(support::block_on(owner.parental_check(&public)).is_ok());
    support::block_on(commands::delete_custom_group(&owner, custom)).unwrap();
    support::block_on(owner.parental_refresh()).unwrap();
    assert!(!owner.parental_explicitly_blocked(&custom_target));
    owner.set_foreground(false);
    assert!(!owner.parental_settings_allowed());
    assert!(owner.flush_files(std::time::Duration::from_secs(2)));
    assert!(owner.drain_services());
    let second = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x79; 32])),
    )));
    second.load_settings();
    second.start_services();
    support::block_on(second.parental_refresh()).unwrap();
    assert!(second.parental_has_pin());
    assert!(support::block_on(second.parental_check(&channel)).is_err());
    assert!(!second.parental_settings_allowed());
    assert!(second.drain_services());
}
