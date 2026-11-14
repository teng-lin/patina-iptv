//! The stored guide's older days, read in on demand.
//!
//! A shown schedule holds the stored guide from [`crate::paging::LOADED_PAST_DAYS`]
//! before its start on ([`window`]: the guide's first screen, a day of
//! "earlier programmes" and the play bar's archive of the last day need
//! nothing more), not every day `epg.keep_past` keeps: on Android a week
//! of a large guide was most of the app's memory. Older days are pages
//! (`iptv_guide::pages`): the first guide cell, programme detail or
//! catch-up that needs one asks [`loader`] for it, which reads it on its
//! own thread (`Store::load_epg` of that day) and puts it in the schedule
//! on screen; `RuntimeEvent::GuideChanged` asks the shell to redraw programmes.
//!
//! Pages go again when the guide's window is back at now ([`bind`]); the
//! day of the programme playing from the archive is pinned meanwhile, and
//! catch-up waits for its day before it resolves the archive ([`ready`]).

use std::rc::Rc;
use std::time::Duration;

use iptv_epg::{GuideStore, UnixTime};
use iptv_guide::EpgSchedule;
use iptv_guide::pages::PageRequest;
use iptv_library::SourceId;

use iptv_core::guide::Schedule;

pub use crate::paging::{kept_from, window};

/// `schedule` (source `source`'s stored guide, as [`window`] at `now`
/// read it) with its older days paged in on demand, back to the first
/// programme stored.
pub async fn paged(
    owner: &Rc<crate::Runtime>,
    schedule: EpgSchedule,
    source: SourceId,
    now: UnixTime,
) -> EpgSchedule {
    let keep = super::stored::keep_past_days(&owner.settings());
    let first = crate::commands::first_programme(owner, source)
        .await
        .unwrap_or_else(|e| {
            log::warn!("iptv data: the stored guide's first day: {e}");
            None
        });
    let window = kept_from(window(now, keep), first, now);
    schedule.with_pages(window, loader(owner, source))
}

/// Reads a page on the store's reader and updates the visible schedule.
/// Called from guide queries (maybe inside a view's computation): it only
/// starts work.
///
/// The schedule that keeps this loader is itself kept by its owner, so the
/// loader holds the owner weakly.
fn loader(owner: &Rc<crate::Runtime>, source: SourceId) -> impl Fn(PageRequest) + 'static {
    let owner = Rc::downgrade(owner);
    move |request: PageRequest| {
        let Some(owner) = owner.upgrade() else {
            return;
        };
        let started = std::time::Instant::now();
        let read = crate::commands::guide_page(&owner, source, request);
        owner.spawn({
            let owner = owner.clone();
            async move {
                let read = read.await;
                let took = started.elapsed();
                let store = read.unwrap_or_else(|e| {
                    log::warn!("iptv data: older guide day: {e}");
                    GuideStore::default()
                });
                arrived(&owner, source, request, store, took);
            }
        });
    }
}

fn arrived(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    request: PageRequest,
    store: GuideStore,
    took: Duration,
) {
    let schedule = owner.schedule();
    let child = combined(&*schedule)
        .and_then(|s| s.source(source.0))
        .or_else(|| {
            (owner.shown.get() == Some(source))
                .then(|| epg(&*schedule))
                .flatten()
        });
    let inserted = child.is_some_and(|s| {
        let kb = store.heap_bytes() / 1024;
        let inserted = s.insert_page(request, store);
        if inserted {
            log::info!(
                "iptv data: guide day -{} paged in ({kb} KiB, read in {} ms); {} held",
                request.page + 1,
                took.as_millis(),
                s.paged_days()
            );
        }
        inserted
    });
    if inserted {
        owner.emit(crate::RuntimeEvent::GuideChanged);
    }
    for waiter in owner
        .page_waiters
        .with(|w| std::mem::take(&mut *w.borrow_mut()))
    {
        waiter();
    }
}

fn epg(schedule: &dyn Schedule) -> Option<&EpgSchedule> {
    schedule.as_any()?.downcast_ref::<EpgSchedule>()
}

fn combined(schedule: &dyn Schedule) -> Option<&iptv_guide::combined::CombinedSchedule> {
    schedule.as_any()?.downcast_ref()
}
fn each(schedule: &dyn Schedule, mut f: impl FnMut(&EpgSchedule)) {
    if let Some(s) = epg(schedule) {
        f(s);
    }
    if let Some(s) = combined(schedule) {
        for p in s.sources() {
            f(&p.schedule);
        }
    }
}
/// A schedule replaced the one on screen (with the whole window after a
/// first screen, `opening.rs`): what waited for its programmes looks again.
pub fn replaced(owner: &Rc<crate::Runtime>) {
    for waiter in owner
        .page_waiters
        .with(|w| std::mem::take(&mut *w.borrow_mut()))
    {
        waiter();
    }
}

/// Asks for the day holding guide `minute` before the grid shows it (where
/// a fling of the timeline lands).
pub fn want(schedule: &dyn Schedule, minute: i32) {
    let unix = iptv_core::guide::unix_of(schedule, minute);
    each(schedule, |s| {
        s.ensure_loaded(unix);
    });
}

/// Whether the programmes around `t` are in memory on `schedule`, so a
/// catch-up there resolves with its programme's real times; when not, its
/// day is pinned and asked for, and `retry` runs once a page arrived.
pub fn ready(
    owner: &Rc<crate::Runtime>,
    schedule: &Rc<dyn Schedule>,
    channel: usize,
    t: UnixTime,
    retry: impl FnOnce() + 'static,
) -> bool {
    let Some((epg, _)) = iptv_guide::combined::epg_channel(&**schedule, channel) else {
        return true;
    };
    epg.pin_page(Some(t));
    if epg.ensure_loaded(t) {
        return true;
    }
    log::info!("iptv data: catch-up waits for its guide day");
    owner
        .page_waiters
        .with(|w| w.borrow_mut().push(Box::new(retry)));
    false
}

/// Pin the active archive day and release other pins when playback changes.
pub fn pin_archive(owner: &Rc<crate::Runtime>, archive: Option<(usize, UnixTime)>) {
    let schedule = owner.schedule();
    each(&*schedule, |s| s.pin_page(None));
    if let Some((channel, time)) = archive
        && let Some((epg, _)) = iptv_guide::combined::epg_channel(&*schedule, channel)
    {
        epg.pin_page(Some(time));
    }
}
pub fn evict(owner: &Rc<crate::Runtime>) {
    let schedule = owner.schedule();
    each(&*schedule, |s| {
        let dropped = s.evict_pages();
        if dropped > 0 {
            log::info!("iptv data: back at now; dropped {dropped} older guide days");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::store_window;
    use iptv_epg::DAY;
    use std::cell::RefCell;

    const NOW: UnixTime = 1_800_000_000 + 1234;

    #[test]
    fn catchup_waits_only_for_its_own_source_page() {
        use iptv_guide::combined::{CombinedSchedule, SourceSchedule};
        let parsed = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Generated\nhttp://stream.example.test/generated\n",
        );
        let requests_a = Rc::new(RefCell::new(Vec::new()));
        let requests_b = Rc::new(RefCell::new(Vec::new()));
        let make = |requests: Rc<RefCell<Vec<PageRequest>>>| {
            Rc::new(
                EpgSchedule::from_catalog(
                    &parsed.groups,
                    &parsed.channels.items,
                    GuideStore::default(),
                    NOW,
                )
                .with_pages(window(NOW, 7), move |request| {
                    requests.borrow_mut().push(request)
                }),
            )
        };
        let a = make(requests_a.clone());
        let b = make(requests_b.clone());
        let schedule: Rc<dyn Schedule> = Rc::new(CombinedSchedule::new(
            vec![
                SourceSchedule {
                    id: 1,
                    name: "Generated A".into(),
                    schedule: a.clone(),
                },
                SourceSchedule {
                    id: 2,
                    name: "Generated B".into(),
                    schedule: b.clone(),
                },
            ],
            NOW,
        ));
        let time = NOW - 2 * DAY;
        let owner = Rc::new(crate::Runtime::default());
        assert!(!ready(&owner, &schedule, 0, time, || {}));
        assert_eq!(requests_a.borrow().len(), 1);
        assert!(requests_b.borrow().is_empty());
        // B remains pending indefinitely. Completing A must still unblock A.
        assert!(!b.ensure_loaded(time));
        let request = requests_a.borrow()[0];
        assert!(a.insert_page(request, GuideStore::default()));
        assert!(ready(&owner, &schedule, 0, time, || panic!(
            "A is already loaded"
        )));
        assert!(!b.ensure_loaded(time));
    }

    #[test]
    fn a_shown_schedule_does_not_keep_its_owner_alive() {
        let owner = Rc::new(crate::Runtime::default());
        let weak = Rc::downgrade(&owner);
        let source = SourceId(1);
        let lookup = crate::data::stored::description_lookup(&owner, source).unwrap();
        let schedule = EpgSchedule::new(Vec::new(), GuideStore::default(), NOW)
            .with_pages(window(NOW, 7), loader(&owner, source))
            .with_description_lookup(lookup);
        // The owner keeps the schedule on screen and its source's part.
        let schedule = Rc::new(schedule);
        crate::data::combined::remember_single(&owner, source, schedule.clone());
        owner.set_schedule(schedule);
        drop(owner);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn the_window_holds_a_day_before_the_hour() {
        let w = window(NOW, 7);
        assert_eq!(w.loaded_from, 1_800_000_000 - DAY);
        assert_eq!(w.oldest, NOW - 7 * DAY);
        assert_eq!(store_window(w).from, w.loaded_from);
        assert_eq!(store_window(w).to, UnixTime::MAX);
        // Keeping less than a day holds no more than is kept.
        assert_eq!(window(NOW, 0).loaded_from, 1_800_000_000);
        // Keeping everything (`u32::MAX`): no oldest day.
        assert!(window(NOW, u32::MAX).oldest < NOW - 1000 * DAY);
    }

    #[test]
    fn the_guide_goes_back_to_its_first_programme_stored() {
        let w = window(NOW, 7);
        // Two days stored of the seven kept: back to the first programme.
        let first = NOW - 2 * DAY + 600;
        assert_eq!(kept_from(w, Some(first), NOW).oldest, first);
        // Older programmes than the store keeps (not pruned yet): no
        // further back than it keeps.
        assert_eq!(kept_from(w, Some(NOW - 9 * DAY), NOW).oldest, w.oldest);
        // Keeping everything: the first programme bounds it.
        let all = window(NOW, u32::MAX);
        assert_eq!(kept_from(all, Some(first), NOW).oldest, first);
        // Nothing stored: no past at all.
        let none = kept_from(w, None, NOW);
        assert_eq!((none.oldest, none.loaded_from), (NOW, w.loaded_from));
    }
}
