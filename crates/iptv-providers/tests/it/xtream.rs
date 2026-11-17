//! Xtream client tests against synthetic fixtures.

use crate::common;

use common::{fixture, for_each_type_mutation};
use iptv_providers::model::{CatchUpKind, ContentKind, StreamLocator, WarningLocation};
use iptv_providers::xtream::*;
use iptv_providers::{HttpResponse, ProviderError};

const NOW: i64 = 1_700_000_000;

fn client() -> XtreamClient {
    XtreamClient::new("http://tv.example.com:8080", "demo", "s3cret").unwrap()
}

fn ok(name: &str) -> HttpResponse {
    HttpResponse::ok(fixture("xtream", name))
}

#[test]
fn server_url_normalization() {
    let n = |s: &str| normalize_server_url(s);
    assert_eq!(
        n("  http://tv.example.com:8080///  ").unwrap(),
        "http://tv.example.com:8080"
    );
    assert_eq!(
        n("http://tv.example.com/player_api.php?username=a&password=b").unwrap(),
        "http://tv.example.com"
    );
    assert_eq!(
        n("http://tv.example.com/PANEL_API.PHP").unwrap(),
        "http://tv.example.com"
    );
    assert_eq!(
        n("http://tv.example.com/get.php?username=a&password=b&type=m3u").unwrap(),
        "http://tv.example.com"
    );
    assert_eq!(
        n("https://example.test/panel/player_api.php?x=1").unwrap(),
        "https://example.test/panel"
    );
    assert_eq!(
        n("HTTPS://Example.Test:443/sub/").unwrap(),
        "https://example.test/sub"
    );
    assert!(matches!(
        n("ftp://tv.example.com"),
        Err(ProviderError::InvalidConfig(_))
    ));
    assert!(matches!(
        n("http://u:p@tv.example.com"),
        Err(ProviderError::InvalidConfig(_))
    ));
    assert!(matches!(n("   "), Err(ProviderError::InvalidConfig(_))));
    assert!(matches!(
        n("tv.example.com"),
        Err(ProviderError::InvalidConfig(_))
    ));
}

#[test]
fn credentials_from_full_urls() {
    let (u, p) = credentials_from_url(
        "http://tv.example.com/get.php?username=%20demo%20&password=pw&type=m3u_plus",
    )
    .unwrap();
    assert_eq!((u.as_str(), p.expose()), ("demo", "pw"));
    assert!(credentials_from_url("http://tv.example.com/get.php?username=demo").is_none());
    assert!(XtreamClient::new("http://tv.example.com", " ", "pw").is_err());
}

#[test]
fn api_requests_encode_credentials_and_send_user_agent() {
    let c = XtreamClient::new(
        "http://tv.example.com/sub/player_api.php",
        " us er ",
        "p@ss&w=rd",
    )
    .unwrap();
    let req = c.live_streams(Some("5"));
    assert_eq!(
        req.url,
        "http://tv.example.com/sub/player_api.php?username=us%20er&password=p%40ss%26w%3Drd&action=get_live_streams&category_id=5"
    );
    assert_eq!(req.header("user-agent"), Some(DEFAULT_USER_AGENT));
    assert_eq!(c.live_streams(Some("  ")).url, c.live_streams(None).url);
    let custom = c.clone().with_user_agent(Some("SyntheticPlayer/2"));
    assert_eq!(
        custom.vod_streams(None).header("User-Agent"),
        Some("SyntheticPlayer/2")
    );
    assert_eq!(
        c.clone().with_user_agent(Some("  ")).user_agent(),
        DEFAULT_USER_AGENT
    );

    let debug = format!("{req:?} {c:?}");
    assert!(
        !debug.contains("p%40ss") && !debug.contains("p@ss") && !debug.contains("us%20er"),
        "{debug}"
    );
}

#[test]
fn every_action_has_its_url() {
    let c = client();
    let q = "http://tv.example.com:8080/player_api.php?username=demo&password=s3cret";
    assert_eq!(c.account_info().url, q);
    let probes: Vec<String> = c.account_info_probes().into_iter().map(|r| r.url).collect();
    assert_eq!(
        probes,
        vec![
            format!("{q}&action=get_account_info"),
            q.to_owned(),
            format!("{q}&action=get_profile")
        ]
    );
    assert_eq!(
        c.live_categories().url,
        format!("{q}&action=get_live_categories")
    );
    assert_eq!(
        c.vod_categories().url,
        format!("{q}&action=get_vod_categories")
    );
    assert_eq!(
        c.series_categories().url,
        format!("{q}&action=get_series_categories")
    );
    assert_eq!(
        c.vod_streams(Some("10")).url,
        format!("{q}&action=get_vod_streams&category_id=10")
    );
    assert_eq!(c.series(None).url, format!("{q}&action=get_series"));
    assert_eq!(
        c.series_info(301).url,
        format!("{q}&action=get_series_info&series_id=301")
    );
    assert_eq!(
        c.vod_info(201).url,
        format!("{q}&action=get_vod_info&vod_id=201")
    );
    assert_eq!(
        c.short_epg(101, Some(4)).url,
        format!("{q}&action=get_short_epg&stream_id=101&limit=4")
    );
    assert_eq!(
        c.short_epg(101, None).url,
        format!("{q}&action=get_short_epg&stream_id=101")
    );
    assert_eq!(
        c.simple_data_table(101).url,
        format!("{q}&action=get_simple_data_table&stream_id=101")
    );
    assert_eq!(
        c.simple_date_table_legacy(101).url,
        format!("{q}&action=get_simple_date_table&stream_id=101")
    );
    assert_eq!(
        c.xmltv_url(),
        "http://tv.example.com:8080/xmltv.php?username=demo&password=s3cret"
    );
    assert_eq!(
        c.m3u_playlist_url("ts"),
        "http://tv.example.com:8080/get.php?username=demo&password=s3cret&type=m3u_plus&output=ts"
    );
}

#[test]
fn stream_urls() {
    let c = XtreamClient::new("http://tv.example.com:8080/", "de mo", "p/w").unwrap();
    assert_eq!(
        c.live_url(101, "ts"),
        "http://tv.example.com:8080/live/de%20mo/p%2Fw/101.ts"
    );
    assert_eq!(
        c.live_url(101, ".m3u8"),
        "http://tv.example.com:8080/live/de%20mo/p%2Fw/101.m3u8"
    );
    assert_eq!(
        c.movie_url(201, Some("mkv")).as_deref(),
        Some("http://tv.example.com:8080/movie/de%20mo/p%2Fw/201.mkv")
    );
    assert_eq!(
        c.movie_url(201, Some(" ")),
        None,
        "the container is never guessed"
    );
    assert_eq!(c.movie_url(0, Some("mkv")), None);
    assert_eq!(
        c.episode_url(4000, Some("mp4")).as_deref(),
        Some("http://tv.example.com:8080/series/de%20mo/p%2Fw/4000.mp4")
    );
    assert_eq!(
        c.playback_headers(),
        vec![("User-Agent".to_owned(), DEFAULT_USER_AGENT.to_owned())]
    );
}

#[test]
fn live_format_resolution() {
    let f = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(StreamFormat::Auto.resolve(&[]), "m3u8");
    assert_eq!(StreamFormat::Auto.resolve(&f(&["ts", "M3U8"])), "m3u8");
    assert_eq!(StreamFormat::Auto.resolve(&f(&["rtmp", "ts"])), "ts");
    assert_eq!(StreamFormat::Auto.resolve(&f(&["rtmp"])), "rtmp");
    assert_eq!(
        StreamFormat::Ts.resolve(&f(&["m3u8"])),
        "m3u8",
        "manual choice not allowed falls back"
    );
    assert_eq!(StreamFormat::Ts.resolve(&[]), "ts");
    assert_eq!(StreamFormat::M3u8.resolve(&f(&["ts", "m3u8"])), "m3u8");
}

#[test]
fn catchup_urls_use_the_panel_clock() {
    let c = client();
    // 2023-11-14 22:13:20 UTC; panel at UTC+3 → 2023-11-15 01:13.
    let rest = c.catchup_url(101, NOW, NOW + 3600, CatchupVariant::RestTs, Some(180), 0);
    assert_eq!(
        rest,
        "http://tv.example.com:8080/timeshift/demo/s3cret/60/2023-11-15:01-13/101.ts"
    );
    // Unknown panel clock: the viewer's (UTC−4 here), not UTC.
    assert_eq!(
        c.catchup_url(
            101,
            NOW,
            NOW + 3600,
            CatchupVariant::RestTs,
            None,
            -4 * 3600
        ),
        "http://tv.example.com:8080/timeshift/demo/s3cret/60/2023-11-14:18-13/101.ts"
    );
    let hls = c.catchup_url(101, NOW, NOW + 30, CatchupVariant::RestM3u8, Some(0), 0);
    assert_eq!(
        hls, "http://tv.example.com:8080/timeshift/demo/s3cret/1/2023-11-14:22-13/101.m3u8",
        "duration is at least 1 minute"
    );
    let legacy = c.catchup_url(101, NOW, NOW + 5400, CatchupVariant::LegacyTs, Some(0), 0);
    assert_eq!(
        legacy,
        "http://tv.example.com:8080/streaming/timeshift.php?username=demo&password=s3cret&stream=101&start=2023-11-14%3A22-13&duration=90&extension=ts"
    );
    assert!(
        !c.catchup_url(101, NOW, NOW + 60, CatchupVariant::Legacy, Some(0), 0)
            .contains("extension")
    );
    assert_eq!(
        c.catchup_probe(rest.clone()).header("Range"),
        Some("bytes=0-1023")
    );

    use CatchupVariant::*;
    assert_eq!(
        catchup_probe_order(&[]),
        vec![RestTs, LegacyTs, RestM3u8, LegacyM3u8, Legacy]
    );
    assert_eq!(
        catchup_probe_order(&["m3u8".into()]),
        vec![RestM3u8, LegacyM3u8, Legacy]
    );
    assert_eq!(
        catchup_probe_order(&["rtmp".into()]),
        vec![RestTs, LegacyTs, Legacy]
    );
}

#[test]
fn server_clock_offsets() {
    assert_eq!(
        ServerClock::new(Some("UTC"), None, None).offset_minutes(),
        Some(0)
    );
    assert_eq!(
        ServerClock::new(Some("UTC+3"), None, None).offset_minutes(),
        Some(180)
    );
    assert_eq!(
        ServerClock::new(Some("GMT-04:30"), None, None).offset_minutes(),
        Some(-270)
    );
    assert_eq!(
        ServerClock::new(Some("UTC+0530"), None, None).offset_minutes(),
        Some(330)
    );
    // An IANA name (unknown to a tz-less crate) uses the clock pair, snapped to 15 min.
    let c = ServerClock::new(
        Some("Europe/Example"),
        Some("2023-11-15 01:13:27"),
        Some(NOW),
    );
    assert_eq!(
        (c.timezone.as_deref(), c.offset_minutes()),
        (Some("Europe/Example"), Some(180))
    );
    assert_eq!(
        ServerClock::new(Some("Etc/GMT+3"), None, None).offset_minutes(),
        None,
        "POSIX sign is not guessed"
    );
    assert_eq!(
        ServerClock::new(None, Some("garbage"), Some(NOW)).offset_minutes(),
        None
    );
}

#[test]
fn account_info_and_status() {
    let info = parse_account_info(&ok("account_info.json"), NOW).unwrap();
    assert_eq!(info.status, AccountStatus::Active);
    assert_eq!(info.expires_at, Some(1_893_456_000));
    assert_eq!(info.max_connections, Some(2));
    assert_eq!(info.active_connections, Some(0));
    assert!(!info.is_trial);
    assert_eq!(info.allowed_output_formats, vec!["m3u8", "ts", "rtmp"]);
    assert_eq!(info.server.port, Some(8080));
    assert_eq!(info.server.clock.offset_minutes(), Some(180));
    assert!(
        !format!("{info:?}").contains("synthetic-pass"),
        "the echoed password is not kept"
    );
    // Past expiry wins over "Active".
    assert_eq!(
        parse_account_info(&ok("account_info.json"), 1_900_000_000)
            .unwrap()
            .status,
        AccountStatus::Expired
    );

    let status = |v: serde_json::Value| resolve_account_status(Some(&v), NOW);
    use serde_json::json;
    assert_eq!(status(json!({"status": "ACTIVE"})), AccountStatus::Active);
    assert_eq!(status(json!({"auth": "1"})), AccountStatus::Active);
    assert_eq!(
        status(json!({"auth": true, "exp_date": "0"})),
        AccountStatus::Active,
        "0 = no expiry"
    );
    assert_eq!(
        status(json!({"auth": 1, "exp_date": "-5"})),
        AccountStatus::Active
    );
    assert_eq!(
        status(json!({"auth": 1, "exp_date": "never"})),
        AccountStatus::Active
    );
    assert_eq!(
        status(json!({"auth": 0, "status": "Active"})),
        AccountStatus::Inactive
    );
    assert_eq!(status(json!({"auth": false})), AccountStatus::Inactive);
    assert_eq!(status(json!({"status": "Expired"})), AccountStatus::Expired);
    assert_eq!(status(json!({"status": "Banned"})), AccountStatus::Inactive);
    assert_eq!(status(json!({})), AccountStatus::Unavailable);
    assert_eq!(
        resolve_account_status(None, NOW),
        AccountStatus::Unavailable
    );
    assert_eq!(
        parse_account_info(&HttpResponse::ok("{}"), NOW)
            .unwrap()
            .status,
        AccountStatus::Unavailable
    );
}

#[test]
fn categories() {
    let cats = parse_categories(&ok("live_categories.json"), ContentKind::Live).unwrap();
    let names: Vec<&str> = cats.items.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, vec!["Synthetic News", "Category 2", "Late Night"]);
    assert_eq!(cats.items[0].id.as_str(), "xt:live-group:1");
    assert!(cats.items[2].adult);
    assert_eq!(cats.skipped, 2);
}

#[test]
fn live_streams() {
    let live = parse_live_streams(&ok("live_streams.json")).unwrap();
    assert_eq!(live.items.len(), 3);
    assert_eq!(live.skipped, 3);
    let news = &live.items[0];
    assert_eq!(news.id.as_str(), "xt:live:101");
    assert_eq!(news.number, Some(1));
    assert_eq!(news.epg_id.as_deref(), Some("news.example"));
    assert_eq!(
        news.groups.iter().map(|g| g.as_str()).collect::<Vec<_>>(),
        vec!["xt:live-group:1", "xt:live-group:3"]
    );
    let cu = news.catch_up.as_ref().unwrap();
    assert_eq!((&cu.kind, cu.days), (&CatchUpKind::Xtream, Some(3)));
    assert_eq!(
        news.stream,
        StreamLocator::Xtream {
            stream_id: 101,
            container_extension: None
        }
    );
    let radio = &live.items[1];
    assert!(
        radio.radio && radio.catch_up.is_none() && radio.groups.is_empty() && radio.logo.is_none()
    );
    assert_eq!(radio.number, Some(2));
    let unnamed = &live.items[2];
    assert_eq!(
        (unnamed.name.as_str(), unnamed.number),
        ("Channel 103", None)
    );
    assert!(
        live.warnings
            .iter()
            .any(|w| w.location == WarningLocation::Index(4))
    );
}

#[test]
fn list_shape_rules() {
    assert_eq!(
        parse_live_streams(&HttpResponse::ok(r#"{"user_info":{"auth":0}}"#)),
        Err(ProviderError::AccountRejected)
    );
    assert!(
        matches!(
            parse_live_streams(&HttpResponse::ok("{}")),
            Err(ProviderError::InvalidBody(_))
        ),
        "{{}} does not prove an empty catalog"
    );
    assert!(matches!(
        parse_live_streams(&HttpResponse::ok(r#""nope""#)),
        Err(ProviderError::InvalidBody(_))
    ));
    assert!(matches!(
        parse_live_streams(&HttpResponse::ok("<html>blocked</html>")),
        Err(ProviderError::InvalidBody(_))
    ));
    assert_eq!(
        parse_live_streams(&HttpResponse::new(503, "")),
        Err(ProviderError::HttpStatus(503))
    );
    assert!(
        parse_live_streams(&HttpResponse::ok(" [] "))
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn vod_streams_keyed_object() {
    let vod = parse_vod_streams(&ok("vod_streams.json")).unwrap();
    assert_eq!(vod.items.len(), 3);
    let by = |id: &str| {
        vod.items
            .iter()
            .find(|v| v.provider_id.as_deref() == Some(id))
            .unwrap()
    };
    let a = by("201");
    assert_eq!((a.rating_x10, a.added), (Some(75), Some(1_690_000_000)));
    assert_eq!(
        a.stream,
        StreamLocator::Xtream {
            stream_id: 201,
            container_extension: Some("mkv".into())
        }
    );
    let b = by("202");
    assert_eq!(b.rating_x10, Some(80), "falls back to rating_5based × 2");
    assert!(
        matches!(&b.stream, StreamLocator::Url { url, .. } if url == "http://cdn.example.net/movies/beta.mp4")
    );
    let g = by("203");
    assert_eq!((g.rating_x10, g.year), (None, Some(2019)));
    assert_eq!(
        g.stream,
        StreamLocator::Xtream {
            stream_id: 203,
            container_extension: None
        },
        "ftp direct_source ignored"
    );
}

#[test]
fn series_list() {
    let s = parse_series_list(&ok("series.json")).unwrap();
    assert_eq!(s.items.len(), 2);
    assert_eq!(s.skipped, 1);
    let a = &s.items[0];
    assert_eq!(
        (a.id.as_str(), a.year, a.rating_x10, a.last_modified),
        ("xt:series:301", Some(2018), Some(80), Some(1_695_000_000))
    );
    let b = &s.items[1];
    assert_eq!((b.year, b.rating_x10, b.last_modified), (None, None, None));
}

#[test]
fn series_info_object_shape() {
    let d = parse_series_info(&ok("series_info_object.json"), 301).unwrap();
    assert_eq!(d.series.id.as_str(), "xt:series:301");
    assert_eq!(d.series.rating_x10, Some(81));
    let eps: Vec<(u32, u32, &str)> = d
        .episodes
        .items
        .iter()
        .map(|e| (e.season, e.number, e.title.as_str()))
        .collect();
    assert_eq!(
        eps,
        vec![
            (1, 1, "Alpha S1E1"),
            (1, 2, "Alpha S1E2"),
            (2, 1, "Alpha S2E1")
        ]
    );
    assert_eq!(d.episodes.skipped, 1);
    let e2 = &d.episodes.items[1];
    assert_eq!(
        (e2.duration_secs, e2.plot.as_deref()),
        (Some(1500), None),
        "N/A plot is empty"
    );
    assert_eq!(
        d.episodes.items[2].duration_secs,
        Some(2530),
        "HH:MM:SS fallback"
    );
    assert_eq!(d.episodes.items[0].id.as_str(), "xt:episode:4000");
    let seasons: Vec<(u32, Option<u32>)> = d
        .seasons
        .iter()
        .map(|s| (s.number, s.episode_count))
        .collect();
    assert_eq!(
        seasons,
        vec![(1, Some(2)), (2, None)],
        "season 2 derived from episodes"
    );
}

#[test]
fn series_info_array_shape() {
    let d = parse_series_info(&ok("series_info_array.json"), 302).unwrap();
    assert_eq!(d.series.name, "Series 302", "info [] tolerated");
    let eps: Vec<(u32, u32)> = d
        .episodes
        .items
        .iter()
        .map(|e| (e.season, e.number))
        .collect();
    assert_eq!(eps, vec![(1, 1), (1, 2), (2, 1)]);
    assert!(
        matches!(&d.episodes.items[2].stream, StreamLocator::Url { url, .. } if url.starts_with("https://cdn.example.net/"))
    );
    assert_eq!(d.seasons.len(), 2);
}

#[test]
fn vod_info_full_and_sparse() {
    let d = parse_vod_info(&ok("vod_info.json"), 201).unwrap();
    assert!(d.playable);
    assert_eq!(
        d.item.stream,
        StreamLocator::Xtream {
            stream_id: 201,
            container_extension: Some("mkv".into())
        }
    );
    assert_eq!(
        d.item.poster.as_deref(),
        Some("http://img.example.com/201-big.jpg")
    );
    assert_eq!(
        (d.item.plot.as_deref(), d.cast.as_deref(), d.item.rating_x10),
        (None, None, None),
        "stand-ins are empty"
    );
    assert_eq!(
        (d.item.year, d.item.duration_secs, d.tmdb_id),
        (Some(2015), Some(5400), Some(12345))
    );
    assert_eq!(d.director.as_deref(), Some("A. Synthetic"));
    assert_eq!(d.backdrops, vec!["http://img.example.com/b1.jpg"]);
    assert_eq!(d.trailer, None);

    let sparse = parse_vod_info(&ok("vod_info_sparse.json"), 201).unwrap();
    assert!(!sparse.playable, "no container and no direct source");
    assert_eq!(sparse.item.name, "Movie Alpha");
    let null = parse_vod_info(&HttpResponse::ok("null"), 9).unwrap();
    assert_eq!((null.item.name.as_str(), null.playable), ("Movie 9", false));
}

#[test]
fn epg_listings() {
    let epg = parse_epg(&ok("short_epg.json"), 180).unwrap();
    assert_eq!(epg.skipped, 2);
    let rows: Vec<(i64, &str, bool)> = epg
        .items
        .iter()
        .map(|p| (p.start, p.title.as_str(), p.has_archive))
        .collect();
    assert_eq!(
        rows,
        vec![
            (NOW, "Morning News", false),
            (NOW + 7200, "Plain title!", true),
            (NOW + 10_000, "Évening Talk", false)
        ],
        "timestamps win; local times use the panel offset; sorted"
    );
    assert_eq!(
        epg.items[0].description.as_deref(),
        Some("Synthetic morning show.")
    );
    assert_eq!(epg.items[0].epg_id.as_deref(), Some("5"));
    assert_eq!(epg.items[2].description, None);
    assert!(
        parse_epg(&HttpResponse::ok(r#"{"epg_listings":[]}"#), 0)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        parse_epg(&HttpResponse::ok("[]"), 0)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn malformed_fields_never_panic() {
    for name in [
        "account_info.json",
        "live_categories.json",
        "live_streams.json",
        "vod_streams.json",
        "series.json",
        "series_info_object.json",
        "series_info_array.json",
        "vod_info.json",
        "short_epg.json",
    ] {
        for_each_type_mutation(&fixture("xtream", name), |body| {
            let r = HttpResponse::ok(body);
            let _ = parse_account_info(&r, NOW);
            let _ = parse_categories(&r, ContentKind::Live);
            let _ = parse_live_streams(&r);
            let _ = parse_vod_streams(&r);
            let _ = parse_series_list(&r);
            let _ = parse_series_info(&r, 1);
            let _ = parse_vod_info(&r, 1);
            let _ = parse_epg(&r, 0);
        });
    }
}

#[test]
fn malformed_clock_values_are_rejected_before_arithmetic() {
    for epoch in [i64::MIN, i64::MAX] {
        assert_eq!(
            ServerClock::new(None, Some("2026-03-01 12:00:00"), Some(epoch)).offset_minutes(),
            None
        );
    }
    for zone in [
        "UTC+2147483647:00",
        "UTC+-1:00",
        "UTC+1:-2",
        "UTC+é1",
        "UTC+15:00",
    ] {
        assert_eq!(
            ServerClock::new(Some(zone), None, None).offset_minutes(),
            None,
            "{zone}"
        );
    }
}
