//! Older days paged in on demand (`pages.rs`).

use std::cell::RefCell;
use std::rc::Rc;

use iptv_core::guide::{Program, Schedule};
use iptv_epg::{
    ChannelKey, DAY, EpgRef, FeedId, GuideStore, MatchKind, ProgramData, SourceBuilder,
    StoreOptions,
};

use super::pages::{MAX_PAGES, PageRequest, PagedWindow};
use super::{EpgSchedule, GuideChannel, LOADING, NO_INFORMATION};

/// Hour-aligned "now".
pub(crate) const NOW: i64 = 1_800_000_000;
/// Programs of 45 minutes: never on the hour slots a loading day shows.
pub(crate) const LEN: i64 = 45 * 60;
pub(crate) const FIRST: i64 = NOW - 6 * DAY + 600;

/// Channel `a`'s programs overlapping `[from, to)`, as the store reads a
/// window back: `P<n>` from [`FIRST`] on, every [`LEN`], until a day ahead.
pub(crate) fn store_between(from: i64, to: i64) -> GuideStore {
    let mut b = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), NOW);
    let titles: Vec<String> = (0..(7 * DAY / LEN)).map(|n| format!("P{n}")).collect();
    for (n, title) in titles.iter().enumerate() {
        let start = FIRST + n as i64 * LEN;
        if start < to && start + LEN > from {
            b.push_data(
                "a.test",
                &ProgramData {
                    start,
                    stop: Some(start + LEN),
                    title,
                    ..Default::default()
                },
            );
        }
    }
    let mut store = GuideStore::new();
    store.replace_source(b.finish());
    let cand = EpgRef {
        source: FeedId(1),
        epg_id: "a.test".into(),
        kind: MatchKind::TvgId,
    };
    store.set_matches([(ChannelKey::new("a"), vec![cand])].into_iter().collect());
    store
}

/// The title airing at unix time `t` in the full guide.
pub(crate) fn title_at(t: i64) -> String {
    format!("P{}", (t - FIRST) / LEN)
}

pub(crate) type Requests = Rc<RefCell<Vec<PageRequest>>>;

/// Holds now − 1 day on; pages back to `oldest`. Requests are recorded.
pub(crate) fn paged(oldest: i64) -> (EpgSchedule, Requests) {
    let loaded_from = NOW - DAY;
    let channel = GuideChannel {
        key: ChannelKey::new("a"),
        name: "Alpha".to_owned(),
        number: 1,
        catchup_days: 7,
    };
    let requests = Requests::default();
    let seen = requests.clone();
    let s = EpgSchedule::new(
        vec![("All".to_owned(), vec![channel])],
        store_between(loaded_from, i64::MAX),
        NOW,
    )
    .with_pages(
        PagedWindow {
            loaded_from,
            oldest,
        },
        [(ChannelKey::new("a"), FIRST)].into(),
        move |r| seen.borrow_mut().push(r),
    );
    (s, requests)
}

/// Delivers every page asked for so far, as the app's loader would.
pub(crate) fn deliver(s: &EpgSchedule, requests: &Requests) {
    for r in requests.borrow_mut().drain(..) {
        assert!(s.insert_page(r, store_between(r.from, r.to)), "{r:?}");
    }
}

pub(crate) fn minute_of(s: &EpgSchedule, t: i64) -> i32 {
    s.minute(t)
}

#[test]
fn the_recent_window_needs_no_page() {
    let (s, requests) = paged(NOW - 7 * DAY);
    for t in [NOW, NOW - 3600, NOW - DAY + 60, NOW + DAY / 2] {
        let p = s.program_at(0, minute_of(&s, t));
        assert_eq!(s.title(0, p), title_at(t));
    }
    assert!(requests.borrow().is_empty());
    assert_eq!(s.loaded_from(), Some(NOW - DAY));
}

#[test]
fn an_older_day_loads_on_demand_and_then_shows() {
    let (s, requests) = paged(NOW - 7 * DAY);
    let t = NOW - DAY - 5 * 3600 - 600;
    let m = minute_of(&s, t);
    let loading = s.program_at(0, m);
    assert_eq!(s.title(0, loading), LOADING);
    let slot = t - t.rem_euclid(3600);
    assert_eq!(
        loading,
        Program {
            start: minute_of(&s, slot),
            end: minute_of(&s, slot + 3600)
        },
        "an hour slot while loading"
    );
    // Asked for once, however often it is read meanwhile.
    s.program_at(0, m - 30);
    assert!(!s.ensure_loaded(t));
    assert_eq!(
        requests
            .borrow()
            .iter()
            .map(|r| (r.page, r.from, r.to))
            .collect::<Vec<_>>(),
        [(0, NOW - 2 * DAY, NOW - DAY)]
    );
    deliver(&s, &requests);
    assert!(s.ensure_loaded(t));
    let p = s.program_at(0, m);
    assert_eq!(s.title(0, p), title_at(t));
    assert_eq!(
        s.unix(p.start),
        FIRST + (t - FIRST) / LEN * LEN,
        "its real start"
    );
    assert_eq!(s.paged_days(), 1);
}

#[test]
fn programs_across_the_window_edge_read_the_same_from_both_sides() {
    let (s, requests) = paged(NOW - 7 * DAY);
    let edge = NOW - DAY;
    // The program airing at the edge started before it.
    let straddling = s.program_at(0, minute_of(&s, edge));
    assert!(s.unix(straddling.start) < edge);
    assert_eq!(s.title(0, straddling), title_at(edge));
    s.program_at(0, minute_of(&s, edge) - 1);
    deliver(&s, &requests);
    let before = s.program_at(0, minute_of(&s, edge) - 1);
    assert_eq!(before, straddling);
    assert_eq!(s.title(0, before), title_at(edge));
}

#[test]
fn days_before_what_the_store_keeps_are_empty_without_a_read() {
    let (s, requests) = paged(NOW - 3 * DAY);
    let t = NOW - 4 * DAY - 7200;
    let p = s.program_at(0, minute_of(&s, t));
    assert_eq!(s.title(0, p), NO_INFORMATION);
    assert!(s.ensure_loaded(t));
    assert!(requests.borrow().is_empty());
}

#[test]
fn pages_are_capped_and_dropped_back_at_now() {
    let (s, requests) = paged(NOW - 7 * DAY);
    let day = |k: i64| NOW - DAY - k * DAY - 3600;
    for k in 0..5 {
        s.ensure_loaded(day(k));
        deliver(&s, &requests);
        assert!(s.paged_days() <= MAX_PAGES);
    }
    assert_eq!(s.paged_days(), MAX_PAGES);
    // The least recently used went: day 4, 3 and 2 are held.
    assert!(!s.ensure_loaded(day(0)));
    requests.borrow_mut().clear();
    for k in 2..5 {
        assert!(s.ensure_loaded(day(k)), "day {k}");
    }
    // The archive playing keeps its day; the rest go back at now.
    s.pin_page(Some(day(3)));
    assert_eq!(s.evict_pages(), 2);
    assert_eq!(s.paged_days(), 1);
    assert!(s.ensure_loaded(day(3)));
    s.pin_page(None);
    assert_eq!(s.evict_pages(), 1);
    assert_eq!(s.paged_days(), 0);
}

#[test]
fn a_page_arriving_after_it_was_dropped_or_for_another_window_is_refused() {
    let (s, requests) = paged(NOW - 7 * DAY);
    s.ensure_loaded(NOW - 2 * DAY);
    let r = requests.borrow()[0];
    s.evict_pages();
    assert!(!s.insert_page(r, store_between(r.from, r.to)));
    assert_eq!(s.paged_days(), 0);
    // Another schedule's window (an hour later): not this one's page.
    s.ensure_loaded(NOW - 2 * DAY);
    let current = requests.borrow()[1];
    let shifted = PageRequest {
        from: r.from + 3600,
        to: r.to + 3600,
        ..current
    };
    assert!(!s.insert_page(shifted, store_between(shifted.from, shifted.to)));
    assert!(!s.insert_page(r, store_between(r.from, r.to)));
    assert!(s.insert_page(current, store_between(current.from, current.to)));
}

#[test]
fn without_pages_the_store_is_the_whole_guide() {
    let channel = GuideChannel {
        key: ChannelKey::new("a"),
        name: "Alpha".to_owned(),
        number: 1,
        catchup_days: 0,
    };
    let s = EpgSchedule::new(
        vec![("All".to_owned(), vec![channel])],
        store_between(i64::MIN, i64::MAX),
        NOW,
    );
    let t = NOW - 5 * DAY;
    assert_eq!(s.title(0, s.program_at(0, minute_of(&s, t))), title_at(t));
    assert_eq!(s.loaded_from(), None);
}

#[test]
fn the_guide_browses_from_the_oldest_day_kept_to_past_its_last_program() {
    let (s, _) = paged(NOW - 3 * DAY);
    let now = minute_of(&s, NOW);
    // The synthetic guide ends a day (and ten minutes) after now.
    let end = minute_of(&s, FIRST + 7 * DAY);
    assert_eq!(
        s.browsable(now),
        (minute_of(&s, NOW - 3 * DAY), end + super::PAST_END)
    );
    // A guide without listings has no retained past.
    let bare = EpgSchedule::new(vec![], GuideStore::new(), NOW);
    let horizon = iptv_core::guide::HORIZON;
    assert_eq!(bare.browsable(0), (0, horizon));
    // Without pages (the whole guide in memory): its first program.
    let whole = EpgSchedule::new(vec![], store_between(NOW - 2 * DAY, NOW + DAY), NOW);
    let first = whole.browsable(minute_of(&whole, NOW)).0;
    let held = minute_of(&whole, NOW - 2 * DAY);
    assert!(
        first <= held && first > held - LEN as i32 / 60,
        "{first} {held}"
    );
    // Data ending before now still leaves a screen ahead to browse.
    let late = end + 500;
    let (first, last) = s.browsable(late);
    assert_eq!(last, late + iptv_core::guide::VISIBLE);
    assert!(first <= late);
}

#[test]
fn equal_windows_and_rerequests_do_not_accept_another_requests_reply() {
    let (a, requests_a) = paged(NOW - 7 * DAY);
    let (b, requests_b) = paged(NOW - 7 * DAY);
    let time = NOW - 2 * DAY;
    assert!(!a.ensure_loaded(time));
    assert!(!b.ensure_loaded(time));
    let old = requests_a.borrow()[0];
    assert!(!b.insert_page(old, GuideStore::default()));
    assert!(b.insert_page(requests_b.borrow()[0], GuideStore::default()));
    a.evict_pages();
    assert!(!a.ensure_loaded(time));
    assert!(!a.insert_page(old, GuideStore::default()));
    assert!(a.insert_page(requests_a.borrow()[1], GuideStore::default()));
}

#[test]
fn failed_pages_do_not_render_retry_or_accept_stale_failure_after_explicit_retry() {
    let (schedule, requests) = paged(NOW - 7 * DAY);
    let time = NOW - 2 * DAY;
    assert!(!schedule.ensure_loaded(time));
    let first = requests.borrow()[0];
    assert!(schedule.fail_page(first));
    for _ in 0..10 {
        schedule.program_at(0, minute_of(&schedule, time));
    }
    assert_eq!(requests.borrow().len(), 1);
    assert_eq!(schedule.paged_days(), 0);
    assert!(!schedule.ensure_loaded(time));
    let second = requests.borrow()[1];
    assert_ne!(first, second);
    assert!(!schedule.fail_page(first));
    assert!(schedule.insert_page(second, store_between(second.from, second.to)));
    assert!(schedule.ensure_loaded(time));
}

#[test]
fn an_unpaged_schedule_needs_no_page_for_full_width_times() {
    let schedule = EpgSchedule::new(Vec::new(), GuideStore::default(), NOW);
    for time in [i64::MIN, 0, NOW, i64::MAX] {
        assert!(schedule.ensure_loaded(time));
        assert_eq!(schedule.pending_page(time), None);
    }
    let held = schedule.held_only(NOW, NOW + 3600);
    assert!(held.ensure_loaded(NOW));
    assert!(!held.ensure_loaded(NOW + 7200));
    assert_eq!(held.pending_page(NOW + 7200), None);
}
