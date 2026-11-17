#![cfg(debug_assertions)]
use super::support::{Resp, TestServer};
use iptv_net::{
    playback::PlaybackPolicy,
    recording::{CaptureState, Failure, capture, capture_resume, capture_resume_with_state},
    recording_file::RecordingFile,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn fixture_policy() -> PlaybackPolicy {
    PlaybackPolicy {
        allow_private_fixture: true,
        ..Default::default()
    }
}
fn packet(value: u8) -> Vec<u8> {
    let mut p = vec![value; 188];
    p[0] = 0x47;
    p[1] = 0x1f;
    p[2] = 0xff;
    p[3] = 0x10;
    p
}
fn output(name: &str) -> std::path::PathBuf {
    let mut random = [0; 16];
    getrandom::fill(&mut random).unwrap();
    std::env::temp_dir().join(format!("patina-generated-recording-{name}-{random:x?}"))
}

#[tokio::test]
async fn resumed_capture_truncates_a_torn_packet_and_appends_without_overwriting() {
    let server = TestServer::http(|_| Resp::ok(packet(7))).await;
    let file = output("resume-torn-packet");
    let mut original = packet(6);
    original.extend_from_slice(&[0x47, 1, 2]);
    std::fs::write(&file, original).unwrap();
    assert_eq!(
        capture_resume(
            &server.url("/generated.ts"),
            &[],
            fixture_policy(),
            &file,
            1024
        )
        .await,
        Err(Failure::Network)
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        [packet(6), packet(7)].concat()
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn hls_retry_keeps_written_sequences_and_captures_intervening_available_segments() {
    for advanced_window in [false, true] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let server = TestServer::http(move |r| {
            if r.path == "/live.m3u8" {
                let request = seen.fetch_add(1, Ordering::SeqCst);
                if request == 1 {
                    return Resp::status(503);
                }
                let last = if request >= 3 || (advanced_window && request == 2) {
                    14
                } else {
                    12
                };
                let mut manifest =
                    String::from("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:10\n");
                for segment in 10..=last {
                    manifest.push_str(&format!("#EXTINF:1,\nsegment-{segment}.ts\n"));
                }
                if request >= 3 {
                    manifest.push_str("#EXT-X-ENDLIST\n");
                }
                Resp::ok(manifest)
            } else {
                let sequence = r
                    .path
                    .strip_prefix("/segment-")
                    .unwrap()
                    .strip_suffix(".ts")
                    .unwrap()
                    .parse::<u8>()
                    .unwrap();
                Resp::ok(packet(sequence))
            }
        })
        .await;
        let file = output("retry-cursor");
        let mut state = CaptureState::default();
        for expected in [packet(12), [packet(12), packet(13), packet(14)].concat()] {
            let outcome = tokio::time::timeout(
                Duration::from_secs(8),
                capture_resume_with_state(
                    &server.url("/live.m3u8"),
                    &[],
                    fixture_policy(),
                    &file,
                    4096,
                    &mut state,
                ),
            )
            .await
            .unwrap();
            assert_eq!(outcome, Err(Failure::Network));
            assert_eq!(
                std::fs::read(&file).unwrap(),
                expected,
                "requests: {:?}",
                server
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|request| &request.path)
                    .collect::<Vec<_>>()
            );
        }
        let requests = server.requests.lock().unwrap();
        for sequence in 12..=14 {
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request.path == format!("/segment-{sequence}.ts"))
                    .count(),
                1,
                "every successfully written segment is fetched once across reload failure"
            );
        }
        assert!(!requests.iter().any(|request| request.path == "/segment-10.ts" || request.path == "/segment-11.ts"));
        drop(requests);
        std::fs::remove_file(file).unwrap();
    }
}

#[tokio::test]
async fn a_reported_hls_gap_recovers_the_available_window_on_the_next_retry() {
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();
    let server = TestServer::http(move |request| {
        if request.path == "/live.m3u8" {
            let step = counter.fetch_add(1, Ordering::SeqCst);
            let first = match step {
                0 => 0,
                1 => 5,
                _ => 8,
            };
            let mut manifest =
                format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{first}\n");
            for sequence in first..first + 3 {
                manifest.push_str(&format!("#EXTINF:1,\nsegment-{sequence}.ts\n"));
            }
            if step >= 3 {
                manifest.push_str("#EXT-X-ENDLIST\n");
            }
            Resp::ok(manifest)
        } else {
            let sequence = request
                .path
                .strip_prefix("/segment-")
                .unwrap()
                .strip_suffix(".ts")
                .unwrap()
                .parse::<u8>()
                .unwrap();
            Resp::ok(packet(sequence))
        }
    })
    .await;
    let file = output("gap-cursor");
    let mut state = CaptureState::default();
    for failure in [Failure::Gap, Failure::Network] {
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(8),
                capture_resume_with_state(
                    &server.url("/live.m3u8"),
                    &[],
                    fixture_policy(),
                    &file,
                    4096,
                    &mut state,
                )
            )
            .await
            .unwrap(),
            Err(failure)
        );
    }
    assert_eq!(
        std::fs::read(&file).unwrap(),
        [packet(2), packet(8), packet(9), packet(10)].concat()
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn hls_records_each_sequence_once_and_detects_missing_segments() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let server = TestServer::http(move |r| match r.path.as_str() {
        "/live.m3u8" => {
            let sequence = match seen.fetch_add(1, Ordering::SeqCst) { 0 | 1 => 3, _ => 5 };
            Resp::ok(format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n#EXTINF:1,\nsegment.ts\n"))
        },
        _ => Resp::ok(packet(7)),
    }).await;
    let file = output("sequences");
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        capture(
            &server.url("/live.m3u8"),
            &[],
            fixture_policy(),
            &file,
            1024,
        ),
    )
    .await
    .unwrap();
    assert_eq!(result, Err(Failure::Gap));
    assert_eq!(std::fs::read(&file).unwrap(), packet(7));
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/segment.ts")
            .count(),
        1
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn caption_metadata_passes_the_relay_without_fetching_external_renditions() {
    let server = TestServer::http(|r| match r.path.as_str() {
        "/master.m3u8" => Resp::ok(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=CLOSED-CAPTIONS,GROUP-ID=\"cc\",NAME=\"English, SDH\",INSTREAM-ID=\"CC1\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=800000,CLOSED-CAPTIONS=\"cc\"\n\
             media.m3u8\n",
        ),
        "/media.m3u8" => Resp::ok(
            "#EXTM3U\n#EXT-X-TARGETDURATION:60\n#EXTINF:60,\nsegment.ts\n#EXT-X-ENDLIST\n",
        ),
        "/external.m3u8" => Resp::ok(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Generated\",URI=\"audio.m3u8\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"audio\"\n\
             media.m3u8\n",
        ),
        _ => Resp::ok(packet(8)),
    })
    .await;
    for (name, expected) in [("/master.m3u8", packet(8)), ("/external.m3u8", vec![])] {
        let file = output("captions");
        let result = capture(&server.url(name), &[], fixture_policy(), &file, 1024).await;
        // Finite input ends honestly; accepting its in-band captions must still
        // retain all TS bytes, whereas unsupported renditions retain none.
        assert_eq!(
            result,
            Err(if expected.is_empty() {
                Failure::Unsupported
            } else {
                Failure::Network
            })
        );
        assert_eq!(std::fs::read(&file).unwrap(), expected);
        std::fs::remove_file(file).unwrap();
    }
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path == "/audio.m3u8")
    );
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/segment.ts")
            .count(),
        1
    );
}

#[tokio::test]
async fn unsupported_media_and_disk_budget_cannot_be_reported_as_success() {
    let server = TestServer::http(|r| match r.path.as_str() {
        "/encrypted.m3u8" => {
            Resp::ok("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:1,\nsegment.ts\n")
        }
        "/bad.ts" => Resp::ok("<html>Generated error page</html>"),
        _ => Resp::ok(packet(4)),
    })
    .await;
    for (url, limit, error) in [
        ("/encrypted.m3u8", 1024, Failure::Unsupported),
        ("/bad.ts", 1024, Failure::Network),
        ("/direct.ts", 100, Failure::Limit),
    ] {
        let file = output("limits");
        assert_eq!(
            capture(&server.url(url), &[], fixture_policy(), &file, limit).await,
            Err(error)
        );
        assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);
        std::fs::remove_file(file).unwrap();
    }
}

#[tokio::test]
async fn recorded_media_uses_capability_and_ranges_without_disclosing_a_file_path() {
    let file = output("playback");
    std::fs::write(&file, packet(9)).unwrap();
    let relay = RecordingFile::start(file.clone()).unwrap();
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
        )
        .no_proxy()
        .build()
        .unwrap();
    let response = client
        .get(&relay.url)
        .header("Range", "bytes=10-19")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.headers()["content-range"], "bytes 10-19/188");
    assert_eq!(&response.bytes().await.unwrap()[..], &[9; 10]);
    let bad = format!(
        "{}/wrong/recording.ts",
        relay.url.rsplit_once('/').unwrap().0
    );
    assert_eq!(client.get(bad).send().await.unwrap().status(), 404);
    drop(relay);
    std::fs::remove_file(file).unwrap();
}

async fn recording_socket(
    url: &str,
    method: &str,
    range: Option<&str>,
) -> std::io::Result<tokio::net::TcpStream> {
    use tokio::io::AsyncWriteExt;
    let url = url::Url::parse(url).unwrap();
    let mut socket = tokio::net::TcpStream::connect((
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap(),
    ))
    .await?;
    let range = range.map_or(String::new(), |range| format!("Range: {range}\r\n"));
    socket
        .write_all(
            format!(
                "{method} {} HTTP/1.1\r\nHost: localhost\r\n{range}Connection: close\r\n\r\n",
                url.path()
            )
            .as_bytes(),
        )
        .await?;
    Ok(socket)
}

async fn recording_response(
    url: &str,
    method: &str,
    range: Option<&str>,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut socket = recording_socket(url, method, range).await?;
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut response)).await??;
    Ok(response)
}

async fn recording_head(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
            assert!(head.len() < 8192);
        }
        head
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn recording_head_invalid_ranges_and_unknown_capabilities_have_bounded_responses() {
    let path = output("head-and-errors");
    std::fs::write(&path, packet(8)).unwrap();
    let relay = RecordingFile::start(path.clone()).unwrap();
    for (url, method, range, status) in [
        (relay.url.clone(), "HEAD", None, "200 OK"),
        (
            relay.url.clone(),
            "HEAD",
            Some("bytes=10-19"),
            "206 Partial Content",
        ),
        (
            relay.url.clone(),
            "GET",
            Some("bytes=188-"),
            "416 Range Not Satisfiable",
        ),
        (format!("{}/wrong", relay.url), "GET", None, "404 Not Found"),
    ] {
        let response = recording_response(&url, method, range).await.unwrap();
        assert!(response.starts_with(format!("HTTP/1.1 {status}\r\n").as_bytes()));
        let body = response
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert_eq!(response.len(), body, "HEAD and empty errors have no body");
    }
    drop(relay);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn four_stalled_recording_clients_expire_and_a_paused_player_can_reopen_a_range() {
    let path = output("stalled-clients");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    let relay = RecordingFile::start(path.clone()).unwrap();
    let mut stalled = Vec::new();
    for _ in 0..4 {
        let mut socket = recording_socket(&relay.url, "GET", None).await.unwrap();
        assert!(
            recording_head(&mut socket)
                .await
                .starts_with(b"HTTP/1.1 200 OK\r\n")
        );
        stalled.push(socket);
    }
    // Let the local send buffers fill before advancing the idle clock. The
    // sockets remain owned and unread throughout the later successful request.
    tokio::time::sleep(Duration::from_millis(200)).await;
    match recording_response(&relay.url, "GET", Some("bytes=10-19")).await {
        Ok(bytes) => assert!(
            bytes.is_empty(),
            "all four serving slots should be occupied"
        ),
        Err(error) => assert!(matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
        )),
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::resume();
    let response = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(response) = recording_response(&relay.url, "GET", Some("bytes=10-19")).await
                && response.starts_with(b"HTTP/1.1 206 Partial Content\r\n")
            {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expired clients must release a serving slot");
    assert!(response.ends_with(&[0; 10]));
    assert!(
        String::from_utf8_lossy(&response).contains("Content-Range: bytes 10-19/134217728\r\n")
    );
    drop(stalled);
    drop(relay);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn dropping_recording_transport_cancels_stalled_body_tasks_and_listener() {
    use tokio::io::AsyncReadExt;
    let path = output("cancel-stalled-clients");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    let relay = RecordingFile::start(path.clone()).unwrap();
    let url = url::Url::parse(&relay.url).unwrap();
    let address = (url.host_str().unwrap().to_owned(), url.port().unwrap());
    let mut socket = recording_socket(&relay.url, "GET", None).await.unwrap();
    recording_head(&mut socket).await;
    drop(relay);
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut buffer = [0_u8; 8192];
        loop {
            match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await
    .expect("owner teardown must abort the active response");
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
    std::fs::remove_file(path).unwrap();
}
