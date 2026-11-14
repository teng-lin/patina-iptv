//! Independent wire-level regressions for the November transport audit.
use super::support::{Resp, TestServer};
use iptv_net::playback::{PlaybackPolicy, PlaybackRelay};

#[tokio::test]
async fn playlist_origin_is_private_on_redirect_and_subresource_boundaries() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    for (supplied, expected) in [
        (
            "https://portal.example.test/watch?token=synthetic",
            Some("https://portal.example.test"),
        ),
        ("null", None),
        ("not-an-origin", None),
        ("https://synthetic:secret@portal.example.test/watch", None),
        ("file:///synthetic", None),
    ] {
        for redirect in [false, true] {
            let second = TestServer::http(|_| Resp::ok(vec![0x47, 0, 1, 2])).await;
            let destination = second.url("/segment.ts");
            let origin = TestServer::http(move |request| {
                if request.path == "/start" && redirect {
                    Resp::redirect(302, &destination)
                } else if request.path == "/start" {
                    Resp::ok(format!("#EXTM3U\n#EXTINF:1,\n{destination}\n"))
                } else {
                    Resp::ok(vec![0x47, 0, 1, 2])
                }
            })
            .await;
            let relay = PlaybackRelay::start_with_policy(
                &origin.url("/start"),
                &[("Origin".into(), supplied.into())],
                PlaybackPolicy {
                    public_only: true,
                    allow_private_fixture: true,
                    ..PlaybackPolicy::default()
                },
            )
            .unwrap();
            let response = reqwest::get(relay.url()).await.unwrap();
            assert!(response.status().is_success());
            if !redirect {
                let playlist = response.text().await.unwrap();
                let segment = playlist
                    .lines()
                    .find(|line| line.starts_with("http://"))
                    .unwrap();
                assert!(reqwest::get(segment).await.unwrap().status().is_success());
            }
            assert_eq!(origin.requests()[0].header("origin"), Some(supplied));
            assert_eq!(second.requests().len(), 1);
            assert_eq!(
                second.requests()[0].header("origin"),
                expected,
                "{supplied}, redirect={redirect}"
            );
        }
    }
}

#[tokio::test]
async fn unauthenticated_idle_connections_release_the_relay_slots() {
    use std::time::Duration;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let origin = TestServer::http(|_| Resp::ok(vec![0x47, 0, 1, 2])).await;
    let relay = PlaybackRelay::start_with_policy(
        &origin.url("/stream.ts"),
        &[],
        PlaybackPolicy {
            allow_private_fixture: true,
            ..PlaybackPolicy::default()
        },
    )
    .unwrap();
    let address = reqwest::Url::parse(relay.url()).unwrap();
    let mut idle = Vec::new();
    for _ in 0..12 {
        idle.push(
            tokio::net::TcpStream::connect(("127.0.0.1", address.port().unwrap()))
                .await
                .unwrap(),
        );
    }
    // More than the one-second unauthenticated deadline, much less than
    // the upstream response budget. Keep all idle peers open throughout.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let response = tokio::time::timeout(Duration::from_secs(3), reqwest::get(relay.url()))
        .await
        .expect("idle sockets must not monopolize the relay")
        .expect("a valid request is admitted after unauthenticated expiry");
    assert!(response.status().is_success());
    assert_eq!(origin.requests().len(), 1);
    drop(idle);
}

#[tokio::test]
async fn unquoted_hls_keys_are_emitted_as_quoted_relay_capabilities() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let other = TestServer::http(|_| Resp::ok(vec![0_u8; 16])).await;
    let key = other.url("/synthetic.key");
    let origin = TestServer::http(move |_| {
        Resp::ok(format!(
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI={key}\n#EXTINF:1,\nsegment.ts\n"
        ))
    })
    .await;
    let relay = PlaybackRelay::start_with_policy(
        &origin.url("/list.m3u8"),
        &[],
        PlaybackPolicy {
            allow_private_fixture: true,
            ..PlaybackPolicy::default()
        },
    )
    .unwrap();
    let text = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let key_line = text
        .lines()
        .find(|line| line.starts_with("#EXT-X-KEY:"))
        .unwrap();
    let rewritten = key_line
        .split_once("URI=\"")
        .expect("key URI must be quoted")
        .1
        .split('"')
        .next()
        .unwrap();
    assert!(
        relay.owns_url(rewritten),
        "key must remain inside this relay capability"
    );
    assert!(
        other.requests().is_empty(),
        "rewriting itself must not fetch the key"
    );
    assert!(reqwest::get(rewritten).await.unwrap().status().is_success());
    assert_eq!(other.requests().len(), 1);
}

#[tokio::test]
async fn dash_encoding_and_control_confusion_is_refused_at_the_http_boundary() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    for body in [
        b"<?xml version=\"1.0\" encoding=\"ISO-2022-JP\"?><MPD/>".to_vec(),
        b"<MPD><Period>\x1b</Period></MPD>".to_vec(),
        b"<MPD><Period>\xff</Period></MPD>".to_vec(),
    ] {
        let origin = TestServer::http(move |_| Resp::ok(body.clone())).await;
        let relay = PlaybackRelay::start_with_policy(
            &origin.url("/manifest.mpd"),
            &[],
            PlaybackPolicy {
                allow_private_fixture: true,
                ..PlaybackPolicy::default()
            },
        )
        .unwrap();
        let response = reqwest::get(relay.url()).await.unwrap();
        assert!(
            !response.status().is_success(),
            "unsafe XML cannot reach the player"
        );
    }
}
