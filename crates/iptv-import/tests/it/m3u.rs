use crate::common;

use common::synth::{self, SimResponse};
use common::{Events, FakeFetch, Reply, block_on, connect_error};
use iptv_import::{
    AuthProblem, CancelToken, FailureKind, FetchErrorKind, ImportError, ImportOptions, NoProgress,
    ParseProblem, Phase, SourceMeta, Step, import_m3u_text, import_source,
};
use iptv_providers::SecretUrl;
use iptv_providers::model::{CatchUpKind, M3uSourceConfig, SourceConfig};

const URL: &str = "http://fixture.example.test:8089/iptv/playlist.m3u";

fn config(url: &str) -> SourceConfig {
    SourceConfig::M3u(M3uSourceConfig {
        location: SecretUrl::new(url),
        user_agent: Some("SyntheticPlayer/1.0".into()),
        referrer: Some("http://ref.example.com/".into()),
    })
}

fn run(fetch: &FakeFetch) -> Result<iptv_import::Catalog, ImportError> {
    block_on(import_source(
        fetch,
        &config(URL),
        &ImportOptions::new(synth::NOW),
        &mut NoProgress,
        &CancelToken::new(),
    ))
}

fn playlist(n: usize) -> String {
    synth::m3u(
        n,
        24,
        Some("http://fixture.example.test:8089/iptv/guide.xml.gz"),
        "http://fixture.example.test:8089",
    )
}

#[test]
fn imports_a_streamed_playlist_with_headers_and_progress() {
    let fetch = FakeFetch::body(playlist(3000));
    let mut events = Events::default();
    let catalog = block_on(import_source(
        &fetch,
        &config(URL),
        &ImportOptions::new(synth::NOW),
        &mut events,
        &CancelToken::new(),
    ))
    .unwrap();
    assert_eq!(catalog.channels.len(), 3000);
    assert_eq!(catalog.groups.len(), 24);
    assert!(catalog.warnings.is_empty());
    let first = &catalog.channels[0];
    assert_eq!(first.epg_id.as_deref(), Some("ch0.example"));
    assert_eq!(first.catch_up.as_ref().unwrap().kind, CatchUpKind::Default);
    assert_eq!(first.catch_up.as_ref().unwrap().days, Some(3));
    let SourceMeta::M3u { header } = &catalog.source else {
        panic!("not m3u")
    };
    assert_eq!(
        header.epg_urls,
        ["http://fixture.example.test:8089/iptv/guide.xml.gz"]
    );
    let epg = catalog.epg_sources(1).unwrap();
    assert_eq!(epg.len(), 1);
    assert_eq!(
        epg[0].url.expose(),
        "http://fixture.example.test:8089/iptv/guide.xml.gz"
    );
    assert_eq!(catalog.guide_urls(), [epg[0].url.clone()]);

    let reqs = fetch.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].header("user-agent"), Some("SyntheticPlayer/1.0"));
    assert_eq!(reqs[0].header("referer"), Some("http://ref.example.com/"));
    assert_eq!(catalog.stats.requests, 1);
    assert!(catalog.stats.bytes > 300_000);

    let downloads: Vec<_> = events
        .0
        .iter()
        .filter(|p| p.phase == Phase::Playlist && p.bytes > 0)
        .collect();
    assert!(downloads.len() >= 2, "throttled but incremental");
    assert!(downloads.windows(2).all(|w| w[0].bytes < w[1].bytes));
    assert!(downloads.windows(2).all(|w| w[0].items <= w[1].items));
    assert_eq!(downloads.last().unwrap().items, 3000);
    assert_eq!(downloads[0].total_bytes, Some(catalog.stats.bytes));
    assert_eq!(events.0.last().unwrap().phase, Phase::Done);
}

#[test]
fn gzip_playlists_are_detected() {
    let fetch = FakeFetch::body(synth::gzip(playlist(500).as_bytes()));
    assert_eq!(run(&fetch).unwrap().channels.len(), 500);
    // Even when the first chunk holds a single byte.
    let mut fetch = FakeFetch::body(synth::gzip(playlist(50).as_bytes()));
    fetch.chunk = 1;
    assert_eq!(run(&fetch).unwrap().channels.len(), 50);
}

#[test]
fn corrupt_gzip_is_a_decode_failure() {
    let mut gz = synth::gzip(playlist(200).as_bytes());
    let mid = gz.len() / 2;
    gz[mid..].iter_mut().for_each(|b| *b ^= 0x5a);
    let err = run(&FakeFetch::body(gz)).unwrap_err();
    assert!(
        matches!(
            &err,
            ImportError::Parse {
                problem: ParseProblem::Decode(_),
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn failures_are_never_empty_catalogs() {
    let cases: Vec<(&str, Vec<u8>, FailureKind)> = vec![
        (
            "hls",
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nseg1.ts\n".to_vec(),
            FailureKind::Parse,
        ),
        (
            "html",
            b"<!doctype html><html><body>Service unavailable</body></html>".to_vec(),
            FailureKind::Parse,
        ),
        (
            "json",
            br#"{"error":"invalid credentials"}"#.to_vec(),
            FailureKind::Parse,
        ),
        (
            "words",
            b"just some text\nnothing here\n".to_vec(),
            FailureKind::Parse,
        ),
        (
            "malformed",
            b"#EXTM3U\n#EXTINF:-1,One\n#EXTINF:-1,Two\n".to_vec(),
            FailureKind::Parse,
        ),
        ("header only", b"#EXTM3U\n".to_vec(), FailureKind::Empty),
        ("empty", Vec::new(), FailureKind::Empty),
        ("blank", b"\r\n  \n".to_vec(), FailureKind::Empty),
    ];
    for (name, body, kind) in cases {
        let err = run(&FakeFetch::body(body)).unwrap_err();
        assert_eq!(err.kind(), kind, "{name}: {err:?}");
        assert_eq!(err.step(), Some(Step::Playlist), "{name}");
    }
    let err = run(&FakeFetch::body(
        b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nlow.m3u8\n".to_vec(),
    ))
    .unwrap_err();
    assert!(matches!(
        err,
        ImportError::Parse {
            problem: ParseProblem::HlsManifest,
            ..
        }
    ));
    let err = run(&FakeFetch::body(b"#EXTM3U\n#EXTINF:-1,A\n".to_vec())).unwrap_err();
    assert!(matches!(
        err,
        ImportError::Parse {
            problem: ParseProblem::NoUsableEntries { skipped: 1 },
            ..
        }
    ));
}

#[test]
fn http_and_network_failures_are_typed() {
    let err = run(&FakeFetch::new(|_, _| SimResponse::status(404).into())).unwrap_err();
    assert_eq!(
        err,
        ImportError::HttpStatus {
            step: Step::Playlist,
            status: 404
        }
    );
    assert!(!err.is_transient());
    let err = run(&FakeFetch::new(|_, _| SimResponse::status(503).into())).unwrap_err();
    assert!(err.is_transient());
    let err = run(&FakeFetch::new(|_, _| SimResponse::status(403).into())).unwrap_err();
    assert_eq!(
        err,
        ImportError::Auth {
            step: Step::Playlist,
            problem: AuthProblem::HttpStatus(403)
        }
    );
    let err = run(&FakeFetch::new(|_, _| connect_error())).unwrap_err();
    assert!(
        matches!(&err, ImportError::Network { error, .. } if error.kind == FetchErrorKind::Connect)
    );
    let body = playlist(100);
    let err = run(&FakeFetch::new(move |_, _| {
        Reply::Truncated(SimResponse::ok(body.as_bytes()[..body.len() / 2].to_vec()))
    }))
    .unwrap_err();
    assert!(
        matches!(&err, ImportError::Network { error, .. } if error.kind == FetchErrorKind::Body),
        "a dropped connection is not a short playlist: {err:?}"
    );
}

#[test]
fn plain_url_lists_without_header_import() {
    let body = "http://fixture.example.test:8089/ch0/index.m3u8\nhttp://fixture.example.test:8089/ch1/index.m3u8\n";
    let catalog = run(&FakeFetch::body(body)).unwrap();
    assert_eq!(catalog.channels.len(), 2);
    assert_eq!(catalog.channels[1].name, "Unnamed channel");
    assert_eq!(catalog.channels[1].epg_name.as_deref(), Some("index"));
}

#[test]
fn blank_location_is_a_config_error() {
    let fetch = FakeFetch::body("");
    let err = block_on(import_source(
        &fetch,
        &config("  "),
        &ImportOptions::new(synth::NOW),
        &mut NoProgress,
        &CancelToken::new(),
    ))
    .unwrap_err();
    assert_eq!(err.kind(), FailureKind::Config);
    assert!(fetch.requests().is_empty());
}

#[test]
fn cancellation_before_during_and_in_flight() {
    let opts = ImportOptions::new(synth::NOW);
    // Before start: no request.
    let fetch = FakeFetch::body(playlist(10));
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = block_on(import_source(
        &fetch,
        &config(URL),
        &opts,
        &mut NoProgress,
        &cancel,
    ));
    assert_eq!(err.unwrap_err(), ImportError::Canceled);
    assert!(fetch.requests().is_empty());

    // Between chunks: the progress callback cancels.
    let fetch = FakeFetch::body(playlist(5000));
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let mut seen = 0;
    let mut on_progress = |p: &iptv_import::Progress| {
        if p.bytes > 0 {
            seen += 1;
            c2.cancel();
        }
    };
    let err = block_on(import_source(
        &fetch,
        &config(URL),
        &opts,
        &mut on_progress,
        &cancel,
    ));
    assert_eq!(err.unwrap_err(), ImportError::Canceled);
    assert_eq!(seen, 1);

    // While the request hangs: another thread cancels and wakes the import.
    let cancel = CancelToken::new();
    let c3 = cancel.clone();
    let fetch = FakeFetch::new(move |_, _| Reply::Hang(Some(c3.clone())));
    let err = block_on(import_source(
        &fetch,
        &config(URL),
        &opts,
        &mut NoProgress,
        &cancel,
    ));
    assert_eq!(err.unwrap_err(), ImportError::Canceled);
}

#[test]
fn text_import_from_memory() {
    let text = playlist(1200);
    let mut events = Events::default();
    let catalog = import_m3u_text(text.as_bytes(), &mut events, &CancelToken::new()).unwrap();
    assert_eq!(catalog.channels.len(), 1200);
    assert_eq!(catalog.stats.requests, 0);
    assert!(
        events
            .0
            .iter()
            .any(|p| p.phase == Phase::Playlist && p.items > 0)
    );
    let gz = synth::gzip(text.as_bytes());
    let catalog = import_m3u_text(&gz, &mut NoProgress, &CancelToken::new()).unwrap();
    assert_eq!(catalog.channels.len(), 1200);
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        import_m3u_text(text.as_bytes(), &mut NoProgress, &cancel).unwrap_err(),
        ImportError::Canceled
    );
    assert_eq!(
        import_m3u_text(b"<html></html>", &mut NoProgress, &CancelToken::new())
            .unwrap_err()
            .kind(),
        FailureKind::Parse
    );
}

#[test]
fn warnings_from_skipped_lines_are_kept() {
    let mut text = playlist(20);
    text.push_str("#EXTINF:-1,Dangling\n");
    let catalog = import_m3u_text(text.as_bytes(), &mut NoProgress, &CancelToken::new()).unwrap();
    assert_eq!(catalog.channels.len(), 20);
    assert_eq!(catalog.stats.skipped, 1);
    assert_eq!(catalog.warnings.len(), 1);
    assert_eq!(catalog.warnings[0].step, Step::Playlist);
}

#[test]
fn unusable_stream_entries_do_not_reject_other_channels() {
    let text = concat!(
        "#EXTM3U\n",
        "#EXTINF:-1 group-title=\"Generated\",First generated channel\n",
        "https://media.example.test/first.m3u8\n",
        "#EXTINF:-1 group-title=\"Discarded\",Unsupported generated stream\n",
        "rtsp://media.example.test/live\n",
        "#EXTINF:-1 group-title=\"Also discarded\",Another unsupported generated stream\n",
        "rtmp://media.example.test/live\n",
        "#EXTINF:-1,Generated placeholder\n",
        "#EXTGRP:Discarded placeholder group\n",
        "not-a-stream-address\n",
        "#EXTINF:-1,Malformed generated address\n",
        "https://\n",
        "#EXTINF:-1 group-title=\"Final generated\",Last generated channel\n",
        "https://media.example.test/last.m3u8",
    );
    let assert_summary = |catalog: &iptv_import::Catalog| {
        assert_eq!(catalog.stats.content_items, 2);
        assert_eq!(catalog.stats.skipped, 4);
        assert_eq!(catalog.warning_count(), 1);
        assert_eq!(catalog.warnings[0].step, Step::Playlist);
        assert!(!catalog.warnings[0].message.contains("example.test"));
        assert!(!catalog.warnings[0].message.contains("not-a-stream-address"));
    };
    let assert_channels = |channels: &[iptv_providers::model::Channel]| {
        let names: Vec<_> = channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(names, ["First generated channel", "Last generated channel"]);
    };
    let assert_groups = |groups: &[iptv_providers::model::Group]| {
        let names: Vec<_> = groups.iter().map(|group| group.name.as_str()).collect();
        assert_eq!(names, ["Generated", "Final generated"]);
    };
    let memory = import_m3u_text(text.as_bytes(), &mut NoProgress, &CancelToken::new()).unwrap();
    assert_summary(&memory);
    assert_channels(&memory.channels);
    assert_groups(&memory.groups);
    let buffered = run(&FakeFetch::body(text)).unwrap();
    assert_summary(&buffered);
    assert_channels(&buffered.channels);
    assert_groups(&buffered.groups);
    let mut stage = Stage::default();
    let mut fetch = FakeFetch::body(text);
    fetch.chunk = 19;
    let streamed = block_on(iptv_import::import_source_into(
        &fetch,
        &config(URL),
        &ImportOptions::new(synth::NOW),
        &mut NoProgress,
        &CancelToken::new(),
        &mut stage,
    ))
    .unwrap();
    assert!(stage.finished);
    assert_summary(&streamed);
    assert_channels(&stage.channels);
    assert_groups(&stage.groups);
    assert!(
        streamed.channels.is_empty(),
        "rows went to the private stage"
    );

    let private =
        format!("{text}\n#EXTINF:-1,Generated private destination\nhttp://127.0.0.1/private\n");
    let mut stage = Stage::default();
    assert!(matches!(
        block_on(iptv_import::import_source_into(
            &FakeFetch::body(private),
            &config(URL),
            &ImportOptions::new(synth::NOW),
            &mut NoProgress,
            &CancelToken::new(),
            &mut stage,
        )),
        Err(ImportError::InvalidConfig(_))
    ));
    assert!(
        !stage.finished,
        "an unsafe destination cannot publish a stage"
    );
    assert_channels(&stage.channels);
}

#[test]
fn picked_loopback_playlist_accepts_only_its_own_origin() {
    let picked = config("http://127.0.0.1:18097/lineup.m3u");
    for (same_request_origin, url, accepted) in [
        (true, "http://127.0.0.1:18097/live.ts", true),
        (true, "http://127.0.0.1:18098/live.ts", false),
        (true, "http://10.0.0.1:18097/live.ts", false),
        (true, "https://media.example.test/live.ts", true),
        (false, "http://127.0.0.1:18097/live.ts", false),
    ] {
        let text = format!("#EXTM3U\n#EXTINF:-1,Synthetic\n{url}");
        let mut buffered = FakeFetch::body(text.clone());
        buffered.same_request_origin = same_request_origin;
        let result = block_on(import_source(
            &buffered,
            &picked,
            &ImportOptions::new(synth::NOW),
            &mut NoProgress,
            &CancelToken::new(),
        ));
        if accepted {
            assert_eq!(result.unwrap().channels.len(), 1, "{url}");
        } else {
            assert!(
                matches!(result, Err(ImportError::InvalidConfig(_))),
                "{url}"
            );
        }

        let mut fetch = FakeFetch::body(format!("{text}\n"));
        fetch.same_request_origin = same_request_origin;
        let mut stage = Stage::default();
        let result = block_on(iptv_import::import_source_into(
            &fetch,
            &picked,
            &ImportOptions::new(synth::NOW),
            &mut NoProgress,
            &CancelToken::new(),
            &mut stage,
        ));
        assert_eq!(result.is_ok(), accepted, "{url}");
        assert_eq!(stage.finished, accepted, "{url}");
        assert_eq!(stage.channels.len(), usize::from(accepted), "{url}");
    }
}

#[derive(Default)]
struct Stage {
    channels: Vec<iptv_providers::model::Channel>,
    groups: Vec<iptv_providers::model::Group>,
    finished: bool,
}

impl iptv_import::CatalogSink for Stage {
    fn push(&mut self, row: iptv_import::CatalogRow) -> Result<(), iptv_import::SinkError> {
        match row {
            iptv_import::CatalogRow::Channel(channel) => self.channels.push(channel),
            iptv_import::CatalogRow::Group(group) => self.groups.push(group),
            _ => {}
        }
        Ok(())
    }
    fn end_step(&mut self, success: bool) -> Result<(), iptv_import::SinkError> {
        assert!(success);
        Ok(())
    }
    fn finish(&mut self) -> Result<(), iptv_import::SinkError> {
        self.finished = true;
        Ok(())
    }
}

#[test]
fn all_unusable_stream_entries_report_no_usable_entries() {
    // The final malformed entry has no newline, so it completes at EOF.
    let text = concat!(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Discarded\",Unsupported generated stream\n",
        "rtmp://media.example.test/live\n",
        "#EXTINF:-1,Generated placeholder\n#EXTGRP:Discarded at EOF\nnot-a-stream-address",
    );
    let matches_failure = |error: ImportError| {
        assert!(matches!(
            error,
            ImportError::Parse {
                step: Step::Playlist,
                problem: ParseProblem::NoUsableEntries { skipped: 2 },
            }
        ));
    };
    matches_failure(
        import_m3u_text(text.as_bytes(), &mut NoProgress, &CancelToken::new()).unwrap_err(),
    );
    matches_failure(run(&FakeFetch::body(text)).unwrap_err());
    let mut stage = Stage::default();
    matches_failure(
        block_on(iptv_import::import_source_into(
            &FakeFetch::body(text),
            &config(URL),
            &ImportOptions::new(synth::NOW),
            &mut NoProgress,
            &CancelToken::new(),
            &mut stage,
        ))
        .unwrap_err(),
    );
    assert!(!stage.finished);
    assert!(stage.channels.is_empty());
    assert!(stage.groups.is_empty());
}

#[test]
fn embedded_local_and_private_destinations_fail_without_a_partial_catalog() {
    for url in [
        "file:///etc/hosts",
        "file://[invalid]/generated.ts",
        "FiLe://localhost:80/generated.ts",
        "\u{1}file:///synthetic/generated.ts",
        "fi\tle:///synthetic/generated.ts",
        "\u{1}/synthetic/generated.ts",
        "\u{1}C:/Synthetic/generated.ts",
        "\u{1}../generated.ts",
        "/etc/hosts",
        "./generated.ts",
        "../generated.ts",
        ".\\generated.ts",
        "..\\generated.ts",
        "C:\\Synthetic\\generated.ts",
        "C:/Synthetic/generated.ts",
        "C:generated.ts",
        "~/generated.ts",
        "~\\generated.ts",
        "http://127.0.0.1/private",
        "http://10.0.0.1/private",
        "http://[::1]/private",
    ] {
        for text in [
            format!(
                "#EXTM3U url-tvg=\"{url}\"\n#EXTINF:-1,Synthetic\nhttps://media.example.test/live\n"
            ),
            format!("#EXTM3U\n#EXTINF:-1,Synthetic\n{url}\n"),
            format!(
                "#EXTM3U\n#EXTINF:-1,Generated valid channel\nhttps://media.example.test/live\n\
                 #EXTINF:-1,Generated rejected destination\n{url}\n"
            ),
        ] {
            assert!(matches!(
                import_m3u_text(text.as_bytes(), &mut NoProgress, &CancelToken::new()),
                Err(ImportError::InvalidConfig(_))
            ));
        }
    }
}

#[test]
fn surplus_header_guides_warn_without_rejecting_or_fetching_them() {
    let cap = iptv_library::guides::MAX_PROVIDER_GUIDES;
    let mut urls: Vec<_> = (0..cap)
        .map(|index| format!("https://guide.example.test/{index}.xml"))
        .collect();
    // A surplus destination must not be validated or fetched after the cap.
    urls.push("http://127.0.0.1/synthetic-ignored.xml".into());
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n#EXTINF:-1,Generated channel\nhttps://media.example.test/live.ts\n",
        urls.join(",")
    );
    let fetch = FakeFetch::body(playlist);
    let catalog = run(&fetch).unwrap();
    assert_eq!(catalog.channels.len(), 1);
    assert_eq!(fetch.requests().len(), 1, "only the playlist was requested");
    assert_eq!(catalog.guide_urls().len(), cap);
    let guides = catalog.epg_sources(1).unwrap();
    assert_eq!(guides.len(), cap);
    assert_eq!(guides.last().unwrap().id.0, cap as u32);
    assert_eq!(catalog.warning_count(), 1);
    assert!(
        catalog.warnings[0]
            .message
            .contains("ignored 1 surplus provider guide URLs")
    );
    assert!(!format!("{:?}", catalog.warnings).contains("synthetic-ignored"));
    assert!(matches!(
        catalog.epg_sources(u32::MAX),
        Err(iptv_library::guides::GuideFeedError::Exhausted)
    ));
}
