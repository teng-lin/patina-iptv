use std::rc::Rc;

use crate::pages::PagedWindow;
use crate::pages_tests::{
    FIRST, NOW, Requests, deliver, minute_of, paged, store_between, title_at,
};
use crate::{EpgSchedule, GuideChannel, LOADING, NO_INFORMATION};
use iptv_core::guide::{Focus, Schedule};
use iptv_epg::{
    ChannelKey, DAY, EpgRef, FeedId, GuideStore, MatchKind, ProgramData, SourceBuilder,
    StoreOptions,
};

#[test]
fn browsing_back_in_the_guide_pages_in_the_previous_day() {
    let (s, requests) = paged(NOW - 7 * DAY);
    let now = minute_of(&s, NOW);
    let mut f = Focus::at(now);
    f.enter_past(&s);
    // Step back until the focus leaves the recent window.
    let edge = minute_of(&s, NOW - DAY);
    for _ in 0..200 {
        if f.cursor < edge {
            break;
        }
        f.apply(iptv_core::guide::Move::Left, &s);
    }
    assert!(f.cursor < edge);
    assert_eq!(s.title(0, f.program(&s)), LOADING);
    assert_eq!(requests.borrow().len(), 1);
    deliver(&s, &requests);
    let shown = f.program(&s);
    assert_eq!(s.title(0, shown), title_at(s.unix(f.cursor)));
    // The cells across the window are real programs again.
    for p in s.programs_between(0, f.window, iptv_core::guide::VISIBLE) {
        assert_ne!(s.title(0, p), LOADING);
        assert_ne!(s.title(0, p), NO_INFORMATION);
    }
}

#[test]
fn retained_coverage_is_per_channel_and_inspectable_before_a_page_is_loaded() {
    use iptv_core::guide::{Move, Pane, home_window, view_range};
    use iptv_core::lists::{ChannelList, FIRST_GROUP, ListView};

    let loaded_from = NOW - DAY;
    // The oldest retained minute is the first moment of a page. P0 began
    // in the previous page, which is entirely before the retention cutoff.
    let cutoff = loaded_from - 5 * DAY;
    let start = cutoff - 10 * 60;
    let recent_only = ChannelKey::new("recent");
    let requests = Requests::default();
    let seen = requests.clone();
    let schedule = EpgSchedule::new(
        vec![
            (
                "With listings".into(),
                ["a", "recent"]
                    .into_iter()
                    .map(|key| GuideChannel {
                        key: ChannelKey::new(key),
                        name: key.into(),
                        number: 1,
                        catchup_days: 0,
                    })
                    .collect(),
            ),
            (
                "Unmapped".into(),
                vec![GuideChannel {
                    key: ChannelKey::new("unmapped"),
                    name: "Unmapped".into(),
                    number: 3,
                    catchup_days: 0,
                }],
            ),
        ],
        store_between(loaded_from, i64::MAX),
        NOW,
    )
    .with_pages(
        PagedWindow {
            loaded_from,
            oldest: cutoff,
        },
        [(ChannelKey::new("a"), start), (recent_only, NOW)].into(),
        move |request| seen.borrow_mut().push(request),
    );
    let floor = schedule.minute(cutoff);
    let now = schedule.minute(NOW);
    let mut focus = Focus::at(now);
    assert_eq!(focus.floor(&schedule), floor);
    focus.tap(0, floor, &schedule);
    assert_eq!(schedule.title(0, focus.program(&schedule)), LOADING);
    assert_eq!(requests.borrow().len(), 1);

    let mut builder = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), NOW);
    builder.push_data(
        "a.test",
        &ProgramData {
            start,
            stop: Some(cutoff + 15 * 60),
            title: "Generated overlap",
            desc: Some("Generated overlap details"),
            ..Default::default()
        },
    );
    let mut page = GuideStore::new();
    page.replace_source(builder.finish());
    page.set_matches(
        [(
            ChannelKey::new("a"),
            vec![EpgRef {
                source: FeedId(1),
                epg_id: "a.test".into(),
                kind: MatchKind::TvgId,
            }],
        )]
        .into_iter()
        .collect(),
    );
    let request = requests.borrow_mut().pop().unwrap();
    assert_eq!(request.from, cutoff);
    assert!(schedule.insert_page(request, page));
    let listing = focus.program(&schedule);
    assert_eq!(listing.start, schedule.minute(start));
    assert_eq!(schedule.title(0, listing), "Generated overlap");
    assert_eq!(
        schedule.description(0, listing).as_deref(),
        Some("Generated overlap details")
    );
    assert!(requests.borrow().is_empty(), "no request before retention");
    focus.apply(Move::Left, &schedule);
    assert_eq!(focus.pane, Pane::Groups);

    focus = Focus::at(now);
    focus.apply(Move::Down, &schedule);
    assert_eq!(focus.floor(&schedule), now, "recent-only row");
    focus.apply(Move::Left, &schedule);
    assert_eq!(focus.pane, Pane::Groups);

    let schedule = Rc::new(schedule);
    let list = ChannelList::build(FIRST_GROUP + 1, &*schedule, &[], &[]);
    let view = ListView::new(schedule, Rc::new(list));
    let mut focus = Focus::at(now);
    assert_eq!(focus.floor(&view), now);
    assert_eq!(view_range(&view, now).0, home_window(now));
    focus.apply(Move::Left, &view);
    assert_eq!(focus.pane, Pane::Groups);
    assert!(
        requests.borrow().is_empty(),
        "unmapped row requests no page"
    );
}

#[test]
fn a_day_back_by_key_or_pan_pages_that_day_in() {
    let (s, requests) = paged(NOW - 7 * DAY);
    let now = minute_of(&s, NOW);
    let mut f = Focus::at(now);
    // Ctrl + Left twice: two days back, past the held day.
    f.step_time(-iptv_core::guide::DAY, &s);
    f.step_time(-iptv_core::guide::DAY, &s);
    let cells = s.programs_between(0, f.window, iptv_core::guide::VISIBLE);
    assert!(cells.iter().all(|&p| s.title(0, p) == LOADING));
    // The window starts an hour before that day: two days are asked for.
    assert_eq!(requests.borrow().len(), 2, "{:?}", requests.borrow());
    deliver(&s, &requests);
    for p in s.programs_between(0, f.window, iptv_core::guide::VISIBLE) {
        assert_eq!(s.title(0, p), title_at(s.unix(p.start.max(f.window))));
    }
    // The oldest day kept bounds the keys.
    for _ in 0..10 {
        f.step_time(-iptv_core::guide::DAY, &s);
    }
    assert_eq!(f.cursor, minute_of(&s, FIRST));
    assert!(f.window <= f.cursor && f.cursor < f.window + iptv_core::guide::VISIBLE);
}

#[test]
fn an_empty_schedule_keeps_the_default_padding_and_horizon() {
    use iptv_core::guide::{HORIZON, home_window, view_range};
    let schedule = EpgSchedule::new(Vec::new(), GuideStore::new(), NOW);
    assert_eq!(view_range(&schedule, 0), (home_window(0), HORIZON));
}
