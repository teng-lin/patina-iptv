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
        write::Wait {
            control: InspectionControl::default(),
            deadline: Instant::now() + Duration::from_millis(30),
            id: preview.id.clone(),
            phase,
            child,
        },
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
            Self::with_xml(XML.to_owned())
        }
        fn with_xml(xml: String) -> Self {
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
                        xml.len(),
                        xml
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
    fn staged_preview_cannot_alias_an_unresolved_manual_feed_identity() {
        let fixture = Fixture::new();
        fixture
            .store()
            .set_epg_override(
                fixture.source,
                &ContentId::from_stored("birch"),
                Some(&EpgOverride::for_feed(
                    iptv_epg::FeedId(u32::MAX),
                    "amber.main",
                )),
            )
            .unwrap();
        let server = Mock::new();
        let (request, _) = proposal(&fixture, &server);
        assert!(matches!(
            fixture.drive(
                fixture
                    .owner
                    .preview_guide_repair(request, InspectionControl::default())
            ),
            Err(Error::InvalidRequest)
        ));
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
                .repair_mapping(fixture.source, &ContentId::from_stored("birch"))
                .unwrap(),
            Some(Some(EpgOverride::for_feed(
                iptv_epg::FeedId(u32::MAX),
                "amber.main"
            )))
        );
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
                    && effect.availability_unknown
                    && effect.after.is_none())
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
    fn one_stage_repairs_500_channels_with_one_bounded_evidence_batch() {
        let fixture = Fixture::new();
        let mut generated = Vec::new();
        let mut xml = String::from("<tv>");
        for index in 0..500 {
            let mut row = channels().remove(0);
            row.id = ContentId::from_stored(format!("synthetic-{index:03}"));
            row.name = format!("Synthetic {index:03}");
            row.epg_id = Some(format!("synthetic.{index:03}"));
            xml.push_str(&format!(
                "<channel id=\"synthetic.{index:03}\"><display-name>Synthetic {index:03}</display-name></channel>\
                <programme channel=\"synthetic.{index:03}\" start=\"20261114120000 +0000\" stop=\"20261114140000 +0000\">\
                <title>Synthetic segment {index:03}</title></programme>"
            ));
            generated.push(row);
        }
        xml.push_str("</tv>");
        fixture
            .store()
            .replace_catalog(fixture.source, &[], &generated, &[], &[])
            .unwrap();
        let server = Mock::with_xml(xml);
        let (mut request, evidence) = proposal(&fixture, &server);
        request.mappings = generated
            .iter()
            .enumerate()
            .map(|(index, row)| MappingRequest {
                channel: ChannelRef {
                    source: fixture.source,
                    content: row.id.clone(),
                },
                target: Target::Staged {
                    evidence: evidence.clone(),
                    epg_channel: format!("synthetic.{index:03}"),
                    addition: Some(0),
                },
                replace_explicit: false,
            })
            .collect();
        let start = Instant::now();
        let preview = fixture.preview(request);
        assert_eq!(preview.mapping_count, 500);
        assert!(
            fixture
                .store()
                .epg_overrides(fixture.source)
                .unwrap()
                .is_empty()
        );
        let saved = fixture.apply(&preview);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert_eq!(saved.commit.added_feeds.len(), 1);
        let feed = saved.commit.added_feeds[0];
        for (index, row) in generated.iter().enumerate() {
            assert_eq!(
                fixture
                    .store()
                    .repair_mapping(fixture.source, &row.id)
                    .unwrap(),
                Some(Some(EpgOverride::for_feed(
                    feed,
                    format!("synthetic.{index:03}")
                )))
            );
        }
        assert_eq!(
            server.requests.load(Ordering::SeqCst),
            1,
            "private stage is reused without a second download"
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

#[test]
fn abandoned_previews_evict_oldest_without_exhausting_the_owner_for_a_day() {
    let fixture = Fixture::new();
    let first = fixture.preview(fixture.request("amber", Target::NoGuide));
    let mut newest = first.clone();
    for _ in 0..40 {
        newest = fixture.preview(fixture.request("amber", Target::NoGuide));
    }
    assert_eq!(fixture.owner.repairs.borrow().plans.len(), MAX_PLANS);
    assert_eq!(
        fixture
            .owner
            .guide_repair_plan(&first.id, first.digest, 0, 0, 20),
        Err(Error::PlanMissing)
    );
    assert!(
        fixture
            .owner
            .guide_repair_plan(&newest.id, newest.digest, 0, 0, 20)
            .is_ok()
    );
    assert!(
        fixture
            .owner
            .release_guide_repair_plan(&newest.id, newest.digest)
            .unwrap()
    );
    assert!(
        fixture
            .store()
            .epg_overrides(fixture.source)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn completed_repairs_release_previews_and_terminal_status_eviction_preserves_receipts() {
    let fixture = Fixture::new();
    let mut first = None;
    for index in 0..130 {
        let mut request = fixture.request(
            "amber",
            if index % 2 == 0 {
                Target::NoGuide
            } else {
                Target::Automatic
            },
        );
        request.mappings[0].replace_explicit = true;
        let window = request.window;
        let plan = fixture.preview(request);
        let outcome = fixture.apply(&plan);
        if first.is_none() {
            first = Some(outcome.commit.clone());
        }
        fixture
            .drive(fixture.owner.verify_guide_repair(
                outcome.commit.id,
                window,
                InspectionControl::default(),
            ))
            .unwrap();
        assert!(fixture.owner.repairs.borrow().plans.is_empty());
        assert!(fixture.owner.repairs.borrow().operations.len() <= MAX_OPERATIONS);
    }
    assert_eq!(
        fixture.owner.repairs.borrow().operations.len(),
        MAX_OPERATIONS
    );
    let first = first.unwrap();
    let recovered = fixture
        .drive(fixture.owner.guide_repair_operation(first.id.clone()))
        .unwrap();
    assert_eq!(recovered.commit.revisions, first.revisions);
}

#[test]
fn all_active_operation_capacity_refuses_new_commit_before_writer_mutation() {
    let fixture = Fixture::new();
    let mut sources = vec![fixture.source];
    let mut store = fixture.store();
    for index in 1..=MAX_OPERATIONS {
        let source = store
            .add_source(&Source {
                name: format!("Synthetic pending source {index}"),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new(format!("https://synthetic.invalid/{index}.m3u")),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        store
            .replace_catalog(source, &[], &channels(), &[], &[])
            .unwrap();
        sources.push(source);
    }
    drop(store);
    for source in &sources[..MAX_OPERATIONS] {
        let mut request = fixture.request("amber", Target::Automatic);
        request.source = *source;
        request.mappings[0].channel.source = *source;
        let plan = fixture.preview(request);
        fixture.apply(&plan);
    }
    let last = *sources.last().unwrap();
    let mut request = fixture.request("amber", Target::NoGuide);
    request.source = last;
    request.mappings[0].channel.source = last;
    let plan = fixture.preview(request);
    assert_eq!(
        fixture.drive(fixture.owner.apply_guide_repair(
            plan.id.clone(),
            plan.digest,
            InspectionControl::default()
        )),
        Err(Error::Busy)
    );
    assert_eq!(
        fixture.owner.repairs.borrow().operations.len(),
        MAX_OPERATIONS
    );
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
            .repair_mapping(last, &ContentId::from_stored("amber"))
            .unwrap(),
        Some(None)
    );
}

#[test]
fn changed_transport_or_disabled_policy_or_new_block_refuses_a_prepared_edit() {
    let fixture = Fixture::new();
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    let mut source = fixture
        .store()
        .source(fixture.source)
        .unwrap()
        .unwrap()
        .source;
    if let SourceConfig::M3u(config) = &mut source.config {
        config.user_agent = Some("Synthetic revised agent".into());
    }
    fixture
        .store()
        .update_source(fixture.source, &source)
        .unwrap();
    assert!(matches!(
        fixture.drive(fixture.owner.apply_guide_repair(
            plan.id,
            plan.digest,
            InspectionControl::default()
        )),
        Err(Error::Conflict(_))
    ));
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    fixture
        .store()
        .set_source_policy(
            fixture.source,
            iptv_store_sqlite::SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        fixture.drive(fixture.owner.apply_guide_repair(
            plan.id,
            plan.digest,
            InspectionControl::default()
        )),
        Err(Error::Inspection(InspectionError::SourceDisabled(
            fixture.source
        )))
    );
    fixture
        .store()
        .set_source_policy(fixture.source, Default::default())
        .unwrap();
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    fixture
        .store()
        .set_blocked(fixture.source, &ContentId::from_stored("amber"), true)
        .unwrap();
    assert_eq!(
        fixture.drive(fixture.owner.apply_guide_repair(
            plan.id,
            plan.digest,
            InspectionControl::default()
        )),
        Err(Error::Inspection(InspectionError::PermissionDenied))
    );
}

#[test]
fn indirect_preview_uses_actual_low_feed_binding_when_high_feed_only_lists_channel() {
    use iptv_epg::{ChannelKey, FeedId, MatchKind, ProgrammeData, StoreOptions};
    use iptv_import::{EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
    use iptv_store_sqlite::{EpgFeedMeta, EpgStage, GuideSource};
    let fixture = Fixture::new();
    let configs: Vec<_> = [1, 2]
        .into_iter()
        .map(|id| GuideSource {
            feed: FeedId(id),
            provider_index: None,
            url: SecretUrl::new(format!("https://synthetic.invalid/{id}.xml")),
            name: None,
            enabled: true,
            shift_minutes: 0,
        })
        .collect();
    let mut store = fixture.store();
    store.set_guide_sources(fixture.source, &configs).unwrap();
    let mut stage = EpgStage::create_in_store(&store, fixture.dir.path()).unwrap();
    for id in [1, 2] {
        stage
            .begin_feed(FeedId(id), 3 - id as i32, StoreOptions::default(), 0)
            .unwrap();
        if id == 2 {
            stage
                .programme(
                    "amber.main",
                    &ProgrammeData {
                        start: 1_794_657_600,
                        stop: Some(1_794_664_800),
                        title: "Synthetic lower priority programme",
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        stage.finish_feed().unwrap();
    }
    let import = EpgImport {
        bindings: vec![(
            ChannelKey::new("amber"),
            iptv_epg::EpgRef {
                source: FeedId(2),
                epg_id: "amber.main".into(),
                kind: MatchKind::TvgId,
            },
        )],
        listed: [1, 2]
            .into_iter()
            .map(|id| {
                (
                    FeedId(id),
                    vec![ListedChannel {
                        id: "amber.main".into(),
                        name: Some("Synthetic amber".into()),
                    }],
                )
            })
            .collect(),
        ..Default::default()
    };
    let feeds: Vec<_> = configs
        .iter()
        .map(|config| EpgFeedMeta {
            config: EpgSourceConfig::new(config.feed, config.url.clone()),
            fetched_at: 1_794_657_600,
            etag: None,
            last_modified: None,
            bytes: 10,
        })
        .collect();
    store
        .replace_epg(fixture.source, &mut stage, &import, &feeds, 0)
        .unwrap();
    let mut request = fixture.request("amber", Target::Automatic);
    request.mappings.clear();
    request.feeds.push(FeedEdit {
        feed: FeedId(2),
        enabled: Some(false),
        shift_minutes: None,
        remove: false,
    });
    let preview = fixture.preview(request);
    let effect = preview
        .indirect
        .iter()
        .find(|effect| effect.channel.content.as_str() == "amber")
        .unwrap();
    assert!(
        matches!(&effect.before,Some(Target::Configured(candidate)) if candidate.feed==FeedId(2))
    );
    assert!(effect.after.is_none() && effect.availability_unknown);
    assert!(!preview.indirect_complete);
    let saved = fixture.apply(&preview);
    // A write alone must not certify the disabled feed's still-stored binding.
    let verified = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id,
            TimeWindow {
                from: 1_794_657_600,
                to: 1_794_664_800,
            },
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(!verified.configuration_effective && !verified.verified_coverage);
}

#[test]
#[cfg(feature = "test-support")]
#[ignore = "50 ms wall-clock budget; tools/check.sh runs this in isolation"]
fn repair_analysis_on_100k_channels_and_1k_groups_keeps_display_lane_responsive() {
    use iptv_providers::{
        GroupId,
        model::{ContentKind, Group},
    };
    let fixture = Fixture::new();
    let groups: Vec<_> = (0..1000)
        .map(|index| Group {
            id: GroupId::from_stored(format!("synthetic-group-{index}")),
            name: format!("Synthetic group {index}"),
            kind: ContentKind::Live,
            provider_id: None,
            adult: false,
        })
        .collect();
    let prototype = channels().remove(0);
    let generated: Vec<_> = (0..100_000)
        .map(|index| {
            let mut row = prototype.clone();
            row.id = ContentId::from_stored(format!("synthetic-{index:06}"));
            row.name = format!("Synthetic {index:06}");
            row.groups = vec![groups[index % 1000].id.clone()];
            row
        })
        .collect();
    fixture
        .store()
        .replace_catalog(fixture.source, &groups, &generated, &[], &[])
        .unwrap();
    fixture
        .drive(fixture.owner.read_library_metadata())
        .unwrap();
    let request = fixture.request("synthetic-000000", Target::NoGuide);
    let future = fixture.owner.preview_guide_repair(
        request,
        InspectionControl {
            timeout: Duration::from_secs(10),
            ..Default::default()
        },
    );
    let mut future = std::pin::pin!(future);
    let waker = Arc::new(Unpark(std::thread::current())).into();
    let mut context = Context::from_waker(&waker);
    let started = Instant::now();
    let mut maximum = Duration::ZERO;
    let preview = loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            break value.unwrap();
        }
        let read = Instant::now();
        fixture
            .drive(fixture.owner.read_library_metadata())
            .unwrap();
        maximum = maximum.max(read.elapsed());
        assert!(started.elapsed() < Duration::from_secs(10));
    };
    assert!(
        maximum < Duration::from_millis(50),
        "display read queued behind repair analysis: {maximum:?}"
    );
    assert_eq!(preview.mapping_count, 1);
    assert_eq!(preview.indirect_count, 0);
    fixture
        .owner
        .release_guide_repair_plan(&preview.id, preview.digest)
        .unwrap();
}

fn retained_guide(directory: &std::path::Path, source: SourceId, title: &str) {
    use iptv_epg::{ChannelKey, FeedId, MatchKind, ProgrammeData, StoreOptions};
    use iptv_import::{EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
    use iptv_store_sqlite::{EpgFeedMeta, EpgStage, GuideSource};
    let mut store = Store::open_encrypted(
        directory.join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x71; 32]),
    )
    .unwrap();
    let config = GuideSource {
        feed: FeedId(1),
        provider_index: None,
        url: SecretUrl::new("https://synthetic.invalid/retained.xml"),
        name: None,
        enabled: true,
        shift_minutes: 0,
    };
    store
        .set_guide_sources(source, std::slice::from_ref(&config))
        .unwrap();
    let mut stage = EpgStage::create_in_store(&store, directory).unwrap();
    stage
        .begin_feed(config.feed, 0, StoreOptions::default(), 0)
        .unwrap();
    stage
        .programme(
            "amber.main",
            &ProgrammeData {
                start: 1_794_657_600,
                stop: Some(1_794_664_800),
                title,
                ..Default::default()
            },
        )
        .unwrap();
    stage.finish_feed().unwrap();
    let import = EpgImport {
        bindings: vec![(
            ChannelKey::new("amber"),
            iptv_epg::EpgRef {
                source: config.feed,
                epg_id: "amber.main".into(),
                kind: MatchKind::TvgId,
            },
        )],
        listed: vec![(
            config.feed,
            vec![ListedChannel {
                id: "amber.main".into(),
                name: Some("Synthetic amber".into()),
            }],
        )],
        ..Default::default()
    };
    store
        .replace_epg(
            source,
            &mut stage,
            &import,
            &[EpgFeedMeta {
                config: EpgSourceConfig::new(config.feed, config.url),
                fetched_at: 1_794_657_600,
                etag: None,
                last_modified: None,
                bytes: 10,
            }],
            iptv_import::matching_fingerprint(
                &channels(),
                &store
                    .epg_overrides(source)
                    .unwrap()
                    .into_iter()
                    .map(|(id, value)| (ChannelKey::new(id.as_str()), value.to_override()))
                    .collect(),
            ),
        )
        .unwrap();
}

#[test]
fn held_verification_cannot_certify_later_programmes_with_unchanged_mapping_and_configuration() {
    let fixture = Fixture::new();
    retained_guide(
        fixture.dir.path(),
        fixture.source,
        "Synthetic first observation",
    );
    let request = fixture.request("amber", Target::Automatic);
    let preview = fixture.preview(request.clone());
    let saved = fixture.apply(&preview);
    let before = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id.clone(),
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(before.verified_coverage);
    // Creation queues the actual first verification snapshot. This FIFO
    // barrier runs after it and holds the final revalidation behind a refresh.
    let verification = fixture.owner.verify_guide_repair(
        saved.commit.id,
        request.window,
        InspectionControl::default(),
    );
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let _barrier = fixture.owner.services().analyze(move |_| {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    let path = fixture.dir.path().to_owned();
    let source = fixture.source;
    let worker = std::thread::spawn(move || {
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        retained_guide(&path, source, "Synthetic refreshed observation");
        release.send(()).unwrap();
    });
    let result = fixture.drive(verification).unwrap();
    worker.join().unwrap();
    assert_eq!(result.examined, result.current);
    assert_eq!(result.expected, result.current);
    assert_ne!(result.evidence_revision, result.current_evidence_revision);
    assert!(!result.current_revision && !result.verified_coverage);
}

#[test]
fn verification_observes_current_disabled_source_policy() {
    let fixture = Fixture::new();
    let request = fixture.request("amber", Target::NoGuide);
    let preview = fixture.preview(request.clone());
    let saved = fixture.apply(&preview);
    fixture
        .store()
        .set_source_policy(
            fixture.source,
            iptv_store_sqlite::SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        fixture.drive(fixture.owner.verify_guide_repair(
            saved.commit.id,
            request.window,
            InspectionControl::default()
        )),
        Err(Error::Inspection(InspectionError::SourceDisabled(
            fixture.source
        )))
    );
}

#[test]
fn verification_from_before_undo_cannot_overwrite_the_new_undo_operation() {
    verification_across_inverse(Target::NoGuide);
}
#[test]
fn no_op_inverse_direction_also_fences_older_verification() {
    verification_across_inverse(Target::Automatic);
}
fn verification_across_inverse(target: Target) {
    let fixture = Fixture::new();
    let request = fixture.request("amber", target);
    let plan = fixture.preview(request.clone());
    let saved = fixture.apply(&plan);
    let verification = fixture.owner.verify_guide_repair(
        saved.commit.id.clone(),
        request.window,
        InspectionControl::default(),
    );
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let _barrier = fixture.owner.services().analyze(move |_| {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    // The first snapshot has completed. Persist and publish the real inverse
    // while its final revalidation waits, retaining the shared durable id.
    let inverse = committed(fixture.store().undo_guide_repair(&saved.commit.id).unwrap()).unwrap();
    let edit = crate::stored::next_guide_edit(&fixture.owner, fixture.source);
    fixture.drive(publication::after_commit(
        &fixture.owner,
        inverse,
        edit,
        None,
    ));
    release.send(()).unwrap();
    let stale = fixture.drive(verification).unwrap();
    assert!(!stale.current_revision);
    let current = fixture
        .drive(fixture.owner.guide_repair_operation(saved.commit.id))
        .unwrap();
    assert!(current.commit.undone);
    assert_ne!(current.phase, OperationPhase::Superseded);
    assert!(
        current.verification.is_none(),
        "old observation belongs to the original direction"
    );
}

#[test]
fn a_verification_accepted_before_cancel_does_not_clear_the_later_cancelled_status() {
    let fixture = Fixture::new();
    let request = fixture.request("amber", Target::NoGuide);
    let plan = fixture.preview(request.clone());
    let saved = fixture.apply(&plan);
    let verification = fixture.owner.verify_guide_repair(
        saved.commit.id.clone(),
        request.window,
        InspectionControl::default(),
    );
    fixture
        .owner
        .cancel_guide_repair_operation(&saved.commit.id)
        .unwrap();
    let observed = fixture.drive(verification).unwrap();
    assert!(
        observed.verified_coverage,
        "saved choice may still be observed after work cancellation"
    );
    assert_eq!(
        fixture
            .drive(
                fixture
                    .owner
                    .guide_repair_operation(saved.commit.id.clone())
            )
            .unwrap()
            .phase,
        OperationPhase::Cancelled
    );
    // A new explicit verification after cancellation may describe the actual
    // saved result and is not an old completion clearing a later marker.
    fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id.clone(),
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert_eq!(
        fixture
            .drive(fixture.owner.guide_repair_operation(saved.commit.id))
            .unwrap()
            .phase,
        OperationPhase::Verified
    );
}

#[test]
fn durable_inverse_receipt_reconciles_a_stale_original_operation_cache() {
    let fixture = Fixture::new();
    let plan = fixture.preview(fixture.request("amber", Target::NoGuide));
    let saved = fixture.apply(&plan);
    let original = fixture
        .drive(
            fixture
                .owner
                .guide_repair_operation(saved.commit.id.clone()),
        )
        .unwrap();
    assert!(!original.commit.undone);
    let phase = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let writer_phase = phase.clone();
    let child = CancelToken::new();
    let token = child.clone();
    let services = fixture.owner.services();
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let id = saved.commit.id.clone();
    let write = services.call(move |store| {
        write::begin(
            &writer_phase,
            &token,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        let result = committed(store.undo_guide_repair(&id)?);
        writer_phase.store(2, std::sync::atomic::Ordering::SeqCst);
        Ok(result)
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let result = fixture.drive(write::wait(
        &services,
        write::Wait {
            control: InspectionControl::default(),
            deadline: Instant::now() + Duration::from_millis(30),
            id: saved.commit.id.clone(),
            phase,
            child,
        },
        write,
    ));
    assert_eq!(
        result,
        Err(Error::CompletionUnknown {
            operation: saved.commit.id.clone()
        })
    );
    release.send(()).unwrap();
    fixture.drive(services.call(|_| Ok(()))).unwrap();
    let recovered = fixture
        .drive(
            fixture
                .owner
                .guide_repair_operation(saved.commit.id.clone()),
        )
        .unwrap();
    assert!(recovered.commit.undone);
    assert_eq!(recovered.effective, Effective::Pending);
    assert!(recovered.verification.is_none());
    assert_eq!(
        fixture
            .drive(fixture.owner.recent_guide_repairs(
                fixture.source,
                10,
                InspectionControl::default()
            ))
            .unwrap()
            .len(),
        1
    );
    // Status lookup reconciles state; it neither replays the inverse nor starts
    // a refresh on behalf of the abandoned caller.
    assert!(
        fixture
            .store()
            .guide_repair_commit(&saved.commit.id)
            .unwrap()
            .unwrap()
            .undone
    );
}

#[test]
fn automatic_verification_waits_for_full_projection_after_no_guide_undo() {
    let fixture = Fixture::new();
    retained_guide(
        fixture.dir.path(),
        fixture.source,
        "Synthetic automatic before",
    );
    let request = fixture.request("amber", Target::NoGuide);
    let saved = fixture.apply(&fixture.preview(request.clone()));
    fixture.drive(std::future::poll_fn(|_| {
        if fixture
            .store()
            .epg_binding(fixture.source, &iptv_epg::ChannelKey::new("amber"))
            .unwrap()
            .is_none()
        {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }));
    let off = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id.clone(),
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(off.configuration_effective);
    assert!(
        fixture
            .store()
            .epg_binding(fixture.source, &iptv_epg::ChannelKey::new("amber"))
            .unwrap()
            .is_none()
    );
    fixture
        .drive(
            fixture
                .owner
                .undo_guide_repair(saved.commit.id.clone(), InspectionControl::default()),
        )
        .unwrap();
    let pending = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id.clone(),
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(!pending.configuration_effective && !pending.verified_coverage);
    assert_eq!(pending.programmes_available, 0);
    // The injected host keeps the coalesced refresh delayed. A complete normal
    // import installs the automatic projection and provides the missing proof.
    retained_guide(
        fixture.dir.path(),
        fixture.source,
        "Synthetic automatic restored",
    );
    let settled = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id,
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(settled.configuration_effective && settled.verified_coverage);
    assert_eq!(settled.programmes_available, 1);
}

#[test]
fn automatic_verification_rejects_stale_binding_and_partial_fingerprint_renewal() {
    let fixture = Fixture::new();
    retained_guide(
        fixture.dir.path(),
        fixture.source,
        "Synthetic old catalogue",
    );
    let mut changed = channels();
    changed[0].epg_id = Some("amber.other".into());
    fixture
        .store()
        .replace_catalog(fixture.source, &[], &changed, &[], &[])
        .unwrap();
    let request = fixture.request("amber", Target::Automatic);
    let saved = fixture.apply(&fixture.preview(request.clone()));
    let stale = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id.clone(),
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(!stale.configuration_effective && !stale.verified_coverage);
    // An unrelated one-row publication may update the legacy whole-source
    // refresh fingerprint, but cannot certify amber's stale automatic binding.
    let fp = iptv_import::matching_fingerprint(&changed, &Default::default());
    assert!(
        fixture
            .store()
            .rebind_epg(
                fixture.source,
                &iptv_epg::ChannelKey::new("birch"),
                None,
                fp
            )
            .unwrap()
    );
    let result = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id,
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert_eq!(
        result.programmes_available, 1,
        "old real programmes remain visible"
    );
    assert!(!result.configuration_effective && !result.verified_coverage);
}

#[test]
fn feed_only_verification_requires_current_full_projection_configuration() {
    let fixture = Fixture::new();
    retained_guide(
        fixture.dir.path(),
        fixture.source,
        "Synthetic cached enabled feed",
    );
    let original = fixture.store().guide_sources(fixture.source).unwrap();
    let mut disabled = original.clone();
    disabled[0].enabled = false;
    fixture
        .store()
        .set_guide_sources(fixture.source, &disabled)
        .unwrap();
    // Preserve the current transport bytes and binding while changing only the
    // requested enabled state. Re-enabling must await a full current projection.
    let mut request = fixture.request("amber", Target::Automatic);
    request.mappings.clear();
    request.feeds.push(FeedEdit {
        feed: iptv_epg::FeedId(1),
        enabled: Some(true),
        shift_minutes: None,
        remove: false,
    });
    let saved = fixture.apply(&fixture.preview(request.clone()));
    // Simulate independently authenticated retained transport metadata. Its
    // current feed origin does not establish the automatic match projection.
    let conn = rusqlite::Connection::open(fixture.dir.path().join("iptv.sqlite")).unwrap();
    conn.pragma_update(None, "key", format!("x'{}'", "71".repeat(32)))
        .unwrap();
    conn.execute(
        "UPDATE store_meta SET value=(SELECT incarnation FROM guide_feed_incarnations
        WHERE source_id=?1 AND feed_id=1) WHERE key='inspection.feed.' || ?1 || '.1'",
        [fixture.source.0],
    )
    .unwrap();
    drop(conn);
    assert!(
        fixture
            .store()
            .inspection_stored_feed_incarnation(fixture.source, iptv_epg::FeedId(1))
            .unwrap()
            .is_some()
    );
    let result = fixture
        .drive(fixture.owner.verify_guide_repair(
            saved.commit.id,
            request.window,
            InspectionControl::default(),
        ))
        .unwrap();
    assert!(!result.configuration_effective && !result.verified_coverage);
}

#[test]
fn held_verification_detects_catalogue_change_without_mapping_or_config_edit() {
    let fixture = Fixture::new();
    retained_guide(
        fixture.dir.path(),
        fixture.source,
        "Synthetic before catalogue change",
    );
    let request = fixture.request("amber", Target::Automatic);
    let saved = fixture.apply(&fixture.preview(request.clone()));
    let verification = fixture.owner.verify_guide_repair(
        saved.commit.id,
        request.window,
        InspectionControl::default(),
    );
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let _barrier = fixture.owner.services().analyze(move |_| {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    let path = fixture.dir.path().to_owned();
    let source = fixture.source;
    let worker = std::thread::spawn(move || {
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut store = Store::open_encrypted(
            path.join("iptv.sqlite"),
            &DatabaseKey::from_bytes([0x71; 32]),
        )
        .unwrap();
        let mut changed = channels();
        changed[0].epg_id = Some("amber.other".into());
        store
            .replace_catalog(source, &[], &changed, &[], &[])
            .unwrap();
        release.send(()).unwrap();
    });
    let result = fixture.drive(verification).unwrap();
    worker.join().unwrap();
    assert_eq!(result.examined, result.current);
    assert_eq!(result.expected, result.current);
    assert_ne!(result.evidence_revision, result.current_evidence_revision);
    assert!(!result.current_revision && !result.verified_coverage);
}
