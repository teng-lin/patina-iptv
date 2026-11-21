//! A cold start's first screen (`EpgSchedule::held_only`): only its hours
//! are held; other times show as loading and ask for nothing.

use std::cell::RefCell;
use std::rc::Rc;

use iptv_core::guide::Schedule;
use iptv_epg::{
    ChannelKey, DAY, EpgRef, FeedId, GuideStore, MatchKind, ProgramData, SourceBuilder,
    StoreOptions,
};

use super::pages::{PageRequest, PagedWindow};
use super::{EpgSchedule, GuideChannel, LOADING, NO_INFORMATION};

/// Hour-aligned "now".
const NOW: i64 = 1_800_000_000;
/// Programs of 45 minutes from two days back: never on the hour slots.
const LEN: i64 = 45 * 60;
const FIRST: i64 = NOW - 2 * DAY + 600;
/// The first screen: the hour before now to six hours on.
const FROM: i64 = NOW - 3600;
const TO: i64 = NOW + 6 * 3600;

/// Channel `a`'s programs overlapping `[from, to)`, as the store reads them.
fn store_between(from: i64, to: i64) -> GuideStore {
    let mut b = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), NOW);
    let titles: Vec<String> = (0..(4 * DAY / LEN)).map(|n| format!("P{n}")).collect();
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

/// The first screen's schedule, paged as the app's (requests recorded).
fn first_screen() -> (EpgSchedule, Rc<RefCell<Vec<PageRequest>>>) {
    let channel = GuideChannel {
        key: ChannelKey::new("a"),
        name: "Alpha".to_owned(),
        number: 1,
        catchup_days: 7,
    };
    let requests = Rc::new(RefCell::new(Vec::new()));
    let seen = requests.clone();
    let s = EpgSchedule::new(
        vec![("All".to_owned(), vec![channel])],
        store_between(FROM, TO),
        NOW,
    )
    .with_pages(
        PagedWindow {
            loaded_from: NOW - DAY,
            oldest: NOW - 2 * DAY,
        },
        [(ChannelKey::new("a"), FIRST)].into(),
        move |r| seen.borrow_mut().push(r),
    )
    .held_only(FROM, TO);
    (s, requests)
}

fn title_at(s: &EpgSchedule, t: i64) -> String {
    let p = s.program_at(0, s.minute(t));
    s.title(0, p)
}

#[test]
fn only_the_first_screen_is_held_and_nothing_is_asked_for() {
    let (s, requests) = first_screen();
    assert_eq!(s.held(), Some((FROM, TO)));
    // Now and the next hours: real programs.
    let now = title_at(&s, NOW);
    assert_eq!(now, format!("P{}", (NOW - FIRST) / LEN));
    assert_ne!(title_at(&s, NOW + 5 * 3600), LOADING);
    // Later, earlier today and older days: loading, with no page read.
    for t in [TO + 3600, NOW + DAY, NOW - 3 * 3600, NOW - DAY - 3600] {
        assert_eq!(title_at(&s, t), LOADING, "{}", t - NOW);
    }
    assert!(!s.ensure_loaded(NOW + DAY));
    assert!(s.ensure_loaded(NOW));
    assert!(requests.borrow().is_empty(), "{:?}", requests.borrow());
}

#[test]
fn a_program_across_the_first_screens_end_is_held_whole() {
    let (s, _) = first_screen();
    // The program airing at the held end reaches past it.
    let across = (TO - FIRST) / LEN;
    let (start, stop) = (FIRST + across * LEN, FIRST + (across + 1) * LEN);
    assert!(start < TO && stop > TO);
    let p = s.program_at(0, s.minute(TO + 60));
    assert_eq!((s.unix(p.start), s.unix(p.end)), (start, stop));
    assert_eq!(s.title(0, p), format!("P{across}"));
    // Cells across the edge follow each other: the loading slot after it
    // starts where it ends.
    let cells = s.programs_between(0, s.minute(TO - 3600), 4 * 60);
    for pair in cells.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "{cells:?}");
    }
    let next = s.program_at(0, s.minute(stop));
    assert_eq!(s.unix(next.start), stop);
    assert_eq!(s.title(0, next), LOADING);
}

#[test]
fn channels_off_the_first_screen_show_as_loading() {
    let channel = |key: &str| GuideChannel {
        key: ChannelKey::new(key),
        name: key.to_owned(),
        number: 1,
        catchup_days: 0,
    };
    let make = || {
        EpgSchedule::new(
            vec![("All".to_owned(), vec![channel("a"), channel("b")])],
            store_between(FROM, TO),
            NOW,
        )
        .held_only(FROM, TO)
    };
    // Every channel held: `b` has no guide.
    let all = make();
    assert_eq!(title_at(&all, NOW), format!("P{}", (NOW - FIRST) / LEN));
    let b = all.program_at(1, all.minute(NOW));
    assert_eq!(all.title(1, b), NO_INFORMATION);
    // Only `a` held: `b` is still loading, in hour slots.
    let first = make().held_channels([ChannelKey::new("a")]);
    assert_eq!(title_at(&first, NOW), format!("P{}", (NOW - FIRST) / LEN));
    let b = first.program_at(1, first.minute(NOW + 60));
    assert_eq!((first.unix(b.start), first.unix(b.end)), (NOW, NOW + 3600));
    assert_eq!(first.title(1, b), LOADING);
}
