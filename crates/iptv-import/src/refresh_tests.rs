use iptv_epg::{ChannelKey, FeedId, Override, Overrides, StoreOptions};
use iptv_providers::model::{Channel, StreamLocator};
use iptv_providers::{ContentId, SecretUrl};
use iptv_settings::{Settings, Target, Value};

use super::*;

const NOW: i64 = 1_800_000_000;

fn feed(id: u32, url: &str) -> EpgSourceConfig {
    EpgSourceConfig::new(FeedId(id), SecretUrl::new(url))
}

fn feeds() -> Vec<EpgSourceConfig> {
    vec![
        feed(1, "http://epg.example.com/a.xml.gz"),
        feed(2, "http://epg.example.com/b.xml"),
    ]
}

/// Fetched `age` seconds ago with programs until `ends_in` from now.
fn stored(age: i64, ends_in: i64, fingerprint: u64) -> GuideState {
    GuideState {
        fetched_at: Some(NOW - age),
        attempted_at: Some(NOW - age),
        last_attempt_failed: false,
        feeds: feeds(),
        match_fingerprint: Some(fingerprint),
        ends_at: Some(NOW + ends_in),
        bound_channels: 10,
    }
}

fn channel(id: &str, tvg: Option<&str>, name: &str) -> Channel {
    Channel {
        id: ContentId::from_stored(id),
        provider_id: None,
        name: name.into(),
        number: None,
        logo: None,
        groups: Vec::new(),
        stream: StreamLocator::Url {
            url: "http://streams.example.com/1.m3u8".into(),
            headers: Vec::new(),
            properties: Vec::new(),
        },
        epg_id: tvg.map(str::to_owned),
        epg_name: None,
        epg_shift_minutes: None,
        catch_up: None,
        radio: false,
    }
}

const FP: u64 = 42;
const DAY_S: i64 = 86_400;

fn decide_at(policy: &RefreshPolicy, trigger: Trigger, state: Option<&GuideState>) -> Decision {
    decide(policy, trigger, &feeds(), FP, state, NOW)
}

#[test]
fn fresh_guide_is_kept_until_the_interval_or_running_out() {
    let p = RefreshPolicy::default();
    let s = stored(HOUR, 3 * DAY_S, FP);
    assert_eq!(
        decide_at(&p, Trigger::Startup, Some(&s)),
        Decision::Keep {
            next_check: Some(NOW - HOUR + DAY_S)
        }
    );
    // Ends in 20 h: early refresh 12 h before the end.
    let s = stored(HOUR, 20 * HOUR, FP);
    assert_eq!(
        decide_at(&p, Trigger::Timer, Some(&s)),
        Decision::Keep {
            next_check: Some(NOW + 8 * HOUR)
        }
    );
}

#[test]
fn stale_or_running_out_refreshes() {
    let p = RefreshPolicy::default();
    let s = stored(25 * HOUR, 3 * DAY_S, FP);
    assert_eq!(
        decide_at(&p, Trigger::Startup, Some(&s)),
        Decision::Refresh(RefreshReason::Stale { age: 25 * HOUR })
    );
    let s = stored(2 * HOUR, 6 * HOUR, FP);
    assert_eq!(
        decide_at(&p, Trigger::Timer, Some(&s)),
        Decision::Refresh(RefreshReason::RunningOut {
            ends_at: NOW + 6 * HOUR
        })
    );
    // A feed with only a few hours of data is not fetched more than hourly.
    let s = stored(HOUR / 2, 2 * HOUR, FP);
    assert_eq!(
        decide_at(&p, Trigger::Timer, Some(&s)),
        Decision::Keep {
            next_check: Some(NOW + HOUR / 2)
        }
    );
}

#[test]
fn missing_or_changed_guides_refresh_even_without_an_interval() {
    let manual_only = RefreshPolicy {
        interval: None,
        ..RefreshPolicy::default()
    };
    assert_eq!(
        decide_at(&manual_only, Trigger::Startup, None),
        Decision::Refresh(RefreshReason::NeverFetched)
    );
    let mut s = stored(3 * DAY_S, -DAY_S, FP);
    assert_eq!(
        decide_at(&manual_only, Trigger::Startup, Some(&s)),
        Decision::Keep { next_check: None },
        "interval 0: old data is only replaced on request"
    );
    s.feeds[1] = feed(2, "http://epg.example.com/moved.xml");
    assert_eq!(
        decide_at(&manual_only, Trigger::Startup, Some(&s)),
        Decision::Refresh(RefreshReason::FeedsChanged)
    );
    let mut s = stored(HOUR, DAY_S, FP);
    s.feeds[0].store = StoreOptions {
        store_descriptions: false,
        ..StoreOptions::default()
    };
    assert_eq!(
        decide_at(&manual_only, Trigger::Timer, Some(&s)),
        Decision::Refresh(RefreshReason::FeedsChanged)
    );
    // Same feeds in another order are the same.
    let mut s = stored(HOUR, 3 * DAY_S, FP);
    s.feeds.reverse();
    assert!(matches!(
        decide_at(&RefreshPolicy::default(), Trigger::Timer, Some(&s)),
        Decision::Keep { .. }
    ));
}

#[test]
fn playlist_changes_follow_the_setting() {
    let p = RefreshPolicy::default();
    let s = stored(HOUR, 3 * DAY_S, FP);
    assert_eq!(
        decide_at(&p, Trigger::PlaylistChanged, Some(&s)),
        Decision::Refresh(RefreshReason::PlaylistChanged)
    );
    let other = stored(HOUR, 3 * DAY_S, FP + 1);
    assert_eq!(
        decide_at(&p, Trigger::Startup, Some(&other)),
        Decision::Refresh(RefreshReason::ChannelsChanged)
    );
    let off = RefreshPolicy {
        on_playlist_change: false,
        ..p
    };
    assert!(matches!(
        decide_at(&off, Trigger::PlaylistChanged, Some(&other)),
        Decision::Keep { .. }
    ));
}

#[test]
fn update_on_start_off_defers_to_the_timer() {
    let p = RefreshPolicy {
        on_start: false,
        ..RefreshPolicy::default()
    };
    let s = stored(25 * HOUR, 3 * DAY_S, FP);
    assert_eq!(
        decide_at(&p, Trigger::Startup, Some(&s)),
        Decision::Keep {
            next_check: Some(NOW + DAY_S)
        }
    );
    assert!(matches!(
        decide_at(&p, Trigger::Timer, Some(&s)),
        Decision::Refresh(RefreshReason::Stale { .. })
    ));
}

#[test]
fn failures_back_off_except_on_request() {
    let p = RefreshPolicy::default();
    let mut s = stored(30 * HOUR, 3 * DAY_S, FP);
    s.attempted_at = Some(NOW - 600);
    s.last_attempt_failed = true;
    assert_eq!(
        decide_at(&p, Trigger::Startup, Some(&s)),
        Decision::Backoff {
            retry_at: NOW - 600 + HOUR
        }
    );
    assert_eq!(
        decide_at(&p, Trigger::Manual, Some(&s)),
        Decision::Refresh(RefreshReason::Requested)
    );
    s.attempted_at = Some(NOW - 2 * HOUR);
    assert!(matches!(
        decide_at(&p, Trigger::Startup, Some(&s)),
        Decision::Refresh(RefreshReason::Stale { .. })
    ));
    // A first fetch that failed backs off too.
    let never = GuideState {
        attempted_at: Some(NOW - 60),
        last_attempt_failed: true,
        ..GuideState::default()
    };
    assert!(matches!(
        decide_at(&p, Trigger::Startup, Some(&never)),
        Decision::Backoff { .. }
    ));
}

#[test]
fn startup_shows_the_stored_guide_first() {
    let p = RefreshPolicy::default();
    let s = stored(25 * HOUR, DAY_S, FP);
    let plan = plan_startup(&p, &feeds(), FP, Some(&s), NOW);
    assert!(plan.use_stored);
    assert!(matches!(plan.decision, Decision::Refresh(_)));
    let plan = plan_startup(&p, &feeds(), FP, None, NOW);
    assert!(!plan.use_stored);
    // The playlist no longer declares a guide: drop the stored one.
    let plan = plan_startup(&p, &[], FP, Some(&s), NOW);
    assert_eq!((plan.use_stored, plan.decision), (false, Decision::Discard));
    assert_eq!(
        decide(&p, Trigger::Startup, &[], FP, None, NOW),
        Decision::Keep { next_check: None }
    );
}

#[test]
fn fingerprint_tracks_matching_inputs_only() {
    let a = channel("m3u:a", Some("a.example"), "Alpha");
    let b = channel("m3u:b", None, "Beta");
    let none = Overrides::new();
    let fp = matching_fingerprint(&[a.clone(), b.clone()], &none);
    assert_eq!(fp, matching_fingerprint(&[b.clone(), a.clone()], &none));
    // Pinned: the value is persisted, so it must not change between builds.
    assert_eq!(fp, 0x233e_9aa6_67c6_a4e4, "{fp:#x}");
    let mut logo = a.clone();
    logo.logo = Some("http://img.example.com/a.png".into());
    assert_eq!(fp, matching_fingerprint(&[logo, b.clone()], &none));
    let mut renamed = a.clone();
    renamed.name = "Alpha HD".into();
    assert_ne!(fp, matching_fingerprint(&[renamed, b.clone()], &none));
    let mut tvg = b.clone();
    tvg.epg_id = Some("b.example".into());
    assert_ne!(fp, matching_fingerprint(&[a.clone(), tvg], &none));
    let mut o = Overrides::new();
    o.insert(ChannelKey::new("m3u:b"), Override::NoEpg);
    let with = matching_fingerprint(&[a.clone(), b.clone()], &o);
    assert_ne!(fp, with);
    o.insert(
        ChannelKey::new("m3u:b"),
        Override::Epg {
            source: Some(FeedId(1)),
            epg_id: "x.example".into(),
        },
    );
    assert_ne!(with, matching_fingerprint(&[a, b], &o));
}

#[test]
fn policy_and_storage_from_settings() {
    let mut settings = Settings::new();
    let at = Target::Global;
    assert_eq!(
        RefreshPolicy::from_settings(&settings, &at),
        RefreshPolicy::default()
    );
    assert_eq!(
        store_options(&settings, &at),
        StoreOptions {
            store_descriptions: true,
            past_days_to_keep: Some(7),
        }
    );
    settings
        .set("epg.update_interval", Value::Duration(0))
        .unwrap();
    settings
        .set("epg.update_on_start", Value::Bool(false))
        .unwrap();
    settings
        .set("epg.store_descriptions", Value::Bool(false))
        .unwrap();
    let p = RefreshPolicy::from_settings(&settings, &at);
    assert_eq!((p.interval, p.on_start), (None, false));
    assert!(!store_options(&settings, &at).store_descriptions);
}
