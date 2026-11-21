use super::*;
use crate::m3u::parse_m3u;

const REQ: CatchupRequest<'static> =
    CatchupRequest::new(1_700_000_000, 1_700_003_600, 1_700_010_000);

fn channel(extinf_attrs: &str, url: &str) -> Channel {
    let text = format!("#EXTM3U\n#EXTINF:-1 {extinf_attrs},Test\n{url}\n");
    parse_m3u(&text).channels.items.remove(0)
}

#[test]
fn shift_sets_utc_and_lutc() {
    let ch = channel(
        r#"catchup="shift" catchup-days="3""#,
        "http://live.example.com/ch/1.m3u8?utc=1&x=y",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://live.example.com/ch/1.m3u8?x=y&utc=1700000000&lutc=1700010000")
    );
}

#[test]
fn archive_days_without_type_fall_back_to_shift() {
    let ch = channel(r#"tvg-rec="7""#, "http://live.example.com/ch/2.ts");
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://live.example.com/ch/2.ts?utc=1700000000&lutc=1700010000")
    );
    // Non-HTTP stream: no replay target.
    let ch = channel(r#"tvg-rec="7""#, "rtp://239.0.0.1:5000");
    assert_eq!(build_catchup_url(&ch, REQ), Err(CatchupError::MalformedUrl));
}

#[test]
fn no_days_keeps_catchup() {
    let ch = channel(r#"catchup="shift""#, "http://live.example.com/ch/3.ts");
    assert_eq!(ch.catch_up.as_ref().unwrap().days, None);
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://live.example.com/ch/3.ts?utc=1700000000&lutc=1700010000")
    );
}

#[test]
fn default_source_templates() {
    let ch = channel(
        r#"catchup="default" catchup-days="2" catchup-source="http://arc.example.com/ch/4/{utc}-{duration:60}.m3u8?s=${start}&e={utcend}&d={Y}{m}{d}&o={offset}""#,
        "http://live.example.com/ch/4.m3u8",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok(
            "http://arc.example.com/ch/4/1700000000-60.m3u8?s=1700000000&e=1700003600&d=20231114&o=10000"
        )
    );
    let ch = channel(
        r#"catchup="default" catchup-days="2" catchup-source="http://arc.example.com/{utc:Y-m-d_H}/{unknown}""#,
        "http://l.example.com/x",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://arc.example.com/2023-11-14_22/{unknown}")
    );
    // Default without a source: nothing to replay.
    let ch = channel(
        r#"catchup="default" catchup-days="2""#,
        "http://l.example.com/x",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ),
        Err(CatchupError::MissingTemplate)
    );
}

#[test]
fn append_flussonic_xc_and_unsupported() {
    let ch = channel(
        r#"catchup="append" catchup-days="1" catchup-source="?utc={utc}&lutc={lutc}""#,
        "http://l.example.com/c.m3u8",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://l.example.com/c.m3u8?utc=1700000000&lutc=1700010000")
    );

    let ch = channel(
        r#"catchup="flussonic" catchup-days="1""#,
        "http://f.example.com/chan/index.m3u8?token=t",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://f.example.com/chan/index-1700000000-3600.m3u8?token=t")
    );
    let ch = channel(
        r#"catchup="fs" catchup-days="1""#,
        "http://f.example.com/chan/mpegts",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://f.example.com/chan/timeshift_abs-1700000000.ts")
    );

    let ch = channel(
        r#"catchup="xc" catchup-days="1""#,
        "http://xc.example.com:8080/live/demo/secret/1001.m3u8",
    );
    assert_eq!(
        build_catchup_url(
            &ch,
            CatchupRequest {
                server_offset_minutes: Some(60),
                ..REQ
            }
        )
        .as_deref(),
        Ok("http://xc.example.com:8080/timeshift/demo/secret/60/2023-11-14:23-13/1001.m3u8")
    );

    let ch = channel(
        r#"catchup="rtsp-magic" catchup-days="1""#,
        "http://l.example.com/x",
    );
    assert!(matches!(
        ch.catch_up.as_ref().unwrap().kind,
        CatchUpKind::Unsupported(_)
    ));
    assert_eq!(build_catchup_url(&ch, REQ), Err(CatchupError::Unsupported));
}

#[test]
fn header_catchup_defaults_apply() {
    let text = "#EXTM3U catchup=\"shift\" catchup-days=\"5\"\n#EXTINF:-1,A\nhttp://l.example.com/a\n#EXTINF:-1 catchup-days=\"1\",B\nhttp://l.example.com/b\n";
    let items = parse_m3u(text).channels.items;
    assert_eq!(items[0].catch_up.as_ref().map(|c| c.days), Some(Some(5)));
    assert_eq!(items[1].catch_up.as_ref().map(|c| c.days), Some(Some(1)));
}

#[test]
fn rejects_future_programs_and_positions() {
    let ch = channel(
        r#"catchup="shift" catchup-days="1""#,
        "http://l.example.com/x",
    );
    let at = |start, end, now| CatchupRequest::new(start, end, now);
    assert_eq!(
        build_catchup_url(&ch, at(2_000, 3_000, 1_999)),
        Err(CatchupError::FutureProgram)
    );
    // On air: playable from its start, but not seeking past now.
    assert!(build_catchup_url(&ch, at(2_000, 3_000, 2_500)).is_ok());
    let seek = |offset_secs| CatchupRequest {
        offset_secs,
        ..at(2_000, 3_000, 2_500)
    };
    assert_eq!(
        build_catchup_url(&ch, seek(400)).as_deref(),
        Ok("http://l.example.com/x?utc=2400&lutc=2500")
    );
    assert_eq!(
        build_catchup_url(&ch, seek(600)),
        Err(CatchupError::FutureProgram)
    );
}

#[test]
fn checks_the_archive_window() {
    let day = 86_400;
    let now = 100 * day;
    let old =
        |days_ago: i64| CatchupRequest::new(now - days_ago * day, now - days_ago * day + 3600, now);
    let declared = channel(
        r#"catchup="shift" catchup-days="2""#,
        "http://l.example.com/x",
    );
    assert!(build_catchup_url(&declared, old(2)).is_ok());
    assert_eq!(
        build_catchup_url(&declared, old(3)),
        Err(CatchupError::OutsideArchive)
    );
    // No catchup-days: the caller's default window applies.
    let unknown = channel(r#"catchup="shift""#, "http://l.example.com/x");
    assert!(build_catchup_url(&unknown, old(7)).is_ok());
    assert_eq!(
        build_catchup_url(&unknown, old(8)),
        Err(CatchupError::OutsideArchive)
    );
    let wide = CatchupRequest {
        default_window_days: 14,
        ..old(8)
    };
    assert!(build_catchup_url(&unknown, wide).is_ok());
    // Seeking past the programme end.
    let past_end = CatchupRequest {
        offset_secs: 3601,
        ..old(1)
    };
    assert_eq!(
        build_catchup_url(&declared, past_end),
        Err(CatchupError::OutsideArchive)
    );
}

#[test]
fn buffers_widen_the_replay() {
    let ch = channel(
        r#"catchup="default" catchup-days="1" catchup-source="http://arc.example.com/{utc}/{duration}/{utcend}""#,
        "http://l.example.com/x",
    );
    let req = CatchupRequest {
        buffer_before_secs: 120,
        buffer_after_secs: 300,
        ..CatchupRequest::new(10_000, 13_600, 20_000)
    };
    assert_eq!(
        build_catchup_url(&ch, req).as_deref(),
        Ok("http://arc.example.com/9880/4020/13900")
    );
    // The end buffer stops at now.
    let req = CatchupRequest { now: 13_700, ..req };
    assert_eq!(
        build_catchup_url(&ch, req).as_deref(),
        Ok("http://arc.example.com/9880/3820/13700")
    );
    // Seeking is measured from the buffered start.
    let req = CatchupRequest {
        offset_secs: 600,
        ..req
    };
    assert_eq!(
        build_catchup_url(&ch, req).as_deref(),
        Ok("http://arc.example.com/10480/3220/13700")
    );
}

#[test]
fn errors_do_not_quote_urls() {
    let ch = channel(
        r#"catchup="xc" catchup-days="1""#,
        "http://xc.example.com/live/demo/hunter2/not-an-id.m3u8?token=tok3n",
    );
    let err = build_catchup_url(&ch, REQ).unwrap_err();
    assert_eq!(err, CatchupError::MalformedUrl);
    for text in [format!("{err:?}"), err.to_string()] {
        assert!(!text.contains("hunter2") && !text.contains("tok3n") && !text.contains("demo"));
    }
}

#[test]
fn xc_keeps_the_live_container() {
    for (live, want) in [
        (
            "http://xc.example.com/live/demo/secret/7.m3u8",
            "http://xc.example.com/timeshift/demo/secret/60/2023-11-14:22-13/7.m3u8",
        ),
        (
            "http://xc.example.com/live/demo/secret/7.ts",
            "http://xc.example.com/timeshift/demo/secret/60/2023-11-14:22-13/7.ts",
        ),
        (
            "http://xc.example.com/demo/secret/7",
            "http://xc.example.com/timeshift/demo/secret/60/2023-11-14:22-13/7.ts",
        ),
    ] {
        let ch = channel(r#"catchup="xc" catchup-days="1""#, live);
        assert_eq!(build_catchup_url(&ch, REQ).as_deref(), Ok(want), "{live}");
    }
}

#[test]
fn date_parts_use_the_viewer_clock() {
    let ch = channel(
        r#"catchup="default" catchup-days="1" catchup-source="http://arc.example.com/{Y}{m}{d}-{H}{M}{S}/{utc:Y-m-d_H:M}/{lutc:H}/{utc}/{lutc}""#,
        "http://l.example.com/x",
    );
    // 1_700_000_000 = 2023-11-14 22:13:20 UTC; viewer at UTC+05:30.
    let req = CatchupRequest {
        local_offset_secs: 5 * 3600 + 1800,
        ..REQ
    };
    assert_eq!(
        build_catchup_url(&ch, req).as_deref(),
        Ok("http://arc.example.com/20231115-034320/2023-11-15_03:43/06/1700000000/1700010000")
    );
}

#[test]
fn shift_keeps_query_bytes() {
    // A signed token whose bytes a re-encoding would change.
    let live = "http://live.example.com/ch/1.m3u8?sig=ab~c%2fd%7e&k=a=b&flag&utc=5#frag";
    let ch = channel(r#"catchup="shift" catchup-days="1""#, live);
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok(
            "http://live.example.com/ch/1.m3u8?sig=ab~c%2fd%7e&k=a=b&flag&utc=1700000000&lutc=1700010000#frag"
        )
    );
    let ch = channel(
        r#"catchup="shift" catchup-days="1""#,
        "http://live.example.com/ch/2.ts",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://live.example.com/ch/2.ts?utc=1700000000&lutc=1700010000")
    );
}

#[test]
fn source_without_placeholders_is_used_verbatim() {
    // Kodi: the source is the full URL. IPTVnator would add utc/lutc here.
    for attrs in [
        r#"catchup="default" catchup-days="2" catchup-source="http://arc.example.com/ch/5?t=a~b""#,
        r#"catchup-days="2" catchup-source="http://arc.example.com/ch/5?t=a~b""#,
        r#"catchup="shift" catchup-days="2" catchup-source="http://arc.example.com/ch/5?t=a~b""#,
    ] {
        let ch = channel(attrs, "http://l.example.com/x");
        assert_eq!(
            build_catchup_url(&ch, REQ).as_deref(),
            Ok("http://arc.example.com/ch/5?t=a~b"),
            "{attrs}"
        );
    }
}

#[test]
fn default_with_a_relative_source_appends_it() {
    let ch = channel(
        r#"catchup="default" catchup-days="2" catchup-source="?archive={utc}""#,
        "http://l.example.com/x.m3u8",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://l.example.com/x.m3u8?archive=1700000000")
    );
}

#[test]
fn flussonic_on_air_and_short_programs() {
    let hls = channel(
        r#"catchup="flussonic" catchup-days="1""#,
        "http://f.example.com/chan/index.m3u8?token=a~b",
    );
    let ts = channel(
        r#"catchup="fs" catchup-days="1""#,
        "http://f.example.com/chan/mpegts?token=a~b",
    );
    // Still on air: open-ended forms (Flussonic DVR docs).
    let on_air = CatchupRequest::new(1_000, 5_000, 2_000);
    assert_eq!(
        build_catchup_url(&hls, on_air).as_deref(),
        Ok("http://f.example.com/chan/index-1000-now.m3u8?token=a~b")
    );
    assert_eq!(
        build_catchup_url(&ts, on_air).as_deref(),
        Ok("http://f.example.com/chan/timeshift_abs-1000.ts?token=a~b")
    );
    // A zero-length guide entry still asks for at least one second.
    let empty = CatchupRequest::new(1_000, 1_000, 2_000);
    assert_eq!(
        build_catchup_url(&hls, empty).as_deref(),
        Ok("http://f.example.com/chan/index-1000-1.m3u8?token=a~b")
    );
    let mono = channel(
        r#"catchup="fs" catchup-days="1""#,
        "http://f.example.com/chan/mono.m3u8",
    );
    assert_eq!(
        build_catchup_url(&mono, CatchupRequest::new(1_000, 1_600, 2_000)).as_deref(),
        Ok("http://f.example.com/chan/mono-1000-600.m3u8")
    );
}

#[test]
fn xc_without_a_server_clock_uses_the_viewer_clock() {
    let ch = channel(
        r#"catchup="xc" catchup-days="1""#,
        "http://xc.example.com/live/demo/secret/7.ts",
    );
    let req = CatchupRequest {
        local_offset_secs: -4 * 3600,
        ..REQ
    };
    assert_eq!(
        build_catchup_url(&ch, req).as_deref(),
        Ok("http://xc.example.com/timeshift/demo/secret/60/2023-11-14:18-13/7.ts")
    );
    // A known panel clock wins over the viewer's.
    let req = CatchupRequest {
        server_offset_minutes: Some(0),
        ..req
    };
    assert_eq!(
        build_catchup_url(&ch, req).as_deref(),
        Ok("http://xc.example.com/timeshift/demo/secret/60/2023-11-14:22-13/7.ts")
    );
}

#[test]
fn live_url_placeholders_are_expanded() {
    assert_eq!(
        expand_live_url(
            "http://l.example.com/c.m3u8?t={lutc}&n=${now}&s=${timestamp}&d={lutc:Ymd}&u={utc}",
            1_700_000_000,
            3600
        ),
        "http://l.example.com/c.m3u8?t=1700000000&n=1700000000&s=1700000000&d=20231114&u={utc}"
    );
    // In a catch-up URL built from the live URL they are now as well.
    let ch = channel(
        r#"catchup="shift" catchup-days="1""#,
        "http://l.example.com/c.m3u8?k=${timestamp}",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://l.example.com/c.m3u8?k=1700010000&utc=1700000000&lutc=1700010000")
    );
    let ch = channel(
        r#"catchup="append" catchup-days="1" catchup-source="&archive={utc}""#,
        "http://l.example.com/c.m3u8?k={lutc}",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://l.example.com/c.m3u8?k=1700010000&archive=1700000000")
    );
    let ch = channel(
        r#"catchup="xc" catchup-days="1""#,
        "http://xc.example.com/live/demo/secret/7.m3u8?k={lutc}",
    );
    assert_eq!(
        build_catchup_url(&ch, REQ).as_deref(),
        Ok("http://xc.example.com/timeshift/demo/secret/60/2023-11-14:22-13/7.m3u8?k=1700010000")
    );
}

#[test]
fn catchup_correction_shifts_program_times() {
    // −1.5 h: the guide is 90 minutes ahead of the archive.
    let text = "#EXTM3U catchup-correction=\"-1.5\"\n\
        #EXTINF:-1 catchup=\"default\" catchup-days=\"1\" catchup-source=\"http://arc.example.com/{utc}/{utcend}/{H}{M}/{lutc}/{offset}\",A\nhttp://l.example.com/a\n\
        #EXTINF:-1 catchup=\"shift\" catchup-days=\"1\" catchup-correction=\"0.5\",B\nhttp://l.example.com/b\n";
    let items = parse_m3u(text).channels.items;
    assert_eq!(
        build_catchup_url(&items[0], REQ).as_deref(),
        Ok("http://arc.example.com/1699994600/1699998200/2043/1700010000/15400")
    );
    assert_eq!(
        build_catchup_url(&items[1], REQ).as_deref(),
        Ok("http://l.example.com/b?utc=1700001800&lutc=1700010000")
    );
    // A correction that moves the start past now is a future programme.
    let late = CatchupRequest::new(1_700_000_000, 1_700_003_600, 1_700_001_000);
    assert_eq!(
        build_catchup_url(&items[1], late),
        Err(CatchupError::FutureProgram)
    );
}

#[test]
fn vod_plays_the_source_or_the_catchup_id() {
    let with_source = channel(
        r#"catchup="vod" catchup-days="3" catchup-source="http://vod.example.com/rec/j.ts?cutv={Y}-{m}-{d}T{H}:{M}:{S}""#,
        "http://l.example.com/x",
    );
    assert_eq!(
        build_catchup_url(&with_source, REQ).as_deref(),
        Ok("http://vod.example.com/rec/j.ts?cutv=2023-11-14T22:13:20")
    );
    // Kodi's default vod source is `{catchup-id}` from the guide.
    let bare = channel(
        r#"catchup="vod" catchup-days="3""#,
        "http://l.example.com/x",
    );
    let req = CatchupRequest {
        catchup_id: Some("http://vod.example.com/rec/42.mp4"),
        ..REQ
    };
    assert_eq!(
        build_catchup_url(&bare, req).as_deref(),
        Ok("http://vod.example.com/rec/42.mp4")
    );
    assert_eq!(
        build_catchup_url(&bare, REQ),
        Err(CatchupError::MissingTemplate)
    );
    // The window still applies.
    let old = CatchupRequest::new(1_000, 2_000, 1_000 + 4 * 86_400);
    assert_eq!(
        build_catchup_url(&with_source, old),
        Err(CatchupError::OutsideArchive)
    );
}

#[test]
fn catchup_id_placeholder() {
    let ch = channel(
        r#"catchup="default" catchup-days="1" catchup-source="http://arc.example.com/play/{catchup-id}.m3u8?s={utc}""#,
        "http://l.example.com/x",
    );
    let req = CatchupRequest {
        catchup_id: Some("ev-77"),
        ..REQ
    };
    assert_eq!(
        build_catchup_url(&ch, req).as_deref(),
        Ok("http://arc.example.com/play/ev-77.m3u8?s=1700000000")
    );
    assert_eq!(
        build_catchup_url(&ch, REQ),
        Err(CatchupError::MissingTemplate),
        "a program without a catchup-id cannot fill the template"
    );
}

#[test]
fn extreme_inputs_do_not_panic() {
    let ch = channel(
        r#"catchup="default" catchup-source="http://arc.example.com/{utc}/{duration}/{offset}/{Y}""#,
        "http://l.example.com/x",
    );
    for req in [
        CatchupRequest::new(i64::MIN, i64::MAX, i64::MAX),
        CatchupRequest {
            local_offset_secs: i32::MAX,
            offset_secs: u32::MAX,
            buffer_before_secs: u32::MAX,
            buffer_after_secs: u32::MAX,
            default_window_days: u32::MAX,
            server_offset_minutes: Some(i32::MAX),
            ..CatchupRequest::new(i64::MIN + 1, i64::MIN + 2, i64::MIN + 3)
        },
    ] {
        let _ = build_catchup_url(&ch, req);
    }
}
