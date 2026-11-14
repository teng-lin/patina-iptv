//! Stalker/Ministra client tests against synthetic fixtures.

use crate::common;

use common::{fixture, for_each_type_mutation};
use iptv_providers::model::{
    CatchUpKind, ContentKind, StalkerCredentials, StalkerLinkFlags, StreamLocator,
};
use iptv_providers::stalker::*;
use iptv_providers::{HttpRequest, HttpResponse, ProviderError, Secret, StalkerErrorKind};

const ENDPOINT: &str = "http://portal.example.com/stalker_portal/server/load.php";
const MAC: &str = "00:1A:79:00:00:01";
/// SHA-1("00:1A:79:00:00:01"), uppercase hex.
const PREHASH: &str = "D0840DF5DE82186EDD17121FE132AE02056D561F";

fn ok(name: &str) -> HttpResponse {
    HttpResponse::ok(fixture("stalker", name))
}

fn portal(mode: PortalMode) -> StalkerPortal {
    StalkerPortal::new(ENDPOINT, MacAddress::parse(MAC).unwrap(), mode).unwrap()
}

fn send(step: AuthStep) -> HttpRequest {
    match step {
        AuthStep::Send(r) => r,
        other => panic!("expected a request, got {other:?}"),
    }
}

fn query(req: &HttpRequest) -> Vec<(String, String)> {
    url::Url::parse(&req.url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn param(req: &HttpRequest, key: &str) -> Option<String> {
    query(req)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// Runs handshake + profile_ok.
fn authorized(portal: StalkerPortal) -> StalkerSession {
    let mut s = StalkerSession::new(portal);
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    send(s.poll());
    s.on_auth_response(&ok("profile_ok.json")).unwrap();
    assert!(matches!(s.poll(), AuthStep::Ready));
    s
}

#[test]
fn mac_addresses() {
    for typed in [
        "00:1a:79:00:00:01",
        "00-1A-79-00-00-01",
        "001a.7900.0001",
        " 00 1a 79 00 00 01 ",
        "001A79000001",
    ] {
        assert_eq!(MacAddress::parse(typed).unwrap().expose(), MAC, "{typed}");
    }
    assert!(MacAddress::parse("00:1A:79:00:00").is_none());
    assert!(MacAddress::parse("zz:1A:79:00:00:01").is_none());
    let mac = MacAddress::parse(MAC).unwrap();
    assert!(mac.has_infomir_oui());
    assert!(
        !MacAddress::parse("02:00:00:00:00:01")
            .unwrap()
            .has_infomir_oui()
    );
    assert_eq!(format!("{mac:?}"), "MacAddress(00:1A:79:**:**:**)");
    assert_eq!(
        MacAddress::from_stored("00-1a-79-00-00-01").expose(),
        "00-1a-79-00-00-01",
        "stored MACs are not rewritten"
    );
    assert_eq!(
        format!("{:?}", MacAddress::from_stored("not a mac")),
        "MacAddress(***)"
    );
}

#[test]
fn endpoint_discovery_candidates() {
    let c = |s: &str| endpoint_candidates(s).unwrap();
    assert_eq!(
        c("http://portal.example.com/c/"),
        vec![
            "http://portal.example.com/portal.php",
            "http://portal.example.com/server/load.php",
            "http://portal.example.com/stalker_portal/server/load.php"
        ]
    );
    assert_eq!(
        c("http://portal.example.com/stalker_portal/c/index.html"),
        vec![
            "http://portal.example.com/stalker_portal/portal.php",
            "http://portal.example.com/stalker_portal/server/load.php"
        ]
    );
    assert_eq!(
        c("http://portal.example.com:8080/server/load.php?junk=1#x"),
        vec![
            "http://portal.example.com:8080/server/load.php",
            "http://portal.example.com:8080/portal.php",
            "http://portal.example.com:8080/stalker_portal/server/load.php"
        ]
    );
    assert_eq!(
        c("https://portal.example.com/tenant-a/portal.php"),
        vec![
            "https://portal.example.com/tenant-a/portal.php",
            "https://portal.example.com/tenant-a/server/load.php",
            "https://portal.example.com/tenant-a/stalker_portal/server/load.php"
        ]
    );
    assert_eq!(c("http://portal.example.com").len(), 3);
    assert!(
        endpoint_candidates("portal.example.com/c/").is_err(),
        "explicit scheme required"
    );
    assert!(endpoint_candidates("ftp://portal.example.com/c/").is_err());
    assert!(is_full_portal_url(ENDPOINT));
    assert!(is_full_portal_url(
        "http://portal.example.com/server/load.php"
    ));
    assert!(!is_full_portal_url("http://portal.example.com/portal.php"));
}

#[test]
fn cmd_reference_encoding() {
    assert_eq!(
        encode_cmd("ffmpeg http://a.example/x?y=1&z=%3A #;ü+"),
        "ffmpeg%20http://a.example/x?y=1%26z=%3A%20%23%3B%C3%BC+"
    );
    assert_eq!(encode_cmd("/media/file_1.mpg"), "/media/file_1.mpg");
    assert_eq!(encode_cmd("a'b\"c"), "a%27b%22c");
}

#[test]
fn handshake_request_shape() {
    let mut s = StalkerSession::new(
        portal(PortalMode::Full)
            .with_language("de")
            .with_timezone("Europe/Example"),
    );
    let hs = send(s.poll());
    assert!(hs.url.starts_with(ENDPOINT));
    assert_eq!(
        query(&hs),
        vec![
            ("type".into(), "stb".into()),
            ("action".into(), "handshake".into()),
            ("token".into(), String::new()),
            ("prehash".into(), PREHASH.into()),
            ("JsHttpRequest".into(), "1-xml".into())
        ]
    );
    assert_eq!(
        hs.header("Cookie"),
        Some("mac=00:1A:79:00:00:01; stb_lang=de; timezone=Europe/Example")
    );
    assert_eq!(hs.header("User-Agent"), Some(MAG_USER_AGENT));
    assert_eq!(hs.header("X-User-Agent"), Some(MAG_USER_AGENT));
    assert_eq!(hs.header("Authorization"), None);
    assert!(
        matches!(s.poll(), AuthStep::Waiting),
        "no duplicate while outstanding"
    );
    assert_eq!(*s.state(), SessionState::Authenticating);
}

#[test]
fn full_login_flow_and_api_requests() {
    let p = portal(PortalMode::Full)
        .with_serial(Some(Secret::new("SYNTHSERIAL01")))
        .with_device_ids(Some(Secret::new("DEV1-SYNTH")), None)
        .with_signatures(None, Some(Secret::new(" ")));
    let mut s = StalkerSession::new(p);
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    let profile = send(s.poll());
    assert_eq!(
        profile.header("Authorization"),
        Some("Bearer SYNTHTOKEN0001")
    );
    assert_eq!(profile.header("SN"), Some("SYNTHSERIAL01"));
    assert_eq!(param(&profile, "action").as_deref(), Some("get_profile"));
    assert_eq!(param(&profile, "stb_type").as_deref(), Some("MAG250"));
    assert_eq!(param(&profile, "sn").as_deref(), Some("SYNTHSERIAL01"));
    assert_eq!(param(&profile, "device_id").as_deref(), Some("DEV1-SYNTH"));
    assert_eq!(
        param(&profile, "device_id2"),
        None,
        "never copied from device_id"
    );
    assert_eq!(param(&profile, "signature2"), None, "blank stays absent");
    assert_eq!(param(&profile, "auth_second_step").as_deref(), Some("0"));
    assert_eq!(param(&profile, "not_valid_token").as_deref(), Some("0"));
    assert_eq!(param(&profile, "prehash").as_deref(), Some(PREHASH));
    let metrics: serde_json::Value =
        serde_json::from_str(&param(&profile, "metrics").unwrap()).unwrap();
    assert_eq!(metrics["mac"], MAC);
    assert_eq!(metrics["sn"], "SYNTHSERIAL01");
    assert_eq!(
        metrics["random"],
        "0123456789abcdef0123456789abcdef01234567"
    );

    s.on_auth_response(&ok("profile_ok.json")).unwrap();
    assert!(s.is_authorized());
    assert_eq!(s.token().map(Secret::expose), Some("SYNTHTOKEN0001"));
    assert_eq!(s.watchdog_interval_secs(), 60);
    let prof = s.profile().unwrap();
    assert_eq!(
        (prof.timeslot, prof.account_name.as_deref()),
        (Some(12), Some("Synthetic Viewer"))
    );

    let genres = s.genres().unwrap();
    assert_eq!(
        genres.header("Authorization"),
        Some("Bearer SYNTHTOKEN0001")
    );
    assert_eq!(param(&genres, "sn"), None, "sn only on get_profile");
    let page = s.channels_page("10", 3).unwrap();
    assert_eq!(
        (
            param(&page, "genre").as_deref(),
            param(&page, "p").as_deref()
        ),
        (Some("10"), Some("3"))
    );
    assert_eq!(
        param(&s.all_channels().unwrap(), "action").as_deref(),
        Some("get_all_channels")
    );
    assert_eq!(
        param(&s.vod_categories().unwrap(), "type").as_deref(),
        Some("vod")
    );
    assert_eq!(
        param(&s.series_categories().unwrap(), "type").as_deref(),
        Some("series")
    );
    assert_eq!(
        param(&s.vod_page("30", 0).unwrap(), "p").as_deref(),
        Some("1")
    );
    assert_eq!(
        param(&s.short_epg("501", 4).unwrap(), "ch_id").as_deref(),
        Some("501")
    );
    let wd = s.watchdog(true).unwrap();
    assert_eq!(
        (param(&wd, "type").as_deref(), param(&wd, "init").as_deref()),
        (Some("watchdog"), Some("1"))
    );
    let link = s
        .create_link("ffmpeg http://x.example/a&b", false, None)
        .unwrap();
    assert!(
        link.url.contains("cmd=ffmpeg%20http://x.example/a%26b&"),
        "{}",
        link.url
    );
    assert_eq!(param(&link, "type").as_deref(), Some("itv"));
    let ep = s.create_link("/media/9002.mpg", true, Some(3)).unwrap();
    assert_eq!(
        (
            param(&ep, "type").as_deref(),
            param(&ep, "series").as_deref()
        ),
        (Some("vod"), Some("3"))
    );

    let debug = format!("{s:?} {genres:?} {profile:?}");
    for secret in [
        "SYNTHTOKEN0001",
        "SYNTHSERIAL01",
        "DEV1-SYNTH",
        "00:00:01",
        "00%3A00%3A01",
    ] {
        assert!(!debug.contains(secret), "{secret} leaked: {debug}");
    }
}

#[test]
fn legacy_default_serial_is_absent() {
    let p = portal(PortalMode::Full).with_serial(Some(Secret::new("BEDACD4569BAF")));
    let mut s = StalkerSession::new(p);
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    let profile = send(s.poll());
    assert_eq!((profile.header("SN"), param(&profile, "sn")), (None, None));
}

#[test]
fn resumed_token_skips_profile_only_when_safe() {
    // Unchanged token + known cadence: no profile round trip.
    let mut s = StalkerSession::resume(
        portal(PortalMode::Full),
        Secret::new("SYNTHTOKEN0001"),
        Some(90),
    );
    let hs = send(s.poll());
    assert_eq!(param(&hs, "token").as_deref(), Some("SYNTHTOKEN0001"));
    assert!(!format!("{hs:?}").contains("SYNTHTOKEN0001"));
    s.on_auth_response(&ok("handshake.json")).unwrap();
    assert!(matches!(s.poll(), AuthStep::Ready));
    assert_eq!(s.watchdog_interval_secs(), 90);

    // not_valid vetoes the shortcut.
    let mut s = StalkerSession::resume(
        portal(PortalMode::Full),
        Secret::new("SYNTHTOKEN0001"),
        Some(90),
    );
    send(s.poll());
    s.on_auth_response(&HttpResponse::ok(
        r#"{"js":{"token":"SYNTHTOKEN0001","not_valid":"1"}}"#,
    ))
    .unwrap();
    let profile = send(s.poll());
    assert_eq!(param(&profile, "not_valid_token").as_deref(), Some("1"));

    // Unknown cadence: profile once to learn it.
    let mut s = StalkerSession::resume(
        portal(PortalMode::Full),
        Secret::new("SYNTHTOKEN0001"),
        None,
    );
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    assert_eq!(
        param(&send(s.poll()), "action").as_deref(),
        Some("get_profile")
    );

    // A different token: profile.
    let mut s = StalkerSession::resume(portal(PortalMode::Full), Secret::new("OLDTOKEN"), Some(90));
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    assert_eq!(
        param(&send(s.poll()), "action").as_deref(),
        Some("get_profile")
    );
}

#[test]
fn status_two_runs_do_auth_then_second_step() {
    let p = portal(PortalMode::Full)
        .with_login(Some(Secret::new("viewer")), Some(Secret::new("pw-synth")));
    let mut s = StalkerSession::new(p);
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    send(s.poll());
    s.on_auth_response(&ok("profile_login.json")).unwrap();
    let auth = send(s.poll());
    assert_eq!(
        (
            param(&auth, "action").as_deref(),
            param(&auth, "login").as_deref()
        ),
        (Some("do_auth"), Some("viewer"))
    );
    assert!(!format!("{auth:?}").contains("pw-synth"));
    s.on_auth_response(&HttpResponse::ok(r#"{"js":true}"#))
        .unwrap();
    let second = send(s.poll());
    assert_eq!(param(&second, "auth_second_step").as_deref(), Some("1"));
    s.on_auth_response(&ok("profile_ok.json")).unwrap();
    assert!(s.is_authorized());
}

#[test]
fn login_refusals_keep_portal_text() {
    let login = || {
        portal(PortalMode::Full).with_login(Some(Secret::new("viewer")), Some(Secret::new("pw")))
    };
    let to_login = |p: StalkerPortal| {
        let mut s = StalkerSession::new(p);
        send(s.poll());
        s.on_auth_response(&ok("handshake.json")).unwrap();
        send(s.poll());
        let r = s.on_auth_response(&ok("profile_login.json"));
        (s, r)
    };
    // No credentials configured.
    let (s, r) = to_login(portal(PortalMode::Full));
    let err = r.unwrap_err();
    assert_eq!(err.kind, StalkerErrorKind::LoginRequired);
    assert_eq!(
        err.portal_text.as_deref(),
        Some("Please log in with your synthetic account")
    );
    assert!(matches!(s.state(), SessionState::Failed(_)));

    // do_auth answers false: the profile's text explains it.
    let (mut s, r) = to_login(login());
    r.unwrap();
    send(s.poll());
    let err = s
        .on_auth_response(&HttpResponse::ok(r#"{"js":false}"#))
        .unwrap_err();
    assert_eq!(
        (err.kind, err.portal_text.as_deref()),
        (
            StalkerErrorKind::LoginRejected,
            Some("Please log in with your synthetic account")
        )
    );

    // Status 2 again on the second step.
    let (mut s, r) = to_login(login());
    r.unwrap();
    send(s.poll());
    s.on_auth_response(&HttpResponse::ok(r#"{"js":true}"#))
        .unwrap();
    send(s.poll());
    let err = s
        .on_auth_response(&HttpResponse::ok(
            r#"{"js":{"status":2,"msg":"Wrong password"}}"#,
        ))
        .unwrap_err();
    assert_eq!(
        (err.kind, err.portal_text.as_deref()),
        (StalkerErrorKind::LoginRejected, Some("Wrong password"))
    );
}

fn profile_error(body: &str) -> iptv_providers::StalkerError {
    let mut s = StalkerSession::new(portal(PortalMode::Full));
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    send(s.poll());
    let err = s
        .on_auth_response(&HttpResponse::ok(body.to_owned()))
        .unwrap_err();
    assert!(matches!(s.poll(), AuthStep::Failed(_)));
    err
}

#[test]
fn profile_refusals() {
    let blocked = profile_error(&fixture("stalker", "profile_blocked.json"));
    assert_eq!(blocked.kind, StalkerErrorKind::Blocked);
    assert_eq!(
        blocked.portal_text.as_deref(),
        Some("Account disabled Contact the synthetic operator")
    );
    let conflict = profile_error(&fixture("stalker", "profile_conflict.json"));
    assert_eq!(conflict.kind, StalkerErrorKind::DeviceConflict);
    assert!(conflict.portal_text.unwrap().contains("device_id mismatch"));
    assert_eq!(
        profile_error(r#"{"js":{"status":1}}"#).kind,
        StalkerErrorKind::Blocked,
        "bare status 1 is a refusal"
    );
    assert_eq!(
        profile_error(r#"{"js":{"block_msg":"Suspended"}}"#).kind,
        StalkerErrorKind::Blocked,
        "refusal text without status"
    );
    assert_eq!(
        profile_error(r#"{"js":[]}"#).kind,
        StalkerErrorKind::Blocked
    );
    assert_eq!(
        profile_error("Authorization failed. 3").kind,
        StalkerErrorKind::AuthFailed
    );
}

#[test]
fn handshake_failures() {
    let mut s = StalkerSession::new(portal(PortalMode::Full));
    send(s.poll());
    assert_eq!(
        s.on_auth_response(&HttpResponse::ok(r#"{"js":{}}"#))
            .unwrap_err()
            .kind,
        StalkerErrorKind::NoToken
    );
    s.reset();
    assert_eq!(*s.state(), SessionState::NeedsHandshake);
    send(s.poll());
    let err = s
        .on_auth_response(&HttpResponse::ok("Access denied."))
        .unwrap_err();
    assert_eq!(
        (err.kind, err.portal_text.as_deref()),
        (StalkerErrorKind::Blocked, Some("Access denied."))
    );
    s.reset();
    send(s.poll());
    assert_eq!(
        s.on_auth_response(&HttpResponse::new(500, ""))
            .unwrap_err()
            .kind,
        StalkerErrorKind::Portal
    );
}

#[test]
fn token_expiry_reauthenticates_once_then_fails() {
    let mut s = authorized(portal(PortalMode::Full));
    assert_eq!(
        s.check_api_response(&HttpResponse::ok("Authorization failed.")),
        Err(ApiError::Reauthenticate)
    );
    assert_eq!(*s.state(), SessionState::TokenExpired);
    assert_eq!(
        s.genres().unwrap_err().kind,
        StalkerErrorKind::NotAuthorized
    );
    let hs = send(s.poll());
    assert_eq!(
        param(&hs, "token").as_deref(),
        Some(""),
        "a rejected token is not re-presented"
    );
    s.on_auth_response(&ok("handshake.json")).unwrap();
    send(s.poll());
    s.on_auth_response(&ok("profile_ok.json")).unwrap();
    // A success resets the counter…
    assert!(s.check_api_response(&ok("genres.json")).is_ok());
    assert_eq!(
        s.check_api_response(&HttpResponse::ok(r#"{"js":{"error":"Invalid token"}}"#)),
        Err(ApiError::Reauthenticate)
    );
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    send(s.poll());
    s.on_auth_response(&ok("profile_ok.json")).unwrap();
    // …but a fresh session rejected again without any success is a loop.
    match s.check_api_response(&HttpResponse::ok("Authorization failed.")) {
        Err(ApiError::Provider(ProviderError::Stalker(e))) => {
            assert_eq!(e.kind, StalkerErrorKind::AuthLoop)
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(s.poll(), AuthStep::Failed(_)));
}

#[test]
fn simple_portals_skip_the_lifecycle() {
    let p = portal(PortalMode::Simple).with_serial(Some(Secret::new("SYNTHSERIAL01")));
    let mut s = StalkerSession::new(p);
    assert!(matches!(s.poll(), AuthStep::Ready));
    let req = s.genres().unwrap();
    assert_eq!(
        (req.header("Authorization"), req.header("SN")),
        (None, None)
    );
    assert!(
        req.header("Cookie")
            .unwrap()
            .starts_with("mac=00:1A:79:00:00:01")
    );
    match s.check_api_response(&HttpResponse::ok("Authorization failed.")) {
        Err(ApiError::Provider(ProviderError::Stalker(e))) => {
            assert_eq!(e.kind, StalkerErrorKind::AuthFailed)
        }
        other => panic!("{other:?}"),
    }
    assert!(
        s.is_authorized(),
        "re-auth cannot help a token-free panel; rediscover instead"
    );
}

#[test]
fn api_errors() {
    let mut s = authorized(portal(PortalMode::Full));
    match s.check_api_response(&ok("create_link_error.json")) {
        Err(ApiError::Provider(ProviderError::Stalker(e))) => {
            assert_eq!(
                (e.kind, e.portal_text.as_deref()),
                (StalkerErrorKind::Portal, Some("nothing_to_play"))
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        s.check_api_response(&HttpResponse::new(404, "")),
        Err(ApiError::Provider(ProviderError::HttpStatus(404)))
    ));
    assert!(matches!(
        s.check_api_response(&HttpResponse::ok("<html>proxy</html>")),
        Err(ApiError::Provider(ProviderError::InvalidBody(_)))
    ));
    assert!(s.is_authorized());
}

#[test]
fn discovery_probe_classification() {
    let p = portal(PortalMode::Simple);
    let probe = p.probe_request();
    assert_eq!(
        (
            param(&probe, "type").as_deref(),
            param(&probe, "action").as_deref()
        ),
        (Some("itv"), Some("get_genres"))
    );
    assert_eq!(classify_probe(&ok("genres.json")), ProbeResult::Data);
    assert_eq!(
        classify_probe(&HttpResponse::ok(r#"{"js":{"data":[]}}"#)),
        ProbeResult::Data
    );
    assert_eq!(
        classify_probe(&HttpResponse::ok(r#"{"js":{"data":[],"error":"x"}}"#)),
        ProbeResult::NotAPortal
    );
    assert_eq!(
        classify_probe(&HttpResponse::ok("Authorization failed.")),
        ProbeResult::AuthRequired
    );
    assert_eq!(
        classify_probe(&HttpResponse::ok(r#"{"js":{"error":"Unauthorized"}}"#)),
        ProbeResult::AuthRequired
    );
    assert_eq!(
        classify_probe(&HttpResponse::new(401, "")),
        ProbeResult::AuthRequired
    );
    assert_eq!(
        classify_probe(&HttpResponse::new(404, "")),
        ProbeResult::NotAPortal
    );
    assert_eq!(
        classify_probe(&HttpResponse::ok("<html><body>Access denied</body></html>")),
        ProbeResult::NotAPortal
    );
}

#[test]
fn genres_and_channels() {
    let g = parse_genres(&ok("genres.json"), ContentKind::Live).unwrap();
    assert_eq!(
        g.items.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        vec!["Portal News", "Late Night"]
    );
    assert_eq!(g.items[0].id.as_str(), "st:live-group:10");
    assert!(g.items[1].adult);
    assert_eq!(g.skipped, 1);

    let page = parse_channel_page(&ok("channels_page.json")).unwrap();
    assert_eq!((page.total_items, page.max_page_items), (Some(5), Some(2)));
    assert_eq!(page.items.items.len(), 3);
    assert_eq!(page.items.skipped, 2);
    let news = &page.items.items[0];
    assert_eq!(
        (news.id.as_str(), news.number, news.epg_id.as_deref()),
        ("st:live:501", Some(1), Some("pnews.example"))
    );
    let cu = news.catch_up.as_ref().unwrap();
    assert_eq!(
        (&cu.kind, cu.days),
        (&CatchUpKind::Stalker, Some(3)),
        "72 hours = 3 days"
    );
    assert_eq!(
        news.stream,
        StreamLocator::Stalker {
            cmd: "ffrt http://localhost/ch/501_".into(),
            link: StalkerLinkFlags {
                use_http_tmp_link: Some(true),
                use_load_balancing: Some(false)
            }
        }
    );
    assert!(
        matches!(&page.items.items[2].stream, StreamLocator::Stalker { cmd, link } if cmd == "ffmpeg http://cdn.example.net/504.ts" && link.use_http_tmp_link.is_none())
    );

    let all = parse_all_channels(&ok("all_channels.json")).unwrap();
    assert_eq!(all.items.len(), 2, "de-duplicated by id");
    assert_eq!(all.skipped, 1);
}

#[test]
fn vod_rows() {
    let page = parse_vod_page(&ok("vod_page.json")).unwrap();
    let v = &page.items.items;
    assert_eq!(v.len(), 4);
    assert_eq!((v[0].id.as_str(), v[0].is_series), ("st:movie:9001", false));
    assert_eq!(
        (v[0].year, v[0].rating_x10, v[0].duration_secs, v[0].added),
        (Some(2016), Some(69), Some(5700), Some(1_700_000_000))
    );
    assert_eq!(
        (v[1].id.as_str(), v[1].is_series, v[1].rating_x10),
        ("st:series:9002", true, Some(72))
    );
    assert!(v[2].is_series);
    assert!(!v[3].is_series, "only true / 1 / \"1\" mark a series");
}

#[test]
fn crawl_with_real_pages() {
    let mut crawl = PagedCrawl::new(30_000);
    let mut pages = 0;
    while let Some(p) = crawl.next_page() {
        pages += 1;
        assert_eq!(p, pages);
        // The synthetic portal ignores `p` and repeats the same page.
        crawl.push(parse_channel_page(&ok("channels_page.json")).unwrap());
    }
    assert_eq!(pages, 2);
    assert_eq!(crawl.skipped, 4);
    assert_eq!(crawl.into_items().len(), 3);
}

#[test]
fn link_resolution() {
    let s = authorized(portal(PortalMode::Full));
    let p = s.portal();
    assert_eq!(p.base_url(), "http://portal.example.com/stalker_portal");
    assert_eq!(
        parse_create_link(&ok("create_link_absolute.json"), p, "x").unwrap(),
        "http://media.example.com/ch/501?token=tmp-synthetic"
    );
    assert_eq!(
        parse_create_link(&ok("create_link_relative.json"), p, "x").unwrap(),
        "http://portal.example.com/stalker_portal/media/file_9001.mpg?play_token=tmp"
    );
    assert_eq!(
        parse_create_link(
            &ok("create_link_query.json"),
            p,
            "ffmpeg http://cdn.example.net/v.mp4"
        )
        .unwrap(),
        "http://cdn.example.net/v.mp4?play_token=tmp-synthetic"
    );
    assert_eq!(
        parse_create_link(&ok("create_link_query.json"), p, "/media/2.mpg").unwrap(),
        "http://portal.example.com/stalker_portal/media/2.mpg?play_token=tmp-synthetic"
    );
    assert!(matches!(
        parse_create_link(&ok("create_link_error.json"), p, "x"),
        Err(ProviderError::Stalker(_))
    ));
    assert!(parse_create_link(&HttpResponse::ok(r#"{"js":{"cmd":"ffrt4://x"}}"#), p, "x").is_err());
    assert!(parse_create_link(&HttpResponse::ok("Authorization failed."), p, "x").is_err());

    let page = parse_channel_page(&ok("channels_page.json")).unwrap();
    match s
        .resolve_stream(&page.items.items[1].stream, false)
        .unwrap()
    {
        Resolve::Play(url) => assert_eq!(url, "http://cdn.example.net/live/502.m3u8"),
        other => panic!("{other:?}"),
    }
    match s
        .resolve_stream(&page.items.items[0].stream, false)
        .unwrap()
    {
        Resolve::CreateLink(req) => assert_eq!(
            param(&req, "cmd").as_deref(),
            Some("ffrt http://localhost/ch/501_")
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn playback_headers_never_leak_to_foreign_hosts() {
    let s = authorized(portal(PortalMode::Full).with_serial(Some(Secret::new("SYNTHSERIAL01"))));
    let own = s.playback_headers("http://portal.example.com:8081/media/1.mpg?token=t");
    let names: Vec<&str> = own.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "User-Agent",
            "Cookie",
            "X-User-Agent",
            "Authorization",
            "SN",
            "Referer"
        ]
    );
    assert!(own.iter().all(|(n, _)| !n.eq_ignore_ascii_case("range")));
    for url in [
        "http://cdn.example.net/live/1.m3u8",
        "http://portal.example.com.evil.example/x",
    ] {
        assert_eq!(
            s.playback_headers(url),
            vec![("User-Agent".to_owned(), MAG_USER_AGENT.to_owned())],
            "{url}"
        );
    }
    let https = authorized(
        StalkerPortal::new(
            "https://portal.example.com/portal.php",
            MacAddress::parse(MAC).unwrap(),
            PortalMode::Full,
        )
        .unwrap(),
    );
    assert_eq!(
        https.playback_headers("http://portal.example.com/x").len(),
        1,
        "https→http downgrade is foreign"
    );
}

#[test]
fn only_a_private_portal_unlocks_private_streams_and_only_on_its_own_host() {
    let session = |endpoint: &str| {
        StalkerSession::new(
            StalkerPortal::new(endpoint, MacAddress::parse(MAC).unwrap(), PortalMode::Full)
                .unwrap(),
        )
    };
    // A LAN installation: the portal's own host may serve the stream, on
    // any port.
    for endpoint in [
        "http://192.168.50.10/stalker_portal/server/load.php",
        "http://10.0.0.7:8080/portal.php",
        "http://portal.local/portal.php",
        "http://mediabox/portal.php",
        "http://[fd00::10]/portal.php",
    ] {
        let lan = session(endpoint);
        let host = url::Url::parse(endpoint).unwrap();
        let host = host.host_str().unwrap();
        for stream in [
            format!("http://{host}/live/1.ts?play_token=t"),
            format!("http://{host}:8888/live/1.m3u8"),
        ] {
            assert!(!lan.stream_must_be_public(&stream), "{endpoint} {stream}");
        }
        // Not another machine on the LAN, a public host, or a non-URL.
        for stream in [
            "http://192.168.50.11/live/1.ts",
            "http://127.0.0.1:8080/admin",
            "http://router.local/reboot",
            "http://cdn.example.net/live/1.m3u8",
            "/live/1.ts",
        ] {
            assert!(lan.stream_must_be_public(stream), "{endpoint} {stream}");
        }
    }
    // A portal on a public address never unlocks a private destination,
    // and its own host stays under the public-address checks (DNS included).
    let public = session(ENDPOINT);
    for stream in [
        "http://192.168.50.10/live/1.ts",
        "http://127.0.0.1/live/1.ts",
        "http://portal.local/live/1.ts",
        "http://portal.example.com/live/1.ts",
        "http://cdn.example.net/live/1.m3u8",
    ] {
        assert!(public.stream_must_be_public(stream), "{stream}");
    }
    // A downgrade from an HTTPS portal is not the portal's own stream.
    let secure = session("https://192.168.50.10/portal.php");
    assert!(!secure.stream_must_be_public("https://192.168.50.10/live/1.ts"));
    assert!(secure.stream_must_be_public("http://192.168.50.10/live/1.ts"));
}

#[test]
fn from_credentials_and_debug_redaction() {
    let creds = StalkerCredentials {
        portal_url: "http://portal.example.com/c/".into(),
        mac: MacAddress::parse(MAC).unwrap(),
        serial: Some(Secret::new("  SYNTHSERIAL01 ")),
        device_id: Some(Secret::new("")),
        device_id2: None,
        signature: None,
        signature2: None,
        login: Some(Secret::new("viewer")),
        password: Some(Secret::new("pw-synth")),
    };
    let shown = format!("{creds:?}");
    for secret in ["SYNTHSERIAL01", "pw-synth", "viewer", "00:00:01"] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
    let endpoint = &endpoint_candidates(&creds.portal_url).unwrap()[1];
    let p = StalkerPortal::from_credentials(&creds, endpoint, PortalMode::Full).unwrap();
    assert!(!format!("{p:?}").contains("pw-synth"));
    let mut s = StalkerSession::new(p);
    send(s.poll());
    s.on_auth_response(&ok("handshake.json")).unwrap();
    let profile = send(s.poll());
    assert_eq!(profile.header("SN"), Some("SYNTHSERIAL01"), "trimmed");
    assert_eq!(
        param(&profile, "device_id"),
        None,
        "blank device id stays absent"
    );
}

#[test]
fn malformed_fields_never_panic() {
    let p = portal(PortalMode::Full);
    for name in [
        "handshake.json",
        "profile_ok.json",
        "profile_conflict.json",
        "genres.json",
        "channels_page.json",
        "all_channels.json",
        "vod_page.json",
        "create_link_relative.json",
    ] {
        for_each_type_mutation(&fixture("stalker", name), |body| {
            let r = HttpResponse::ok(body);
            let _ = parse_genres(&r, ContentKind::Live);
            let _ = parse_channel_page(&r);
            let _ = parse_all_channels(&r);
            let _ = parse_vod_page(&r);
            let _ = parse_create_link(&r, &p, "/media/1.mpg");
            let _ = classify_probe(&r);
            let mut s = StalkerSession::new(p.clone());
            while let AuthStep::Send(_) = s.poll() {
                if s.on_auth_response(&r).is_err() {
                    break;
                }
            }
            let _ = s.check_api_response(&r);
        });
    }
}

#[test]
fn malformed_optional_commands_and_duplicate_keys_keep_valid_page_rows() {
    let response = HttpResponse::ok(
        "{\"js\":{\"data\":[null,17,[],\"ignored\",\
         {\"id\":1,\"name\":\"Old\",\"name\":\"Synthetic\",\"cmd\":\"http://media.example.test/1\",\"cmds\":false},\
         {\"id\":2,\"name\":\"Synthetic 2\",\"cmds\":[null,17,{\"url\":\"http://media.example.test/2\"}]}]}}",
    );
    let page = iptv_providers::stalker::parse_channel_page(&response).unwrap();
    assert_eq!(page.items.items.len(), 2);
    assert_eq!(page.items.skipped, 4);
    assert_eq!(page.items.items[0].name, "Synthetic");
}
