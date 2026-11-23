use iptv_providers::{ProviderError, StalkerError, StalkerErrorKind};

use super::*;

#[test]
fn startup_and_periodic_playlist_refreshes_follow_the_source_policy() {
    let mut policy = SourcePolicy::default();
    assert!(
        policy.due(100, Some(100), true),
        "startup refreshes immediately"
    );
    assert!(
        !policy.due(100, None, false),
        "periodic refresh is disabled by default"
    );

    policy.refresh_on_start = false;
    assert!(
        !policy.due(100, None, true),
        "startup refresh can be disabled"
    );
    policy.refresh_seconds = 60;
    assert!(
        policy.due(100, None, false),
        "a source without an attempt is due"
    );
    assert!(
        !policy.due(159, Some(100), true),
        "the last attempt starts the interval"
    );
    assert!(
        policy.due(160, Some(100), false),
        "the refresh interval is inclusive"
    );
    assert!(
        !policy.due(i64::MIN, Some(i64::MAX), false),
        "a backwards clock delays refresh"
    );
    assert!(
        policy.due(i64::MAX, Some(i64::MIN), false),
        "elapsed time saturates"
    );

    policy.enabled = false;
    policy.refresh_on_start = true;
    assert!(
        !policy.due(160, None, true),
        "disabled sources stay out of both schedules"
    );
}

#[test]
fn channel_order_keys_keep_custom_and_provider_group_identities_distinct() {
    assert_eq!(
        OrderScope::All.key(),
        "",
        "existing all-channel preference key"
    );
    assert_eq!(
        OrganizationScope::group_channels("custom:23"),
        OrganizationScope::CustomChannels(CustomGroupId(23))
    );
    for stored in [
        "synthetic:news",
        "custom:news",
        "custom:9223372036854775808",
    ] {
        let OrganizationScope::GroupChannels(group) = OrganizationScope::group_channels(stored)
        else {
            panic!("{stored} is an opaque provider group identity");
        };
        assert_eq!(OrderScope::Group(group).key(), stored);
    }
}

#[test]
fn provider_failures_persist_only_a_stable_failure_category() {
    use ImportErrorKind::*;

    for (error, expected, slug) in [
        (ProviderError::HttpStatus(503), HttpStatus, "http_status"),
        (
            ProviderError::InvalidBody("synthetic response".into()),
            InvalidBody,
            "invalid_body",
        ),
        (
            ProviderError::AccountRejected,
            AccountRejected,
            "account_rejected",
        ),
        (
            StalkerError::new(StalkerErrorKind::DeviceConflict).into(),
            Blocked,
            "blocked",
        ),
        (
            StalkerError::new(StalkerErrorKind::NoToken).into(),
            AuthFailed,
            "auth_failed",
        ),
        (
            StalkerError::with_text(StalkerErrorKind::Portal, Some("synthetic detail".into()))
                .into(),
            Other,
            "other",
        ),
    ] {
        let kind = ImportErrorKind::from(&error);
        assert_eq!(kind, expected, "{error:?}");
        assert_eq!(kind.slug(), slug, "stored failure category");
        assert_eq!(ImportErrorKind::from_slug(slug), expected);
    }
}

#[test]
fn an_inspection_failure_retains_its_source_when_it_crosses_into_repair() {
    use crate::{guide_repair::Error, inspection::InspectionError};

    let error = InspectionError::SourceDisabled(SourceId(23));
    assert!(error.to_string().contains("23"));
    let repair = Error::from(error.clone());
    assert_eq!(repair, Error::Inspection(error));
    assert!(repair.to_string().contains("SourceDisabled"));
    assert!(repair.to_string().contains("23"));
}
