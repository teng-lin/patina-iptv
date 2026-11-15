#![cfg(debug_assertions)]
use super::support::{Resp, TestServer};
use iptv_net::{
    playback::PlaybackPolicy,
    recording::{Failure, capture},
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
    p
}
fn output(name: &str) -> std::path::PathBuf {
    let mut random = [0; 16];
    getrandom::fill(&mut random).unwrap();
    std::env::temp_dir().join(format!("patina-generated-recording-{name}-{random:x?}"))
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
