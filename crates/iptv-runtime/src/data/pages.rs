//! The stored guide's older days, read in on demand.
//!
//! A shown schedule holds the stored guide from `crate::paging::LOADED_PAST_DAYS`
//! before its start on ([`window`]: the guide's first screen, a day of
//! "earlier programs" and the play bar's archive of the last day need
//! nothing more), not every day `epg.keep_past` keeps: on Android a week
//! of a large guide was most of the app's memory. Older days are pages
//! (`iptv_guide::pages`): the first guide cell, program detail or
//! catch-up that needs one asks `loader` for it, which reads it on its
//! own thread (`Store::load_epg` of that day) and puts it in the schedule
//! on screen; `RuntimeEvent::GuideChanged` asks the shell to redraw programs.
//!
//! Pages go again when the guide's window is back at now (`bind`); the
//! day of the program playing from the archive is pinned meanwhile, and
//! catch-up waits for its day before it resolves the archive ([`ready`]).

use std::rc::Rc;
use std::time::Duration;

use iptv_epg::{GuideStore, UnixTime};
use iptv_guide::EpgSchedule;
use iptv_guide::pages::PageRequest;
use iptv_library::SourceId;

use iptv_core::guide::Schedule;

/// A stored guide page could not be read; a later explicit lookup may retry it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageReadError;
impl std::fmt::Display for PageReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("guide page read failed")
    }
}
impl std::error::Error for PageReadError {}
pub(crate) struct Waiter {
    // None waits for a held first-screen schedule to be replaced; it has no
    // older-page request that can complete its missing program window.
    request: Option<PageRequest>,
    complete: Box<dyn FnOnce(Result<(), PageReadError>)>,
}

pub use crate::paging::{kept_from, window};

/// `schedule` (source `source`'s stored guide, as [`window`] at `now`
/// read it) with its older days paged in on demand, back to the first
/// program stored.
pub async fn paged(
    owner: &Rc<crate::Runtime>,
    schedule: EpgSchedule,
    source: SourceId,
    now: UnixTime,
) -> EpgSchedule {
    let keep = super::stored::stored_past_days();
    let window = window(now, keep);
    let starts = match crate::commands::guide_listing_starts(owner, source, window.oldest).await {
        Ok(starts) => starts,
        Err(error) => {
            log::warn!("iptv data: the stored guide's listing starts: {error}");
            return schedule;
        }
    };
    let window = kept_from(window, starts.values().copied().min(), now);
    let incarnation = schedule.source_authority().and_then(|authority| {
        (authority.source() == source.0).then(|| authority.incarnation().to_owned())
    });
    schedule.with_pages(window, starts, loader(owner, source, incarnation))
}

/// Reads a page on the store's reader and updates the visible schedule.
/// Called from guide queries (maybe inside a view's computation): it only
/// starts work.
///
/// The schedule that keeps this loader is itself kept by its owner, so the
/// loader holds the owner weakly.
fn loader(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    incarnation: Option<String>,
) -> impl Fn(PageRequest) + 'static {
    let owner = Rc::downgrade(owner);
    move |request: PageRequest| {
        let Some(owner) = owner.upgrade() else {
            return;
        };
        let started = crate::time::Instant::now();
        let read = crate::commands::guide_page(&owner, source, incarnation.clone(), request);
        owner.spawn({
            let owner = owner.clone();
            async move {
                let read = read.await;
                let took = started.elapsed();
                arrived(&owner, source, request, read, took);
            }
        });
    }
}

fn arrived(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    request: PageRequest,
    store: Result<GuideStore, crate::Error>,
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
    let mut result = Ok(());
    let accepted = child.is_some_and(|schedule| match store {
        Ok(store) => {
            let kb = store.heap_bytes() / 1024;
            let inserted = schedule.insert_page(request, store);
            if inserted {
                log::info!(
                    "iptv data: guide day -{} paged in ({kb} KiB, read in {} ms); {} held",
                    request.page + 1,
                    took.as_millis(),
                    schedule.paged_days()
                );
            }
            inserted
        }
        Err(error) => {
            log::warn!("iptv data: older guide day: {error}");
            result = Err(PageReadError);
            schedule.fail_page(request)
        }
    });
    // An evicted request still owns its original waiter. Retire that waiter
    // with failure even when its late reply cannot update the current page;
    // never let it consume a newer attempt's completion.
    if !accepted {
        result = Err(PageReadError);
    }
    let completed = owner.page_waiters.with(|waiters| {
        let mut waiters = waiters.borrow_mut();
        let mut completed = Vec::new();
        let mut remaining = Vec::new();
        for waiter in waiters.drain(..) {
            if waiter.request == Some(request) {
                completed.push(waiter.complete);
            } else {
                remaining.push(waiter);
            }
        }
        *waiters = remaining;
        completed
    });
    if accepted {
        owner.emit(crate::RuntimeEvent::GuideChanged);
    }
    for complete in completed {
        complete(result);
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
/// first screen, `opening.rs`): what waited for its programs looks again.
pub fn replaced(owner: &Rc<crate::Runtime>) {
    for waiter in owner
        .page_waiters
        .with(|w| std::mem::take(&mut *w.borrow_mut()))
    {
        (waiter.complete)(Ok(()));
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

/// Whether the programs around `t` are in memory on `schedule`, so a
/// catch-up there resolves with its program's real times; when not, its
/// day is pinned and asked for, and `retry` runs once a page arrived.
pub fn ready(
    owner: &Rc<crate::Runtime>,
    schedule: &Rc<dyn Schedule>,
    channel: usize,
    t: UnixTime,
    retry: impl FnOnce(Result<(), PageReadError>) + 'static,
) -> bool {
    let Some((epg, local)) = iptv_guide::combined::epg_channel(&**schedule, channel) else {
        return true;
    };
    let t = epg.stored_epg_time(local, t);
    epg.pin_page(Some(t));
    if epg.ensure_loaded(t) {
        return true;
    }
    log::info!("iptv data: catch-up waits for its guide day");
    let request = epg.pending_page(t);
    owner.page_waiters.with(|w| {
        w.borrow_mut().push(Waiter {
            request,
            complete: Box::new(retry),
        })
    });
    false
}

/// Pin the active archive day and release other pins when playback changes.
pub fn pin_archive(owner: &Rc<crate::Runtime>, archive: Option<(usize, UnixTime)>) {
    let schedule = owner.schedule();
    each(&*schedule, |s| s.pin_page(None));
    if let Some((channel, time)) = archive
        && let Some((epg, local)) = iptv_guide::combined::epg_channel(&*schedule, channel)
    {
        epg.pin_page(Some(epg.stored_epg_time(local, time)));
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

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::paging::store_window;
    use iptv_epg::DAY;
    use std::cell::RefCell;

    const NOW: UnixTime = 1_800_000_000 + 1234;

    type Requests = Rc<RefCell<Vec<PageRequest>>>;
    fn paged_schedule(requests: &Requests) -> Rc<EpgSchedule> {
        let parsed = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Generated\nhttp://stream.example.test/generated\n",
        );
        let requests = requests.clone();
        Rc::new(
            EpgSchedule::from_catalog(
                &parsed.groups,
                &parsed.channels.items,
                GuideStore::default(),
                NOW,
            )
            .with_pages(window(NOW, 7), Default::default(), move |request| {
                requests.borrow_mut().push(request)
            }),
        )
    }
    fn install_single(owner: &Rc<crate::Runtime>, source: SourceId, schedule: &Rc<EpgSchedule>) {
        owner.shown.set(Some(source));
        owner.set_schedule(schedule.clone());
    }

    #[test]
    fn page_completion_reports_only_its_own_source_success_or_failure() {
        use iptv_guide::combined::{CombinedSchedule, SourceSchedule};
        let owner = Rc::new(crate::Runtime::default());
        let requests_a = Rc::new(RefCell::new(Vec::new()));
        let requests_b = Rc::new(RefCell::new(Vec::new()));
        let a = paged_schedule(&requests_a);
        let b = paged_schedule(&requests_b);
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
        owner.set_schedule(schedule.clone());
        let results = Rc::new(RefCell::new(Vec::new()));
        let time = NOW - 2 * DAY;
        for (channel, name) in [(0, "A"), (1, "B")] {
            let results = results.clone();
            assert!(!ready(&owner, &schedule, channel, time, move |result| {
                results.borrow_mut().push((name, result));
            }));
        }
        arrived(
            &owner,
            SourceId(1),
            requests_a.borrow()[0],
            Ok(GuideStore::default()),
            Duration::ZERO,
        );
        assert_eq!(*results.borrow(), [("A", Ok(()))]);
        assert_eq!(owner.page_waiters.with(|waiters| waiters.borrow().len()), 1);
        arrived(
            &owner,
            SourceId(2),
            requests_b.borrow()[0],
            Err(crate::Error::unavailable("synthetic page failure")),
            Duration::ZERO,
        );
        assert_eq!(
            *results.borrow(),
            [("A", Ok(())), ("B", Err(PageReadError))]
        );
        assert_eq!(b.paged_days(), 0, "failed reads are not cached empty pages");
        for _ in 0..5 {
            b.program_at(0, -2 * 24 * 60);
        }
        assert_eq!(
            requests_b.borrow().len(),
            1,
            "rendering must not retry failure"
        );
        assert!(!b.ensure_loaded(time));
        assert_eq!(requests_b.borrow().len(), 2, "an explicit lookup may retry");
    }

    #[test]
    fn replacement_schedule_ignores_old_success_and_failure_completions() {
        let owner = Rc::new(crate::Runtime::default());
        let requests_old = Rc::new(RefCell::new(Vec::new()));
        let old = paged_schedule(&requests_old);
        install_single(&owner, SourceId(1), &old);
        let time = NOW - 2 * DAY;
        let retired = Rc::new(RefCell::new(Vec::new()));
        let seen = retired.clone();
        assert!(!ready(&owner, &owner.schedule(), 0, time, move |result| {
            seen.borrow_mut().push(result)
        }));
        let old_request = requests_old.borrow()[0];
        let requests_new = Rc::new(RefCell::new(Vec::new()));
        let current = paged_schedule(&requests_new);
        install_single(&owner, SourceId(1), &current);
        replaced(&owner);
        assert_eq!(*retired.borrow(), [Ok(())]);
        let results = Rc::new(RefCell::new(Vec::new()));
        let seen = results.clone();
        assert!(!ready(&owner, &owner.schedule(), 0, time, move |result| {
            seen.borrow_mut().push(result)
        }));
        arrived(
            &owner,
            SourceId(1),
            old_request,
            Ok(GuideStore::default()),
            Duration::ZERO,
        );
        arrived(
            &owner,
            SourceId(1),
            old_request,
            Err(crate::Error::unavailable("synthetic old failure")),
            Duration::ZERO,
        );
        assert!(results.borrow().is_empty());
        assert_eq!(current.paged_days(), 0);
        assert_eq!(owner.page_waiters.with(|waiters| waiters.borrow().len()), 1);
        arrived(
            &owner,
            SourceId(1),
            requests_new.borrow()[0],
            Ok(GuideStore::default()),
            Duration::ZERO,
        );
        assert_eq!(*results.borrow(), [Ok(())]);
        assert!(current.ensure_loaded(time));
    }

    #[test]
    fn evicted_reply_retires_its_waiter_without_consuming_the_new_attempt() {
        let owner = Rc::new(crate::Runtime::default());
        let requests = Rc::new(RefCell::new(Vec::new()));
        let schedule = paged_schedule(&requests);
        install_single(&owner, SourceId(1), &schedule);
        let time = NOW - 2 * DAY;
        let old = Rc::new(RefCell::new(Vec::new()));
        let seen = old.clone();
        assert!(!ready(&owner, &owner.schedule(), 0, time, move |result| {
            seen.borrow_mut().push(result)
        }));
        let first = requests.borrow()[0];
        schedule.pin_page(None);
        schedule.evict_pages();
        let current = Rc::new(RefCell::new(Vec::new()));
        let seen = current.clone();
        assert!(!ready(&owner, &owner.schedule(), 0, time, move |result| {
            seen.borrow_mut().push(result)
        }));
        let second = requests.borrow()[1];
        assert_ne!(first, second);
        arrived(
            &owner,
            SourceId(1),
            first,
            Ok(GuideStore::default()),
            Duration::ZERO,
        );
        assert_eq!(*old.borrow(), [Err(PageReadError)]);
        assert!(current.borrow().is_empty());
        arrived(
            &owner,
            SourceId(1),
            first,
            Err(crate::Error::unavailable("synthetic stale failure")),
            Duration::ZERO,
        );
        assert_eq!(old.borrow().len(), 1);
        arrived(
            &owner,
            SourceId(1),
            second,
            Err(crate::Error::unavailable("synthetic current failure")),
            Duration::ZERO,
        );
        assert_eq!(*current.borrow(), [Err(PageReadError)]);
        assert_eq!(owner.page_waiters.with(|waiters| waiters.borrow().len()), 0);
    }

    #[test]
    fn held_first_screen_waits_for_replacement_before_resolving_catchup() {
        let owner = Rc::new(crate::Runtime::default());
        let parsed = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Generated\nhttp://stream.example.test/generated\n",
        );
        let held = Rc::new(
            EpgSchedule::from_catalog(
                &parsed.groups,
                &parsed.channels.items,
                GuideStore::default(),
                NOW,
            )
            .held_only(NOW, NOW + 3600),
        );
        install_single(&owner, SourceId(1), &held);
        let result = Rc::new(RefCell::new(Vec::new()));
        let seen = result.clone();
        assert!(!ready(
            &owner,
            &owner.schedule(),
            0,
            NOW - 7200,
            move |result| seen.borrow_mut().push(result)
        ));
        assert!(result.borrow().is_empty());
        assert!(
            owner
                .page_waiters
                .with(|waiters| waiters.borrow()[0].request.is_none())
        );
        let full = Rc::new(EpgSchedule::from_catalog(
            &parsed.groups,
            &parsed.channels.items,
            GuideStore::default(),
            NOW,
        ));
        install_single(&owner, SourceId(1), &full);
        replaced(&owner);
        assert_eq!(*result.borrow(), [Ok(())]);
        assert!(ready(&owner, &owner.schedule(), 0, NOW - 7200, |_| panic!(
            "already loaded"
        )));
    }

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
                .with_pages(window(NOW, 7), Default::default(), move |request| {
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
        assert!(!ready(&owner, &schedule, 0, time, |_| {}));
        assert_eq!(requests_a.borrow().len(), 1);
        assert!(requests_b.borrow().is_empty());
        // B remains pending indefinitely. Completing A must still unblock A.
        assert!(!b.ensure_loaded(time));
        let request = requests_a.borrow()[0];
        assert!(a.insert_page(request, GuideStore::default()));
        assert!(ready(&owner, &schedule, 0, time, |_| panic!(
            "A is already loaded"
        )));
        assert!(!b.ensure_loaded(time));
    }

    #[test]
    fn queued_pages_reject_recreated_disabled_or_unowned_sources() {
        use crate::model::{DatabaseKey, SourcePolicy};
        use iptv_providers::model::{M3uSourceConfig, Source, SourceConfig};
        use std::{
            sync::mpsc,
            task::{Context, Poll, Waker},
        };

        for change in ["unchanged", "recreated", "disabled", "unowned"] {
            let directory = tempfile::tempdir().unwrap();
            let key = DatabaseKey::from_bytes([0x79; 32]);
            let config = SourceConfig::M3u(M3uSourceConfig {
                location: iptv_providers::SecretUrl::new(
                    "https://synthetic.example.test/guide.m3u",
                ),
                user_agent: None,
                referrer: None,
            });
            let definition = Source {
                name: "Generated page owner".into(),
                config: config.clone(),
            };
            let owner = Rc::new(crate::Runtime::new(crate::Ports::new(
                directory.path().into(),
                move |_| Ok(key.clone()),
            )));
            let tasks = Rc::new(RefCell::new(Vec::<crate::LocalTask>::new()));
            let queued = tasks.clone();
            owner.bind_tasks(
                move |task| queued.borrow_mut().push(task),
                |_| Box::pin(std::future::pending()),
            );
            owner.start_services();
            let services = owner.services();
            let added = definition.clone();
            let (source, incarnation) = services
                .block_on(services.call(move |store| {
                    let source = store.add_source(&added)?;
                    Ok((source, store.source_incarnation(source)?))
                }))
                .unwrap();
            let parsed = iptv_providers::m3u::parse_m3u(
                "#EXTM3U\n#EXTINF:-1,Generated\nhttps://synthetic.example.test/channel.ts\n",
            );
            let schedule = EpgSchedule::from_catalog(
                &parsed.groups,
                &parsed.channels.items,
                GuideStore::default(),
                NOW,
            )
            .with_source_authority((change != "unowned").then(|| {
                std::sync::Arc::new(iptv_guide::playback::SourceAuthority::new(
                    source.0,
                    incarnation.clone(),
                    config,
                ))
            }));
            let accepted = schedule
                .source_authority()
                .map(|authority| authority.incarnation().to_owned());
            let schedule = Rc::new(schedule.with_pages(
                window(NOW, 7),
                Default::default(),
                loader(&owner, source, accepted),
            ));
            install_single(&owner, source, &schedule);

            // A prior reader snapshot keeps the old shown schedule installed
            // while this exact page waits behind it. WAL permits the edit.
            let (entered_tx, entered_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let held = services.read(move |store| {
                store.read_snapshot(|store| {
                    assert!(store.source(source)?.is_some());
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
            });
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let results = Rc::new(RefCell::new(Vec::new()));
            let seen = results.clone();
            assert!(!ready(
                &owner,
                &owner.schedule(),
                0,
                NOW - 2 * DAY,
                move |result| seen.borrow_mut().push(result)
            ));
            let mut task = tasks.borrow_mut().pop().expect("one page read");
            assert!(matches!(
                task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                Poll::Pending
            ));
            match change {
                "recreated" => {
                    let expected = incarnation.clone();
                    services
                        .block_on(services.call(move |store| {
                            assert!(store.delete_source(source)?);
                            assert_eq!(store.add_source(&definition)?, source);
                            assert_ne!(store.source_incarnation(source)?, expected);
                            Ok(())
                        }))
                        .unwrap();
                }
                "disabled" => services
                    .block_on(services.call(move |store| {
                        store.set_source_policy(
                            source,
                            SourcePolicy {
                                enabled: false,
                                ..SourcePolicy::default()
                            },
                        )
                    }))
                    .unwrap(),
                _ => {}
            }
            release_tx.send(()).unwrap();
            services.block_on(held).unwrap();
            services.block_on(task);
            if change == "unchanged" {
                assert_eq!(*results.borrow(), [Ok(())]);
                assert_eq!(
                    schedule.paged_days(),
                    1,
                    "the same authority can load its page"
                );
                assert!(schedule.ensure_loaded(NOW - 2 * DAY));
            } else {
                assert_eq!(*results.borrow(), [Err(PageReadError)], "{change}");
                assert_eq!(
                    schedule.paged_days(),
                    0,
                    "{change}: failure must not become an empty page"
                );
            }
            assert!(
                owner
                    .page_waiters
                    .with(|waiters| waiters.borrow().is_empty())
            );
            assert!(owner.drain_services());
        }
    }

    #[test]
    fn a_shown_schedule_does_not_keep_its_owner_alive() {
        let owner = Rc::new(crate::Runtime::default());
        let weak = Rc::downgrade(&owner);
        let source = SourceId(1);
        let lookup = crate::data::stored::description_lookup(&owner, source).unwrap();
        let schedule = EpgSchedule::new(Vec::new(), GuideStore::default(), NOW)
            .with_pages(
                window(NOW, 7),
                Default::default(),
                loader(&owner, source, None),
            )
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
    fn the_guide_goes_back_to_its_first_program_stored() {
        let w = window(NOW, 7);
        // Two days stored of the seven kept: back to the first program.
        let first = NOW - 2 * DAY + 600;
        assert_eq!(kept_from(w, Some(first), NOW).oldest, first);
        // Older programs than the store keeps (not pruned yet): no
        // further back than it keeps.
        assert_eq!(kept_from(w, Some(NOW - 9 * DAY), NOW).oldest, w.oldest);
        // Keeping everything: the first program bounds it.
        let all = window(NOW, u32::MAX);
        assert_eq!(kept_from(all, Some(first), NOW).oldest, first);
        // Nothing stored: no past at all.
        let none = kept_from(w, None, NOW);
        assert_eq!((none.oldest, none.loaded_from), (NOW, w.loaded_from));
    }
}
