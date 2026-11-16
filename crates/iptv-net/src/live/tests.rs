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
