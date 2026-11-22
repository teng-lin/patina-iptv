//! Generated paged-guide fixtures and an in-memory page delivery probe.

use std::{cell::RefCell, rc::Rc};

use crate::pages::{PageRequest, PagedWindow};
use crate::{EpgSchedule, GuideChannel};
use iptv_epg::{
    ChannelKey, DAY, EpgRef, FeedId, GuideStore, MatchKind, ProgramData, SourceBuilder,
    StoreOptions,
};

/// Hour-aligned "now".
pub const NOW: i64 = 1_800_000_000;
/// Programs of 45 minutes: never on the hour slots a loading day shows.
pub const LEN: i64 = 45 * 60;
pub const FIRST: i64 = NOW - 6 * DAY + 600;

/// Channel `a`'s programs overlapping `[from, to)`, as the store reads a
/// window back: `P<n>` from [`FIRST`] on, every [`LEN`], until a day ahead.
pub fn store_between(from: i64, to: i64) -> GuideStore {
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
pub fn title_at(t: i64) -> String {
    format!("P{}", (t - FIRST) / LEN)
}

pub type Requests = Rc<RefCell<Vec<PageRequest>>>;

/// Holds now − 1 day on; pages back to `oldest`. Requests are recorded.
pub fn paged(oldest: i64) -> (EpgSchedule, Requests) {
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
pub fn deliver(s: &EpgSchedule, requests: &Requests) {
    for r in requests.borrow_mut().drain(..) {
        assert!(s.insert_page(r, store_between(r.from, r.to)), "{r:?}");
    }
}

pub fn minute_of(s: &EpgSchedule, t: i64) -> i32 {
    s.minute(t)
}
