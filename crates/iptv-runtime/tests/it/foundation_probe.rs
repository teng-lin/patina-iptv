//! Proposed guides are inspected without changing configured sources, mappings
//! or the viewed schedule. Every endpoint and programme in these tests is generated.
use crate::support;
use iptv_epg::FeedId;
use iptv_library::{GuideSource, SourceId};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_runtime::{Ports, Runtime, inspection::*};
use iptv_store_sqlite::{DatabaseKey, Store};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

const XML: &str = concat!(
    "<tv><channel id='amber.main'><display-name>Synthetic Amber</display-name></channel>",
    "<channel id='amber.alt'><display-name>Synthetic Amber East</display-name>",
    "<display-name>Synthetic Alternate</display-name></channel>",
    "<programme channel='amber.alt' start='20261114120000 +0000' stop='20261114130000 +0000'>",
    "<title>Synthetic Segment 01</title></programme>",
    "<programme channel='amber.alt' start='20261114140000 +0200' stop='20261114150000 +0200'>",
    "<title>Synthetic Segment 02</title></programme></tv>",
);

struct Mock {
    address: String,
    stopped: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    headers: Arc<Mutex<Vec<String>>>,
    seen: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
    task: Option<thread::JoinHandle<()>>,
}
impl Mock {
    fn new(body: Vec<u8>, hold: bool) -> Self {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        Self::response(response, hold)
    }
    fn response(response: Vec<u8>, hold: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let headers = Arc::new(Mutex::new(Vec::new()));
        let captured = headers.clone();
        let (seen_tx, seen) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let mut wait = hold.then_some(wait);
        let task = thread::spawn(move || {
            for connection in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = connection else {
                    break;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                while !bytes.windows(4).any(|p| p == b"\r\n\r\n") {
                    let Ok(n) = stream.read(&mut buffer) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                count.fetch_add(1, Ordering::SeqCst);
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&bytes).into_owned());
                let _ = seen_tx.send(());
                if let Some(wait) = wait.take() {
                    let _ = wait.recv();
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let _ = stream.write_all(&response);
            }
        });
        Self {
            address,
            stopped,
            requests,
            headers,
            seen,
            release: Some(release),
            task: Some(task),
        }
    }
    fn url(&self) -> SecretUrl {
        SecretUrl::new(format!("http://{}/synthetic.xml", self.address))
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.release.take();
        let _ = TcpStream::connect(&self.address);
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}
fn seed(directory: &std::path::Path, url: SecretUrl, shift: i32) -> SourceId {
    std::fs::create_dir_all(directory).unwrap();
    let mut store = Store::open_encrypted(
        directory.join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x71; 32]),
    )
    .unwrap();
    let source = store
        .add_source(&Source {
            name: "Synthetic catalogue".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://synthetic.invalid/generated.m3u"),
                user_agent: Some("Synthetic-Private-Agent".into()),
                referrer: None,
            }),
        })
        .unwrap();
    store
        .set_guide_sources(
            source,
            &[GuideSource {
                feed: FeedId(1),
                provider_index: None,
                url,
                name: Some("Synthetic guide".into()),
                enabled: true,
                shift_minutes: shift,
            }],
        )
        .unwrap();
    source
}
fn owner(directory: &std::path::Path) -> Runtime {
    let runtime = Runtime::new(Ports::new(directory.to_owned(), |_| {
        Ok(DatabaseKey::from_bytes([0x71; 32]))
    }));
    runtime.start_services();
    runtime
}
fn control() -> InspectionControl {
    InspectionControl {
        timeout: Duration::from_secs(60),
        ..Default::default()
    }
}
fn probe(runtime: &Runtime, source: SourceId) -> InspectionResult<ProbeSummary> {
    support::block_on(runtime.probe_feed(
        ProbeRequest {
            source,
            target: ProbeTarget::Configured(FeedId(1)),
        },
        control(),
    ))
}
fn window() -> TimeWindow {
    TimeWindow {
        from: iptv_epg::parse_xmltv_time("20261114120000 +0000", iptv_epg::TimePolicy::UTC)
            .unwrap(),
        to: iptv_epg::parse_xmltv_time("20261114160000 +0000", iptv_epg::TimePolicy::UTC).unwrap(),
    }
}

#[test]
fn unmatched_candidates_have_samples_from_the_exact_encrypted_snapshot_without_live_mutation() {
    let server = Mock::new(XML.as_bytes().to_vec(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 60);
    let runtime = owner(directory.path());
    let before = runtime.browser_generation();
    let summary = probe(&runtime, source).unwrap();
    assert_eq!((summary.channels, summary.programmes), (2, 2));
    let candidates = support::block_on(runtime.probe_candidates(
        summary.evidence.clone(),
        String::new(),
        None,
        200,
        control(),
    ))
    .unwrap();
    assert_eq!(candidates.items.len(), 2);
    let candidate = candidates
        .items
        .iter()
        .find(|c| c.key.epg_channel == "amber.alt")
        .unwrap();
    assert_eq!(
        candidate.names,
        ["Synthetic Amber East", "Synthetic Alternate"]
    );
    let samples = support::block_on(runtime.probe_schedule(
        summary.evidence.clone(),
        candidate.key.clone(),
        window(),
        control(),
    ))
    .unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].start, window().from + 3600);
    assert_eq!(
        samples[0].title, "Synthetic Segment 01",
        "equivalent-offset duplicates follow shared schedule settlement"
    );
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        1,
        "sample inspection cannot refetch changed bytes"
    );
    assert_eq!(runtime.browser_generation(), before);
    assert!(server.headers.lock().unwrap()[0].contains("Synthetic-Private-Agent"));
    let store = Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x71; 32]),
    )
    .unwrap();
    assert_eq!(store.guide_sources(source).unwrap().len(), 1);
    assert!(store.guide_channels(source).unwrap().is_empty());
    let files: Vec<_> = std::fs::read_dir(directory.path().join("guide-probes"))
        .unwrap()
        .flatten()
        .flat_map(|e| {
            std::fs::read_dir(e.path())
                .unwrap()
                .flatten()
                .map(|f| f.path())
                .collect::<Vec<_>>()
        })
        .filter(|p| p.extension().is_some_and(|e| e == "sqlite"))
        .collect();
    assert_eq!(files.len(), 1);
    let bytes = std::fs::read(&files[0]).unwrap();
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(
        !bytes
            .windows(b"Synthetic Amber".len())
            .any(|p| p == b"Synthetic Amber")
    );
    let lease = runtime.lease_probe(&summary.evidence).unwrap();
    runtime.drain_services();
    assert_eq!(lease.validate_alive(), Err(InspectionError::OwnerClosed));
    assert!(
        files[0].exists(),
        "lease pins bytes until in-flight readers finish"
    );
    drop(lease);
    assert!(!files[0].exists());
}

fn assert_probe_cache_empty(directory: &std::path::Path) {
    let cache = directory.join("guide-probes");
    if cache.try_exists().unwrap() {
        assert!(
            std::fs::read_dir(cache).unwrap().next().is_none(),
            "probe shutdown must remove its partial stages and owner directory"
        );
    }
}

#[test]
fn end_to_end_deadlines_allow_expiry_before_http_and_remove_partial_stages() {
    for timeout in [Duration::ZERO, Duration::from_millis(250)] {
        let server = Mock::new(XML.as_bytes().to_vec(), true);
        let directory = tempfile::tempdir().unwrap();
        let source = seed(directory.path(), server.url(), 0);
        let runtime = owner(directory.path());
        let mut options = control();
        options.timeout = timeout;
        let request = runtime.probe_feed(
            ProbeRequest {
                source,
                target: ProbeTarget::Configured(FeedId(1)),
            },
            options,
        );
        // This budget includes encrypted-store and worker startup. Expiry can
        // precede the connection; the mock must not be required to see HTTP.
        assert_eq!(support::block_on(request), Err(InspectionError::Deadline));
        assert!(runtime.drain_services());
        assert_probe_cache_empty(directory.path());
        if timeout.is_zero() {
            assert_eq!(server.requests.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn cancellation_ends_an_acknowledged_request_and_removes_its_partial_stage() {
    let server = Mock::new(XML.as_bytes().to_vec(), true);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let runtime = owner(directory.path());
    let options = control();
    let cancel = options.cancel.clone();
    let request = runtime.probe_feed(
        ProbeRequest {
            source,
            target: ProbeTarget::Configured(FeedId(1)),
        },
        options,
    );
    // Acknowledge transport entry before cancellation, with an operation budget
    // that leaves room for cold encrypted-store startup on shared runners.
    server.seen.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        std::fs::read_dir(directory.path().join("guide-probes"))
            .unwrap()
            .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "sqlite")),
        "the acknowledged request must have created a partial stage"
    );
    cancel.cancel();
    assert_eq!(support::block_on(request), Err(InspectionError::Cancelled));
    assert!(runtime.drain_services());
    assert_probe_cache_empty(directory.path());
}

#[cfg(feature = "test-support")]
#[test]
fn shutdown_waits_for_a_cancelled_probe_worker_to_release_its_partial_stage() {
    let server = Mock::new(XML.as_bytes().to_vec(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let runtime = owner(directory.path());
    let held = runtime.hold_probe_stage();
    let draining = runtime.observe_probe_drain();
    let options = control();
    let cancel = options.cancel.clone();
    let request = runtime.probe_feed(
        ProbeRequest {
            source,
            target: ProbeTarget::Configured(FeedId(1)),
        },
        options,
    );
    assert!(held.wait_entered(Duration::from_secs(30)));
    cancel.cancel();
    assert_eq!(support::block_on(request), Err(InspectionError::Cancelled));
    let path = directory.path().to_owned();
    let release = thread::spawn(move || {
        assert!(draining.wait_entered(Duration::from_secs(30)));
        assert!(
            std::fs::read_dir(path.join("guide-probes"))
                .unwrap()
                .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
                .any(|entry| entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "sqlite")),
            "teardown must still be waiting while the partial stage is held"
        );
        held.release();
    });
    assert!(runtime.drain_services());
    release.join().unwrap();
    assert_probe_cache_empty(directory.path());
}

#[test]
fn malformed_html_entities_and_truncated_xml_never_become_probe_evidence() {
    for body in [
        "<html><channel id='a'/></html>",
        "<tv><channel id='a'>",
        "<!DOCTYPE tv [<!ENTITY local SYSTEM 'file:///synthetic-secret'>]><tv/>",
        "<tv><channel id='a'/><channel id='a'/></tv>",
    ] {
        let server = Mock::new(body.as_bytes().to_vec(), false);
        let directory = tempfile::tempdir().unwrap();
        let source = seed(directory.path(), server.url(), 0);
        let runtime = owner(directory.path());
        assert_eq!(
            probe(&runtime, source),
            Err(InspectionError::InvalidFeed),
            "{body}"
        );
        runtime.drain_services();
    }
}

#[test]
fn changed_config_and_forged_evidence_cannot_reuse_a_stage() {
    let server = Mock::new(XML.as_bytes().to_vec(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let runtime = owner(directory.path());
    let summary = probe(&runtime, source).unwrap();
    let selected = support::block_on(runtime.probe_candidates(
        summary.evidence.clone(),
        String::new(),
        None,
        50,
        control(),
    ))
    .unwrap()
    .items
    .remove(0)
    .key;
    let mut forged = summary.evidence.clone();
    forged.digest[0] ^= 1;
    assert!(matches!(
        runtime.lease_probe(&forged),
        Err(InspectionError::StaleEvidence)
    ));
    let mut store = Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x71; 32]),
    )
    .unwrap();
    let mut guides = store.guide_sources(source).unwrap();
    guides[0].shift_minutes = 60;
    store.set_guide_sources(source, &guides).unwrap();
    assert_eq!(
        support::block_on(runtime.probe_candidates(
            summary.evidence,
            String::new(),
            None,
            50,
            control()
        )),
        Err(InspectionError::StaleEvidence)
    );
    let refreshed = probe(&runtime, source).unwrap();
    assert_ne!(selected.incarnation, refreshed.evidence.incarnation);
    // A new snapshot must never retarget the candidate selected from the old list.
    assert_eq!(
        support::block_on(runtime.probe_schedule(
            refreshed.evidence,
            selected,
            TimeWindow {
                from: 1_800_000_000,
                to: 1_800_007_200
            },
            control(),
        )),
        Err(InspectionError::StaleEvidence)
    );
    runtime.drain_services();
}

#[test]
fn abandoned_crash_cache_is_cleaned_without_deleting_an_active_owner() {
    let server = Mock::new(XML.as_bytes().to_vec(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let base = directory.path().join("guide-probes");
    let dead = base.join("a".repeat(32));
    let active = base.join("b".repeat(32));
    for path in [&dead, &active] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("owner.lock"), b"").unwrap();
        std::fs::write(path.join("unreadable.sqlite"), b"generated crash artifact").unwrap();
    }
    let live_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(active.join("owner.lock"))
        .unwrap();
    live_lock.lock().unwrap();
    let runtime = owner(directory.path());
    probe(&runtime, source).unwrap();
    assert!(!dead.exists());
    assert!(active.exists());
    runtime.drain_services();
}

#[test]
fn a_reused_feed_id_cannot_borrow_its_previous_cached_channel_list() {
    use iptv_import::{EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
    use iptv_store_sqlite::{EpgFeedMeta, EpgStage};
    let directory = tempfile::tempdir().unwrap();
    let url = SecretUrl::new("https://synthetic.invalid/generated-guide.xml");
    let source = seed(directory.path(), url.clone(), 0);
    let mut store = Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x71; 32]),
    )
    .unwrap();
    let mut parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"amber.main\",Synthetic Amber\nhttp://stream.example.test/a\n",
    );
    parsed.channels.items[0].name = "ZZ: Synthetic Amber HD".into();
    parsed.channels.items[0].epg_id = None;
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    let mut stage = EpgStage::create_in_store(&store, directory.path()).unwrap();
    stage
        .begin_feed(FeedId(1), 0, iptv_epg::StoreOptions::default(), 0)
        .unwrap();
    stage.finish_feed().unwrap();
    let listing = EpgImport {
        listed: vec![(
            FeedId(1),
            vec![ListedChannel {
                id: "amber.main".into(),
                name: Some("Synthetic Amber".into()),
            }],
        )],
        ..Default::default()
    };
    store
        .replace_epg(
            source,
            &mut stage,
            &listing,
            &[EpgFeedMeta {
                config: EpgSourceConfig::new(FeedId(1), url),
                fetched_at: 0,
                etag: None,
                last_modified: None,
                bytes: 128,
            }],
            0,
        )
        .unwrap();
    let runtime = owner(directory.path());
    let key = ChannelRef {
        source,
        content: parsed.channels.items[0].id.clone(),
    };
    let first =
        support::block_on(runtime.epg_candidates(key.clone(), Query::source(source), control()))
            .unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.items[0].evidence.contains(&Evidence::NormalizedName));
    let old = first.items[0].key.incarnation.clone();
    let original = store.guide_sources(source).unwrap();
    store.set_guide_sources(source, &[]).unwrap();
    store.set_guide_sources(source, &original).unwrap();
    assert_ne!(
        store.feed_incarnation(source, FeedId(1)).unwrap(),
        Some(old)
    );
    assert!(
        !store.guide_channels(source).unwrap().is_empty(),
        "old cached rows deliberately remain"
    );
    let second =
        support::block_on(runtime.epg_candidates(key, Query::source(source), control())).unwrap();
    assert!(
        second.items.is_empty(),
        "new feed incarnation cannot borrow old listing evidence"
    );
    runtime.drain_services();
}

#[test]
fn supplied_urls_do_not_borrow_provider_credentials_or_accept_url_userinfo() {
    let server = Mock::new(XML.as_bytes().to_vec(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let runtime = owner(directory.path());
    let url = SecretUrl::new(format!(
        "http://synthetic-user:synthetic-password@{}/guide.xml",
        server.address
    ));
    assert_eq!(
        support::block_on(runtime.probe_feed(
            ProbeRequest {
                source,
                target: ProbeTarget::Supplied(url)
            },
            control()
        )),
        Err(InspectionError::AddressPolicy)
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    #[cfg(feature = "test-support")]
    {
        let summary = support::block_on(runtime.probe_feed(
            ProbeRequest {
                source,
                target: ProbeTarget::Supplied(server.url()),
            },
            control(),
        ))
        .unwrap();
        let headers = server.headers.lock().unwrap();
        assert!(!headers[0].contains("Synthetic-Private-Agent"));
        drop(headers);
        let lease = runtime.lease_probe(&summary.evidence).unwrap();
        let candidate = CandidateRef {
            source,
            feed: FeedId(0),
            incarnation: summary.evidence.incarnation.clone(),
            epg_channel: "amber.alt".into(),
        };
        let cancel = iptv_import::CancelToken::new();
        cancel.cancel();
        assert_eq!(
            lease.validate_candidates_with_control(
                &[candidate],
                &cancel,
                std::time::Instant::now() + Duration::from_secs(3)
            ),
            Err(InspectionError::Cancelled)
        );
        assert_eq!(
            lease.listed_channels_with_control(
                &cancel,
                std::time::Instant::now() + Duration::from_secs(3)
            ),
            Err(InspectionError::Cancelled)
        );
    }
    runtime.drain_services();
}

#[test]
fn probes_stop_after_three_redirects_and_at_the_download_byte_budget() {
    let redirect=Mock::response(b"HTTP/1.1 302 Found\r\nLocation: /synthetic.xml\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), redirect.url(), 0);
    let runtime = owner(directory.path());
    assert_eq!(probe(&runtime, source), Err(InspectionError::Network));
    assert_eq!(redirect.requests.load(Ordering::SeqCst), 4);
    runtime.drain_services();

    let mut xml = String::from("<tv><channel id='amber'/>");
    let comment = format!("<!-- {} -->", "x".repeat(1024));
    while xml.len() <= 32 << 20 {
        xml.push_str(&comment);
    }
    xml.push_str("</tv>");
    let server = Mock::new(xml.into_bytes(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let runtime = owner(directory.path());
    assert_eq!(probe(&runtime, source), Err(InspectionError::LimitExceeded));
    runtime.drain_services();
}

#[cfg(not(feature = "test-support"))]
#[test]
fn configured_local_feed_redirects_cannot_reach_another_private_origin() {
    let destination = Mock::new(XML.as_bytes().to_vec(), false);
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        destination.url().expose()
    );
    let origin = Mock::response(response.into_bytes(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), origin.url(), 0);
    let runtime = owner(directory.path());
    assert_eq!(probe(&runtime, source), Err(InspectionError::AddressPolicy));
    assert_eq!(destination.requests.load(Ordering::SeqCst), 0);
    runtime.drain_services();
}

#[test]
fn owner_shutdown_cancels_a_held_probe_before_the_transport_idle_timeout() {
    let server = Mock::new(XML.as_bytes().to_vec(), true);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let runtime = owner(directory.path());
    let request = runtime.probe_feed(
        ProbeRequest {
            source,
            target: ProbeTarget::Configured(FeedId(1)),
        },
        control(),
    );
    server.seen.recv_timeout(Duration::from_secs(5)).unwrap();
    let start = std::time::Instant::now();
    runtime.drain_services();
    assert_eq!(
        support::block_on(request),
        Err(InspectionError::OwnerClosed)
    );
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "shutdown waited for the15second transport idle timeout"
    );
}

#[test]
fn missing_identity_and_a_failed_feed_are_distinct_and_invalidate_audit_pages() {
    let directory = tempfile::tempdir().unwrap();
    let source = seed(
        directory.path(),
        SecretUrl::new("https://synthetic.invalid/guide.xml"),
        0,
    );
    let mut store = Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x71; 32]),
    )
    .unwrap();
    let mut parsed = iptv_providers::m3u::parse_m3u(concat!(
        "#EXTM3U\n#EXTINF:-1,Synthetic Amber\nhttp://stream.example.test/a\n",
        "#EXTINF:-1,Synthetic Birch\nhttp://stream.example.test/b\n"
    ));
    parsed.channels.items[0].name.clear();
    parsed.channels.items[0].epg_id = None;
    parsed.channels.items[0].epg_name = None;
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    let runtime = owner(directory.path());
    let first =
        support::block_on(runtime.epg_audit(Query::source(source), window(), control())).unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .find(|c| c.channel.key.content == parsed.channels.items[0].id)
            .unwrap()
            .diagnosis,
        Diagnosis::MissingMetadata
    );
    let mut paged = Query::source(source);
    paged.limit = 1;
    let cursor = support::block_on(runtime.epg_audit(paged.clone(), window(), control()))
        .unwrap()
        .next
        .unwrap();
    store
        .record_epg_feed_results(
            source,
            &[(FeedId(1), Some(iptv_library::ImportErrorKind::Network))],
        )
        .unwrap();
    paged.cursor = Some(cursor);
    assert_eq!(
        support::block_on(runtime.epg_audit(paged, window(), control())),
        Err(InspectionError::StaleCursor)
    );
    let failed =
        support::block_on(runtime.epg_audit(Query::source(source), window(), control())).unwrap();
    assert!(
        failed
            .items
            .iter()
            .all(|c| c.diagnosis == Diagnosis::InputFailure)
    );
    runtime.drain_services();
}

#[test]
fn prewindow_successors_settle_missing_stops_before_filtering_in_any_document_order() {
    let from = window().from;
    let mut rows: Vec<_> = (0..300)
        .map(|i| {
            (
                from - 100_000 - i * 60,
                Some(from - 99_980 - i * 60),
                "Synthetic Past",
            )
        })
        .collect();
    rows.extend([
        (from - 600, None, "Synthetic A must not reach the window"),
        (from - 300, None, "Synthetic shorter duplicate"),
        (from - 300, Some(from - 60), "Synthetic B boundary"),
        (from - 120, Some(from - 180), "Synthetic invalid boundary"),
        (from + 600, Some(from + 1200), "Synthetic C visible"),
        (from + 1500, None, "Synthetic D inferred stop"),
        (from + 2400, Some(from + 3000), "Synthetic later lookahead"),
        (
            from + 2100,
            Some(from + 2200),
            "Synthetic nearest lookahead",
        ),
    ]);
    for order in 0..3 {
        let mut ordered = rows.clone();
        if order == 1 {
            ordered.reverse();
        } else if order == 2 {
            ordered.sort_by_key(|row| (row.0 % 7, row.0));
        }
        let mut xml = String::from(
            "<tv><channel id='amber.alt'><display-name>Synthetic Boundary</display-name></channel>",
        );
        for (start, stop, title) in ordered {
            let start = iptv_epg::format_xmltv_time(start);
            let stop = stop
                .map(|time| format!(" stop='{}'", iptv_epg::format_xmltv_time(time)))
                .unwrap_or_default();
            xml.push_str(&format!("<programme channel='amber.alt' start='{start}'{stop}><title>{title}</title></programme>"));
        }
        xml.push_str("</tv>");
        let server = Mock::new(xml.into_bytes(), false);
        let directory = tempfile::tempdir().unwrap();
        let source = seed(directory.path(), server.url(), 0);
        let runtime = owner(directory.path());
        let summary = probe(&runtime, source).unwrap();
        let candidate = CandidateRef {
            source,
            feed: FeedId(1),
            incarnation: summary.evidence.incarnation.clone(),
            epg_channel: "amber.alt".into(),
        };
        let samples = support::block_on(runtime.probe_schedule(
            summary.evidence,
            candidate,
            TimeWindow {
                from,
                to: from + 1800,
            },
            control(),
        ))
        .unwrap();
        let actual: Vec<_> = samples
            .iter()
            .map(|row| (row.title.as_str(), row.start, row.stop))
            .collect();
        assert_eq!(
            actual,
            [
                ("Synthetic C visible", from + 600, from + 1200),
                ("Synthetic D inferred stop", from + 1500, from + 2100),
            ],
            "document order {order}; hundreds of past rows cannot exhaust the sample bound"
        );
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        runtime.drain_services();
    }
}

#[cfg(feature = "test-support")]
#[test]
fn staged_reads_share_query_admission_until_the_actual_blocking_workers_exit() {
    use std::{future::Future, pin::Pin, time::Instant};
    let server = Mock::new(XML.as_bytes().to_vec(), false);
    let directory = tempfile::tempdir().unwrap();
    let source = seed(directory.path(), server.url(), 0);
    let runtime = owner(directory.path());
    let summary = probe(&runtime, source).unwrap();
    let candidate = CandidateRef {
        source,
        feed: FeedId(1),
        incarnation: summary.evidence.incarnation.clone(),
        epg_channel: "amber.alt".into(),
    };
    let (entered, release) = runtime.hold_analysis();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut pending: Vec<Pin<Box<dyn Future<Output = InspectionResult<()>>>>> = Vec::new();
    for _ in 0..8 {
        let candidates =
            runtime.probe_candidates(summary.evidence.clone(), String::new(), None, 50, control());
        pending.push(Box::pin(async move { candidates.await.map(|_| ()) }));
        let samples = runtime.probe_schedule(
            summary.evidence.clone(),
            candidate.clone(),
            window(),
            control(),
        );
        pending.push(Box::pin(async move { samples.await.map(|_| ()) }));
    }
    let started = Instant::now();
    assert_eq!(
        support::block_on(runtime.probe_candidates(
            summary.evidence.clone(),
            String::new(),
            None,
            50,
            control(),
        )),
        Err(InspectionError::Busy)
    );
    assert_eq!(
        support::block_on(runtime.probe_schedule(
            summary.evidence.clone(),
            candidate,
            window(),
            control(),
        )),
        Err(InspectionError::Busy)
    );
    assert_eq!(
        support::block_on(runtime.query_channels(Query::source(source), control())),
        Err(InspectionError::Busy)
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "admission rejection cannot wait behind held analysis"
    );
    drop(pending);
    assert_eq!(
        support::block_on(runtime.probe_candidates(
            summary.evidence.clone(),
            String::new(),
            None,
            50,
            control(),
        )),
        Err(InspectionError::Busy),
        "dropping replies cannot release slots held by live workers"
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match support::block_on(runtime.probe_candidates(
            summary.evidence.clone(),
            String::new(),
            None,
            50,
            control(),
        )) {
            Err(InspectionError::Busy) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10))
            }
            result => {
                assert_eq!(result.unwrap().items.len(), 2);
                break;
            }
        }
    }
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        1,
        "all inspection uses the retained snapshot"
    );
    runtime.drain_services();
}

#[test]
fn two_held_feed_downloads_refuse_more_work_until_heavy_workers_finish() {
    let mut servers = [
        Mock::new(XML.as_bytes().to_vec(), true),
        Mock::new(XML.as_bytes().to_vec(), true),
    ];
    let directory = tempfile::tempdir().unwrap();
    let sources = servers
        .each_ref()
        .map(|server| seed(directory.path(), server.url(), 0));
    let runtime = owner(directory.path());
    let requests = sources.map(|source| {
        runtime.probe_feed(
            ProbeRequest {
                source,
                target: ProbeTarget::Configured(FeedId(1)),
            },
            control(),
        )
    });
    for server in &servers {
        server.seen.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    assert_eq!(probe(&runtime, sources[0]), Err(InspectionError::Busy));
    assert_eq!(probe(&runtime, sources[1]), Err(InspectionError::Busy));
    for server in &mut servers {
        server.release.take().unwrap().send(()).unwrap();
    }
    for request in requests {
        assert_eq!(support::block_on(request).unwrap().channels, 2);
    }
    assert_eq!(probe(&runtime, sources[0]).unwrap().channels, 2);
    assert_eq!(servers[0].requests.load(Ordering::SeqCst), 2);
    assert_eq!(servers[1].requests.load(Ordering::SeqCst), 1);
    runtime.drain_services();
}
