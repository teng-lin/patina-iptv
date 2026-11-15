//! Native VOD metadata and episode links use bounded, cancellable portal sessions.
use crate::common::synth::{SimResponse, StalkerSim};
use crate::common::{FakeFetch, Reply, block_on, connect_error};
use iptv_import::{CancelToken, FailureKind, ImportError, LimitKind, Step, on_demand};
use iptv_providers::model::{ContentKind, MacAddress, StalkerCredentials};
use iptv_providers::stalker::{PortalMode, StalkerPortal, StalkerSession};
use iptv_providers::{HttpRequest, StalkerError, StalkerErrorKind};
use std::{rc::Rc, time::Duration};

fn credentials() -> StalkerCredentials {
    StalkerCredentials {
        portal_url: "http://127.0.0.1:8091/stalker_portal/c/".into(),
        mac: MacAddress::parse("00:1A:79:00:00:73").unwrap(),
        serial: None,
        device_id: None,
        device_id2: None,
        signature: None,
        signature2: None,
        login: None,
        password: None,
    }
}
fn session(mode: PortalMode) -> StalkerSession {
    StalkerSession::new(
        StalkerPortal::from_credentials(
            &credentials(),
            "http://127.0.0.1:8091/stalker_portal/server/load.php",
            mode,
        )
        .unwrap(),
    )
}

#[test]
fn playback_session_discovery_does_not_download_a_catalogue() {
    for simple in [false, true] {
        for metadata_api in [false, true] {
            let sim = Rc::new(StalkerSim {
                simple,
                ..Default::default()
            });
            let provider = sim.clone();
            let fetch = FakeFetch::new(move |_, request| provider.handle(request).into());
            let token = CancelToken::new();
            let result = if metadata_api {
                block_on(on_demand::stalker_session(&fetch, &credentials(), &token))
            } else {
                block_on(iptv_import::stalker_session(&fetch, &credentials(), &token))
            }
            .unwrap();
            assert!(result.is_authorized());
            assert_eq!(
                result.portal().mode(),
                if simple {
                    PortalMode::Simple
                } else {
                    PortalMode::Full
                }
            );
            assert_eq!(sim.handshakes(), u32::from(!simple));
            assert!(
                fetch
                    .requests()
                    .iter()
                    .all(|request| !request.url.contains("get_ordered_list"))
            );
        }
    }
}

#[test]
fn buffered_metadata_preserves_responses_and_has_typed_transport_and_time_failures() {
    let body = r#"{"info":{"name":"Generated metadata"}}"#;
    let fetch = FakeFetch::body(body);
    let request = || HttpRequest::get("https://metadata.example.test/generated");
    let response = block_on(on_demand::detail(&fetch, request(), &CancelToken::new())).unwrap();
    assert_eq!(response.body, body.as_bytes());
    assert_eq!(fetch.requests()[0].timeout, Some(Duration::from_secs(300)));
    let fetch = FakeFetch::new(|_, _| connect_error());
    let error = block_on(on_demand::detail(&fetch, request(), &CancelToken::new())).unwrap_err();
    assert_eq!(error.kind(), FailureKind::Network);
    assert_eq!(error.step(), Some(Step::Items(ContentKind::Movie)));
    let fetch =
        FakeFetch::new(|_, _| Reply::Truncated(SimResponse::ok("generated interrupted response")));
    assert!(matches!(
        block_on(on_demand::detail(&fetch, request(), &CancelToken::new())),
        Err(ImportError::Network { .. })
    ));
    let fetch = FakeFetch::new(|_, _| {
        Reply::Slow(
            Duration::from_secs(301),
            SimResponse::ok("generated late response"),
        )
    });
    assert!(
        matches!(block_on(on_demand::detail(&fetch,request(),&CancelToken::new())),
        Err(ImportError::Limit {error,..}) if error.kind==LimitKind::Elapsed)
    );
}

#[test]
fn metadata_cancellation_stops_before_io_and_while_a_request_is_pending() {
    let token = CancelToken::new();
    token.cancel();
    let fetch = FakeFetch::body("generated");
    assert_eq!(
        block_on(on_demand::detail(
            &fetch,
            HttpRequest::get("https://metadata.example.test/generated"),
            &token
        )),
        Err(ImportError::Cancelled)
    );
    assert!(fetch.requests().is_empty());
    let token = CancelToken::new();
    let pending = token.clone();
    let fetch = FakeFetch::new(move |_, _| Reply::Hang(Some(pending.clone())));
    assert_eq!(
        block_on(on_demand::detail(
            &fetch,
            HttpRequest::get("https://metadata.example.test/generated"),
            &token
        )),
        Err(ImportError::Cancelled)
    );
    assert_eq!(fetch.requests().len(), 1);
}

#[test]
fn series_metadata_authenticates_and_refreshes_an_expired_token_once() {
    let sim = Rc::new(StalkerSim::default());
    let provider = sim.clone();
    let fetch = FakeFetch::new(move |_, request| {
        if request.param("type") == Some("series") {
            assert_eq!(request.param("movie_id"), Some("generated-show"));
            assert_eq!(request.param("season_id"), Some("opaque-season-two"));
            if provider.handshakes() < 2 {
                provider.expired.set(true);
                SimResponse::ok("Authorization failed.").into()
            } else {
                SimResponse::ok(
                    r#"{"js":{"data":[{"id":"generated-episode","name":"Generated episode"}]}}"#,
                )
                .into()
            }
        } else {
            provider.handle(request).into()
        }
    });
    let mut session = session(PortalMode::Full);
    let response = block_on(on_demand::stalker_detail(
        &fetch,
        &mut session,
        |session| session.series_page("generated-show", Some("opaque-season-two"), 1, true),
        &CancelToken::new(),
    ))
    .unwrap();
    assert!(
        String::from_utf8(response.body)
            .unwrap()
            .contains("generated-episode")
    );
    assert!(session.is_authorized());
    assert_eq!(sim.handshakes(), 2);
    assert_eq!(
        fetch
            .requests()
            .iter()
            .filter(|request| request.url.contains("type=series"))
            .count(),
        2
    );
}

#[test]
fn a_persistently_rejected_detail_is_not_retried_indefinitely() {
    let sim = Rc::new(StalkerSim::default());
    let provider = sim.clone();
    let fetch = FakeFetch::new(move |_, request| {
        if request.param("type") == Some("series") {
            SimResponse::ok("Authorization failed.").into()
        } else {
            provider.handle(request).into()
        }
    });
    let mut session = session(PortalMode::Full);
    let error = block_on(on_demand::stalker_detail(
        &fetch,
        &mut session,
        |session| session.series_page("generated-show", None, 1, true),
        &CancelToken::new(),
    ))
    .unwrap_err();
    assert_eq!(error.kind(), FailureKind::Auth);
    assert_eq!(sim.handshakes(), 2);
    assert_eq!(
        fetch
            .requests()
            .iter()
            .filter(|request| request.url.contains("type=series"))
            .count(),
        2
    );
}

#[test]
fn episode_links_keep_opaque_commands_and_episode_coordinates() {
    for episode in [None, Some(4)] {
        let fetch = FakeFetch::new(move |_, request| {
            assert_eq!(request.param("type"), Some("vod"));
            assert_eq!(request.param("action"), Some("create_link"));
            assert_eq!(
                request.param("cmd"),
                Some("ffmpeg /media/generated season & part#4.ts")
            );
            assert_eq!(request.param("series"), episode.map(|_| "4"));
            SimResponse::ok(
                r#"{"js":{"cmd":"ffmpeg /media/generated-resolved.mp4?token=generated"}}"#,
            )
            .into()
        });
        let mut session = session(PortalMode::Simple);
        let url = block_on(on_demand::stalker_episode_link(
            &fetch,
            &mut session,
            "ffmpeg /media/generated season & part#4.ts",
            episode,
            &CancelToken::new(),
        ))
        .unwrap();
        assert_eq!(
            url,
            "http://127.0.0.1:8091/stalker_portal/media/generated-resolved.mp4?token=generated"
        );
        assert_eq!(fetch.requests().len(), 1);
    }
}

#[test]
fn malformed_episode_links_and_invalid_builds_fail_without_playback_fallback() {
    let fetch = FakeFetch::body(r#"{"js":{}}"#);
    let mut portal = session(PortalMode::Simple);
    assert!(matches!(
        block_on(on_demand::stalker_episode_link(
            &fetch,
            &mut portal,
            "/media/generated-original",
            Some(4),
            &CancelToken::new()
        )),
        Err(ImportError::InvalidConfig(_))
    ));
    let fetch = FakeFetch::body("must not be requested");
    let result = block_on(on_demand::stalker_detail(
        &fetch,
        &mut portal,
        |_| Err(StalkerError::new(StalkerErrorKind::NotAuthorized)),
        &CancelToken::new(),
    ));
    assert_eq!(result.unwrap_err().kind(), FailureKind::Auth);
    assert!(fetch.requests().is_empty());
}
