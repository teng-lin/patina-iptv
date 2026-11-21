//! The schedules' lookup indexes (key → channel, channel → group) agree
//! with the trait's scans and stay cheap on a very large playlist.

use std::rc::Rc;

use iptv_core::guide::Schedule;
use iptv_core::lists::{group_of, scan_group_of};
use iptv_epg::{ChannelKey, GuideStore};

use super::combined::{CombinedSchedule, SourceSchedule, qualified_key};
use super::{EpgSchedule, GuideChannel, Organization};

const NOW: i64 = 1_800_000_000;

/// `groups` groups of `per_group` channels, keyed `c{n}`, in group order.
fn grouped(groups: usize, per_group: usize) -> EpgSchedule {
    let groups = (0..groups)
        .map(|g| {
            let members = (0..per_group)
                .map(|i| {
                    let n = g * per_group + i;
                    GuideChannel {
                        key: ChannelKey::new(format!("c{n}")),
                        name: format!("Channel {n}"),
                        number: n as u32 + 1,
                        catchup_days: 0,
                    }
                })
                .collect();
            (format!("Group {g}"), members)
        })
        .collect();
    EpgSchedule::new(groups, GuideStore::default(), NOW)
}

/// 100,000 channels in 1,000 groups: every key finds its channel and every
/// channel its group, without the scans (which would ask each group for a
/// copy of the playlist order: about 10⁸ steps per channel here).
#[test]
fn a_hundred_thousand_channels_find_their_key_and_group() {
    let schedule = grouped(1_000, 100);
    assert_eq!(schedule.channel_count(), 100_000);
    for n in 0..100_000 {
        assert_eq!(schedule.find_channel(&format!("c{n}")), Some(n));
        assert_eq!(group_of(&schedule, n), Some(n / 100));
    }
    assert_eq!(schedule.find_channel("c100000"), None);
    assert_eq!(group_of(&schedule, 100_000), None);
}

/// Organization changes the groups (hidden, reordered, custom lists): the
/// index follows, and agrees with the scan everywhere.
#[test]
fn the_group_index_follows_the_organization_and_agrees_with_the_scan() {
    let schedule = grouped(6, 5);
    let agree = |schedule: &EpgSchedule| {
        for channel in 0..schedule.channel_count() {
            assert_eq!(
                schedule.channel_group(channel),
                scan_group_of(schedule, channel),
                "channel {channel}"
            );
        }
    };
    agree(&schedule);
    let group = |g: usize, hidden: bool| (format!("Group {g}"), format!("Name {g}"), hidden);
    schedule.set_organization(Organization {
        channels: Vec::new(),
        groups: vec![group(4, false), group(1, true), group(0, false)],
        ..Default::default()
    });
    agree(&schedule);
    assert_eq!(group_of(&schedule, 7), None, "its group is hidden");
    assert_eq!(group_of(&schedule, 22), Some(0), "group 4 comes first");
    let keys = |ns: &[usize]| ns.iter().map(|n| format!("c{n}")).collect();
    schedule.set_group_lists(vec![
        ("custom:1".into(), "Mine".into(), keys(&[29, 3, 3, 12])),
        ("Group 0".into(), "Name 0".into(), keys(&[0, 1, 2, 3, 4])),
    ]);
    agree(&schedule);
    assert_eq!(group_of(&schedule, 3), Some(0), "the first list holding it");
    assert_eq!(group_of(&schedule, 5), None);
}

/// A combined guide: keys qualified by source, categories before the
/// playlist rows, and a folded source's channels under its playlist row,
/// as the scans find them.
#[test]
fn the_combined_indexes_agree_with_the_scans() {
    let part = |id: i64, groups: usize| SourceSchedule {
        id,
        name: format!("Source {id}"),
        schedule: Rc::new(grouped(groups, 4)),
    };
    let all = CombinedSchedule::new(vec![part(7, 3), part(9, 2)], NOW);
    let folded = all.with_browser_expansion([9]);
    for schedule in [&all, &folded] {
        for channel in 0..schedule.channel_count() {
            let key = schedule.channel_key(channel).unwrap();
            assert_eq!(schedule.find_channel(&key), Some(channel));
            assert_eq!(
                schedule.channel_group(channel),
                scan_group_of(schedule, channel),
                "channel {channel}"
            );
        }
        assert_eq!(schedule.find_channel(&qualified_key(8, "c0")), None);
        assert_eq!(schedule.find_channel("c0"), None, "not source-qualified");
    }
    assert_eq!(group_of(&all, 13), Some(5), "source 9's first category");
    assert_eq!(group_of(&folded, 13), Some(4), "source 9's playlist row");
}

/// The cost of the indexes on the large playlist. Debug builds are far
/// slower; run in release:
/// `cargo test --release -p iptv-guide index_tests -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn lookups_on_a_hundred_thousand_channels_stay_cheap() {
    use std::time::{Duration, Instant};
    let schedule = grouped(1_000, 100);
    let started = Instant::now();
    assert_eq!(schedule.find_channel("c99999"), Some(99_999));
    assert_eq!(group_of(&schedule, 99_999), Some(999));
    let first = started.elapsed();
    let started = Instant::now();
    for n in (0..100_000).step_by(7) {
        assert_eq!(schedule.find_channel(&format!("c{n}")), Some(n));
        assert_eq!(group_of(&schedule, n), Some(n / 100));
    }
    let each = started.elapsed() / (100_000 / 7);
    println!("indexes built and first lookups: {first:?}; then per key and group: {each:?}");
    assert!(first < Duration::from_millis(250), "{first:?}");
    assert!(each < Duration::from_micros(20), "{each:?}");
}
