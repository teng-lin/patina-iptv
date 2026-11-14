//! Transport behaviour against a local server.

use crate::support;

use std::io::Write as _;
use std::time::Duration;

use iptv_import::{BodyStream, FetchErrorKind};
use iptv_net::{NetClient, NetConfig};
use iptv_providers::HttpRequest;
use support::{Resp, TestServer, self_signed};

fn client() -> NetClient {
    NetClient::with_defaults().unwrap()
}

#[tokio::test]
async fn buffered_fetch_sends_headers_as_given() {
    let server = TestServer::http(|_| Resp::ok("hello")).await;
    let net = client();
    let resp = net
        .fetch(
            HttpRequest::get(server.url("/a?x=1"))
                .with_header("User-Agent", "SyntheticPlayer/1.0")
                .with_header("Cookie", "mac=00:1A:79:00:00:01; stb_lang=en")
                .with_header("Referer", "http://ref.example.com/"),
        )
        .await
        .unwrap();
    assert_eq!((resp.status, resp.body.as_slice()), (200, &b"hello"[..]));
    let seen = &server.requests()[0];
    assert_eq!(seen.path, "/a");
    assert_eq!(seen.param("x"), Some("1"));
    assert_eq!(seen.header("user-agent"), Some("SyntheticPlayer/1.0"));
    assert_eq!(
        seen.header("cookie"),
        Some("mac=00:1A:79:00:00:01; stb_lang=en")
    );
    assert_eq!(seen.header("referer"), Some("http://ref.example.com/"));

    // Without a User-Agent the configured default is sent.
    net.fetch(HttpRequest::get(server.url("/b"))).await.unwrap();
    assert_eq!(
        server.requests()[1].header("user-agent"),
        Some(iptv_providers::xtream::DEFAULT_USER_AGENT)
    );
    // Non-success statuses are responses, not errors.
    let server = TestServer::http(|_| Resp::status(404)).await;
    assert_eq!(
        net.fetch(HttpRequest::get(server.url("/")))
            .await
            .unwrap()
            .status,
        404
    );
}

#[tokio::test]
async fn content_encoding_is_decoded_but_gz_files_pass_through() {
    let gz = support::synth::gzip(b"#EXTM3U\n");
    let mut deflate = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    deflate.write_all(b"deflated body").unwrap();
    let deflate = deflate.finish().unwrap();
    let (g, d) = (gz.clone(), deflate.clone());
    let server = TestServer::http(move |req| match req.path.as_str() {
        "/gzip" => Resp::ok(g.clone()).header("Content-Encoding", "gzip"),
        "/deflate" => Resp::ok(d.clone()).header("Content-Encoding", "deflate"),
        _ => Resp::ok(g.clone()).header("Content-Type", "application/gzip"),
    })
    .await;
    let net = client();
    let body = |p: &'static str| {
        let net = net.clone();
        let url = server.url(p);
        async move { net.fetch(HttpRequest::get(url)).await.unwrap().body }
    };
    assert_eq!(body("/gzip").await, b"#EXTM3U\n");
    assert_eq!(body("/deflate").await, b"deflated body");
    assert_eq!(body("/guide.xml.gz").await, gz, "file bytes untouched");
    let accept = server.requests()[0]
        .header("accept-encoding")
        .unwrap()
        .to_owned();
    assert!(
        accept.contains("gzip") && accept.contains("deflate"),
        "{accept}"
    );
}

#[tokio::test]
async fn guide_gzip_has_identical_bytes_and_budget_for_http_encoding_and_file() {
    let xml = format!("<tv>{}</tv>", " ".repeat(4096));
    let gzip = support::synth::gzip(xml.as_bytes());
    assert!(gzip.len() < xml.len());
    let payload = gzip.clone();
    let server = TestServer::http(move |req| {
        let response = Resp::ok(payload.clone());
        if req.path == "/encoded" {
            response.header("Content-Encoding", "gzip")
        } else {
            response
        }
    })
    .await;
    let net = NetClient::new(NetConfig {
        max_streamed_bytes: gzip.len() as u64,
        ..Default::default()
    })
    .unwrap();
    for path in ["/encoded", "/guide.xml.gz"] {
        let mut request = HttpRequest::get(server.url(path)).with_header("Accept-Encoding", "gzip");
        request.preserve_gzip = true;
        assert_eq!(net.fetch(request).await.unwrap().body, gzip);
    }
}

#[tokio::test]
async fn redirects_relative_cross_origin_and_cross_protocol() {
    let target = TestServer::http(|req| Resp::ok(format!("landed {}", req.path))).await;
    let to = target.url("/final");
    let origin = TestServer::http(move |req| match req.path.as_str() {
        "/rel" => Resp::redirect(302, "/abs"),
        "/abs" => Resp::redirect(301, &to),
        "/loop" => Resp::redirect(302, "/loop"),
        _ => Resp::status(404),
    })
    .await;
    let net = NetClient::new(NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    let resp = net
        .fetch(
            HttpRequest::get(origin.url("/rel?username=demo-user&password=synthetic-pass"))
                .with_header("Cookie", "mac=00:1A:79:00:00:02")
                .with_header("Authorization", "Bearer tok-synthetic")
                .with_header("SN", "SERIAL-SYNTHETIC")
                .with_header("User-Agent", "SyntheticPlayer/1.0")
                .with_header("X-User-Agent", "Model: Synthetic"),
        )
        .await
        .unwrap();
    assert_eq!(resp.body, b"landed /final");
    // Identity stays with the original origin, across same-origin hops.
    let same = &origin.requests()[1];
    assert_eq!(same.path, "/abs");
    for h in ["cookie", "authorization", "sn"] {
        assert!(same.header(h).is_some(), "{h} kept on a same-origin hop");
    }
    let landed = &target.requests()[0];
    for h in ["cookie", "authorization", "sn"] {
        assert_eq!(landed.header(h), None, "{h} must not reach another origin");
    }
    // No automatic Referer: it would carry the credentials in the query.
    assert_eq!(landed.header("referer"), None);
    assert_eq!(same.header("referer"), None);
    assert_eq!(landed.header("user-agent"), Some("SyntheticPlayer/1.0"));
    assert_eq!(landed.header("x-user-agent"), None);

    let err = net
        .fetch(HttpRequest::get(origin.url("/loop")))
        .await
        .unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Redirect, "{err}");

    // https → http is refused before contacting the insecure target.
    let (acceptor, der) = self_signed();
    let to = target.url("/plain");
    let secure = TestServer::https(move |_| Resp::redirect(302, &to), acceptor).await;
    let trusting = NetClient::new(NetConfig {
        allow_private_fixture: true,
        extra_roots_der: vec![der],
        ..NetConfig::default()
    })
    .unwrap();
    let before = target.requests().len();
    let error = trusting
        .fetch(HttpRequest::get(secure.url("/start")))
        .await
        .unwrap_err();
    assert_eq!(error.kind, FetchErrorKind::Redirect);
    assert_eq!(target.requests().len(), before);
}

#[tokio::test]
async fn cross_origin_redirects_keep_only_transport_headers_and_the_referer_origin() {
    let target = TestServer::http(|_| Resp::ok("landed")).await;
    let to = target.url("/final");
    let origin = TestServer::http(move |req| match req.path.as_str() {
        "/start" => Resp::redirect(302, "/same"),
        _ => Resp::redirect(307, &to),
    })
    .await;
    let net = NetClient::new(NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    // A playlist's own credential header, and a Referer whose path and query
    // hold a token (a portal page, an embedding site).
    let referer = origin.url("/portal/c/index.html?token=synthetic-secret");
    let sent = [
        ("User-Agent", "SyntheticPlayer/1.0"),
        ("Accept-Language", "en"),
        ("X-Api-Key", "synthetic-secret"),
        ("X-Auth-Token", "synthetic-secret"),
        ("Referer", referer.as_str()),
        ("Origin", "https://app.example.test"),
    ];
    let mut request = HttpRequest::get(origin.url("/start"));
    for (name, value) in sent {
        request = request.with_header(name, value);
    }
    assert_eq!(net.fetch(request).await.unwrap().body, b"landed");

    // Same origin, including a same-origin redirect: exactly as given.
    let first_party = origin.requests();
    assert_eq!(first_party.len(), 2);
    for request in &first_party {
        for (name, value) in sent {
            assert_eq!(request.header(name), Some(value), "{name} {}", request.path);
        }
    }
    let landed = &target.requests()[0];
    for name in ["x-api-key", "x-auth-token"] {
        assert_eq!(
            landed.header(name),
            None,
            "{name} must not reach another origin"
        );
    }
    assert_eq!(landed.header("user-agent"), Some("SyntheticPlayer/1.0"));
    assert_eq!(landed.header("accept-language"), Some("en"));
    assert_eq!(landed.header("origin"), Some("https://app.example.test"));
    // Path and query stay behind; the host a CDN may check is still named.
    let referer_origin = origin.url("/");
    assert_eq!(landed.header("referer"), Some(referer_origin.as_str()));
    for (name, value) in &landed.headers {
        assert!(!value.contains("synthetic-secret"), "{name}");
    }
}

#[tokio::test]
async fn a_body_sent_again_to_another_origin_keeps_its_media_type_only() {
    let target = TestServer::http(|_| Resp::ok("landed")).await;
    let to = target.url("/final");
    let origin = TestServer::http(move |req| match req.path.as_str() {
        "/again" => Resp::redirect(307, &to),
        _ => Resp::redirect(303, &to),
    })
    .await;
    let net = NetClient::new(NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    let post = |path: &str| {
        let mut request = HttpRequest::get(origin.url(path))
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("X-Api-Key", "synthetic-secret");
        request.method = iptv_providers::Method::Post;
        request.body = Some(b"synthetic=1".to_vec());
        request
    };
    net.fetch(post("/again")).await.unwrap();
    net.fetch(post("/other")).await.unwrap();
    let landed = target.requests();
    // 307: the same request again, described but without the credential.
    assert_eq!(
        landed[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(landed[0].header("content-length"), Some("11"));
    // 303: a GET without the body, so nothing describes one.
    assert_eq!(landed[1].header("content-type"), None);
    for request in &landed {
        assert_eq!(request.header("x-api-key"), None);
    }
}

#[tokio::test]
async fn tls_uses_webpki_roots_plus_extra_roots() {
    let (acceptor, der) = self_signed();
    let server = TestServer::https(|_| Resp::ok("secure"), acceptor).await;
    let err = client()
        .fetch(HttpRequest::get(server.url("/")))
        .await
        .unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Tls, "{err}");
    let trusting = NetClient::new(NetConfig {
        allow_private_fixture: true,
        extra_roots_der: vec![der],
        ..NetConfig::default()
    })
    .unwrap();
    let resp = trusting
        .fetch(HttpRequest::get(server.url("/")))
        .await
        .unwrap();
    assert_eq!(resp.body, b"secure");
    assert!(
        NetClient::new(NetConfig {
            extra_roots_der: vec![b"not der".to_vec()],
            ..NetConfig::default()
        })
        .is_err()
    );
}

#[tokio::test]
async fn streaming_delivers_chunks_as_they_arrive() {
    let pieces: Vec<Vec<u8>> = (0..5).map(|i| vec![b'a' + i; 1000]).collect();
    let p2 = pieces.clone();
    let server =
        TestServer::http(move |_| Resp::ok("").chunked(p2.clone(), Duration::from_millis(30)))
            .await;
    let resp = client()
        .fetch_stream(HttpRequest::get(server.url("/big")))
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.content_length, None);
    let mut body = resp.body;
    let mut got = Vec::new();
    let mut chunks = 0;
    while let Some(c) = body.next_chunk().await.unwrap() {
        assert!(!c.is_empty());
        chunks += 1;
        got.extend(c);
    }
    assert_eq!(got, pieces.concat());
    assert!(
        chunks >= 5,
        "chunks are not coalesced into one buffer: {chunks}"
    );
    assert_eq!(body.next_chunk().await.unwrap(), None, "stays finished");
}

#[tokio::test]
async fn timeouts_and_connection_failures() {
    let net = NetClient::new(NetConfig {
        read_timeout: Duration::from_millis(200),
        ..NetConfig::default()
    })
    .unwrap();
    let server = TestServer::http(|req| match req.path.as_str() {
        "/slow-head" => Resp::ok("late").delayed(Duration::from_secs(3)),
        _ => Resp::ok("").chunked(vec![b"x".to_vec(), b"y".to_vec()], Duration::from_secs(3)),
    })
    .await;
    // The fixture listens on IPv4. Avoid localhost's IPv6 fallback delay
    // consuming the short read timeout on Windows before HTTP even starts.
    let fixture_url = |path: &str| server.url(path).replace("localhost", "127.0.0.1");
    let err = net
        .fetch(HttpRequest::get(fixture_url("/slow-head")))
        .await
        .unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Timeout, "{err}");
    let resp = net
        .fetch_stream(HttpRequest::get(fixture_url("/stall")))
        .await
        .unwrap();
    let mut body = resp.body;
    assert_eq!(body.next_chunk().await.unwrap().as_deref(), Some(&b"x"[..]));
    let err = body.next_chunk().await.unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Timeout, "{err}");

    // A closed port; credentials in the URL never reach the error.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let url = format!(
        "http://127.0.0.1:{port}/player_api.php?username=demo-user&password=synthetic-pass"
    );
    // Connection refusal can take longer than the deliberate 200 ms read timeout.
    let err = client().fetch(HttpRequest::get(url)).await.unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Connect, "{err}");
    let shown = format!("{err} {err:?}");
    assert!(
        !shown.contains("synthetic-pass") && !shown.contains("demo-user"),
        "{shown}"
    );
    assert!(!shown.contains("127.0.0.1"), "no URL in errors: {shown}");
}

#[tokio::test]
async fn a_slow_steady_download_outlives_the_idle_timeout_and_a_stalled_one_does_not() {
    // A stall is what ends a download, not its total time.
    let defaults = NetConfig::default();
    assert_eq!(defaults.connect_timeout, Duration::from_secs(15));
    assert_eq!(defaults.read_timeout, Duration::from_secs(60));
    assert_eq!(defaults.request_timeout, Duration::from_secs(30 * 60));

    let idle = Duration::from_millis(400);
    let pieces: Vec<Vec<u8>> = (0..15).map(|i| vec![b'a' + i; 100]).collect();
    let steady = pieces.clone();
    let server = TestServer::http(move |req| match req.path.as_str() {
        // 15 pauses of 40 ms: longer in total than the idle timeout, never
        // silent for that long.
        "/steady" => Resp::ok("").chunked(steady.clone(), Duration::from_millis(40)),
        _ => Resp::ok("").chunked(vec![b"x".to_vec(), b"y".to_vec()], Duration::from_secs(5)),
    })
    .await;
    let net = NetClient::new(NetConfig {
        read_timeout: idle,
        ..NetConfig::default()
    })
    .unwrap();
    let fixture_url = |path: &str| server.url(path).replace("localhost", "127.0.0.1");

    let started = std::time::Instant::now();
    let mut body = net
        .fetch_stream(HttpRequest::get(fixture_url("/steady")))
        .await
        .unwrap()
        .body;
    let mut got = Vec::new();
    while let Some(chunk) = body.next_chunk().await.unwrap() {
        got.extend(chunk);
    }
    assert_eq!(got, pieces.concat());
    assert!(started.elapsed() > idle, "{:?}", started.elapsed());

    let mut body = net
        .fetch_stream(HttpRequest::get(fixture_url("/stall")))
        .await
        .unwrap()
        .body;
    assert_eq!(body.next_chunk().await.unwrap().as_deref(), Some(&b"x"[..]));
    let stalled = std::time::Instant::now();
    let err = body.next_chunk().await.unwrap_err();
    // An idle timeout, which an import retries; not the total deadline.
    assert_eq!(err.kind, FetchErrorKind::Timeout, "{err}");
    assert!(
        stalled.elapsed() < Duration::from_secs(4),
        "{:?}",
        stalled.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn the_wait_before_a_retry_is_a_runtime_sleep() {
    let net = client();
    let before = tokio::time::Instant::now();
    iptv_import::Fetch::sleep(&net, Duration::from_secs(8)).await;
    // The paused clock moved by the wait; no wall-clock time passed.
    assert_eq!(before.elapsed(), Duration::from_secs(8));
}

#[tokio::test]
async fn buffered_bodies_are_bounded() {
    let server = TestServer::http(|req| match req.path.as_str() {
        "/sized" => Resp::ok(vec![b'z'; 4096]),
        _ => Resp::ok("").chunked(vec![vec![b'z'; 3000]; 2], Duration::from_millis(1)),
    })
    .await;
    let net = NetClient::new(NetConfig {
        max_buffered_bytes: 4000,
        ..NetConfig::default()
    })
    .unwrap();
    for p in ["/sized", "/chunked"] {
        let err = net
            .fetch(HttpRequest::get(server.url(p)))
            .await
            .unwrap_err();
        assert_eq!(err.kind, FetchErrorKind::TooLarge, "{p}");
    }
}

#[tokio::test]
async fn local_files_and_unsupported_requests() {
    fn picked(value: impl Into<String>) -> HttpRequest {
        let mut req = HttpRequest::get(value);
        req.picked_file = true;
        req
    }
    let dir = std::env::temp_dir().join(format!("iptv-net-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("list.m3u");
    std::fs::write(&path, b"#EXTM3U\n#EXTINF:-1,Local\nhttp://127.0.0.1/1.ts\n").unwrap();
    let net = client();
    let by_url = net
        .fetch(picked(format!("file://{}", path.display())))
        .await
        .unwrap();
    let by_path = net.fetch(picked(path.display().to_string())).await.unwrap();
    assert_eq!(by_url.body, by_path.body);
    assert!(by_url.body.starts_with(b"#EXTM3U"));
    let err = net
        .fetch(picked(dir.join("missing.m3u").display().to_string()))
        .await
        .unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Io);
    assert!(
        !err.to_string().contains("missing.m3u"),
        "no paths in errors"
    );
    std::fs::remove_dir_all(&dir).unwrap();

    let no_files = NetClient::new(NetConfig {
        allow_files: false,
        ..NetConfig::default()
    })
    .unwrap();
    let err = no_files
        .fetch(picked("file:///etc/hostname"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Unsupported);
    let err = net
        .fetch(picked("ftp://files.example.com/x.m3u"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::Unsupported);
    let err = net.fetch(picked("not a url")).await.unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::InvalidRequest);
    let err = net
        .fetch(picked("http://127.0.0.1/").with_header("X-Bad", "line\nbreak"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, FetchErrorKind::InvalidRequest);
    assert!(
        !err.to_string().contains("break"),
        "header values are not quoted"
    );
}

#[test]
fn debug_output_is_redacted() {
    let net = NetClient::new(NetConfig {
        proxy: Some("http://proxyuser:proxy-secret@127.0.0.1:3128".into()),
        ..NetConfig::default()
    })
    .unwrap();
    let shown = format!("{net:?}");
    assert!(!shown.contains("proxy-secret"), "{shown}");
}

#[tokio::test]
async fn streamed_responses_have_byte_and_total_time_budgets() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        loop {
            if stream.write_all(b"1\r\nx\r\n").await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let net = NetClient::new(NetConfig {
        request_timeout: Duration::from_millis(80),
        read_timeout: Duration::from_secs(1),
        ..Default::default()
    })
    .unwrap();
    let mut body = net
        .fetch_stream(HttpRequest::get(format!("http://{address}/synthetic")))
        .await
        .unwrap()
        .body;
    let error = loop {
        match body.next_chunk().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("fixture must not finish"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind, FetchErrorKind::Deadline);
    server.abort();

    let server = TestServer::http(|_| Resp::ok(vec![b'x'; 1000])).await;
    let net = NetClient::new(NetConfig {
        max_streamed_bytes: 16,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        net.fetch(HttpRequest::get(server.url("/synthetic")))
            .await
            .unwrap_err()
            .kind,
        FetchErrorKind::TooLarge
    );
}

#[tokio::test]
async fn content_requests_refuse_private_hosts_and_unpicked_files() {
    let net = client();
    for url in [
        "http://localhost/private",
        "http://127.0.0.1/private",
        "http://[::1]/private",
        "file:///etc/hosts",
    ] {
        let mut request = HttpRequest::get(url);
        request.public_only = true;
        assert!(matches!(
            net.fetch(request).await.unwrap_err().kind,
            FetchErrorKind::InvalidRequest | FetchErrorKind::Unsupported
        ));
    }
    assert_eq!(
        net.fetch(HttpRequest::get("file:///etc/hosts"))
            .await
            .unwrap_err()
            .kind,
        FetchErrorKind::Unsupported
    );
    let target = TestServer::http(|_| Resp::ok("must not be reached")).await;
    let location = target.url("/private");
    let origin = TestServer::http(move |_| Resp::redirect(302, &location)).await;
    assert_eq!(
        net.fetch(HttpRequest::get(origin.url("/picked-source")))
            .await
            .unwrap_err()
            .kind,
        FetchErrorKind::InvalidRequest
    );
    assert!(target.requests().is_empty());
}
