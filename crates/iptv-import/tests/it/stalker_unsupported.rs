//! A build without Stalker portals (feature `stalker` off, as a browser
//! TV package builds it): a stored Stalker source fails as unsupported,
//! before any request, from the import and from playback authorization.

use crate::common::{FakeFetch, block_on};
use iptv_import::{CancelToken, FailureKind, ImportError, ImportOptions, NoProgress};
use iptv_providers::model::{MacAddress, SourceConfig, StalkerCredentials};

fn credentials() -> StalkerCredentials {
    StalkerCredentials {
        portal_url: "http://portal.example.test/stalker_portal/c/".into(),
        mac: MacAddress::parse("00:1A:79:00:00:42").unwrap(),
        serial: None,
        device_id: None,
        device_id2: None,
        signature: None,
        signature2: None,
        login: None,
        password: None,
    }
}

#[test]
fn a_stalker_source_is_unsupported_and_nothing_is_requested() {
    let fetch = FakeFetch::new(|_, _| panic!("no request is sent"));
    let imported = block_on(iptv_import::import_source(
        &fetch,
        &SourceConfig::Stalker(credentials()),
        &ImportOptions::new(0),
        &mut NoProgress,
        &CancelToken::new(),
    ));
    let error = imported.unwrap_err();
    assert_eq!(error, ImportError::Unsupported("Stalker portals"));
    assert_eq!(error.kind(), FailureKind::Config);
    assert_eq!(error.step(), None);
    assert_eq!(
        error.to_string(),
        "Stalker portals are not supported in this build"
    );

    let session = block_on(iptv_import::stalker_session(
        &fetch,
        &credentials(),
        &CancelToken::new(),
    ));
    assert_eq!(
        session.unwrap_err(),
        ImportError::Unsupported("Stalker portals")
    );
    assert!(fetch.requests().is_empty());
}
