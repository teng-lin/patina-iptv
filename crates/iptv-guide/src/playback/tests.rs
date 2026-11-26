//! Stream resolution against synthetic sources (reserved example domains
//! and loopback only).

use iptv_core::guide::{Program, Schedule};
use iptv_epg::GuideStore;
use iptv_providers::CatchupError;
use iptv_providers::HttpResponse;
use iptv_providers::m3u::parse_m3u;
use iptv_providers::model::{
    CatchUp, CatchUpKind, Channel, ContentKind, M3uSourceConfig, MacAddress, SourceKind,
    StalkerLinkFlags, StreamLocator, XtreamCredentials,
};
use iptv_providers::stalker::{MAG_USER_AGENT, PortalMode, StalkerPortal, StalkerSession};
use iptv_providers::xtream::{CatchupVariant, StreamFormat, parse_account_info};
use iptv_providers::{ContentId, SecretUrl};

use super::{Archive, Clock, PlaybackContext, Resolution, StalkerLink, Unavailable, resolve};
use crate::EpgSchedule;

/// 2027-01-15 08:00:00 UTC.
const NOW: i64 = 1_800_000_000;
const CLOCK: Clock = Clock {
    now: NOW,
    local_offset_secs: 0,
};

fn creds(user_agent: Option<&str>) -> XtreamCredentials {
    XtreamCredentials {
        server_url: "http://tv.example.com:8080/player_api.php".into(),
        username: "demo".into(),
        password: "s3cret".into(),
        user_agent: user_agent.map(str::to_owned),
    }
}

/// An account allowing `formats`, on a panel one hour ahead of UTC.
fn account(formats: &str) -> iptv_providers::xtream::AccountInfo {
    let body = format!(
        r#"{{"user_info":{{"auth":1,"status":"Active","allowed_output_formats":[{formats}]}},
            "server_info":{{"timezone":"UTC+1"}}}}"#
    );
    parse_account_info(&HttpResponse::ok(body), NOW).expect("synthetic account parses")
}

fn xtream_channel(stream_id: u64, archive_days: Option<u32>) -> Channel {
    Channel {
        id: ContentId::from_provider(
            SourceKind::Xtream,
            ContentKind::Live,
            &stream_id.to_string(),
        ),
        provider_id: Some(stream_id.to_string()),
        name: format!("Synthetic {stream_id}"),
        number: None,
        logo: None,
        groups: Vec::new(),
        stream: StreamLocator::Xtream {
            stream_id,
            container_extension: None,
        },
        epg_id: None,
        epg_name: None,
        epg_shift_minutes: None,
        catch_up: archive_days.map(|days| CatchUp {
            kind: CatchUpKind::Xtream,
            days: Some(days),
            source: None,
            correction_secs: 0,
        }),
        radio: false,
    }
}

fn play(r: Resolution) -> iptv_core::playback::PlaySource {
    match r {
        Resolution::Play(s) => s,
        other => panic!("expected a ready source, got {other:?}"),
    }
}

#[test]
fn m3u_playback_only_trusts_the_picked_loopback_origin() {
    let context = PlaybackContext::m3u(&M3uSourceConfig {
        location: SecretUrl::new("http://127.0.0.1:18097/lineup.m3u"),
        user_agent: None,
        referrer: None,
    });
    for (url, public_only) in [
        ("http://127.0.0.1:18097/live.ts", false),
        ("http://127.0.0.1:18098/live.ts", true),
        ("https://127.0.0.1:18097/live.ts", true),
        ("https://media.example.test/live.ts", true),
    ] {
        let playlist = parse_m3u(&format!("#EXTM3U\n#EXTINF:-1,Synthetic\n{url}\n"));
        let channel = &playlist.channels.items[0];
        assert_eq!(
            play(resolve(Some(&context), channel, None, CLOCK)).public_only,
            public_only,
            "{url}"
        );
        assert!(play(resolve(None, channel, None, CLOCK)).public_only);
    }
}

#[test]
fn xtream_live_urls_follow_the_allowed_format() {
    let ch = xtream_channel(42, None);
    let hls = PlaybackContext::xtream(&creds(None), None, StreamFormat::Auto).unwrap();
    let s = play(resolve(Some(&hls), &ch, None, CLOCK));
    assert_eq!(s.url, "http://tv.example.com:8080/live/demo/s3cret/42.m3u8");
    assert_eq!(s.header("User-Agent"), Some("VLC/3.0.18 LibVLC/3.0.18"));

    let ts_only = account(r#""ts""#);
    let ts = PlaybackContext::xtream(
        &creds(Some("SyntheticBox/2")),
        Some(&ts_only),
        StreamFormat::Auto,
    )
    .unwrap();
    let s = play(resolve(Some(&ts), &ch, None, CLOCK));
    assert_eq!(s.url, "http://tv.example.com:8080/live/demo/s3cret/42.ts");
    assert_eq!(s.header("user-agent"), Some("SyntheticBox/2"));

    // Credentials are percent-encoded in the path.
    let mut odd = creds(None);
    odd.password = "p@ss/word".into();
    let ctx = PlaybackContext::xtream(&odd, None, StreamFormat::Ts).unwrap();
    assert_eq!(
        play(resolve(Some(&ctx), &ch, None, CLOCK)).url,
        "http://tv.example.com:8080/live/demo/p%40ss%2Fword/42.ts"
    );
}

#[test]
fn xtream_without_its_account_cannot_play() {
    let ch = xtream_channel(42, Some(3));
    let m3u = PlaybackContext::m3u(&M3uSourceConfig {
        location: SecretUrl::new("http://127.0.0.1:8089/p.m3u"),
        user_agent: None,
        referrer: None,
    });
    for ctx in [None, Some(&m3u)] {
        assert!(matches!(
            resolve(ctx, &ch, None, CLOCK),
            Resolution::Unavailable(Unavailable::NeedsAccount)
        ));
    }
}

#[test]
fn xtream_timeshift_uses_the_panel_clock_and_program_length() {
    let ch = xtream_channel(42, Some(3));
    let acc = account(r#""m3u8","ts""#);
    let ctx = PlaybackContext::xtream(&creds(None), Some(&acc), StreamFormat::Auto).unwrap();
    // A program that started 30 minutes ago and runs an hour.
    let archive = Archive {
        start: NOW - 1800,
        end: NOW + 1800,
    };
    let s = play(resolve(Some(&ctx), &ch, Some(archive), CLOCK));
    // 07:30 UTC is 08:30 on the panel (UTC+1); MPEG-TS is probed first.
    assert_eq!(
        s.url,
        "http://tv.example.com:8080/timeshift/demo/s3cret/60/2027-01-15:08-30/42.ts"
    );
    assert_eq!(s.header("User-Agent"), Some("VLC/3.0.18 LibVLC/3.0.18"));

    // Without an account the viewer's clock is used.
    let bare = PlaybackContext::xtream(&creds(None), None, StreamFormat::Auto)
        .unwrap()
        .with_catchup_variant(CatchupVariant::RestM3u8);
    let viewer = Clock {
        now: NOW,
        local_offset_secs: -5 * 3600,
    };
    assert_eq!(
        play(resolve(Some(&bare), &ch, Some(archive), viewer)).url,
        "http://tv.example.com:8080/timeshift/demo/s3cret/60/2027-01-15:02-30/42.m3u8"
    );
    let legacy = PlaybackContext::xtream(&creds(None), None, StreamFormat::Auto)
        .unwrap()
        .with_catchup_variant(CatchupVariant::LegacyTs);
    assert_eq!(
        play(resolve(Some(&legacy), &ch, Some(archive), CLOCK)).url,
        "http://tv.example.com:8080/streaming/timeshift.php?username=demo&password=s3cret&stream=42&start=2027-01-15%3A07-30&duration=60&extension=ts"
    );
}

#[test]
fn xtream_archive_bounds() {
    let acc = account(r#""ts""#);
    let ctx = PlaybackContext::xtream(&creds(None), Some(&acc), StreamFormat::Auto).unwrap();
    let no = |ch: &Channel, start: i64, end: i64| match resolve(
        Some(&ctx),
        ch,
        Some(Archive { start, end }),
        CLOCK,
    ) {
        Resolution::Unavailable(Unavailable::NoArchive(e)) => e,
        other => panic!("expected no archive, got {other:?}"),
    };
    let ch = xtream_channel(7, Some(2));
    assert_eq!(no(&ch, NOW + 60, NOW + 120), CatchupError::FutureProgram);
    assert_eq!(
        no(&ch, NOW - 3 * 86_400, NOW - 3 * 86_400 + 60),
        CatchupError::OutsideArchive
    );
    assert_eq!(
        no(&xtream_channel(7, None), NOW - 60, NOW),
        CatchupError::Unsupported
    );
}

#[test]
fn m3u_headers_combine_playlist_and_channel_values() {
    let list = parse_m3u(
        "#EXTM3U\n\
         #EXTINF:-1 tvg-id=\"a.test\",Alpha\n\
         #EXTVLCOPT:http-user-agent=ChannelAgent/1\n\
         http://127.0.0.1:8089/live/a.m3u8\n\
         #EXTINF:-1 tvg-id=\"b.test\",Beta\n\
         http://127.0.0.1:8089/live/b.m3u8|Referer=http://pipe.example.com/\n\
         #EXTINF:-1,Gamma\n\
         http://127.0.0.1:8089/live/c.m3u8\n",
    );
    let chans = &list.channels.items;
    let ctx = PlaybackContext::m3u(&M3uSourceConfig {
        location: SecretUrl::new("http://127.0.0.1:8089/p.m3u"),
        user_agent: Some("PlaylistAgent/1".into()),
        referrer: Some(" http://ref.example.com/ ".into()),
    });
    let a = play(resolve(Some(&ctx), &chans[0], None, CLOCK));
    assert_eq!(a.url, "http://127.0.0.1:8089/live/a.m3u8");
    assert_eq!(
        a.header("User-Agent"),
        Some("ChannelAgent/1"),
        "channel wins"
    );
    assert_eq!(a.header("Referer"), Some("http://ref.example.com/"));
    let b = play(resolve(Some(&ctx), &chans[1], None, CLOCK));
    assert_eq!(b.url, "http://127.0.0.1:8089/live/b.m3u8");
    assert_eq!(b.header("User-Agent"), Some("PlaylistAgent/1"));
    assert_eq!(b.header("Referer"), Some("http://pipe.example.com/"));
    let c = play(resolve(None, &chans[2], None, CLOCK));
    assert!(c.headers.is_empty(), "no context: nothing added");
}

#[test]
fn m3u_catch_up_from_the_start_of_a_live_program() {
    let list = parse_m3u(
        "#EXTM3U\n\
         #EXTINF:-1 catchup=\"shift\" catchup-days=\"3\",Alpha\n\
         #EXTVLCOPT:http-referrer=http://ref.example.com/\n\
         http://127.0.0.1:8089/live/a.m3u8?token=t1\n\
         #EXTINF:-1,Plain\n\
         http://127.0.0.1:8089/live/p.m3u8\n",
    );
    let chans = &list.channels.items;
    let archive = Archive {
        start: NOW - 1200,
        end: NOW + 2400,
    };
    let s = play(resolve(None, &chans[0], Some(archive), CLOCK));
    assert_eq!(
        s.url,
        format!(
            "http://127.0.0.1:8089/live/a.m3u8?token=t1&utc={}&lutc={NOW}",
            NOW - 1200
        )
    );
    assert_eq!(s.header("Referer"), Some("http://ref.example.com/"));
    assert!(matches!(
        resolve(None, &chans[1], Some(archive), CLOCK),
        Resolution::Unavailable(Unavailable::NoArchive(CatchupError::Unsupported))
    ));
}

fn portal() -> StalkerPortal {
    StalkerPortal::new(
        "http://portal.example.com/stalker_portal/server/load.php",
        MacAddress::parse("00:1A:79:00:00:42").unwrap(),
        PortalMode::Simple,
    )
    .unwrap()
}

fn stalker_channel(cmd: &str, tmp_link: Option<bool>) -> Channel {
    let mut ch = xtream_channel(1, None);
    ch.id = ContentId::from_provider(SourceKind::Stalker, ContentKind::Live, "1");
    ch.stream = StreamLocator::Stalker {
        cmd: cmd.into(),
        link: StalkerLinkFlags {
            use_http_tmp_link: tmp_link,
            use_load_balancing: tmp_link.map(|_| false),
        },
    };
    ch
}

#[test]
fn stalker_static_commands_play_and_dynamic_ones_need_a_link() {
    let ctx = PlaybackContext::stalker(StalkerSession::new(portal()));
    let fixed = stalker_channel("ffmpeg http://cdn.example.net/live/1.m3u8", Some(false));
    let s = play(resolve(Some(&ctx), &fixed, None, CLOCK));
    assert_eq!(s.url, "http://cdn.example.net/live/1.m3u8");
    assert_eq!(s.header("User-Agent"), Some(MAG_USER_AGENT));
    assert_eq!(
        s.header("Cookie"),
        None,
        "no portal identity to a third party"
    );

    let dynamic = stalker_channel("ffmpeg http://localhost/ch/1", Some(true));
    let link = match resolve(Some(&ctx), &dynamic, None, CLOCK) {
        Resolution::StalkerLink(l) => l,
        other => panic!("expected a link, got {other:?}"),
    };
    assert_eq!(link.cmd, "ffmpeg http://localhost/ch/1");
    let owned = StalkerLink::source(
        &link.session,
        "http://portal.example.com/live/1.ts?play_token=x".into(),
        &link.channel_headers,
    );
    assert!(owned.header("Cookie").is_some(), "portal-owned stream");
    assert_eq!(
        owned.header("Referer"),
        Some("http://portal.example.com/stalker_portal/")
    );
    assert!(owned.public_only, "a public portal's own host is checked");
    assert!(matches!(
        resolve(None, &dynamic, None, CLOCK),
        Resolution::Unavailable(Unavailable::NeedsAccount)
    ));
    let mut archived = dynamic;
    archived.catch_up = Some(CatchUp {
        kind: CatchUpKind::Stalker,
        days: Some(1),
        source: None,
        correction_secs: 0,
    });
    assert!(matches!(
        resolve(
            Some(&ctx),
            &archived,
            Some(Archive {
                start: NOW - 60,
                end: NOW
            }),
            CLOCK
        ),
        Resolution::Unavailable(Unavailable::NoArchive(CatchupError::Unsupported))
    ));
}

#[test]
fn a_resolved_stalker_link_may_be_private_only_on_a_private_portals_own_host() {
    let session = |endpoint: &str| {
        StalkerSession::new(
            StalkerPortal::new(
                endpoint,
                MacAddress::parse("00:1A:79:00:00:42").unwrap(),
                PortalMode::Simple,
            )
            .unwrap(),
        )
    };
    let link = |session: &StalkerSession, url: &str| StalkerLink::source(session, url.into(), &[]);

    // A portal on the LAN that serves its streams itself plays.
    let lan = session("http://192.168.50.10/stalker_portal/server/load.php");
    let own = link(&lan, "http://192.168.50.10:8888/live/1.ts?play_token=x");
    assert!(!own.public_only);
    assert!(own.header("Cookie").is_some(), "the portal's own stream");
    // Another private host stays refused, even from a private portal.
    let other = link(&lan, "http://192.168.50.11/live/1.ts");
    assert!(other.public_only);
    assert_eq!(other.header("Cookie"), None);
    assert!(link(&lan, "http://cdn.example.net/live/1.m3u8").public_only);

    // A public portal that hands back a private address is still refused.
    let public = session("http://portal.example.com/stalker_portal/server/load.php");
    for url in [
        "http://192.168.50.10/live/1.ts",
        "http://127.0.0.1:8080/live/1.ts",
        "http://mediabox.local/live/1.ts",
        "http://portal.example.com/live/1.ts",
    ] {
        assert!(link(&public, url).public_only, "{url}");
    }
}

#[test]
fn a_static_stalker_link_follows_the_same_private_portal_rule() {
    let lan_portal = StalkerPortal::new(
        "http://192.168.50.10/stalker_portal/server/load.php",
        MacAddress::parse("00:1A:79:00:00:42").unwrap(),
        PortalMode::Simple,
    )
    .unwrap();
    let lan = PlaybackContext::stalker(StalkerSession::new(lan_portal));
    let own = stalker_channel("ffmpeg http://192.168.50.10:8888/live/1.ts", Some(false));
    assert!(!play(resolve(Some(&lan), &own, None, CLOCK)).public_only);
    let other = stalker_channel("ffmpeg http://192.168.50.11/live/1.ts", Some(false));
    assert!(play(resolve(Some(&lan), &other, None, CLOCK)).public_only);

    let public = PlaybackContext::stalker(StalkerSession::new(portal()));
    let private = stalker_channel("ffmpeg http://192.168.50.10/live/1.ts", Some(false));
    assert!(play(resolve(Some(&public), &private, None, CLOCK)).public_only);
}

#[test]
fn the_schedule_resolves_by_guide_row_and_finds_channels_by_key() {
    let channels = vec![xtream_channel(5, Some(1)), xtream_channel(6, None)];
    let acc = account(r#""ts""#);
    let ctx = PlaybackContext::xtream(&creds(None), Some(&acc), StreamFormat::Auto).unwrap();
    let s =
        EpgSchedule::from_catalog(&[], &channels, GuideStore::new(), NOW).with_playback(Some(ctx));
    assert_eq!(
        s.stream_url(1).as_deref(),
        Some("http://tv.example.com:8080/live/demo/s3cret/6.ts")
    );
    // Guide minutes are relative to the schedule's now.
    let p = Program {
        start: -30,
        end: 30,
    };
    let url = s.catchup_url(0, p, 0).expect("archive");
    assert!(
        url.contains("/timeshift/demo/s3cret/60/2027-01-15:08-30/5.ts"),
        "{url}"
    );
    assert_eq!(s.catchup_url(1, p, 0), None);
    let key = s.channel_key(1).unwrap();
    assert_eq!(key, channels[1].id.as_str());
    assert_eq!(s.find_channel(&key), Some(1));
    assert_eq!(s.find_channel("xt:live:missing"), None);
    let any = s.as_any().unwrap();
    assert!(any.downcast_ref::<EpgSchedule>().is_some());
    assert!(matches!(
        s.resolve(9, None, CLOCK),
        Resolution::Unavailable(Unavailable::NoChannel)
    ));
}

#[test]
fn resolution_authority_diagnostics_never_include_stream_or_credential_details() {
    let channel = parse_m3u("#EXTM3U\n#EXTINF:-1,Generated secret channel\nhttps://example.test/generated-private-stream|X-Generated-Auth=generated-channel-secret\n")
        .channels.items.remove(0);
    let authority = super::SourceAuthority::new(
        81,
        "generated-incarnation".into(),
        iptv_providers::model::SourceConfig::Xtream(creds(Some("GeneratedPrivateAgent/81"))),
    )
    .resolve(&channel);
    let debug = format!("{authority:?}");
    for secret in [
        "generated-private-stream",
        "generated-channel-secret",
        "s3cret",
        "GeneratedPrivateAgent",
        "generated-incarnation",
    ] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn xtream_public_accounts_keep_dns_checks_and_lan_trust_is_origin_scoped() {
    let channel = xtream_channel(42, None);
    for (server, public) in [
        ("https://panel.example.test", true),
        ("http://127.0.0.1:8080", false),
    ] {
        let mut credentials = creds(None);
        credentials.server_url = server.into();
        let context = PlaybackContext::xtream(&credentials, None, StreamFormat::Auto).unwrap();
        assert_eq!(
            play(resolve(Some(&context), &channel, None, CLOCK)).public_only,
            public
        );
        let client = iptv_providers::xtream::XtreamClient::from_credentials(&credentials).unwrap();
        for target in [
            "http://127.0.0.1:8081/stream",
            "http://10.0.0.2/stream",
            "https://cdn.example.test/stream",
        ] {
            assert!(client.stream_must_be_public(target));
        }
    }
}
