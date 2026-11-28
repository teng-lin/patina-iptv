//! Runtime stream resolution: per-purpose cancellation, shutdown sweep,
//! interactive deadline, and stale portal-session protection.
#![cfg(feature = "test-support")]

use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use iptv_guide::playback::{PlaybackContext, Resolution, StalkerLink};
use iptv_providers::{
    model::MacAddress,
    stalker::{PortalMode, StalkerPortal, StalkerSession},
};
use iptv_runtime::{
    Ports, Runtime,
    playback::{ResolveFailure, StreamPurpose},
};

const WAIT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct PortalState {
    slow_asked: usize,
    slow_abandoned: usize,
    quick_asked: usize,
    handshakes: usize,
    held_responded: bool,
    release_held: bool,
}

struct MockPortal {
    address: String,
    state: Arc<(Mutex<PortalState>, Condvar)>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MockPortal {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let state = Arc::new((Mutex::new(PortalState::default()), Condvar::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let stop = stopped.clone();
        let handshakes = Arc::new(AtomicUsize::new(0));
        let worker = thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let state = shared.clone();
                let handshakes = handshakes.clone();
                thread::spawn(move || handle(stream, &state, &handshakes));
            }
        });
        Self {
            address,
            state,
            stopped,
            worker: Some(worker),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/portal.php", self.address)
    }

    fn wait_for(&self, label: &str, predicate: impl Fn(&PortalState) -> bool) {
        let (lock, condvar) = &*self.state;
        let guard = lock.lock().unwrap();
        let (guard, result) = condvar
            .wait_timeout_while(guard, WAIT, |state| !predicate(state))
            .unwrap();
        assert!(
            predicate(&guard) && !result.timed_out(),
            "timed out waiting for {label}"
        );
    }

    fn release_held(&self) {
        let (lock, condvar) = &*self.state;
        lock.lock().unwrap().release_held = true;
        condvar.notify_all();
    }

    fn resolution(&self, cmd: &str) -> (Resolution, Rc<PlaybackContext>) {
        let portal = StalkerPortal::new(
            &self.endpoint(),
            MacAddress::parse("00:1A:79:00:00:99").unwrap(),
            PortalMode::Full,
        )
        .unwrap();
        let session = StalkerSession::new(portal);
        let context = Rc::new(PlaybackContext::stalker(session.clone()));
        let resolution = Resolution::StalkerLink(Box::new(StalkerLink {
            session,
            cmd: cmd.to_owned(),
            channel_headers: Vec::new(),
        }));
        (resolution, context)
    }
}

impl Drop for MockPortal {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.release_held();
        let _ = TcpStream::connect(&self.address);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle(
    mut stream: TcpStream,
    state: &(Mutex<PortalState>, Condvar),
    handshakes: &AtomicUsize,
) {
    let _ = stream.set_read_timeout(Some(WAIT));
    let mut request = Vec::new();
    let mut buffer = [0u8; 2048];
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buffer[..n]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let path = request.split_whitespace().nth(1).unwrap_or("");
    let action = path
        .split_once('?')
        .into_iter()
        .flat_map(|(_, query)| query.split('&'))
        .find_map(|pair| pair.strip_prefix("action="))
        .unwrap_or("");
    let body = match action {
        "handshake" => {
            let n = handshakes.fetch_add(1, Ordering::SeqCst) + 1;
            {
                let mut guard = state.0.lock().unwrap();
                guard.handshakes = n;
            }
            state.1.notify_all();
            format!(r#"{{"js":{{"token":"token-{n}","random":"rand-{n}"}}}}"#)
        }
        "get_profile" => r#"{"js":{"status":0,"watchdog_timeout":60}}"#.to_owned(),
        "create_link" if path.contains("slow") => {
            {
                state.0.lock().unwrap().slow_asked += 1;
            }
            state.1.notify_all();
            if wait_for_close(&mut stream, WAIT) {
                state.0.lock().unwrap().slow_abandoned += 1;
                state.1.notify_all();
            }
            return;
        }
        "create_link" if path.contains("held") => {
            {
                state.0.lock().unwrap().slow_asked += 1;
            }
            state.1.notify_all();
            let guard = state.0.lock().unwrap();
            let _ = state
                .1
                .wait_timeout_while(guard, WAIT, |s| !s.release_held)
                .unwrap();
            let body = r#"{"js":{"cmd":"ffmpeg http://stream.example.test/held-resolved.m3u8"}}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
            drop(stream);
            state.0.lock().unwrap().held_responded = true;
            state.1.notify_all();
            return;
        }
        "create_link" => {
            {
                state.0.lock().unwrap().quick_asked += 1;
            }
            state.1.notify_all();
            r#"{"js":{"cmd":"ffmpeg http://stream.example.test/quick-resolved.m3u8"}}"#.to_owned()
        }
        _ => r#"{"js":{}}"#.to_owned(),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn wait_for_close(stream: &mut TcpStream, budget: Duration) -> bool {
    let _ = stream.set_read_timeout(Some(budget));
    loop {
        match stream.read(&mut [0u8; 256]) {
            Ok(0) => return true,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => {
                return matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::BrokenPipe
                );
            }
            Ok(_) => {}
        }
    }
}

fn runtime() -> (tempfile::TempDir, Rc<Runtime>) {
    let directory = tempfile::tempdir().unwrap();
    let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x42; 32]))
    })));
    owner.start_services();
    (directory, owner)
}

#[tokio::test(flavor = "current_thread")]
async fn starting_a_second_resolution_for_the_same_purpose_cancels_the_first() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = MockPortal::new();
            let (_dir, owner) = runtime();
            let (slow, ctx_a) = portal.resolution("ffmpeg http://stream.example.test/slow/1.ts");
            let (quick, ctx_b) = portal.resolution("ffmpeg http://stream.example.test/quick/2.ts");

            let first = owner.resolve_stream(StreamPurpose::External, slow, Some(ctx_a));
            portal.wait_for("first slow create_link", |s| s.slow_asked == 1);

            let second = owner.resolve_stream(StreamPurpose::External, quick, Some(ctx_b));
            portal.wait_for("first request cancelled", |s| s.slow_abandoned == 1);

            assert_eq!(first.await, Err(ResolveFailure::Cancelled));
            let resolved = second.await.expect("second resolution succeeds");
            assert_eq!(
                resolved.url,
                "http://stream.example.test/quick-resolved.m3u8"
            );
            assert!(owner.drain_services());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_stream_resolution_cancels_the_portal_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = MockPortal::new();
            let (_dir, owner) = runtime();
            let (slow, ctx) = portal.resolution("ffmpeg http://stream.example.test/slow/cancel.ts");

            let pending = owner.resolve_stream(StreamPurpose::Cast, slow, Some(ctx));
            portal.wait_for("cast create_link", |s| s.slow_asked == 1);

            owner.cancel_stream_resolution(StreamPurpose::Cast);
            portal.wait_for("cast request cancelled", |s| s.slow_abandoned == 1);

            assert_eq!(pending.await, Err(ResolveFailure::Cancelled));
            assert!(owner.drain_services());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_the_resolution_future_cancels_the_portal_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = MockPortal::new();
            let (_dir, owner) = runtime();
            let (slow, ctx) = portal.resolution("ffmpeg http://stream.example.test/slow/drop.ts");

            let pending = owner.resolve_stream(StreamPurpose::External, slow, Some(ctx));
            portal.wait_for("external create_link", |s| s.slow_asked == 1);

            drop(pending);
            portal.wait_for("dropped future cancelled request", |s| {
                s.slow_abandoned == 1
            });
            assert!(owner.drain_services());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_cancels_pending_external_and_cast_resolutions() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = MockPortal::new();
            let (_dir, owner) = runtime();
            let (ext_res, ext_ctx) =
                portal.resolution("ffmpeg http://stream.example.test/slow/ext.ts");
            let (cast_res, cast_ctx) =
                portal.resolution("ffmpeg http://stream.example.test/slow/cast.ts");

            let ext = owner.resolve_stream(StreamPurpose::External, ext_res, Some(ext_ctx));
            let cast = owner.resolve_stream(StreamPurpose::Cast, cast_res, Some(cast_ctx));
            portal.wait_for("both slow create_links", |s| s.slow_asked == 2);

            assert!(owner.drain_services());
            portal.wait_for("both requests cancelled on shutdown", |s| {
                s.slow_abandoned == 2
            });

            assert_eq!(ext.await, Err(ResolveFailure::Cancelled));
            assert_eq!(cast.await, Err(ResolveFailure::Cancelled));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn interactive_deadline_produces_timed_out_and_cancels_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = MockPortal::new();
            let (_dir, owner) = runtime();
            let (slow, ctx) =
                portal.resolution("ffmpeg http://stream.example.test/slow/timeout.ts");

            let pending = owner.resolve_stream_with_timeout(
                StreamPurpose::Surface(0),
                slow,
                Some(ctx),
                Duration::from_millis(250),
            );
            assert_eq!(pending.await, Err(ResolveFailure::TimedOut));
            portal.wait_for("timed-out request cancelled", |s| s.slow_abandoned == 1);
            assert!(owner.drain_services());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn stale_result_does_not_overwrite_portal_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = MockPortal::new();
            let (_dir, owner) = runtime();
            let (held, shared_ctx) =
                portal.resolution("ffmpeg http://stream.example.test/held/stale.ts");

            // Request 1 authenticates (getting "token-1") and completes on the
            // network runtime before its future is polled again on this thread.
            let first =
                owner.resolve_stream(StreamPurpose::External, held, Some(shared_ctx.clone()));
            portal.wait_for("held create_link after handshake", |s| {
                s.handshakes == 1 && s.slow_asked == 1
            });
            portal.release_held();
            portal.wait_for("held response written", |s| s.held_responded);
            tokio::time::sleep(Duration::from_millis(40)).await;

            // Supersede Request 1 with Request 2 on the same purpose, which completes
            // and writes "token-2" into the shared PlaybackContext.
            let (quick, _) = portal.resolution("ffmpeg http://stream.example.test/quick/fresh.ts");
            let second =
                owner.resolve_stream(StreamPurpose::External, quick, Some(shared_ctx.clone()));
            let resolved = second.await.expect("second resolution succeeds");
            assert_eq!(
                resolved.url,
                "http://stream.example.test/quick-resolved.m3u8"
            );

            // Request 1 already has Ok((Ok(source), token-1)) waiting on its oneshot;
            // finish_stream_resolution must reject it as stale and keep "token-2".
            assert_eq!(first.await, Err(ResolveFailure::Cancelled));

            let page =
                iptv_providers::stalker::parse_channel_page(&iptv_providers::HttpResponse::ok(
                    concat!(
                        r#"{"js":{"total_items":"1","max_page_items":"14","data":["#,
                        r#"{"id":"100","name":"Probe","number":"1","#,
                        r#""cmd":"ffmpeg http://stream.example.test/probe.ts","#,
                        r#""tv_genre_id":"1","use_http_tmp_link":"1","use_load_balancing":"0"}]}}"#,
                    ),
                ))
                .unwrap();
            let Resolution::StalkerLink(next) = iptv_guide::playback::resolve(
                Some(&shared_ctx),
                &page.items.items[0],
                None,
                iptv_guide::playback::Clock {
                    now: 0,
                    local_offset_secs: 0,
                },
            ) else {
                panic!("expected StalkerLink resolution");
            };
            assert_eq!(
                next.session.token().map(|t| t.expose()),
                Some("token-2"),
                "stale result must not overwrite the newer portal session"
            );
            assert!(owner.drain_services());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn executor_refusal_reports_network_failure_instead_of_cancelled() {
    struct DroppingTasks;
    impl iptv_runtime::execution::Tasks for DroppingTasks {
        fn spawn(&self, task: iptv_runtime::execution::Task) {
            drop(task);
        }
        fn spawn_blocking(&self, work: iptv_runtime::execution::Job) {
            drop(work);
        }
        fn block_on(&self, future: iptv_runtime::execution::LocalTask<'_>) {
            drop(future);
        }
        fn delay(&self, _: Duration) -> iptv_runtime::execution::Task {
            Box::pin(std::future::pending())
        }
    }
    struct RefusingExecution;
    impl iptv_runtime::execution::Execution for RefusingExecution {
        fn lane(
            &self,
            _: &'static str,
        ) -> std::io::Result<Box<dyn iptv_runtime::execution::Lane>> {
            Err(std::io::Error::other("unused"))
        }
        fn worker(&self, _: String, _: iptv_runtime::execution::Job) -> std::io::Result<()> {
            Err(std::io::Error::other("unused"))
        }
        fn tasks(&self) -> Box<dyn iptv_runtime::execution::Tasks> {
            Box::new(DroppingTasks)
        }
    }

    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = MockPortal::new();
            let directory = tempfile::tempdir().unwrap();
            let mut ports = Ports::new(directory.path().into(), |_| {
                Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x42; 32]))
            });
            ports.execution = Arc::new(RefusingExecution);
            let owner = Rc::new(Runtime::new(ports));
            let (slow, ctx) = portal.resolution("ffmpeg http://stream.example.test/slow/refuse.ts");

            let result = owner
                .resolve_stream(StreamPurpose::External, slow, Some(ctx))
                .await;
            assert_eq!(result, Err(ResolveFailure::Network));
        })
        .await;
}
