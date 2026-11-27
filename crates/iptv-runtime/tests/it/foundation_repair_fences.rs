//! A real refresh is held after reading its guide, then released after repair.
//! Public native APIs and generated HTTP fixtures exercise the publication fence.
#![cfg(feature = "test-support")]

use iptv_core::guide::Listing;
use iptv_library::{
    Choice,
    guide_repair::{Effective, FeedEdit, OperationPhase, Request, Target},
    inspection::{ChannelRef, Query, TimeWindow},
};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_runtime::{LocalTask, Ports, Runtime, RuntimeEvent, data, inspection::InspectionControl};
use iptv_store_sqlite::DatabaseKey;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::{Future, poll_fn},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    task::{Context, Poll, Wake},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const KEY: [u8; 32] = [0x71; 32];
const TITLE: &str = "Synthetic Amber Live";
type GuideBarrier = (Arc<AtomicBool>, mpsc::Receiver<()>);
type Timers = Rc<RefCell<Vec<(Instant, futures_channel::oneshot::Sender<()>)>>>;

struct Server {
    address: String,
    stopped: Arc<AtomicBool>,
    guides: Arc<AtomicUsize>,
    fail_guides: Arc<AtomicBool>,
    barrier: Arc<Mutex<Option<GuideBarrier>>>,
    task: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(now: i64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let guides = Arc::new(AtomicUsize::new(0));
        let count = guides.clone();
        let fail_guides = Arc::new(AtomicBool::new(false));
        let failed = fail_guides.clone();
        let barrier: Arc<Mutex<Option<GuideBarrier>>> = Arc::default();
        let held = barrier.clone();
        let xml = format!(
            "<tv><channel id='amber.main'><display-name>Synthetic Amber</display-name>\
             </channel><programme channel='amber.main' start='{}' stop='{}'>\
             <title>{TITLE}</title></programme></tv>",
            iptv_epg::format_xmltv_time(now - 60),
            iptv_epg::format_xmltv_time(now + 3600),
        );
        let task = thread::spawn(move || {
            for connection in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let mut stream = connection.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|p| p == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                }
                let body = if request.starts_with(b"GET /guide.xml ") {
                    count.fetch_add(1, Ordering::SeqCst);
                    if let Some((entered, release)) = held.lock().unwrap().take() {
                        entered.store(true, Ordering::SeqCst);
                        let _ = release.recv_timeout(Duration::from_secs(10));
                    }
                    xml.as_str()
                } else {
                    "#EXTM3U\n#EXTINF:-1 tvg-id=\"amber.main\",Synthetic Amber\n\
                     https://synthetic.invalid/amber.ts\n"
                };
                let status =
                    if request.starts_with(b"GET /guide.xml ") && failed.load(Ordering::SeqCst) {
                        "404 Synthetic unavailable"
                    } else {
                        "200 OK"
                    };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            address,
            stopped,
            guides,
            fail_guides,
            barrier,
            task: Some(task),
        }
    }
    fn url(&self, path: &str) -> SecretUrl {
        SecretUrl::new(format!("http://{}{path}", self.address))
    }
    fn hold_next_guide(&self) -> (Arc<AtomicBool>, mpsc::Sender<()>) {
        let entered = Arc::new(AtomicBool::new(false));
        let (release, wait) = mpsc::channel();
        *self.barrier.lock().unwrap() = Some((entered.clone(), wait));
        (entered, release)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(&self.address);
        self.task.take().unwrap().join().unwrap();
    }
}

struct Unpark(thread::Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
struct Host {
    owner: Rc<Runtime>,
    tasks: Rc<RefCell<VecDeque<LocalTask>>>,
    timers: Timers,
    _directory: tempfile::TempDir,
}
impl Host {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
            Ok(DatabaseKey::from_bytes(KEY))
        })));
        let tasks = Rc::new(RefCell::new(VecDeque::new()));
        let queued = tasks.clone();
        let timers = Rc::new(RefCell::new(Vec::new()));
        let delayed = timers.clone();
        owner.bind_tasks(
            move |task| queued.borrow_mut().push_back(task),
            move |duration| {
                let (done, wait) = futures_channel::oneshot::channel();
                delayed.borrow_mut().push((Instant::now() + duration, done));
                Box::pin(async move {
                    let _ = wait.await;
                })
            },
        );
        owner.start_services();
        Self {
            owner,
            tasks,
            timers,
            _directory: directory,
        }
    }
    fn drive<F: Future>(&self, future: F) -> F::Output {
        let waker = Arc::new(Unpark(thread::current())).into();
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let mut timers = self.timers.borrow_mut();
            let mut index = 0;
            while index < timers.len() {
                if timers[index].0 <= Instant::now() {
                    let (_, ready) = timers.swap_remove(index);
                    let _ = ready.send(());
                } else {
                    index += 1;
                }
            }
            drop(timers);
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
            let count = self.tasks.borrow().len();
            for _ in 0..count {
                let Some(mut task) = self.tasks.borrow_mut().pop_front() else {
                    break;
                };
                if task.as_mut().poll(&mut cx).is_pending() {
                    self.tasks.borrow_mut().push_back(task);
                }
            }
            assert!(Instant::now() < deadline, "native fixture host timed out");
            thread::park_timeout(Duration::from_millis(2));
        }
    }
    fn until(&self, mut condition: impl FnMut() -> bool) {
        self.drive(poll_fn(|_| {
            if condition() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }));
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.owner.host_handle().hold_reports(false);
        self.owner.drain_services();
        self.tasks.borrow_mut().clear();
    }
}

#[test]
fn an_old_refresh_report_cannot_restore_programs_after_a_committed_repair() {
    repair_during_old_work(true);
}

#[test]
fn an_old_guide_download_cannot_republish_after_a_committed_repair() {
    repair_during_old_work(false);
}

fn repair_during_old_work(hold_report: bool) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = Server::new(now);
    let host = Host::new();
    let owner = &host.owner;
    let weak = Rc::downgrade(owner);
    let repaired = Rc::new(Cell::new(false));
    let after = repaired.clone();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let captured = seen.clone();
    owner.bind(move |event| {
        if let RuntimeEvent::ScheduleChanged { current, .. } = event {
            let owner = weak.upgrade().unwrap();
            assert!(Rc::ptr_eq(&owner.schedule(), &current));
            if current.channel_count() == 1 && after.get() {
                let minute = ((now - current.epoch_unix()) / 60) as i32;
                let program = current.program_at(0, minute);
                captured.borrow_mut().push(current.listing(0, program));
            }
        }
    });
    let (source, channel) = import_fixture(&host, &server, now);
    let source_id = channel.source;
    let handle = owner.host_handle();
    let download = (!hold_report).then(|| server.hold_next_guide());
    handle.hold_reports(hold_report);
    data::start_import(
        owner,
        data::Job {
            source,
            epg_url: None,
            stored: Some(source_id),
            purpose: data::Purpose::Refresh,
            edit: false,
        },
        |_| {},
    );
    host.until(|| match &download {
        Some((entered, _)) => entered.load(Ordering::SeqCst),
        None => handle.reports_waiting() == 1,
    });
    let old = owner.pending_import_completion(source_id).unwrap();
    let window = TimeWindow {
        from: now - 60,
        to: now + 3600,
    };
    let plan = host
        .drive(owner.preview_guide_repair(
            Request::mapping(channel.clone(), Target::NoGuide, window),
            InspectionControl::default(),
        ))
        .unwrap();
    let outcome = host
        .drive(owner.apply_guide_repair(plan.id, plan.digest, InspectionControl::default()))
        .unwrap();
    assert!(outcome.mapping_saved);
    let committed = host
        .drive(owner.guide_repair_operation(outcome.operation.clone()))
        .unwrap();
    // Drain publication while the old worker remains blocked at its real report barrier.
    host.until(|| {
        host.drive(owner.guide_repair_operation(committed.id.clone()))
            .unwrap()
            .effective
            == Effective::Installed
    });
    if hold_report {
        assert_eq!(handle.reports_waiting(), 1);
    }
    repaired.set(true);
    let requests = server.guides.load(Ordering::SeqCst);
    handle.hold_reports(false);
    if let Some((_, release)) = download {
        let _ = release.send(());
    }
    host.drive(old);
    assert_eq!(
        server.guides.load(Ordering::SeqCst),
        requests,
        "the superseded post-playlist guide phase must not download again"
    );
    let current = host
        .drive(owner.inspect_channel(channel.clone(), InspectionControl::default()))
        .unwrap();
    assert_eq!(current.choice, Choice::NoGuide);
    let programs = host
        .drive(owner.query_programs(
            Query::source(source_id),
            window,
            Some(channel),
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(programs.items.is_empty());
    if hold_report {
        assert!(
            !seen.borrow().is_empty(),
            "old catalog report was actually presented"
        );
    }
    let schedule = owner.schedule();
    let minute = ((now - schedule.epoch_unix()) / 60) as i32;
    assert_ne!(
        schedule.listing(0, schedule.program_at(0, minute)),
        Some(Listing::Title(TITLE.into()))
    );
    assert!(
        seen.borrow()
            .iter()
            .all(|listing| listing != &Some(Listing::Title(TITLE.into()))),
        "the old in-memory schedule bypassed the newer stored mapping"
    );
}

fn import_fixture(host: &Host, server: &Server, now: i64) -> (Source, ChannelRef) {
    let owner = &host.owner;
    let source = Source {
        name: "Synthetic repair fence".into(),
        config: SourceConfig::M3u(M3uSourceConfig {
            location: server.url("/playlist.m3u"),
            user_agent: None,
            referrer: None,
        }),
    };
    data::start_import(
        owner,
        data::Job {
            source: source.clone(),
            epg_url: Some(server.url("/guide.xml")),
            stored: None,
            purpose: data::Purpose::Add,
            edit: false,
        },
        |_| {},
    );
    host.until(|| {
        let schedule = owner.schedule();
        if schedule.channel_count() != 1 {
            return false;
        }
        let minute = ((now - schedule.epoch_unix()) / 60) as i32;
        schedule.listing(0, schedule.program_at(0, minute)) == Some(Listing::Title(TITLE.into()))
    });
    let source_id = owner.shown_source().unwrap();
    if let Some(initial) = owner.pending_import_completion(source_id) {
        host.drive(initial);
    }
    let before = host
        .drive(owner.query_channels(Query::source(source_id), InspectionControl::default()))
        .unwrap();
    let channel: ChannelRef = before.items[0].key.clone();
    (source, channel)
}

#[test]
fn a_canceled_guide_update_cannot_report_success_after_guide_deletion() {
    canceled_update_after_deletion(false);
}

#[test]
fn a_guide_update_revoked_before_dispatch_cannot_report_the_workers_are_busy() {
    canceled_update_after_deletion(true);
}

fn canceled_update_after_deletion(before_dispatch: bool) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = Server::new(now);
    let host = Host::new();
    let owner = &host.owner;
    let (_, channel) = import_fixture(&host, &server, now);
    let source = channel.source;
    data::refresh_guide_status(owner);
    host.until(|| !owner.source_lines(source).is_empty());

    let notices = Rc::new(RefCell::new(Vec::new()));
    let observed = notices.clone();
    owner.bind(move |event| {
        if let RuntimeEvent::Notify(message) = event {
            observed.borrow_mut().push(message);
        }
    });
    let download = (!before_dispatch).then(|| server.hold_next_guide());
    let result = Rc::new(Cell::new(None));
    let completed = result.clone();
    assert!(data::update_source_guide(owner, source, move |success| {
        completed.set(Some(success));
    }));
    // Poll the update separately so its completion can arrive after Clear's.
    let mut update = host.tasks.borrow_mut().pop_back().unwrap();
    if let Some((entered, _)) = &download {
        host.drive(poll_fn(|cx| {
            assert!(update.as_mut().poll(cx).is_pending());
            if entered.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }));
    }
    let deleted = Rc::new(Cell::new(None));
    let cleared = deleted.clone();
    assert!(data::clear_source_guide(owner, source, move |success| {
        cleared.set(Some(success));
    }));
    host.until(|| deleted.get().is_some());
    assert_eq!(deleted.get(), Some(true));
    assert_eq!(result.get(), None, "the canceled update is still unwinding");

    if before_dispatch {
        owner.host_handle().refuse_work(1);
    }
    host.drive(update);
    assert_eq!(
        result.get(),
        Some(false),
        "a revoked update must not replace the deletion confirmation with success"
    );
    assert!(notices.borrow().is_empty(), "no stale admission notice");
    assert!(!owner.is_updating(source), "the update is no longer busy");
    assert!(
        owner.host_handle().guide_check_registered(source),
        "revoked work leaves a guide check"
    );
}

#[test]
fn an_unbound_runtime_updates_its_catalog_organization_and_favorites() {
    use iptv_core::organization::GroupPreference;
    use iptv_guide::combined::{epg_channel, qualified_key};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = Server::new(now);
    let host = Host::new();
    let owner = &host.owner;
    let (_, channel) = import_fixture(&host, &server, now);
    let source = channel.source;
    let before = owner.schedule();
    let key = before.channel_key(0).unwrap();
    owner.toggle_favorite(0);
    owner.record_played(&key);
    assert_eq!(owner.favorite_channels(), [0]);
    assert_eq!(owner.recent_channels(), [0]);

    host.drive(iptv_runtime::commands::set_group_preference(
        owner,
        source,
        "__other__".into(),
        GroupPreference::EpgOffsetMinutes(15),
    ))
    .unwrap();
    data::organization_changed(owner, source);
    let offset = |row| {
        let schedule = owner.schedule();
        epg_channel(&*schedule, row).map(|(epg, local)| epg.epg_offset_minutes(local))
    };
    host.until(|| offset(0) == Some(15));
    assert!(!Rc::ptr_eq(&owner.schedule(), &before));
    assert_eq!(owner.favorites_channel_of(&key), Some(0));
    assert_eq!(owner.favorite_channels(), [0]);
    assert_eq!(owner.recent_channels(), [0]);

    // Adding a second playlist takes the already prepared single-source
    // schedule into combined mode. Its edit and key-based lists carry over.
    data::start_import(
        owner,
        data::Job {
            source: Source {
                name: "Synthetic second catalog".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: server.url("/second.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            },
            epg_url: None,
            stored: None,
            purpose: data::Purpose::Add,
            edit: false,
        },
        |_| {},
    );
    host.until(|| data::combined::active(owner) && owner.schedule().channel_count() == 2);
    let combined_key = qualified_key(source.0, &key);
    let row = owner.schedule().find_channel(&combined_key).unwrap();
    assert_eq!(offset(row), Some(15));
    assert_eq!(owner.favorites_channel_of(&combined_key), Some(row));
    assert_eq!(owner.favorite_channels(), [row]);
    assert_eq!(owner.recent_channels(), [row]);
}

#[test]
fn last_single_source_deletion_installs_an_empty_guide_without_a_subscriber() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = Server::new(now);
    let host = Host::new();
    let owner = &host.owner;
    let (_, channel) = import_fixture(&host, &server, now);
    let key = owner.schedule().channel_key(0).unwrap();
    owner.toggle_favorite(0);
    owner.record_played(&key);
    host.drive(owner.flush_favorites());

    host.drive(data::delete(owner, channel.source)).unwrap();
    assert!(owner.shown_source().is_none());
    assert_eq!(owner.schedule().channel_count(), 0);
    assert!(owner.favorites_channel_of(&key).is_none());
    assert!(owner.favorite_channels().is_empty());
    assert!(owner.recent_channels().is_empty());
}

#[test]
fn a_failed_postcommit_refresh_recovers_without_repeating_the_saved_mutation() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = Server::new(now);
    let host = Host::new();
    let owner = &host.owner;
    let (_, channel) = import_fixture(&host, &server, now);
    let source = channel.source;
    let initial = TimeWindow {
        from: now - 60,
        to: now + 3600,
    };
    let programs = host
        .drive(owner.query_programs(
            Query::source(source),
            initial,
            Some(channel.clone()),
            InspectionControl::default(),
        ))
        .unwrap();
    let feed = programs.items[0].candidate.feed;
    let shifted = TimeWindow {
        from: now + 3540,
        to: now + 7200,
    };
    let mut request = Request::mapping(channel, Target::Automatic, shifted);
    request.feeds.push(FeedEdit {
        feed,
        enabled: None,
        shift_minutes: Some(60),
        remove: false,
    });
    let plan = host
        .drive(owner.preview_guide_repair(request, InspectionControl::default()))
        .unwrap();
    server.fail_guides.store(true, Ordering::SeqCst);
    let saved = host
        .drive(owner.apply_guide_repair(plan.id, plan.digest, InspectionControl::default()))
        .unwrap();
    assert!(saved.mapping_saved && saved.configuration_committed);
    assert_eq!(saved.effective, Effective::Pending);
    host.until(|| {
        let operation = host
            .drive(owner.guide_repair_operation(saved.operation.clone()))
            .unwrap();
        operation.phase == OperationPhase::Failed && server.guides.load(Ordering::SeqCst) >= 2
    });
    let failed_operation = host
        .drive(owner.guide_repair_operation(saved.operation.clone()))
        .unwrap();
    assert_eq!(failed_operation.effective, Effective::Failed);
    let failed = host
        .drive(owner.verify_guide_repair(
            saved.operation.clone(),
            shifted,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(
        failed.current_revision,
        "the saved mutation survives download failure"
    );
    assert!(!failed.configuration_effective && !failed.verified_coverage);
    server.fail_guides.store(false, Ordering::SeqCst);
    data::refresh_guide_status(owner);
    host.until(|| !owner.source_lines(source).is_empty());
    let done = Rc::new(Cell::new(false));
    let completed = done.clone();
    assert!(data::update_source_guide(owner, source, move |accepted| {
        assert!(accepted);
        completed.set(true);
    }));
    host.until(|| done.get());
    let recovered = host
        .drive(owner.verify_guide_repair(
            saved.operation.clone(),
            shifted,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(
        recovered.current_revision
            && recovered.configuration_effective
            && recovered.verified_coverage
    );
    assert_eq!(recovered.programs_available, 1);
    let recovered_operation = host
        .drive(owner.guide_repair_operation(saved.operation.clone()))
        .unwrap();
    assert_eq!(recovered_operation.effective, Effective::Installed);
    assert_eq!(recovered_operation.phase, OperationPhase::Verified);
    let history = host
        .drive(owner.recent_guide_repairs(source, 10, InspectionControl::default()))
        .unwrap();
    assert_eq!(
        history.len(),
        1,
        "recovery refresh must not create another saved repair"
    );
    assert_eq!(history[0].id, saved.commit.id);
}
