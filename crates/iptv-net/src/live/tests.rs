use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::oneshot;

struct Fixture {
    url: String,
    requests: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn start(hls: bool) -> Self {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!(
            "http://{}/live.{}",
            listener.local_addr().unwrap(),
            if hls { "m3u8" } else { "ts" }
        );
        let requests = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let count = active.clone();
        let high = peak.clone();
        let task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = clients.join_next(), if !clients.is_empty() => {},
                    connection = listener.accept() => {
                        let (mut socket, _) = connection.unwrap();
                        let seen = seen.clone(); let count = count.clone(); let high = high.clone();
                        clients.spawn(async move {
                            let mut head = Vec::new();
                            while !head.ends_with(b"\r\n\r\n") { head.push(socket.read_u8().await?); }
                            let request = String::from_utf8(head).unwrap();
                            assert!(request.to_ascii_lowercase().contains("x-generated-auth: generated-secret\r\n"));
                            let step = seen.fetch_add(1, Ordering::SeqCst);
                            let current = count.fetch_add(1, Ordering::SeqCst) + 1;
                            high.fetch_max(current, Ordering::SeqCst);
                            struct Active(Arc<AtomicUsize>);
                            impl Drop for Active { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); } }
                            let _active = Active(count);
                            if hls {
                                let body = if request.starts_with("GET /live.m3u8 ") {
                                    format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{}\n#EXTINF:1,\nsegment-{step}.ts\n", step / 2).into_bytes()
                                } else { packet(step as u8).repeat(8) };
                                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
                                socket.write_all(&body).await?;
                            } else {
                                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
                                for n in 0..1000 {
                                    let bytes = packet(n as u8).repeat(8);
                                    socket.write_all(format!("{:x}\r\n", bytes.len()).as_bytes()).await?;
                                    socket.write_all(&bytes).await?;
                                    socket.write_all(b"\r\n").await?;
                                    tokio::time::sleep(Duration::from_millis(20)).await;
                                }
                            }
                            Ok::<(), io::Error>(())
                        });
                    }
                }
            }
        });
        Self {
            url,
            requests,
            active,
            peak,
            task,
        }
    }
}
fn packet(value: u8) -> Vec<u8> {
    let mut bytes = vec![value; 188];
    bytes[0] = 0x47;
    bytes
}
fn policy() -> PlaybackPolicy {
    PlaybackPolicy {
        public_only: false,
        #[cfg(debug_assertions)]
        allow_private_fixture: true,
        ..Default::default()
    }
}
async fn start(fixture: &Fixture) -> LiveInput {
    LiveInput::start(
        &fixture.url,
        &[("X-Generated-Auth".into(), "generated-secret".into())],
        policy(),
    )
    .await
    .unwrap()
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn ts_playback_capture_and_zap_retain_one_upstream_then_release() {
    let fixture = Fixture::start(false).await;
    let playback = start(&fixture).await;
    let capture = playback.clone();
    let client = recording::client().unwrap();
    let mut response = client.get(playback.url()).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response
            .chunk()
            .await
            .unwrap()
            .unwrap()
            .starts_with(&[0x47])
    );
    let mut reader = capture.subscribe();
    assert_eq!(reader.next().await.unwrap()[0], 0x47);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.active.load(Ordering::SeqCst), 1);
    drop(response);
    drop(playback);
    for _ in 0..3 {
        assert_eq!(reader.next().await.unwrap()[0], 0x47);
    }
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    drop(reader);
    drop(capture);
    until(|| fixture.active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn hls_segments_have_one_ingestor_for_both_consumers_and_auth_stays_upstream() {
    let fixture = Fixture::start(true).await;
    let playback = start(&fixture).await;
    let capture = playback.clone();
    let mut one = playback.subscribe();
    let mut two = capture.subscribe();
    for _ in 0..3 {
        assert_eq!(one.next().await.unwrap(), two.next().await.unwrap());
    }
    assert_eq!(
        fixture.requests.load(Ordering::SeqCst),
        8,
        "one manifest and one segment per sequence"
    );
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    drop(playback);
    drop(capture);
    drop(one);
    drop(two);
    until(|| fixture.active.load(Ordering::SeqCst) == 0).await;
}
#[tokio::test]
async fn stopping_capture_does_not_change_playback_or_reopen_upstream() {
    let fixture = Fixture::start(false).await;
    let playback = start(&fixture).await;
    let mut reader = playback.subscribe();
    let path = std::env::temp_dir().join(format!(
        "patina-generated-shared-{}.ts",
        playback.url().split('/').nth(3).unwrap()
    ));
    let input = playback.clone();
    let output = path.clone();
    let capture =
        tokio::spawn(async move { recording::capture_shared(&input, &output, 1024 * 1024).await });
    until(|| std::fs::metadata(&path).is_ok_and(|m| m.len() >= 188)).await;
    capture.abort();
    let _ = capture.await;
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len() % 188, 0);
    assert!(
        bytes
            .as_chunks::<188>()
            .0
            .iter()
            .all(|packet| packet[0] == 0x47)
    );
    for _ in 0..3 {
        reader.next().await.unwrap();
    }
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert!(playback.is_running());
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
async fn bounded_lag_is_explicit_and_does_not_block_other_readers() {
    let shared = Arc::new(Shared {
        buffer: Mutex::default(),
        changed: Notify::new(),
    });
    let mut governor = LiveReader::new(shared.clone());
    let mut lagged = LiveReader::new(shared.clone());
    let mut sink = Fanout {
        shared: shared.clone(),
        ready: None,
        validated: None,
        pending: vec![],
        first: Arc::new(AtomicBool::new(false)),
    };
    for _ in 0..(BUFFER / CHUNK + 2) {
        sink.write(&packet(1).repeat(CHUNK / 188)).await.unwrap();
        governor.next().await.unwrap();
    }
    assert_eq!(lagged.next().await, Err(Failure::Gap));
    let mut recent = LiveReader::new(shared.clone());
    recent.cursor = shared.lock().chunks.front().unwrap().0;
    assert_eq!(recent.next().await.unwrap().len(), CHUNK);
    assert!(shared.lock().bytes <= BUFFER);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_first_capture_does_not_block_playback_and_reports_a_gap() {
    let shared = Arc::new(Shared {
        buffer: Mutex::default(),
        changed: Notify::new(),
    });
    let mut stalled_capture = LiveReader::new(shared.clone());
    let mut playback = LiveReader::new(shared.clone());
    let mut sink = Fanout {
        shared: shared.clone(),
        ready: None,
        validated: None,
        pending: vec![],
        first: Arc::new(AtomicBool::new(false)),
    };
    for sequence in 0..(BUFFER / CHUNK + 2) {
        let chunk = packet(sequence as u8).repeat(CHUNK / 188);
        tokio::time::timeout(Duration::from_secs(1), sink.write(&chunk))
            .await
            .expect("advancing playback must keep ingestion moving")
            .unwrap();
        assert_eq!(playback.next().await.unwrap().as_ref(), chunk.as_slice());
        assert!(shared.lock().bytes <= BUFFER);
    }
    assert_eq!(stalled_capture.next().await, Err(Failure::Gap));
}

#[tokio::test]
async fn all_stalled_consumers_bound_read_ahead_until_one_advances() {
    let shared = Arc::new(Shared {
        buffer: Mutex::default(),
        changed: Notify::new(),
    });
    let _stalled_capture = LiveReader::new(shared.clone());
    let mut playback = LiveReader::new(shared.clone());
    let mut sink = Fanout {
        shared: shared.clone(),
        ready: None,
        validated: None,
        pending: vec![],
        first: Arc::new(AtomicBool::new(false)),
    };
    let chunk = packet(1).repeat(CHUNK / 188);
    for _ in 0..(BUFFER / 2).div_ceil(CHUNK) {
        sink.write(&chunk).await.unwrap();
    }
    let boundary = shared.lock().next;
    let mut next = Box::pin(sink.write(&chunk));
    for _ in 0..2 {
        let state = std::future::poll_fn(|cx| std::task::Poll::Ready(next.as_mut().poll(cx))).await;
        assert!(
            state.is_pending(),
            "all stalled readers must bound ingestion"
        );
        let buffer = shared.lock();
        assert_eq!(buffer.next, boundary);
        assert!(buffer.bytes <= BUFFER / 2 + CHUNK);
    }
    assert_eq!(playback.next().await.unwrap().as_ref(), chunk.as_slice());
    let state = std::future::poll_fn(|cx| std::task::Poll::Ready(next.as_mut().poll(cx))).await;
    assert!(matches!(state, std::task::Poll::Ready(Ok(()))));
    assert_eq!(shared.lock().next, boundary + 1);
}

#[tokio::test]
async fn producer_waits_for_a_consumer_and_capture_continues_while_playback_is_paused() {
    let shared = Arc::new(Shared {
        buffer: Mutex::default(),
        changed: Notify::new(),
    });
    let mut sink = Fanout {
        shared: shared.clone(),
        ready: None,
        validated: None,
        pending: vec![],
        first: Arc::new(AtomicBool::new(false)),
    };
    let producer =
        tokio::spawn(async move { sink.write(&packet(1).repeat(BUFFER / 188 * 3)).await });
    until(|| shared.lock().next > 0).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        shared.lock().next,
        1,
        "preparation only retains the first chunk before a consumer arrives"
    );
    let playback = LiveReader::new(shared.clone());
    let mut capture = LiveReader::new(shared.clone());
    // An advancing capture controls progress while the decoder is paused.
    // Playback can exceed the rewind window without stopping the recording.
    while !producer.is_finished() {
        let _ = tokio::time::timeout(Duration::from_millis(40), capture.next()).await;
    }
    assert!(shared.lock().bytes <= BUFFER);
    let mut playback = playback;
    assert_eq!(playback.next().await, Err(Failure::Gap));
    producer.await.unwrap().unwrap();
}

#[tokio::test]
async fn late_capture_gets_the_bounded_current_pat_interval_in_order() {
    let shared = Arc::new(Shared {
        buffer: Mutex::default(),
        changed: Notify::new(),
    });
    let mut pat = packet(0xff);
    pat[..4].copy_from_slice(&[0x47, 0x40, 0, 0x10]);
    pat[4] = 0;
    pat[5..17].copy_from_slice(&[0, 0xb0, 13, 0, 1, 0xc1, 0, 0, 0, 1, 0xe1, 0]);
    let mut pmt = packet(0xff);
    pmt[..4].copy_from_slice(&[0x47, 0x41, 0, 0x10]);
    let media = packet(7);
    {
        let mut buffer = shared.lock();
        retain_preroll(
            &mut buffer,
            &[pat.clone(), pmt.clone(), media.clone()].concat(),
        );
        buffer.chunks.push_back((0, Arc::from(media.clone())));
        buffer.next = 1;
    }
    let mut capture = LiveReader::new(shared.clone());
    assert_eq!(&*capture.next().await.unwrap(), [pat, pmt, media].concat());
    assert_eq!(capture.cursor, 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), capture.next())
            .await
            .is_err()
    );
    shared.lock().ended = Some(Failure::Network);
    shared.changed.notify_waiters();
    assert_eq!(capture.next().await, Err(Failure::Network));
}

#[tokio::test]
async fn unsupported_probe_joins_upstream_retirement_before_fallback() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/generated.m3u8", listener.local_addr().unwrap());
    let (closed, closure) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        let body = b"#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nsegment.ts\n";
        socket
            .write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(body).await.unwrap();
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
        let _ = closed.send(());
    });
    assert!(matches!(
        LiveInput::start(&url, &[], policy()).await,
        Err(Failure::Unsupported)
    ));
    tokio::time::timeout(Duration::from_secs(1), closure)
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn natural_preroll_replaces_old_pat_versions_and_preserves_continuations() {
    let mut buffer = Buffer::default();
    let mut pat = packet(0);
    pat[..4].copy_from_slice(&[0x47, 0x40, 0, 0x10]);
    let mut continuation = packet(1);
    continuation[..4].copy_from_slice(&[0x47, 0, 0, 0x11]);
    for version in 0..40 {
        pat[10] = version;
        retain_preroll(&mut buffer, &[pat.clone(), continuation.clone()].concat());
        assert_eq!(
            buffer.pre_roll,
            [pat.clone(), continuation.clone()].concat()
        );
    }
    let mut shared = Shared {
        buffer: Mutex::new(buffer),
        changed: Notify::new(),
    };
    shared.buffer.get_mut().unwrap().pre_roll_at =
        Some(tokio::time::Instant::now() - Duration::from_secs(3));
    let reader = LiveReader::new(Arc::new(shared));
    assert!(
        reader.prelude.is_none(),
        "a stale PAT interval is never prepended to a late recording"
    );
}

#[tokio::test]
async fn capture_reader_itself_retains_incoming_owner_after_live_input_is_dropped() {
    let fixture = Fixture::start(false).await;
    let input = start(&fixture).await;
    let mut reader = input.subscribe();
    drop(input);
    for _ in 0..3 {
        reader.next().await.unwrap();
    }
    assert_eq!(fixture.active.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    drop(reader);
    until(|| fixture.active.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test]
async fn cancelling_pending_preparation_waits_for_its_upstream_to_close() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/generated.ts", listener.local_addr().unwrap());
    let (entered, request_seen) = oneshot::channel();
    let (closed, socket_closed) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let _ = entered.send(());
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
        let _ = closed.send(());
    });
    let cancel = CancelToken::new();
    let signal = cancel.clone();
    let preparing =
        tokio::spawn(
            async move { LiveInput::start_with_cancel(&url, &[], policy(), signal).await },
        );
    request_seen.await.unwrap();
    cancel.cancel();
    assert!(matches!(preparing.await.unwrap(), Err(Failure::Network)));
    tokio::time::timeout(Duration::from_secs(1), socket_closed)
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_transient_accept_error_backs_off_and_keeps_the_capability_serving() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let listener = &listener;
    let attempts = AtomicUsize::new(0);
    let accept = || {
        let first = attempts.fetch_add(1, Ordering::SeqCst) == 0;
        async move {
            if first {
                Err(io::Error::other("generated transient accept failure"))
            } else {
                listener.accept().await
            }
        }
    };
    let shared = Arc::new(Shared {
        buffer: Mutex::default(),
        changed: Notify::new(),
    });
    let serving = serve_connections(accept, "/generated/live.ts".into(), Arc::downgrade(&shared));
    tokio::pin!(serving);
    for _ in 0..2 {
        let state =
            std::future::poll_fn(|cx| std::task::Poll::Ready(serving.as_mut().poll(cx))).await;
        assert!(
            state.is_pending(),
            "an accept error must not retire the server"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "retry must back off");
    }
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::time::resume();
    let request = async {
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"HEAD /generated/live.ts HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    };
    tokio::select! {
        _ = &mut serving => panic!("the same capability must remain available"),
        result = tokio::time::timeout(Duration::from_secs(5), request) => result.unwrap(),
    }
    assert!(attempts.load(Ordering::SeqCst) >= 2);
    assert!(shared.lock().ended.is_none(), "capture remains live");
}

/// A segment's first packets: a PAT and one more.
fn first_packets() -> Vec<u8> {
    let mut pat = packet(0);
    pat[..4].copy_from_slice(&[0x47, 0x40, 0, 0x10]);
    [pat, packet(1)].concat()
}

/// How a generated live HLS provider answers one segment request.
struct Segment {
    /// The declared `Content-Length`; a shorter body drops the connection.
    length: usize,
    /// Sent at once.
    first: Vec<u8>,
    /// Waited for before `rest`.
    hold: Option<Arc<Notify>>,
    rest: Vec<u8>,
    /// Set once the whole body was written.
    sent: Option<Arc<AtomicBool>>,
}

fn whole(first: Vec<u8>, rest: Vec<u8>) -> Segment {
    Segment {
        length: first.len() + rest.len(),
        first,
        hold: None,
        rest,
        sent: None,
    }
}

/// A live HLS provider whose media playlist names one segment; `segment`
/// answers the n-th segment request (from 0). Abort the handle to stop it.
async fn hls_provider(
    segment: impl Fn(usize) -> Segment + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/live.m3u8", listener.local_addr().unwrap());
    let segment = Arc::new(segment);
    let requests = Arc::new(AtomicUsize::new(0));
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (segment, requests) = (segment.clone(), requests.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await?);
                }
                if request.starts_with(b"GET /live.m3u8 ") {
                    let body = b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n\
                                 #EXTINF:1,\nsegment.ts\n";
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                    socket.write_all(head.as_bytes()).await?;
                    socket.write_all(body).await?;
                    return Ok(());
                }
                let answer = segment(requests.fetch_add(1, Ordering::SeqCst));
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    answer.length
                );
                socket.write_all(head.as_bytes()).await?;
                socket.write_all(&answer.first).await?;
                socket.flush().await?;
                if let Some(hold) = answer.hold {
                    hold.notified().await;
                }
                socket.write_all(&answer.rest).await?;
                socket.flush().await?;
                if let Some(sent) = answer.sent {
                    sent.store(true, Ordering::SeqCst);
                }
                Ok::<(), io::Error>(())
            });
        }
    });
    (url, server)
}

fn begin(url: &str) -> LiveInput {
    LiveInput::begin_with_cancel(url, &[], policy(), CancelToken::new()).unwrap()
}

#[tokio::test]
async fn an_hls_input_is_ready_with_its_first_packets_before_the_segment_ends() {
    let release = Arc::new(Notify::new());
    let held = release.clone();
    // A provider's live-edge segment slow to arrive: its first packets now,
    // the rest only once released.
    let (url, server) = hls_provider(move |_| Segment {
        length: 8 * 188,
        first: first_packets(),
        hold: Some(held.clone()),
        rest: packet(2).repeat(6),
        sent: None,
    })
    .await;
    let input = begin(&url);
    tokio::time::timeout(Duration::from_secs(2), input.ready())
        .await
        .expect("ready before the segment's last packets arrive")
        .unwrap();
    let validated = input.first_segment();
    // The player joins at the latest PAT, the segment's first packet.
    let mut reader = input.subscribe();
    assert_eq!(&reader.next().await.unwrap()[..], &first_packets()[..]);
    release.notify_waiters();
    assert_eq!(&reader.next().await.unwrap()[..], &packet(2).repeat(6)[..]);
    assert_eq!(validated.await, Ok(()), "the whole segment was valid");
    drop(reader);
    drop(input);
    server.abort();
}

/// First segments that start as TS and fail validation only later: the
/// input offers its first packets, then reports why it cannot be shared
/// (the runtime then plays the provider's own transport).
#[tokio::test]
async fn a_first_segment_failing_after_its_first_packets_is_reported_unshareable() {
    let mut broken = packet(3).repeat(4);
    broken[2 * 188] = 0x00;
    let cases = [
        (
            "misaligned",
            [packet(2), vec![0x47; 94]].concat(),
            Failure::Unsupported,
        ),
        ("sync loss partway", broken, Failure::Unsupported),
        (
            "over the limit",
            packet(4).repeat(32 * 1024 * 1024 / 188),
            Failure::Limit,
        ),
    ];
    for (case, rest, failure) in cases {
        let rest = Arc::new(rest);
        let release = Arc::new(Notify::new());
        let held = release.clone();
        let (url, server) = hls_provider(move |n| Segment {
            // The first input's rest arrives after its first packets.
            hold: (n == 0).then(|| held.clone()),
            ..whole(first_packets(), rest.to_vec())
        })
        .await;
        let input = begin(&url);
        assert_eq!(input.ready().await, Ok(()), "{case}: first packets");
        release.notify_one();
        let mut reader = input.subscribe();
        assert_eq!(input.first_segment().await, Err(failure), "{case}");
        while let Ok(bytes) = reader.next().await {
            assert!(bytes.len() % 188 == 0 && bytes[0] == 0x47, "{case}");
        }
        assert!(!input.is_running(), "{case}: the input ended");
        drop(reader);
        drop(input);
        assert_eq!(
            LiveInput::start(&url, &[], policy()).await.err(),
            Some(failure),
            "{case}: a start that waits for its first segment refuses it"
        );
        server.abort();
    }
}

/// The connection drops in the middle of a segment: the player sees whole
/// packets and then an end, never a torn packet; the retry's fresh input
/// starts cleanly at the next segment's PAT.
#[tokio::test]
async fn a_segment_dropped_partway_ends_the_input_and_a_retry_starts_clean() {
    let drop_now = Arc::new(Notify::new());
    let dropped = drop_now.clone();
    let (url, server) = hls_provider(move |n| {
        if n == 0 {
            // Half a packet after the first ones, then the connection
            // closes short of its declared length.
            Segment {
                length: 8 * 188,
                first: [first_packets(), packet(5)[..94].to_vec()].concat(),
                hold: Some(dropped.clone()),
                rest: Vec::new(),
                sent: None,
            }
        } else {
            whole(first_packets(), packet(6).repeat(6))
        }
    })
    .await;
    let input = begin(&url);
    input.ready().await.unwrap();
    let mut reader = input.subscribe();
    drop_now.notify_one();
    let mut received = Vec::new();
    let end = loop {
        match reader.next().await {
            Ok(bytes) => received.extend_from_slice(&bytes),
            Err(failure) => break failure,
        }
    };
    assert_eq!(end, Failure::Network);
    assert_eq!(
        received,
        first_packets(),
        "only whole packets reach the player"
    );
    assert_eq!(input.first_segment().await, Err(Failure::Network));
    drop(reader);
    drop(input);
    let retry = begin(&url);
    retry.ready().await.unwrap();
    let mut reader = retry.subscribe();
    let mut fresh = Vec::new();
    while fresh.len() < 8 * 188 {
        fresh.extend_from_slice(&reader.next().await.unwrap());
    }
    assert_eq!(fresh, [first_packets(), packet(6).repeat(6)].concat());
    drop(reader);
    drop(retry);
    server.abort();
}

/// A paused player stops reading past the fanout's window: the segment's
/// body is still read to its end (bounded by the segment limit), so the
/// provider's response is not left open until the client's 30 s read
/// timeout ends the input; playback then resumes where it paused.
#[tokio::test]
async fn a_paused_player_does_not_leave_the_segment_body_open() {
    // More than the loopback socket buffers hold for a body nobody reads.
    let packets = 24 * 1024 * 1024 / 188;
    let sent = Arc::new(AtomicBool::new(false));
    let done = sent.clone();
    let (url, server) = hls_provider(move |_| Segment {
        sent: Some(done.clone()),
        ..whole(first_packets(), packet(7).repeat(packets))
    })
    .await;
    let input = begin(&url);
    input.ready().await.unwrap();
    // The player reads its first packets, then pauses.
    let mut reader = input.subscribe();
    let first = reader.next().await.unwrap().len();
    until(|| sent.load(Ordering::SeqCst)).await;
    assert!(input.is_running(), "the pause ended the input");
    // Playback resumes: the rest of the segment follows in order.
    let validated = input.first_segment();
    let playing = tokio::spawn(async move {
        let mut received = first;
        while received < (packets + 2) * 188 {
            let bytes = reader.next().await.unwrap();
            assert!(bytes[0] == 0x47 && bytes.len() % 188 == 0);
            received += bytes.len();
        }
    });
    assert_eq!(validated.await, Ok(()));
    tokio::time::timeout(Duration::from_secs(5), playing)
        .await
        .unwrap()
        .unwrap();
    drop(input);
    server.abort();
}

/// An input whose task went away before its first segment was decided (its
/// runtime shut down) reports so, instead of pending for ever.
#[test]
fn a_validation_without_its_input_task_is_abandoned() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (validation, input) = runtime.block_on(async {
        let (url, _server) = hls_provider(|_| Segment {
            hold: Some(Arc::new(Notify::new())),
            length: 8 * 188,
            ..whole(first_packets(), Vec::new())
        })
        .await;
        let input = begin(&url);
        input.ready().await.unwrap();
        (input.validation(), input)
    });
    assert_eq!(validation.state(), Validated::Pending);
    drop(runtime);
    assert_eq!(validation.state(), Validated::Abandoned);
    drop(input);
}

#[tokio::test]
async fn a_validation_is_settled_with_when_it_was_decided() {
    let (url, server) = hls_provider(|_| whole(first_packets(), packet(2).repeat(6))).await;
    let before = std::time::Instant::now();
    let input = begin(&url);
    input.ready().await.unwrap();
    // A player takes the packets (the fanout waits for one).
    let mut reader = input.subscribe();
    let playing = tokio::spawn(async move { while reader.next().await.is_ok() {} });
    assert_eq!(input.first_segment().await, Ok(()));
    match input.validation().state() {
        Validated::Settled(Ok(()), at) => {
            assert!(at >= before && at <= std::time::Instant::now());
        }
        state => panic!("{state:?}"),
    }
    drop(input);
    playing.abort();
    server.abort();
}

/// A start that waits for its first segment takes the packets itself: a
/// segment arriving in parts, larger than one fanout chunk, still validates
/// with no player attached yet.
#[tokio::test]
async fn a_start_validates_a_segment_arriving_in_parts_without_a_player() {
    let release = Arc::new(Notify::new());
    let held = release.clone();
    let (url, server) = hls_provider(move |_| Segment {
        hold: Some(held.clone()),
        ..whole(first_packets(), packet(2).repeat(2 * CHUNK / 188))
    })
    .await;
    let later = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.notify_one();
    });
    let input = tokio::time::timeout(
        Duration::from_secs(5),
        LiveInput::start(&url, &[], policy()),
    )
    .await
    .expect("validated without a player")
    .unwrap();
    assert!(input.is_running());
    later.await.unwrap();
    drop(input);
    server.abort();
}

/// A provider playlist that ends (`#EXT-X-ENDLIST`, as the 300-second
/// fixtures do) after one segment. Abort the handle to stop it.
async fn finite_hls_provider() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/vod.m3u8", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await?);
                }
                let body = if request.starts_with(b"GET /vod.m3u8 ") {
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n\
                      #EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n"
                        .to_vec()
                } else {
                    [first_packets(), packet(2).repeat(6)].concat()
                };
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                socket.write_all(head.as_bytes()).await?;
                socket.write_all(&body).await?;
                Ok::<(), io::Error>(())
            });
        }
    });
    (url, server)
}

/// #137: once its playlist reached its end, an input has no media for a
/// new connection (mpv's live spare, a reconnect). It refuses it with a 503
/// a player retries, never an empty 200 the player reads as a stream in an
/// unsupported format.
#[tokio::test]
async fn an_ended_input_refuses_new_connections_rather_than_sending_nothing() {
    let (url, server) = finite_hls_provider().await;
    let input = begin(&url);
    input.ready().await.unwrap();
    until(|| !input.is_running()).await;
    let client = recording::client().unwrap();
    for method in [reqwest::Method::GET, reqwest::Method::HEAD] {
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            client.request(method.clone(), input.url()).send(),
        )
        .await
        .expect("answered at once")
        .unwrap();
        assert_eq!(response.status(), 503, "{method}");
        assert!(response.bytes().await.unwrap().is_empty(), "{method}");
    }
    drop(input);
    server.abort();
}
