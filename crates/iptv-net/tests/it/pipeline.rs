//! The whole import pipeline (iptv-import over iptv-net) against a local
//! server serving synthetic M3U, XMLTV, Xtream and Stalker endpoints.

use crate::support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iptv_epg::{ChannelKey, Overrides};
use iptv_import::{
    CancelToken, FailureKind, ImportError, ImportOptions, MemoryGuide, NoProgress, Phase, Progress,
    SourceMeta, import_epg, import_source,
};
use iptv_net::NetClient;
use iptv_providers::model::{
    M3uSourceConfig, MacAddress, SourceConfig, StalkerCredentials, XtreamCredentials,
};
use iptv_providers::{Secret, SecretUrl};
use support::synth::{self, NOW, StalkerSim, XtreamSim};
use support::{Resp, TestServer};

const CHANNELS: usize = 600;
const HOUR: i64 = 3600;

/// One server for every source type. `base` is filled in after binding.
async fn server() -> TestServer {
    let base = Arc::new(Mutex::new(String::new()));
    let b2 = base.clone();
    let guide = synth::gzip(
        synth::xmltv(
            &synth::m3u_epg_channels(CHANNELS),
            NOW - 7 * 24 * HOUR,
            NOW + 2 * 24 * HOUR,
            2 * HOUR,
        )
        .as_bytes(),
    );
    let xtream = XtreamSim::default();
    let stalker = Mutex::new(StalkerSim {
        channels: 40,
        expire_on_page: Some(2),
        ..Default::default()
    });
    let s = TestServer::http(move |req| {
        let base = b2.lock().unwrap().clone();
        match req.path.as_str() {
            "/iptv/playlist.m3u" => Resp::ok(synth::m3u(
                CHANNELS,
                24,
                Some(&format!("{base}/iptv/guide.xml.gz")),
                &base,
            )),
            "/iptv/playlist.m3u.gz" => Resp::ok(synth::gzip(
                synth::m3u(CHANNELS, 24, None, &base).as_bytes(),
            )),
            "/iptv/guide.xml.gz" | "/xmltv.php" => Resp::ok(synth::gzip(&guide))
                .header("Content-Type", "application/gzip")
                .header("Content-Encoding", "gzip"),
            "/moved.m3u" => Resp::redirect(302, "/iptv/playlist.m3u"),
            "/player_api.php" => xtream.handle(req).into(),
            p if p.starts_with("/stalker_portal/") => stalker.lock().unwrap().handle(req).into(),
            _ => Resp::status(404),
        }
    })
    .await;
    *base.lock().unwrap() = format!("http://127.0.0.1:{}", s.addr.port());
    s
}

fn m3u(url: String) -> SourceConfig {
    SourceConfig::M3u(M3uSourceConfig {
        location: SecretUrl::new(url),
        user_agent: Some("SyntheticPlayer/1.0".into()),
        referrer: None,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn m3u_playlist_then_its_url_tvg_guide() {
    let server = server().await;
    let net = NetClient::new(iptv_net::NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    let cancel = CancelToken::new();
    let mut opts = ImportOptions::new(NOW);
    opts.allow_private_fixture = true;
    let catalog = import_source(
        &net,
        &m3u(server.url("/moved.m3u")),
        &opts,
        &mut NoProgress,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(catalog.channels.len(), CHANNELS);
    assert_eq!(catalog.groups.len(), 24);
    assert_eq!(
        server.requests()[1].header("user-agent"),
        Some("SyntheticPlayer/1.0")
    );

    let sources = catalog.epg_sources(1);
    assert_eq!(sources.len(), 1, "url-tvg from the header");
    let mut phases = Vec::new();
    let mut on = |p: &Progress| phases.push(p.phase);
    let mut sink = MemoryGuide::new();
    let epg = import_epg(
        &net,
        &sources,
        &catalog.channels,
        &Overrides::new(),
        NOW,
        &mut sink,
        &mut on,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(epg.matches.by_tvg_id, CHANNELS);
    let guide = sink.into_guide(&epg);
    assert_eq!(guide.bound_count(), CHANNELS);
    let stats = epg.sources[0].result.as_ref().unwrap();
    assert_eq!(stats.parse.programmes, (CHANNELS * 9 * 12) as u64);
    let key = ChannelKey::new(catalog.channels[3].id.as_str());
    let now = guide.programme_at(&key, NOW + 1).unwrap();
    assert_eq!((now.start(), now.stop()), (NOW, NOW + 2 * HOUR));
    assert!(phases.contains(&Phase::Matching));

    // A gzip file served as-is.
    let catalog = import_source(
        &net,
        &m3u(server.url("/iptv/playlist.m3u.gz")),
        &opts,
        &mut NoProgress,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(catalog.channels.len(), CHANNELS);
}

#[tokio::test(flavor = "multi_thread")]
async fn xtream_with_vod_and_its_xmltv_export() {
    let server = server().await;
    let net = NetClient::new(iptv_net::NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    let config = SourceConfig::Xtream(XtreamCredentials {
        server_url: format!("{}/player_api.php", server.url("")),
        username: Secret::new(synth::XT_USER),
        password: Secret::new(synth::XT_PASS),
        user_agent: Some("SyntheticXtream/3".into()),
    });
    let cancel = CancelToken::new();
    let catalog = import_source(
        &net,
        &config,
        &ImportOptions::new(NOW).with_vod(true, true),
        &mut NoProgress,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            catalog.channels.len(),
            catalog.vod.len(),
            catalog.series.len()
        ),
        (50, 20, 10)
    );
    assert!(matches!(
        catalog.source,
        SourceMeta::Xtream {
            account: Some(_),
            ..
        }
    ));
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r.header("user-agent") == Some("SyntheticXtream/3"))
    );
    let epg = import_epg(
        &net,
        &catalog.epg_sources(1),
        &catalog.channels,
        &Overrides::new(),
        NOW,
        &mut MemoryGuide::new(),
        &mut NoProgress,
        &cancel,
    )
    .await
    .unwrap();
    // The panel's channels are ch1..ch50; the guide has ch0..ch599.
    assert_eq!(epg.matches.by_tvg_id, 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn stalker_full_portal_with_token_expiry() {
    let server = server().await;
    let net = NetClient::new(iptv_net::NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    let config = SourceConfig::Stalker(StalkerCredentials {
        portal_url: server.url("/stalker_portal/c/"),
        mac: MacAddress::parse("00:1A:79:00:00:99").unwrap(),
        serial: None,
        device_id: None,
        device_id2: None,
        signature: None,
        signature2: None,
        login: None,
        password: None,
    });
    let catalog = import_source(
        &net,
        &config,
        &ImportOptions::new(NOW),
        &mut NoProgress,
        &CancelToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(catalog.channels.len(), 40);
    assert_eq!(catalog.groups.len(), 4);
    let handshakes = server
        .requests()
        .iter()
        .filter(|r| r.param("action") == Some("handshake"))
        .count();
    assert_eq!(handshakes, 2, "re-authenticated once mid-crawl");
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r.header("cookie").is_some_and(|c| c.starts_with("mac=")))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_over_the_network_are_typed() {
    let server = server().await;
    let net = NetClient::new(iptv_net::NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    let run = |cfg: SourceConfig| {
        let net = net.clone();
        async move {
            import_source(
                &net,
                &cfg,
                &ImportOptions::new(NOW),
                &mut NoProgress,
                &CancelToken::new(),
            )
            .await
            .unwrap_err()
        }
    };
    let err = run(m3u(server.url("/nothing.m3u"))).await;
    assert_eq!(err.kind(), FailureKind::Server);
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let err = run(m3u(format!("http://127.0.0.1:{port}/list.m3u"))).await;
    assert_eq!(err.kind(), FailureKind::Network);
    let err = run(SourceConfig::Xtream(XtreamCredentials {
        server_url: server.url(""),
        username: Secret::new(synth::XT_USER),
        password: Secret::new("wrong-synthetic"),
        user_agent: None,
    }))
    .await;
    assert_eq!(err.kind(), FailureKind::Auth);
}

#[tokio::test(flavor = "multi_thread")]
async fn spawned_import_is_send_and_cancels_mid_download() {
    // A playlist that trickles in over ~10 s.
    let body = synth::m3u(2000, 8, None, "http://127.0.0.1:1");
    let pieces: Vec<Vec<u8>> = body.as_bytes().chunks(4096).map(<[u8]>::to_vec).collect();
    let server =
        TestServer::http(move |_| Resp::ok("").chunked(pieces.clone(), Duration::from_millis(200)))
            .await;
    let net = NetClient::new(iptv_net::NetConfig {
        allow_private_fixture: true,
        ..Default::default()
    })
    .unwrap();
    let cancel = CancelToken::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = {
        let (net, cancel, cfg) = (net.clone(), cancel.clone(), m3u(server.url("/slow.m3u")));
        tokio::spawn(async move {
            let mut on = move |p: &Progress| {
                let _ = tx.send(p.bytes);
            };
            import_source(&net, &cfg, &ImportOptions::new(NOW), &mut on, &cancel).await
        })
    };
    let _ = rx.recv().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), ImportError::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "cancel does not wait for the body"
    );
}

#[tokio::test]
async fn malformed_non_ascii_guide_metadata_returns_an_error_without_losing_the_worker() {
    let worker = tokio::spawn(async {
        let result = iptv_import::import_m3u_text(
            "#EXTM3U url-tvg=\"aaaaaaé\"\n#EXTINF:-1,Synthetic\nhttps://media.example.test/live.ts\n".as_bytes(),
            &mut NoProgress, &CancelToken::new(),
        );
        assert!(matches!(result, Err(ImportError::InvalidConfig(_))));
        // The adapter independently remains safe even if a caller bypasses parsing.
        NetClient::with_defaults()
            .unwrap()
            .fetch(iptv_providers::HttpRequest::get("aaaaaaé"))
            .await
    });
    let error = worker
        .await
        .expect("malformed metadata must not panic in a worker")
        .unwrap_err();
    assert_eq!(error.kind, iptv_import::FetchErrorKind::InvalidRequest);
}
