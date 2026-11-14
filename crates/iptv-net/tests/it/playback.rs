//! Real HTTP requests through the same relay used by both player hosts.
#![cfg(debug_assertions)]
use super::support::{Resp, TestServer};
use iptv_net::playback::{PlaybackPolicy, PlaybackRelay};

fn fixture_relay(url: &str, headers: &[(String, String)]) -> PlaybackRelay {
    let _ = rustls::crypto::ring::default_provider().install_default();
    PlaybackRelay::start_with_policy(
        url,
        headers,
        PlaybackPolicy {
            public_only: true,
            allow_private_fixture: true,
            ..PlaybackPolicy::default()
        },
    )
    .unwrap()
}

fn identity() -> Vec<(String, String)> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    ["Authorization", "Cookie", "SN", "X-User-Agent"]
        .into_iter()
        .map(|name| (name.into(), "synthetic-identity".into()))
        .collect()
}

#[tokio::test]
async fn hls_redirect_segments_keys_and_maps_scope_headers_by_exact_origin() {
    let second = TestServer::http(|_| Resp::ok(vec![0x47, 0, 1, 2, 3, 4])).await;
    let external = second.url("/other.ts");
    let to = second.url("/redirected.ts");
    let origin = TestServer::http(move |request| match request.path.as_str() {
        "/start.m3u8" => Resp::redirect(302, "/path/list.m3u8"),
        "/path/list.m3u8" => Resp::ok(format!(
            "#EXTM3U\n\
             #EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n\
             #EXT-X-MAP:URI=\"init.mp4\"\n\
             #EXT-X-MEDIA:TYPE=AUDIO,URI=\"{external}\"\n\
             #EXT-X-I-FRAME-STREAM-INF:URI=\"iframes.m3u8\"\n\
             #EXTINF:2,\nsegment.ts\n\
             #EXTINF:2,\nredirect.ts\n\
             #EXTINF:2,\n{external}\n"
        )),
        "/path/redirect.ts" => Resp::redirect(302, &to),
        "/path/iframes.m3u8" => Resp::ok("#EXTM3U\n"),
        _ => Resp::ok(vec![0x47, 0, 1, 2, 3, 4]),
    })
    .await;
    let relay = fixture_relay(&origin.url("/start.m3u8"), &identity());
    let text = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.starts_with("#EXTM3U"));
    let mut urls = Vec::new();
    for line in text.lines() {
        if line.starts_with("http://") {
            urls.push(line.to_owned());
        }
        if let Some((_, rest)) = line.split_once("URI=\"") {
            urls.push(rest.split('"').next().unwrap().to_owned());
        }
    }
    assert_eq!(urls.len(), 7);
    for url in urls {
        assert!(reqwest::get(url).await.unwrap().status().is_success());
    }
    for request in origin.requests() {
        for name in ["authorization", "cookie", "sn", "x-user-agent"] {
            assert_eq!(
                request.header(name),
                Some("synthetic-identity"),
                "{name} {}",
                request.path
            );
        }
    }
    assert_eq!(second.requests().len(), 3);
    for request in second.requests() {
        for name in ["authorization", "cookie", "sn", "x-user-agent"] {
            assert_eq!(request.header(name), None);
        }
    }
}

/// What a playlist or a portal attaches to a stream besides identity: a
/// credential header under a name of its own, and a `Referer` whose path
/// and query hold a session token.
const SCOPED_SECRET: &str = "synthetic-secret";
const SCOPED_REFERER: &str = "https://portal.example.test/watch/42?token=synthetic-secret";

fn scoped() -> Vec<(String, String)> {
    [
        ("User-Agent", "SyntheticPlayer/1.0"),
        ("Accept-Language", "en"),
        ("X-Api-Key", SCOPED_SECRET),
        ("X-Auth-Token", SCOPED_SECRET),
        ("Authorization", "Bearer synthetic-secret"),
        ("Referer", SCOPED_REFERER),
        ("Origin", "https://portal.example.test"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect()
}

/// The stream's own origin gets every header as configured.
fn assert_first_party(request: &super::support::synth::SimRequest) {
    for (name, value) in scoped() {
        assert_eq!(
            request.header(&name),
            Some(value.as_str()),
            "{name} {}",
            request.path
        );
    }
}

/// Another origin gets transport headers, `Origin` and the Referer's origin.
fn assert_third_party(request: &super::support::synth::SimRequest) {
    let path = &request.path;
    for name in ["x-api-key", "x-auth-token", "authorization", "cookie"] {
        assert_eq!(request.header(name), None, "{name} {path}");
    }
    assert_eq!(
        request.header("referer"),
        Some("https://portal.example.test/"),
        "{path}"
    );
    assert_eq!(
        request.header("origin"),
        Some("https://portal.example.test"),
        "{path}"
    );
    assert_eq!(
        request.header("user-agent"),
        Some("SyntheticPlayer/1.0"),
        "{path}"
    );
    assert_eq!(request.header("accept-language"), Some("en"), "{path}");
    for (name, value) in &request.headers {
        assert!(!value.contains(SCOPED_SECRET), "{name} {path}");
    }
}

/// Every relay URL a rewritten manifest names (lines and quoted attributes).
fn relay_urls(relay: &PlaybackRelay, manifest: &str) -> Vec<String> {
    manifest
        .split(['"', '\n', '<', '>'])
        .map(str::trim)
        .filter(|part| relay.owns_url(part))
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn hls_segments_keys_and_redirects_on_another_origin_get_no_scoped_headers() {
    let second = TestServer::http(|_| Resp::ok(vec![0x47, 0, 1, 2, 3, 4])).await;
    let (key, segment, redirected) = (
        second.url("/keys/key.bin"),
        second.url("/media/other.ts"),
        second.url("/media/redirected.ts"),
    );
    let origin = TestServer::http(move |request| match request.path.as_str() {
        "/live/list.m3u8" => Resp::ok(format!(
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"{key}\"\n#EXTINF:2,\nsame.ts\n#EXTINF:2,\n{segment}\n#EXTINF:2,\nredirect.ts\n"
        )),
        "/live/redirect.ts" => Resp::redirect(302, &redirected),
        _ => Resp::ok(vec![0x47, 0, 1, 2, 3, 4]),
    })
    .await;
    let relay = fixture_relay(&origin.url("/live/list.m3u8"), &scoped());
    let manifest = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let urls = relay_urls(&relay, &manifest);
    assert_eq!(urls.len(), 4, "{manifest}");
    for url in urls {
        assert!(reqwest::get(url).await.unwrap().status().is_success());
    }
    let first_party = origin.requests();
    assert_eq!(
        first_party
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        ["/live/list.m3u8", "/live/same.ts", "/live/redirect.ts"]
    );
    first_party.iter().for_each(assert_first_party);
    let mut third_party = second.requests();
    third_party.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        third_party
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        ["/keys/key.bin", "/media/other.ts", "/media/redirected.ts"]
    );
    third_party.iter().for_each(assert_third_party);
}

#[tokio::test]
async fn dash_resources_on_another_origin_get_no_scoped_headers() {
    let second = TestServer::http(|_| Resp::ok(vec![0, 0, 0, 1])).await;
    let (base, absolute) = (second.url("/cdn/"), second.url("/media/two.m4s"));
    let origin = TestServer::http(move |request| match request.path.as_str() {
        "/vod/main.mpd" => Resp::ok(format!(
            "<MPD><Period><AdaptationSet><Representation><SegmentList>\
             <Initialization sourceURL=\"init.mp4\"/><SegmentURL media=\"{absolute}\"/>\
             </SegmentList></Representation><Representation><BaseURL>{base}</BaseURL>\
             <SegmentList><SegmentURL media=\"three.m4s\"/></SegmentList></Representation>\
             </AdaptationSet></Period></MPD>"
        )),
        _ => Resp::ok(vec![0, 0, 0, 1]),
    })
    .await;
    let relay = fixture_relay(&origin.url("/vod/main.mpd"), &scoped());
    let manifest = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(manifest.starts_with("<MPD>"), "{manifest}");
    // The BaseURL itself is a directory, not a resource a player requests.
    let urls: Vec<String> = relay_urls(&relay, &manifest)
        .into_iter()
        .filter(|url| !url.ends_with("/cdn/"))
        .collect();
    assert_eq!(urls.len(), 3, "{manifest}");
    for url in urls {
        assert!(reqwest::get(url).await.unwrap().status().is_success());
    }
    let first_party = origin.requests();
    assert_eq!(
        first_party
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        ["/vod/main.mpd", "/vod/init.mp4"]
    );
    first_party.iter().for_each(assert_first_party);
    let mut third_party = second.requests();
    third_party.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        third_party
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        ["/cdn/three.m4s", "/media/two.m4s"]
    );
    third_party.iter().for_each(assert_third_party);
}

#[tokio::test]
async fn a_server_behind_an_extra_root_certificate_plays_as_well_as_it_imports() {
    let (acceptor, der) = super::support::self_signed();
    let origin = TestServer::https(|_| Resp::ok(vec![0x47, 0, 1, 2]), acceptor).await;
    let policy = |extra_roots_der| PlaybackPolicy {
        public_only: true,
        extra_roots_der,
        allow_private_fixture: true,
    };
    // Not in the Mozilla roots: the handshake is refused.
    let unknown =
        PlaybackRelay::start_with_policy(&origin.url("/media.ts"), &[], policy(Vec::new()))
            .unwrap();
    assert_eq!(reqwest::get(unknown.url()).await.unwrap().status(), 502);
    assert!(origin.requests().is_empty());

    let trusting =
        PlaybackRelay::start_with_policy(&origin.url("/media.ts"), &[], policy(vec![der])).unwrap();
    let response = reqwest::get(trusting.url()).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), &[0x47, 0, 1, 2]);

    assert!(
        PlaybackRelay::start_with_policy(
            &origin.url("/media.ts"),
            &[],
            policy(vec![b"not der".to_vec()])
        )
        .is_err()
    );
}

/// The Android host refuses any load outside this shape (`RelayGuard.kt`):
/// `http://127.0.0.1:<port>/<hex token>/`, then `key/` for an HLS key, then
/// the absolute `http(s)` address the relay fetches, without dot segments.
#[tokio::test]
async fn relay_addresses_keep_the_shape_the_android_guard_admits() {
    let origin = TestServer::http(|request| match request.path.as_str() {
        "/a/list.m3u8" => Resp::ok("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"../key.bin\"\n#EXT-X-MAP:URI=\"./init.mp4\"\n#EXTINF:2,\n../b/./segment.ts?x=1\n"),
        _ => Resp::ok(vec![0x47, 0, 1, 2, 3, 4]),
    })
    .await;
    let relay = fixture_relay(&origin.url("/a/list.m3u8"), &identity());
    let prefix = relay.prefix();
    let rest = prefix.strip_prefix("http://127.0.0.1:").unwrap();
    let (port, token) = rest.trim_end_matches('/').split_once('/').unwrap();
    assert!(port.parse::<u16>().unwrap() > 0);
    assert!(
        token.len() >= 32
            && token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    );
    let text = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let mut urls = vec![relay.url().to_owned()];
    for line in text.lines() {
        if line.starts_with("http") {
            urls.push(line.to_owned());
        }
        if let Some((_, rest)) = line.split_once("URI=\"") {
            urls.push(rest.split('"').next().unwrap().to_owned());
        }
    }
    assert_eq!(urls.len(), 4);
    for url in urls {
        let target = url.strip_prefix(prefix).unwrap();
        let target = target.strip_prefix("key/").unwrap_or(target);
        assert!(target.starts_with("http://"), "{target}");
        let path = target["http://".len()..].split(['?', '#']).next().unwrap();
        assert!(path.split('/').all(|s| s != "." && s != ".."), "{target}");
    }
}

#[tokio::test]
async fn relay_preserves_ranges_and_cancels_retired_streams() {
    let origin = TestServer::http(|request| {
        if request.path == "/slow.ts" {
            return Resp::ok("later").delayed(std::time::Duration::from_secs(20));
        }
        assert_eq!(request.header("range"), Some("bytes=2-4"));
        Resp {
            status: 206,
            ..Resp::ok(vec![0, 1, 2])
        }
        .header("Content-Range", "bytes 2-4/6")
        .header("Accept-Ranges", "bytes")
    })
    .await;
    let relay = fixture_relay(&origin.url("/range.ts"), &identity());
    let response = reqwest::Client::new()
        .get(relay.url())
        .header("Range", "bytes=2-4")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.headers()["content-range"], "bytes 2-4/6");
    drop(relay);
    let relay = fixture_relay(&origin.url("/slow.ts"), &identity());
    let url = relay.url().to_owned();
    let request = tokio::spawn(async move { reqwest::get(url).await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drop(relay);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn oversized_manifest_is_refused_without_exposing_upstream_details() {
    let origin = TestServer::http(|_| {
        Resp::ok([b"#EXTM3U\n#".as_slice(), &vec![b'x'; 2 * 1024 * 1024]].concat())
    })
    .await;
    let relay = fixture_relay(&origin.url("/large.m3u8"), &identity());
    let response = reqwest::get(relay.url()).await.unwrap();
    assert_eq!(response.status(), 502);
    assert_eq!(response.text().await.unwrap(), "Manifest exceeds limit");
}

#[tokio::test]
async fn split_manifest_signature_is_rewritten_and_other_playlists_fail_closed() {
    let origin = TestServer::http(|request| match request.path.as_str() {
        "/split" => Resp::ok(Vec::new()).chunked(
            vec![b"#EX".to_vec(), b"TM3U\n#EXTINF:2,\nrelative.ts\n".to_vec()],
            std::time::Duration::from_millis(5),
        ),
        "/unsafe" => Resp::ok("[playlist]\nFile1=http://127.0.0.1/private\n"),
        "/unicode" => Resp::ok("[playlist]\nTitle1=Cañal\nFile1=http://127.0.0.1/private\n"),
        "/m3u" => Resp::ok("#é\nhttp://127.0.0.1/private\n"),
        "/asx" => Resp::ok("<ASX><ENTRY><REF HREF=\"http://127.0.0.1/private\"/></ENTRY></ASX>"),
        _ => Resp::ok("WEBVTT\n\n00:00.000 --> 00:01.000\nSynthetic caption\n"),
    })
    .await;
    let relay = fixture_relay(&origin.url("/split"), &identity());
    let body = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.starts_with("#EXTM3U"));
    assert!(body.contains("/http://localhost:"));
    assert!(!body.lines().any(|line| line == "relative.ts"));
    for path in ["/unsafe", "/asx", "/unicode", "/m3u"] {
        let relay = fixture_relay(&origin.url(path), &identity());
        assert!(
            !reqwest::get(relay.url())
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert_eq!(
            relay.take_error_code(),
            Some("ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED")
        );
    }
    let relay = fixture_relay(&origin.url("/captions.vtt"), &identity());
    assert!(
        reqwest::get(relay.url())
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .starts_with("WEBVTT")
    );
}

#[tokio::test]
async fn public_content_cannot_reach_local_hosts_and_trust_never_follows_origin_change() {
    let second = TestServer::http(|_| Resp::ok("synthetic")).await;
    let target = second.url("/private");
    let origin = TestServer::http(move |_| Resp::redirect(302, &target)).await;
    let relay = PlaybackRelay::start(&origin.url("/root"), &identity()).unwrap();
    assert_eq!(reqwest::get(relay.url()).await.unwrap().status(), 502);
    assert!(origin.requests().is_empty());
    let trusted = PlaybackRelay::start_with_policy(
        &origin.url("/root"),
        &identity(),
        PlaybackPolicy {
            public_only: false,
            ..PlaybackPolicy::default()
        },
    )
    .unwrap();
    assert_eq!(reqwest::get(trusted.url()).await.unwrap().status(), 502);
    assert_eq!(origin.requests().len(), 1);
    assert!(second.requests().is_empty());
}

/// The transport half of a Stalker portal on a private network: a link the
/// player may open only on public addresses is refused before any request,
/// and one trusted for its own origin (the portal's host) plays, without
/// that trust reaching another private origin.
#[tokio::test]
async fn a_private_stream_plays_only_when_its_own_origin_is_trusted() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let other = TestServer::http(|_| Resp::ok(vec![0x47, 9, 9])).await;
    let elsewhere = other.url("/other.ts");
    let portal = TestServer::http(move |request| match request.path.as_str() {
        "/live/list.m3u8" => Resp::ok(format!(
            "#EXTM3U\n#EXTINF:2,\nsegment.ts\n#EXTINF:2,\n{elsewhere}\n"
        )),
        _ => Resp::ok(vec![0x47, 0, 1, 2]),
    })
    .await;
    let private = portal
        .url("/live/list.m3u8")
        .replace("localhost", "127.0.0.1");
    let policy = |public_only| PlaybackPolicy {
        public_only,
        ..PlaybackPolicy::default()
    };

    let refused = PlaybackRelay::start_with_policy(&private, &[], policy(true)).unwrap();
    assert_eq!(reqwest::get(refused.url()).await.unwrap().status(), 502);
    assert!(portal.requests().is_empty(), "refused before any request");

    let trusted = PlaybackRelay::start_with_policy(&private, &[], policy(false)).unwrap();
    let manifest = reqwest::get(trusted.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let urls = relay_urls(&trusted, &manifest);
    assert_eq!(urls.len(), 2, "{manifest}");
    let segment = reqwest::get(&urls[0]).await.unwrap();
    assert_eq!(segment.status(), 200);
    assert_eq!(segment.bytes().await.unwrap().as_ref(), &[0x47, 0, 1, 2]);
    assert_eq!(reqwest::get(&urls[1]).await.unwrap().status(), 502);
    assert!(other.requests().is_empty(), "another private origin");
}

#[tokio::test]
async fn rewritten_ranges_are_whole_entities_and_mislabeled_media_still_streams() {
    let origin = TestServer::http(|request| match request.path.as_str() {
        "/ranged.m3u8" if request.header("range").is_some() => Resp {
            status: 206,
            ..Resp::ok("#EXTM3U\n")
        }
        .header("Content-Range", "bytes 0-7/100"),
        "/ranged.m3u8" => Resp::ok("#EXTM3U\n#EXTINF:2,\nsegment.ts\n"),
        "/binary.m3u8" => {
            Resp::ok(vec![0x47, 0, 1, 2]).header("Content-Type", "application/vnd.apple.mpegurl")
        }
        "/refused.m3u8" => Resp {
            status: 403,
            ..Resp::ok("Forbidden: synthetic expired")
        },
        _ => Resp::ok(vec![0, 1, 2]),
    })
    .await;
    let relay = fixture_relay(&origin.url("/ranged.m3u8"), &identity());
    let response = reqwest::Client::new()
        .get(relay.url())
        .header("Range", "bytes=0-")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("content-range").is_none());
    let length: usize = response.headers()["content-length"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let body = response.text().await.unwrap();
    assert_eq!(length, body.len());
    assert!(body.ends_with("/segment.ts\n"));
    assert_eq!(origin.requests().len(), 2);
    assert_eq!(origin.requests()[1].header("range"), None);
    let relay = fixture_relay(&origin.url("/binary.m3u8"), &identity());
    assert_eq!(
        reqwest::get(relay.url())
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        &[0x47, 0, 1, 2]
    );
    let relay = fixture_relay(&origin.url("/refused.m3u8"), &identity());
    assert_eq!(reqwest::get(relay.url()).await.unwrap().status(), 403);
}

#[tokio::test]
async fn url_basic_auth_is_origin_scoped_and_survives_header_updates() {
    let second = TestServer::http(|_| Resp::ok(vec![0, 1, 2])).await;
    let target = second.url("/other.ts");
    let origin = TestServer::http(move |request| {
        if request.path == "/redirect" {
            Resp::redirect(302, &target)
        } else {
            Resp::ok(vec![0, 1, 2])
        }
    })
    .await;
    let url = origin
        .url("/media.ts")
        .replace("http://", "http://demo:synthetic@");
    let relay = fixture_relay(&url, &[]);
    assert!(!relay.url().contains("demo:synthetic"));
    relay
        .update_headers(&[("User-Agent".into(), "Synthetic/1.0".into())])
        .unwrap();
    assert!(
        reqwest::get(relay.url())
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        origin.requests()[0]
            .header("authorization")
            .unwrap()
            .starts_with("Basic ")
    );
    let url = origin
        .url("/redirect")
        .replace("http://", "http://demo:synthetic@");
    let relay = fixture_relay(&url, &[]);
    assert!(
        reqwest::get(relay.url())
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(second.requests()[0].header("authorization").is_none());
}

#[tokio::test]
async fn response_cookies_persist_only_on_the_exact_origin() {
    let other = TestServer::http(|_| Resp::ok(vec![0x47, 0, 1])).await;
    let external = other.url("/part.ts");
    let origin = TestServer::http(move |request| match request.path.as_str() {
        "/start" => Resp::redirect(302, "/list").header("Set-Cookie", "session=synthetic; Path=/"),
        "/list" => Resp::ok(format!(
            "#EXTM3U\n#EXTINF:2,\n/part.ts\n#EXTINF:2,\n{external}\n"
        )),
        _ => Resp::ok(vec![0x47, 0, 1]),
    })
    .await;
    let relay = fixture_relay(
        &origin.url("/start"),
        &[("Cookie".into(), " initial=fixture; ".into())],
    );
    let body = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for url in body.lines().filter(|line| line.starts_with("http://")) {
        assert!(reqwest::get(url).await.unwrap().status().is_success());
    }
    for request in origin.requests().into_iter().skip(1) {
        let cookie = request.header("cookie").unwrap();
        assert!(cookie.contains("initial=fixture"));
        assert!(cookie.contains("session=synthetic"));
    }
    assert_eq!(other.requests().len(), 1);
    assert_eq!(other.requests()[0].header("cookie"), None);
}

#[tokio::test]
async fn later_http_refusal_clears_an_earlier_transport_error() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let refused = Arc::new(AtomicBool::new(false));
    let state = refused.clone();
    let origin = TestServer::http(move |_| {
        if state.load(Ordering::SeqCst) {
            Resp::status(403)
        } else {
            Resp::ok("[playlist]\nFile1=http://localhost/private\n")
        }
    })
    .await;
    let relay = fixture_relay(&origin.url("/stream"), &[]);
    assert_eq!(reqwest::get(relay.url()).await.unwrap().status(), 415);
    // Do not consume the original unsupported-format error: the next upstream
    // response must replace it so the player can report the actual refusal.
    refused.store(true, Ordering::SeqCst);
    assert_eq!(reqwest::get(relay.url()).await.unwrap().status(), 403);
    assert_eq!(relay.take_error_code(), None);
}

#[tokio::test]
async fn head_preserves_media_length_without_a_body() {
    let origin = TestServer::http(|_| Resp::ok(vec![0; 123])).await;
    let relay = fixture_relay(&origin.url("/media.ts"), &[]);
    let response = reqwest::Client::new()
        .head(relay.url())
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-length"], "123");
    assert!(response.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn head_describes_the_rewritten_manifest() {
    let origin = TestServer::http(|_| Resp::ok("#EXTM3U\n#EXTINF:2,\npart.ts\n")).await;
    let relay = fixture_relay(&origin.url("/manifest"), &[]);
    let body = reqwest::get(relay.url())
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let head = reqwest::Client::new()
        .head(relay.url())
        .send()
        .await
        .unwrap();
    assert_eq!(head.headers()["content-length"], body.len().to_string());
    assert!(head.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn printable_aes_key_bytes_are_not_mistaken_for_a_playlist() {
    let origin = TestServer::http(|request| match request.path.as_str() {
        "/key" => Resp::ok("0123456789abcdef"),
        _ => Resp::ok("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n"),
    })
    .await;
    let relay = fixture_relay(&origin.url("/manifest"), &[]);
    let body = reqwest::get(relay.url())
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let url = body
        .split_once("URI=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap();
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        b"0123456789abcdef"
    );
}

#[tokio::test]
async fn ascii_slice_of_binary_media_keeps_its_original_range() {
    let origin = TestServer::http(|request| {
        if request.header("range").is_some() {
            Resp {
                status: 206,
                ..Resp::ok("ftypisom")
            }
            .header("Content-Range", "bytes 4-11/128")
        } else {
            Resp::ok(vec![0; 128])
        }
    })
    .await;
    let relay = fixture_relay(&origin.url("/media.mp4"), &[]);
    let response = reqwest::Client::new()
        .get(relay.url())
        .header("Range", "bytes=4-11")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.headers()["content-range"], "bytes 4-11/128");
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"ftypisom");
}

#[tokio::test]
async fn header_updates_remove_revoked_seeded_cookies() {
    let origin = TestServer::http(|_| Resp::ok(vec![0, 1, 2])).await;
    let relay = fixture_relay(
        &origin.url("/media"),
        &[("Cookie".into(), "a=1; b=2;".into())],
    );
    reqwest::get(relay.url()).await.unwrap();
    relay
        .update_headers(&[("Cookie".into(), "a=3".into())])
        .unwrap();
    reqwest::get(relay.url()).await.unwrap();
    relay.update_headers(&[]).unwrap();
    reqwest::get(relay.url()).await.unwrap();
    let requests = origin.requests();
    assert_eq!(requests[1].header("cookie"), Some("a=3"));
    assert_eq!(requests[2].header("cookie"), None);
}

#[tokio::test]
async fn changed_full_probe_never_authorizes_a_raw_ranged_manifest() {
    let origin = TestServer::http(|request| {
        if request.header("range").is_some() {
            Resp {
                status: 206,
                ..Resp::ok("#EXTM3U\n#EXTINF:2,\nhttp://127.0.0.1/private\n")
            }
            .header("Content-Range", "bytes 0-50/51")
        } else {
            Resp::ok(vec![0; 128])
        }
    })
    .await;
    let relay = fixture_relay(&origin.url("/media"), &[]);
    let response = reqwest::Client::new()
        .get(relay.url())
        .header("Range", "bytes=0-")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), &[0; 128]);
}

#[tokio::test]
async fn failed_full_probe_does_not_prepend_stale_slice_bytes() {
    let origin = TestServer::http(|request| {
        if request.header("range").is_some() {
            Resp {
                status: 206,
                ..Resp::ok("ftypisom")
            }
            .header("Content-Range", "bytes 4-11/128")
        } else {
            Resp {
                status: 403,
                ..Resp::ok("refused")
            }
        }
    })
    .await;
    let relay = fixture_relay(&origin.url("/media"), &[]);
    let response = reqwest::Client::new()
        .get(relay.url())
        .header("Range", "bytes=4-11")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(response.text().await.unwrap(), "refused");
}
