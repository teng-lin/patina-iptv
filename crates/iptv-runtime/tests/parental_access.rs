//! Synthetic catalogue exercises direct-ID gates independently of shell menus.
#[path = "../../iptv-store-sqlite/tests/common/mod.rs"]
mod common;
mod support;
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
    let mut catalogue = common::catalog(2, 2, common::XT_PASSWORD);
    catalogue.groups[0].adult = true;
    common::replace(&mut store, source, &catalogue);
    common::replace(&mut store, other, &catalogue);
    drop(store);
    let owner = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        move |_| Ok(key.clone()),
    )));
    owner.start_services();
    support::block_on(owner.parental_refresh()).unwrap();
    let recovery =
        support::block_on(owner.parental_change_pin(None, Pin::parse("2468").unwrap())).unwrap();
    assert_eq!(recovery.len(), 32);
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
    let channel = Target::live(source.0, catalogue.channels[0].id.as_str());
    let public = Target::live(source.0, catalogue.channels[1].id.as_str());
    let movie = Target {
        source: source.0,
        kind: ContentKind::Movie,
        id: catalogue.vod[0].id.as_str().into(),
    };
    support::block_on(owner.parental_set_blocked(movie.clone(), true)).unwrap();
    owner.parental_lock();
    assert!(support::block_on(owner.parental_check(&public)).is_ok());
    assert!(support::block_on(owner.parental_check(&channel)).is_err());
    assert!(support::block_on(owner.parental_check(&movie)).is_err());
    assert!(
        support::block_on(commands::channel(
            &owner,
            source,
            catalogue.channels[0].id.clone()
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
            catalogue.channels[0].id.clone()
        ))
        .unwrap()
        .is_some()
    );
    assert!(!owner.parental_allowed(&Target::live(other.0, catalogue.channels[0].id.as_str())));
    assert!(!owner.parental_settings_allowed());
    owner.parental_unblocked_channel_played();
    assert!(support::block_on(owner.parental_check(&channel)).is_err());
    support::block_on(owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()))
        .unwrap();
    assert!(owner.parental_settings_allowed());
    owner.set_foreground(false);
    assert!(!owner.parental_settings_allowed());
    assert!(owner.drain_services());
}
