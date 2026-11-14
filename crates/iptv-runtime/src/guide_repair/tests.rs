//! Runtime consumer tests use injected host tasks, generated content and real SQLite.
use super::*;
use crate::{LocalTask, Ports};
use iptv_providers::{
    ContentId, SecretUrl,
    model::{Channel, M3uSourceConfig, Source, SourceConfig, StreamLocator},
};
use iptv_store_sqlite::{DatabaseKey, EpgOverride, Store};
use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, mpsc},
    task::{Context, Poll, Wake},
};

struct Unpark(std::thread::Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    owner: Rc<Runtime>,
    source: SourceId,
    tasks: Rc<RefCell<VecDeque<LocalTask>>>,
    delays: Rc<RefCell<Vec<futures_channel::oneshot::Sender<()>>>>,
}
fn channels() -> Vec<Channel> {
    ["amber", "birch", "cedar"]
        .into_iter()
        .map(|key| Channel {
            id: ContentId::from_stored(key),
            provider_id: None,
            name: format!("Synthetic {key}"),
            number: None,
            logo: None,
            groups: vec![],
            stream: StreamLocator::Url {
                url: "https://synthetic.invalid/stream.ts".into(),
                headers: vec![],
                properties: vec![],
            },
            epg_id: Some(format!("{key}.main")),
            epg_name: None,
            epg_shift_minutes: None,
            catch_up: None,
            radio: false,
        })
        .collect()
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_encrypted(
            dir.path().join("iptv.sqlite"),
            &DatabaseKey::from_bytes([0x71; 32]),
        )
        .unwrap();
        let source = store
            .add_source(&Source {
                name: "Synthetic repair runtime".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://synthetic.invalid/list.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        store
            .replace_catalog(source, &[], &channels(), &[], &[])
            .unwrap();
        drop(store);
        let owner = Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), |_| {
            Ok(DatabaseKey::from_bytes([0x71; 32]))
        })));
        let tasks = Rc::new(RefCell::new(VecDeque::new()));
        let queued = tasks.clone();
        let delays = Rc::new(RefCell::new(Vec::new()));
        let waiting = delays.clone();
        owner.bind_tasks(
            move |task| queued.borrow_mut().push_back(task),
            move |_| {
                let (sender, receiver) = futures_channel::oneshot::channel();
                waiting.borrow_mut().push(sender);
                Box::pin(async move {
                    let _ = receiver.await;
                })
            },
        );
        owner.start_services();
        Self {
            dir,
            owner,
            source,
            tasks,
            delays,
        }
    }
    fn store(&self) -> Store {
        Store::open_encrypted(
            self.dir.path().join("iptv.sqlite"),
            &DatabaseKey::from_bytes([0x71; 32]),
        )
        .unwrap()
    }
    fn request(&self, key: &str, target: Target) -> Request {
        Request::mapping(
            ChannelRef {
                source: self.source,
                content: ContentId::from_stored(key),
            },
            target,
            TimeWindow {
                from: 1_794_657_600,
                to: 1_794_664_800,
            },
        )
    }
    fn drive<F: Future>(&self, future: F) -> F::Output {
        let waker = Arc::new(Unpark(std::thread::current())).into();
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        let end = Instant::now() + Duration::from_secs(10);
        loop {
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
            assert!(
                Instant::now() < end,
                "synthetic host future exceeded ten seconds"
            );
            std::thread::park_timeout(Duration::from_millis(2));
        }
    }
    fn preview(&self, request: Request) -> Preview {
        self.drive(
            self.owner
                .preview_guide_repair(request, InspectionControl::default()),
        )
        .unwrap()
    }
    fn apply(&self, preview: &Preview) -> Outcome {
        self.drive(self.owner.apply_guide_repair(
            preview.id.clone(),
            preview.digest,
            InspectionControl::default(),
        ))
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.owner.drain_services();
        self.tasks.borrow_mut().clear();
        self.delays.borrow_mut().clear();
    }
}

#[test]
fn headless_preview_apply_verify_undo_uses_real_domain_services() {
    let fixture = Fixture::new();
    let request = fixture.request("amber", Target::NoGuide);
    let window = request.window;
    let before = fixture.owner.browser_generation();
    let plan = fixture.preview(request);
    assert_eq!(plan.mapping_count, 1);
    assert_eq!(plan.mappings[0].before, Choice::Automatic);
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(None)
    );
    let outcome = fixture.apply(&plan);
    assert!(
        outcome.configuration_committed && outcome.mapping_saved && outcome.verification_pending
    );
    assert_eq!(
        outcome.effective,
        Effective::Pending,
        "commit reply never guesses cache publication"
    );
    assert_eq!(fixture.owner.browser_generation(), before);
    let verified = fixture
        .drive(fixture.owner.verify_guide_repair(
            outcome.commit.id.clone(),
            window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(
        verified.current_revision && verified.configuration_effective && verified.verified_coverage
    );
    assert_eq!(verified.programmes_available, 0);
    assert!(verified.identity_unresolved);
    let undo = fixture
        .drive(
            fixture
                .owner
                .undo_guide_repair(outcome.commit.id.clone(), InspectionControl::default()),
        )
        .unwrap();
    assert!(undo.commit.undone);
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(None)
    );
}

#[test]
fn preview_protects_manual_choices_and_requires_precise_opt_in() {
    let fixture = Fixture::new();
    fixture
        .store()
        .set_epg_override(
            fixture.source,
            &ContentId::from_stored("amber"),
            Some(&EpgOverride::Disabled),
        )
        .unwrap();
    let mut request = fixture.request("amber", Target::Automatic);
    assert!(matches!(
        fixture.drive(
            fixture
                .owner
                .preview_guide_repair(request.clone(), InspectionControl::default())
        ),
        Err(Error::Conflict(_))
    ));
    request.mappings[0].replace_explicit = true;
    let plan = fixture.preview(request);
    assert!(plan.mappings[0].explicit_replacement);
    fixture.apply(&plan);
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(None)
    );
}

#[test]
fn no_op_refresh_apply_and_unrelated_edit_undo_preserve_identity() {
    let fixture = Fixture::new();
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    fixture
        .store()
        .replace_catalog(fixture.source, &[], &channels(), &[], &[])
        .unwrap();
    let outcome = fixture.apply(&plan);
    fixture
        .store()
        .set_epg_override(
            fixture.source,
            &ContentId::from_stored("birch"),
            Some(&EpgOverride::Disabled),
        )
        .unwrap();
    let undo = fixture
        .drive(
            fixture
                .owner
                .undo_guide_repair(outcome.commit.id.clone(), InspectionControl::default()),
        )
        .unwrap();
    assert!(undo.commit.undone);
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(None)
    );
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("birch"))
            .unwrap(),
        Some(Some(EpgOverride::Disabled))
    );
}

#[test]
fn late_verification_cannot_certify_a_later_edit_and_retry_keeps_original_revision() {
    let fixture = Fixture::new();
    let request = fixture.request("amber", Target::NoGuide);
    let window = request.window;
    let plan = fixture.preview(request);
    let outcome = fixture.apply(&plan);
    fixture
        .store()
        .set_epg_override(
            fixture.source,
            &ContentId::from_stored("birch"),
            Some(&EpgOverride::Disabled),
        )
        .unwrap();
    let verified = fixture
        .drive(fixture.owner.verify_guide_repair(
            outcome.commit.id.clone(),
            window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(!verified.current_revision && !verified.verified_coverage);
    assert_eq!(verified.expected, outcome.commit.revisions);
    assert_ne!(verified.expected, verified.current);
    let retry = fixture.apply(&plan);
    assert_eq!(retry.commit.revisions, outcome.commit.revisions);
}

#[test]
fn bad_digest_expired_plan_and_blocked_known_id_make_no_changes() {
    let fixture = Fixture::new();
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    assert_eq!(
        fixture.drive(fixture.owner.apply_guide_repair(
            plan.id.clone(),
            [0; 32],
            InspectionControl::default()
        )),
        Err(Error::WrongDigest)
    );
    fixture
        .owner
        .repairs
        .borrow_mut()
        .plans
        .get_mut(&plan.id)
        .unwrap()
        .preview
        .expires_at = now() - 1;
    assert_eq!(
        fixture.drive(fixture.owner.apply_guide_repair(
            plan.id.clone(),
            plan.digest,
            InspectionControl::default()
        )),
        Err(Error::PlanExpired)
    );
    fixture
        .store()
        .set_blocked(fixture.source, &ContentId::from_stored("birch"), true)
        .unwrap();
    assert_eq!(
        fixture.drive(fixture.owner.preview_guide_repair(
            fixture.request("birch", Target::NoGuide),
            InspectionControl::default()
        )),
        Err(Error::Inspection(InspectionError::PermissionDenied))
    );
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(None)
    );
}

#[test]
fn closing_owner_expires_previews_but_committed_inverse_survives_restart() {
    let fixture = Fixture::new();
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    let unapplied = fixture.preview(fixture.request("birch", Target::NoGuide));
    let saved = fixture.apply(&plan);
    fixture.owner.drain_services();
    fixture.tasks.borrow_mut().clear();
    fixture.owner.start_services();
    assert_eq!(
        fixture.drive(fixture.owner.apply_guide_repair(
            unapplied.id,
            unapplied.digest,
            InspectionControl::default()
        )),
        Err(Error::PlanMissing)
    );
    let recovered = fixture
        .drive(
            fixture
                .owner
                .guide_repair_operation(saved.commit.id.clone()),
        )
        .unwrap();
    assert_eq!(recovered.commit.revisions, saved.commit.revisions);
    let undo = fixture
        .drive(
            fixture
                .owner
                .undo_guide_repair(saved.commit.id, InspectionControl::default()),
        )
        .unwrap();
    assert!(undo.commit.undone);
}

#[test]
fn repair_preview_deadline_includes_waiting_behind_held_analysis() {
    let fixture = Fixture::new();
    let services = fixture.owner.services();
    // Warm the dedicated analysis connection before holding its lane.
    fixture.preview(fixture.request("amber", Target::NoGuide));
    let held = Arc::new((Mutex::new(false), Condvar::new()));
    let barrier = held.clone();
    let (entered, wait) = mpsc::channel();
    let _pending = services.analyze(move |_| {
        entered.send(()).unwrap();
        let (lock, ready) = &*barrier;
        let mut done = lock.lock().unwrap();
        while !*done {
            done = ready.wait(done).unwrap();
        }
        Ok(())
    });
    wait.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    let result = fixture.drive(fixture.owner.preview_guide_repair(
        fixture.request("birch", Target::NoGuide),
        InspectionControl {
            timeout: Duration::from_millis(30),
            ..Default::default()
        },
    ));
    *held.0.lock().unwrap() = true;
    held.1.notify_all();
    assert_eq!(result, Err(Error::Inspection(InspectionError::Deadline)));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn queued_writer_deadline_prevents_any_later_mutation() {
    let fixture = Fixture::new();
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    let services = fixture.owner.services();
    let held = Arc::new((Mutex::new(false), Condvar::new()));
    let barrier = held.clone();
    let (entered, wait) = mpsc::channel();
    let pending = services.call(move |_| {
        entered.send(()).unwrap();
        let (lock, ready) = &*barrier;
        let mut done = lock.lock().unwrap();
        while !*done {
            done = ready.wait(done).unwrap();
        }
        Ok(())
    });
    wait.recv_timeout(Duration::from_secs(2)).unwrap();
    let result = fixture.drive(fixture.owner.apply_guide_repair(
        plan.id.clone(),
        plan.digest,
        InspectionControl {
            timeout: Duration::from_millis(30),
            ..Default::default()
        },
    ));
    *held.0.lock().unwrap() = true;
    held.1.notify_all();
    fixture.drive(pending).unwrap();
    fixture.drive(services.call(|_| Ok(()))).unwrap();
    assert_eq!(result, Err(Error::Inspection(InspectionError::Deadline)));
    assert!(
        fixture
            .store()
            .guide_repair_commit(&plan.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(None)
    );
}

#[test]
fn an_inflight_deadline_reports_uncertainty_and_the_journal_reconciles_actual_commit() {
    let fixture = Fixture::new();
    let preview = fixture.preview(fixture.request("amber", Target::NoGuide));
    let plan = fixture.owner.repairs.borrow().plans[&preview.id].clone();
    let phase = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let writer_phase = phase.clone();
    let child = CancelToken::new();
    let token = child.clone();
    let services = fixture.owner.services();
    let held = Arc::new((Mutex::new(false), Condvar::new()));
    let barrier = held.clone();
    let (entered, wait) = mpsc::channel();
    let id = preview.id.clone();
    let digest = digest_hex(&preview.digest);
    let reply = services.call(move |store| {
        write::begin(
            &writer_phase,
            &token,
            Instant::now() + Duration::from_secs(5),
        )
        .map_err(|_| iptv_store_sqlite::StoreError::Unavailable("synthetic command refused"))?;
        entered.send(()).unwrap();
        let (lock, ready) = &*barrier;
        let mut done = lock.lock().unwrap();
        while !*done {
            done = ready.wait(done).unwrap();
        }
        // Exactly as in the real writer, cancellation after begin cannot claim
        // rollback of an atomic command which may have durably committed.
        let result = committed(store.apply_guide_repair(&id, &digest, &plan.batch)?);
        writer_phase.store(
            if result.is_ok() { 2 } else { 3 },
            std::sync::atomic::Ordering::SeqCst,
        );
        Ok(result)
    });
    wait.recv_timeout(Duration::from_secs(2)).unwrap();
    let result = fixture.drive(write::wait(
        &services,
        InspectionControl::default(),
        Instant::now() + Duration::from_millis(30),
        preview.id.clone(),
        phase,
        child,
        reply,
    ));
    *held.0.lock().unwrap() = true;
    held.1.notify_all();
    fixture.drive(services.call(|_| Ok(()))).unwrap();
    assert_eq!(
        result,
        Err(Error::CompletionUnknown {
            operation: preview.id.clone()
        })
    );
    let recovered = fixture
        .drive(fixture.owner.guide_repair_operation(preview.id))
        .unwrap();
    assert_eq!(recovered.commit.source, fixture.source);
    assert_eq!(
        fixture
            .store()
            .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(Some(EpgOverride::Disabled))
    );
}

#[test]
fn bulk_500_preview_paginates_and_one_affected_conflict_prevents_every_change() {
    let fixture = Fixture::new();
    let mut generated = Vec::new();
    for index in 0..500 {
        let mut row = channels().remove(0);
        row.id = ContentId::from_stored(format!("synthetic-{index:03}"));
        row.name = format!("Synthetic channel {index:03}");
        generated.push(row);
    }
    fixture
        .store()
        .replace_catalog(fixture.source, &[], &generated, &[], &[])
        .unwrap();
    let mut request = fixture.request("synthetic-000", Target::NoGuide);
    request.mappings = generated
        .iter()
        .map(|row| MappingRequest {
            channel: ChannelRef {
                source: fixture.source,
                content: row.id.clone(),
            },
            target: Target::NoGuide,
            replace_explicit: false,
        })
        .collect();
    let plan = fixture.preview(request.clone());
    assert_eq!(plan.mapping_count, 500);
    assert_eq!(plan.mappings.len(), 200);
    assert_eq!(plan.next_mapping, Some(200));
    let tail = fixture
        .owner
        .guide_repair_plan(&plan.id, plan.digest, 400, 0, 200)
        .unwrap();
    assert_eq!(tail.mappings.len(), 100);
    assert_eq!(tail.next_mapping, None);
    fixture
        .store()
        .set_epg_override(
            fixture.source,
            &generated[499].id,
            Some(&EpgOverride::Disabled),
        )
        .unwrap();
    assert!(matches!(
        fixture.drive(fixture.owner.apply_guide_repair(
            plan.id,
            plan.digest,
            InspectionControl::default()
        )),
        Err(Error::Conflict(_))
    ));
    for row in &generated[..499] {
        assert_eq!(
            fixture
                .store()
                .repair_mapping(fixture.source, &row.id)
                .unwrap(),
            Some(None)
        );
    }
    let new = fixture.preview(request);
    let saved = fixture.apply(&new);
    assert!(saved.mapping_saved);
    for row in generated {
        assert_eq!(
            fixture
                .store()
                .repair_mapping(fixture.source, &row.id)
                .unwrap(),
            Some(Some(EpgOverride::Disabled))
        );
    }
}

#[cfg(feature = "test-support")]
mod staged {
    use super::*;
    use crate::inspection::{ProbeRequest, ProbeTarget};
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    const XML: &str = r#"<tv>
      <channel id="amber.main"><display-name>Synthetic Amber</display-name></channel>
      <channel id="birch.main"><display-name>Synthetic Birch</display-name></channel>
      <programme channel="amber.main" start="20261114120000 +0000" stop="20261114140000 +0000"><title>Synthetic Segment 01</title></programme>
      <programme channel="birch.main" start="20261114120000 +0000" stop="20261114140000 +0000"><title>Synthetic Segment 02</title></programme>
    </tv>"#;
    struct Mock {
        address: std::net::SocketAddr,
        stop: Arc<AtomicBool>,
        requests: Arc<AtomicUsize>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Mock {
        fn new() -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let done = stop.clone();
            let requests = Arc::new(AtomicUsize::new(0));
            let counted = requests.clone();
            let thread = std::thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    let Ok((mut connection, _)) = listener.accept() else {
                        break;
                    };
                    if done.load(Ordering::SeqCst) {
                        break;
                    }
                    connection
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut data = Vec::new();
                    let mut buffer = [0; 1024];
                    while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                        match connection.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => data.extend_from_slice(&buffer[..n]),
                        }
                    }
                    counted.fetch_add(1, Ordering::SeqCst);
                    let _ = write!(
                        connection,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/xml\r\nConnection: close\r\n\r\n{}",
                        XML.len(),
                        XML
                    );
                }
            });
            Self {
                address,
                stop,
                requests,
                thread: Some(thread),
            }
        }
        fn url(&self) -> SecretUrl {
            SecretUrl::new(format!("http://{}/synthetic.xml", self.address))
        }
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.address);
            self.thread.take().unwrap().join().unwrap();
        }
    }
    fn proposal(
        fixture: &Fixture,
        server: &Mock,
    ) -> (Request, iptv_library::inspection::ProbeEvidence) {
        let summary = fixture
            .drive(fixture.owner.probe_feed(
                ProbeRequest {
                    source: fixture.source,
                    target: ProbeTarget::Supplied(server.url()),
                },
                InspectionControl {
                    timeout: Duration::from_secs(10),
                    ..Default::default()
                },
            ))
            .unwrap();
        assert_eq!(summary.evidence.feed, None);
        let request = Request {
            source: fixture.source,
            mappings: vec![MappingRequest {
                channel: ChannelRef {
                    source: fixture.source,
                    content: ContentId::from_stored("amber"),
                },
                target: Target::Staged {
                    evidence: summary.evidence.clone(),
                    epg_channel: "amber.main".into(),
                    addition: Some(0),
                },
                replace_explicit: false,
            }],
            feeds: vec![],
            additions: vec![FeedAddition {
                evidence: summary.evidence.clone(),
                name: Some("Synthetic inspected guide".into()),
            }],
            feed_order: None,
            window: TimeWindow {
                from: 1_794_657_600,
                to: 1_794_664_800,
            },
        };
        (request, summary.evidence)
    }
    #[test]
    fn staged_add_allocates_durable_feed_at_commit_and_reports_indirect_effects() {
        let fixture = Fixture::new();
        let server = Mock::new();
        let (request, _) = proposal(&fixture, &server);
        let window = request.window;
        let plan = fixture.preview(request);
        assert!(
            fixture
                .store()
                .guide_sources(fixture.source)
                .unwrap()
                .is_empty()
        );
        assert_eq!(plan.feeds[0].feed, None);
        assert!(
            plan.indirect
                .iter()
                .any(|effect| effect.channel.content.as_str() == "birch"
                    && matches!(effect.after, Some(Target::Staged { .. })))
        );
        let saved = fixture.apply(&plan);
        let feeds = fixture.store().guide_sources(fixture.source).unwrap();
        assert_eq!(feeds.len(), 1);
        assert_ne!(feeds[0].feed, iptv_epg::FeedId(0));
        assert_eq!(saved.commit.added_feeds, [feeds[0].feed]);
        assert_eq!(
            fixture
                .store()
                .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
                .unwrap(),
            Some(Some(EpgOverride::for_feed(feeds[0].feed, "amber.main")))
        );
        let before = fixture
            .drive(fixture.owner.verify_guide_repair(
                saved.commit.id.clone(),
                window,
                InspectionControl::default(),
            ))
            .unwrap();
        assert!(!before.configuration_effective && !before.verified_coverage);
        // Advance only the injected settle timer; no wall-clock sleeping or UI.
        for delay in fixture.delays.borrow_mut().drain(..) {
            let _ = delay.send(());
        }
        let operation = saved.commit.id.clone();
        fixture.drive(async {
            loop {
                let state = fixture
                    .owner
                    .guide_repair_operation(operation.clone())
                    .await
                    .unwrap();
                if matches!(
                    state.phase,
                    OperationPhase::ReadyToVerify | OperationPhase::Failed
                ) {
                    break;
                }
                let mut first = true;
                std::future::poll_fn(|cx| {
                    if first {
                        first = false;
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    } else {
                        Poll::Ready(())
                    }
                })
                .await;
            }
        });
        let after = fixture
            .drive(fixture.owner.verify_guide_repair(
                saved.commit.id,
                window,
                InspectionControl::default(),
            ))
            .unwrap();
        assert!(
            after.configuration_effective && after.verified_coverage,
            "{after:?}"
        );
        assert_eq!(after.programmes_available, 1);
        assert_eq!(after.covered_seconds, 7200);
        assert_eq!(
            server.requests.load(Ordering::SeqCst),
            2,
            "one probe plus one coalesced import"
        );
    }
    #[test]
    fn evicted_stage_cannot_be_applied_and_never_adds_a_live_feed() {
        let fixture = Fixture::new();
        let server = Mock::new();
        let (request, _) = proposal(&fixture, &server);
        let plan = fixture.preview(request);
        // Rotate the disposable cache: evidence is evicted, while the runtime
        // services and storage owner remain alive.
        fixture.owner.inspection.renew();
        let result = fixture.drive(fixture.owner.apply_guide_repair(
            plan.id,
            plan.digest,
            InspectionControl::default(),
        ));
        assert!(
            matches!(
                result,
                Err(Error::Inspection(InspectionError::StaleEvidence))
            ),
            "{result:?}"
        );
        assert!(
            fixture
                .store()
                .guide_sources(fixture.source)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fixture
                .store()
                .repair_mapping(fixture.source, &ContentId::from_stored("amber"))
                .unwrap(),
            Some(None)
        );
    }
}
