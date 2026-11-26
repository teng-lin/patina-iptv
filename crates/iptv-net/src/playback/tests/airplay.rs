//! Synthetic HTTP checks for the native sender's LAN capability boundary.
use super::*;

#[test]
fn airplay_requires_a_concrete_private_lan_interface() {
    for listen in ["0.0.0.0:0", "127.0.0.1:0", "203.0.113.10:0", "[::1]:0"] {
        assert!(
            PlaybackRelay::start_airplay(
                "https://media.example.test/live.m3u8",
                &[],
                PlaybackPolicy::default(),
                listen.parse().unwrap()
            )
            .is_err()
        );
    }
}

fn fixture(url: &str) -> (PlaybackRelay, ExternalAdmission) {
    // Loopback substitutes for a private LAN interface in this hermetic test.
    // The production entry point refuses it; all HTTP routing and gates are real.
    PlaybackRelay::start_gated(
        url,
        &[("Authorization".into(), "Bearer generated-secret".into())],
        PlaybackPolicy {
            public_only: false,
            ..Default::default()
        },
        Some(ExternalNetwork {
            listen: "127.0.0.1:0".parse().unwrap(),
            peer: None,
        }),
    )
    .unwrap()
}

#[tokio::test]
async fn airplay_priming_cannot_open_the_provider_until_admitted_and_never_reveals_its_identity() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (relay, mut gate) = fixture(&format!(
        "http://{}/provider-secret/live?token=generated-secret",
        upstream.local_addr().unwrap()
    ));
    let url = relay.url().to_owned();
    assert!(url.ends_with("/stream.m3u8"));
    let client = tokio::spawn(async move {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url)
            .header("Range", "bytes=0-255")
            .send()
            .await
            .unwrap()
    });
    assert!(
        tokio::time::timeout(Duration::from_secs(5), gate.connected())
            .await
            .unwrap()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(80), upstream.accept())
            .await
            .is_err()
    );
    assert!(gate.allow());
    let (mut socket, request) = upstream_request(&upstream).await;
    assert!(request.starts_with("GET /provider-secret/live?token=generated-secret HTTP/"));
    assert!(
        request
            .to_lowercase()
            .contains("authorization: bearer generated-secret")
    );
    assert!(!request.to_lowercase().contains("\r\nrange:"));
    let playlist = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2,generated-secret\nsegment.ts?token=generated-secret\n";
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{playlist}", playlist.len()).as_bytes()).await.unwrap();
    let response = client.await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    let rewritten = response.text().await.unwrap();
    assert!(!rewritten.contains("generated-secret"));
    assert!(!rewritten.contains("provider-secret"));
    assert!(!rewritten.contains(&upstream.local_addr().unwrap().to_string()));
    assert!(
        rewritten
            .lines()
            .any(|line| line.starts_with(relay.prefix()) && line.ends_with(".ts"))
    );
    relay.shutdown().await;
}

#[tokio::test]
async fn cancelling_native_admission_never_opens_an_upstream_connection() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (relay, mut gate) = fixture(&format!(
        "http://{}/generated/index.m3u8",
        upstream.local_addr().unwrap()
    ));
    let url = relay.url().to_owned();
    let client = tokio::spawn(async move { reqwest::get(url).await.unwrap().status() });
    assert!(
        tokio::time::timeout(Duration::from_secs(5), gate.connected())
            .await
            .unwrap()
    );
    drop(gate);
    assert_eq!(client.await.unwrap(), 403);
    relay.shutdown().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(80), upstream.accept())
            .await
            .is_err()
    );
}
