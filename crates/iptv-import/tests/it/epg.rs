use crate::common;

use common::synth::{self, NOW, SimResponse};
use common::{Events, FakeFetch, Reply, block_on};
use iptv_epg::{ChannelKey, FeedId, GuideStore, Override, Overrides, StoreOptions};
use iptv_import::{
    CancelToken, EpgImport, EpgSourceConfig, FailureKind, ImportError, MemoryGuide, NoProgress,
    ParseProblem, Phase, ProgressSink, Step, import_epg, import_m3u_text,
};
use iptv_providers::SecretUrl;
use iptv_providers::model::Channel;

const HOUR: i64 = 3600;

fn channels(n: usize) -> Vec<Channel> {
    let text = synth::m3u(
        n,
        4,
        Some("http://fixture.example.test:8089/iptv/guide.xml.gz"),
        "http://fixture.example.test:8089",
    );
    import_m3u_text(text.as_bytes(), &mut NoProgress, &CancelToken::new())
        .unwrap()
        .channels
}

fn source(id: u32, path: &str) -> EpgSourceConfig {
    EpgSourceConfig::new(
        FeedId(id),
        SecretUrl::new(format!("http://fixture.example.test:8089{path}")),
    )
}

/// The import into a [`MemoryGuide`]: how it matched and the guide.
fn import(
    fetch: &FakeFetch,
    sources: &[EpgSourceConfig],
    chans: &[Channel],
    overrides: &Overrides,
    progress: &mut impl ProgressSink,
    cancel: &CancelToken,
) -> Result<(EpgImport, GuideStore), ImportError> {
    let mut sink = MemoryGuide::new();
    let epg = block_on(import_epg(
        fetch, sources, chans, overrides, NOW, &mut sink, progress, cancel,
    ))?;
    let guide = sink.into_guide(&epg);
    Ok((epg, guide))
}

fn run(
    fetch: &FakeFetch,
    sources: &[EpgSourceConfig],
    chans: &[Channel],
) -> Result<(EpgImport, GuideStore), ImportError> {
    import(
        fetch,
        sources,
        chans,
        &Overrides::new(),
        &mut NoProgress,
        &CancelToken::new(),
    )
}

fn key(c: &Channel) -> ChannelKey {
    ChannelKey::new(c.id.as_str())
}

#[test]
fn header_url_tvg_guide_matches_by_tvg_id() {
    let chans = channels(200);
    let text = synth::m3u(
        200,
        4,
        Some("http://fixture.example.test:8089/iptv/guide.xml.gz"),
        "http://fixture.example.test:8089",
    );
    let catalog = import_m3u_text(text.as_bytes(), &mut NoProgress, &CancelToken::new()).unwrap();
    let sources = catalog.epg_sources(1).unwrap();
    assert_eq!(sources.len(), 1);

    let guide = synth::gzip(
        synth::xmltv(
            &synth::m3u_epg_channels(200),
            NOW - 7 * 24 * HOUR,
            NOW + 2 * 24 * HOUR,
            HOUR,
        )
        .as_bytes(),
    );
    let fetch = FakeFetch::body(guide);
    let mut events = Events::default();
    let (epg, guide) = import(
        &fetch,
        &sources,
        &chans,
        &Overrides::new(),
        &mut events,
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(epg.matches.channels, 200);
    assert_eq!(epg.matches.by_tvg_id, 200);
    assert_eq!(epg.bound_count(), 200);
    assert_eq!(guide.bound_count(), 200);
    assert_eq!(epg.listed(FeedId(1)).len(), 200);
    let stats = epg.sources[0].result.as_ref().unwrap();
    assert_eq!(stats.parse.programs, 200 * 9 * 24);
    assert_eq!(stats.skipped, 0);
    let now = guide.program_at(&key(&chans[7]), NOW + 60).unwrap();
    assert_eq!(now.start(), NOW);
    assert_eq!(now.title(), format!("Show 7-{}", 7 * 24));
    assert!(
        events
            .0
            .iter()
            .any(|p| p.phase == Phase::Guide(FeedId(1)) && p.items > 0)
    );
    assert!(events.0.iter().any(|p| p.phase == Phase::Matching));
}

#[test]
fn priorities_names_overrides_and_partial_failure() {
    let chans = channels(10);
    // Source 1 (low priority): ids for all ten. Source 2 (high): names only
    // for the first five, under different ids. Source 3: 404.
    let low = synth::xmltv(&synth::m3u_epg_channels(10), NOW - HOUR, NOW + HOUR, HOUR);
    let high_channels: Vec<(String, String)> = (0..5)
        .map(|i| (format!("hi-{i}"), format!("Synthetic {i}")))
        .collect();
    let high = synth::xmltv(&high_channels, NOW - HOUR, NOW + HOUR, HOUR / 2);
    let fetch = FakeFetch::new(move |_, req| match req.path.as_str() {
        "/low.xml" => SimResponse::ok(low.clone()).into(),
        "/high.xml" => SimResponse::ok(high.clone()).into(),
        _ => SimResponse::status(404).into(),
    });
    let sources = [
        source(1, "/low.xml").with_priority(1),
        source(2, "/high.xml").with_priority(5),
        source(3, "/missing.xml"),
    ];
    let mut overrides = Overrides::new();
    overrides.insert(key(&chans[9]), Override::NoEpg);
    overrides.insert(
        key(&chans[8]),
        Override::Epg {
            source: Some(FeedId(1)),
            epg_id: "ch2.example".into(),
        },
    );
    let (epg, guide) = import(
        &fetch,
        &sources,
        &chans,
        &overrides,
        &mut NoProgress,
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(epg.sources.len(), 3);
    assert_eq!(
        epg.sources[2].result,
        Err(ImportError::HttpStatus {
            step: Step::Guide(FeedId(3)),
            status: 404
        })
    );
    // Channel 0: high-priority source by name wins over tvg-id in the low one.
    assert_eq!(guide.binding(&key(&chans[0])), Some((FeedId(2), "hi-0")));
    assert_eq!(
        guide.binding(&key(&chans[6])),
        Some((FeedId(1), "ch6.example"))
    );
    assert_eq!(
        guide.binding(&key(&chans[8])),
        Some((FeedId(1), "ch2.example"))
    );
    assert_eq!(guide.binding(&key(&chans[9])), None);
    assert_eq!(epg.bound_count(), guide.bound_count());
    assert_eq!(epg.binding(&key(&chans[0])).unwrap().epg_id, "hi-0");
    assert_eq!(epg.matches.disabled, 1);
    assert_eq!(epg.matches.by_override, 1);
}

#[test]
fn every_source_failing_is_an_error() {
    let chans = channels(3);
    let fetch = FakeFetch::new(|_, req| match req.path.as_str() {
        "/html" => SimResponse::ok("<!doctype html><p>nope</p>").into(),
        "/empty" => SimResponse::ok("").into(),
        _ => SimResponse::ok(vec![0x1f, 0x8b, 8, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]).into(),
    });
    let err = run(&fetch, &[source(1, "/html")], &chans)
        .err()
        .expect("an error");
    assert_eq!(
        err,
        ImportError::Parse {
            step: Step::Guide(FeedId(1)),
            problem: ParseProblem::NotXmltv
        }
    );
    let err = run(&fetch, &[source(1, "/empty")], &chans)
        .err()
        .expect("an error");
    assert_eq!(err.kind(), FailureKind::Empty);
    let err = run(&fetch, &[source(1, "/gz")], &chans)
        .err()
        .expect("an error");
    assert!(
        matches!(
            err,
            ImportError::Parse {
                problem: ParseProblem::Decode(_),
                ..
            }
        ),
        "{err:?}"
    );
    let err = run(&fetch, &[source(4, "/html"), source(5, "/empty")], &chans)
        .err()
        .expect("an error");
    assert_eq!(
        err.step(),
        Some(Step::Guide(FeedId(4))),
        "first failure reported"
    );
}

#[test]
fn no_sources_is_an_empty_guide_not_an_error() {
    let (epg, guide) = run(&FakeFetch::body(""), &[], &channels(5)).unwrap();
    assert_eq!(epg.bound_count(), 0);
    assert_eq!(guide.bound_count(), 0);
    assert_eq!(epg.matches.unmatched, 5);
}

#[test]
fn retention_drops_old_programs() {
    let chans = channels(2);
    let body = synth::xmltv(
        &synth::m3u_epg_channels(2),
        NOW - 7 * 24 * HOUR,
        NOW + 24 * HOUR,
        HOUR,
    );
    let mut cfg = source(1, "/g.xml");
    cfg.store = StoreOptions {
        store_descriptions: false,
        past_days_to_keep: Some(1),
    };
    let (epg, guide) = run(&FakeFetch::body(body), &[cfg], &chans).unwrap();
    let ingest = epg.sources[0].result.as_ref().unwrap().ingest;
    assert_eq!(ingest.kept, 2 * 48);
    assert!(ingest.dropped_retention > 0);
    let p = guide.program_at(&key(&chans[0]), NOW).unwrap();
    assert_eq!(p.desc(), None);
}

#[test]
fn cancellation_stops_the_guide() {
    let chans = channels(50);
    let body = synth::xmltv(&synth::m3u_epg_channels(50), NOW - 7 * 24 * HOUR, NOW, HOUR);
    let fetch = FakeFetch::body(body);
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let mut on = |p: &iptv_import::Progress| {
        if p.bytes > 0 {
            c.cancel();
        }
    };
    let err = import(
        &fetch,
        &[source(1, "/g")],
        &chans,
        &Overrides::new(),
        &mut on,
        &cancel,
    );
    assert_eq!(err.err().expect("an error"), ImportError::Canceled);

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let fetch = FakeFetch::new(move |_, _| Reply::Hang(Some(c.clone())));
    let err = import(
        &fetch,
        &[source(1, "/g")],
        &chans,
        &Overrides::new(),
        &mut NoProgress,
        &cancel,
    );
    assert_eq!(err.err().expect("an error"), ImportError::Canceled);
}

#[test]
fn source_user_agent_is_sent() {
    let fetch = FakeFetch::body(synth::xmltv(
        &synth::m3u_epg_channels(1),
        NOW,
        NOW + HOUR,
        HOUR,
    ));
    let mut cfg = source(1, "/g.xml");
    cfg.user_agent = Some("SyntheticEpg/2".into());
    run(&fetch, &[cfg], &channels(1)).unwrap();
    assert_eq!(
        fetch.requests()[0].header("User-Agent"),
        Some("SyntheticEpg/2")
    );
}

#[test]
fn programs_of_channels_nothing_can_match_are_not_kept() {
    // The feed lists 10 channels; the playlist has the first 3.
    let chans = channels(3);
    let body = synth::xmltv(&synth::m3u_epg_channels(10), NOW - HOUR, NOW + HOUR, HOUR);
    let (epg, guide) = run(&FakeFetch::body(body), &[source(1, "/g.xml")], &chans).unwrap();
    let stats = epg.sources[0].result.as_ref().unwrap();
    assert_eq!(stats.parse.programs, 20);
    assert_eq!(stats.skipped, 14);
    assert_eq!(stats.ingest.programs_in, 6);
    assert_eq!(stats.ingest.kept, 6);
    assert_eq!(guide.bound_count(), 3);
    // The feed's channel list is complete (for choosing a guide by hand).
    let listed = epg.listed(FeedId(1));
    assert_eq!(listed.len(), 10);
    assert_eq!(listed[9].id, "ch9.example");
    assert_eq!(listed[9].name.as_deref(), Some("Synthetic 9"));
}

#[test]
fn a_channel_declared_after_its_programs_still_binds() {
    let chans = channels(2);
    let full = synth::xmltv(&synth::m3u_epg_channels(2), NOW - HOUR, NOW + HOUR, HOUR);
    // Move the first <channel> after the programmes.
    let first = full.lines().find(|l| l.starts_with("<channel")).unwrap();
    let body = full
        .replacen(&format!("{first}\n"), "", 1)
        .replace("</tv>", &format!("{first}\n</tv>"));
    let (epg, guide) = run(&FakeFetch::body(body), &[source(1, "/g.xml")], &chans).unwrap();
    assert_eq!(epg.bound_count(), 2);
    let p = guide.program_at(&key(&chans[0]), NOW).unwrap();
    assert_eq!(p.title(), "Show 0-1");
}

/// Records what the import asks of its sink, failing on request.
#[derive(Default)]
struct Recording {
    inner: MemoryGuide,
    calls: Vec<String>,
    fail_after: Option<usize>,
    programs: usize,
}

impl iptv_import::GuideSink for Recording {
    fn begin_feed(
        &mut self,
        feed: FeedId,
        priority: i32,
        store: StoreOptions,
        now: i64,
    ) -> Result<(), iptv_import::SinkError> {
        self.calls.push(format!("begin {}", feed.0));
        self.inner.begin_feed(feed, priority, store, now)
    }

    fn program(
        &mut self,
        channel: &str,
        p: &iptv_epg::ProgramData<'_>,
    ) -> Result<(), iptv_import::SinkError> {
        self.programs += 1;
        if self.fail_after.is_some_and(|n| self.programs > n) {
            return Err(iptv_import::SinkError("disk full".into()));
        }
        self.inner.program(channel, p)
    }

    fn finish_feed(&mut self) -> Result<iptv_epg::IngestReport, iptv_import::SinkError> {
        self.calls.push("finish".into());
        self.inner.finish_feed()
    }

    fn discard_feed(&mut self) -> Result<(), iptv_import::SinkError> {
        self.calls.push("discard".into());
        self.inner.discard_feed()
    }

    fn has_programs(&self, feed: FeedId, epg_id: &str) -> bool {
        self.inner.has_programs(feed, epg_id)
    }
}

#[test]
fn a_failed_feed_is_discarded_and_a_sink_failure_ends_the_import() {
    let chans = channels(2);
    let body = synth::xmltv(&synth::m3u_epg_channels(2), NOW - HOUR, NOW + HOUR, HOUR);
    let fetch = FakeFetch::new(move |_, req| match req.path.as_str() {
        "/g.xml" => SimResponse::ok(body.clone()).into(),
        // A refusal: a 5xx would be tried again (see the retry tests below).
        _ => SimResponse::status(404).into(),
    });
    let sources = [source(1, "/broken"), source(2, "/g.xml")];
    let mut sink = Recording::default();
    let epg = block_on(import_epg(
        &fetch,
        &sources,
        &chans,
        &Overrides::new(),
        NOW,
        &mut sink,
        &mut NoProgress,
        &CancelToken::new(),
    ))
    .unwrap();
    assert_eq!(sink.calls, ["begin 1", "discard", "begin 2", "finish"]);
    assert_eq!(epg.bound_count(), 2);
    assert!(epg.sources[0].result.is_err());

    let mut sink = Recording {
        fail_after: Some(1),
        ..Recording::default()
    };
    let err = block_on(import_epg(
        &fetch,
        &[source(2, "/g.xml"), source(3, "/g.xml")],
        &chans,
        &Overrides::new(),
        NOW,
        &mut sink,
        &mut NoProgress,
        &CancelToken::new(),
    ))
    .unwrap_err();
    assert_eq!(err, ImportError::Storage("disk full".into()));
    assert_eq!(err.kind(), FailureKind::Storage);
    // The failing feed is discarded and the next one never starts.
    assert_eq!(sink.calls, ["begin 2", "discard"]);
}

/// A guide for the first `n` playlist channels.
fn guide_body(n: usize) -> Vec<u8> {
    synth::xmltv(&synth::m3u_epg_channels(n), NOW - HOUR, NOW + HOUR, HOUR).into()
}

fn secs(list: &[u64]) -> Vec<std::time::Duration> {
    list.iter()
        .map(|s| std::time::Duration::from_secs(*s))
        .collect()
}

#[test]
fn a_source_that_uses_up_its_budget_does_not_end_the_others() {
    use iptv_import::{GUIDE_SOURCE_TIME, LimitKind};
    let chans = channels(4);
    let body = guide_body(4);
    let fetch = FakeFetch::new(move |_, req| match req.path.as_str() {
        // Still arriving when the source's own time is over.
        "/slow.xml" => Reply::Slow(
            GUIDE_SOURCE_TIME + std::time::Duration::from_secs(1),
            SimResponse::ok(body.clone()),
        ),
        _ => SimResponse::ok(body.clone()).into(),
    });
    let sources = [
        source(1, "/first.xml").with_priority(3),
        source(2, "/slow.xml").with_priority(2),
        source(3, "/last.xml").with_priority(1),
    ];
    let (epg, guide) = run(&fetch, &sources, &chans).unwrap();
    assert!(epg.sources[0].result.is_ok(), "loaded before the slow one");
    assert!(
        matches!(
            &epg.sources[1].result,
            Err(ImportError::Limit { step: Step::Guide(FeedId(2)), error })
                if error.kind == LimitKind::Elapsed
        ),
        "{:?}",
        epg.sources[1].result
    );
    // The clock is past the slow source's budget; the next one has its own.
    assert!(fetch.clock.get() > GUIDE_SOURCE_TIME);
    assert!(epg.sources[2].result.is_ok(), "{:?}", epg.sources[2].result);
    assert_eq!(guide.bound_count(), 4);
    assert_eq!(epg.listed(FeedId(3)).len(), 4);
    assert!(epg.listed(FeedId(2)).is_empty());
    // A budget is not a passing failure: one request per source, no waits.
    assert_eq!(fetch.requests().len(), 3);
    assert!(fetch.slept.borrow().is_empty());

    // The slow source first: the old shared budget ended the whole import.
    let (epg, guide) = run(&fetch, &[sources[1].clone(), sources[2].clone()], &chans).unwrap();
    assert!(epg.sources[0].result.is_err());
    assert!(epg.sources[1].result.is_ok());
    assert_eq!(guide.bound_count(), 4);
}

#[test]
fn passing_failures_are_retried_after_doubling_waits() {
    let chans = channels(4);
    let body = guide_body(4);
    let clean = {
        let (epg, _) = run(
            &FakeFetch::body(body.clone()),
            &[source(1, "/g.xml")],
            &chans,
        )
        .unwrap();
        epg.sources[0].result.clone().unwrap()
    };
    let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = attempts.clone();
    let fetch = FakeFetch::new(move |_, _| {
        seen.set(seen.get() + 1);
        match seen.get() {
            1 => common::connect_error(),
            2 => SimResponse::status(503).into(),
            // Every programme arrives, then the connection breaks.
            3 => Reply::Truncated(SimResponse::ok(body.clone())),
            _ => SimResponse::ok(body.clone()).into(),
        }
    });
    let (epg, guide) = run(&fetch, &[source(1, "/g.xml")], &chans).unwrap();
    assert_eq!(attempts.get(), 4);
    assert_eq!(*fetch.slept.borrow(), secs(&[2, 4, 8]));
    // The interrupted attempt left nothing behind: counts as a clean import.
    assert_eq!(epg.sources[0].result.as_ref().unwrap(), &clean);
    assert_eq!(guide.bound_count(), 4);
}

#[test]
fn retries_end_after_three_and_the_next_source_still_loads() {
    let chans = channels(2);
    let body = guide_body(2);
    let fetch = FakeFetch::new(move |_, req| match req.path.as_str() {
        "/down.xml" => SimResponse::status(502).into(),
        _ => SimResponse::ok(body.clone()).into(),
    });
    let mut sink = Recording::default();
    let epg = block_on(import_epg(
        &fetch,
        &[source(1, "/down.xml"), source(2, "/up.xml")],
        &chans,
        &Overrides::new(),
        NOW,
        &mut sink,
        &mut NoProgress,
        &CancelToken::new(),
    ))
    .unwrap();
    assert_eq!(
        epg.sources[0].result,
        Err(ImportError::HttpStatus {
            step: Step::Guide(FeedId(1)),
            status: 502
        })
    );
    assert!(epg.sources[1].result.is_ok());
    assert_eq!(epg.bound_count(), 2);
    // Every attempt starts the feed again and is discarded before the next.
    let mut calls = ["begin 1", "discard"].repeat(4);
    calls.extend(["begin 2", "finish"]);
    assert_eq!(sink.calls, calls);
    let paths: Vec<String> = fetch
        .requests()
        .iter()
        .map(|r| r.url.rsplit('/').next().unwrap().to_owned())
        .collect();
    assert_eq!(
        paths,
        ["down.xml", "down.xml", "down.xml", "down.xml", "up.xml"]
    );
    assert_eq!(*fetch.slept.borrow(), secs(&[2, 4, 8]));
}

#[test]
fn refusals_bad_content_and_budgets_are_not_retried() {
    use iptv_import::{FetchError, FetchErrorKind};
    let chans = channels(2);
    let fetch = FakeFetch::new(|_, req| {
        let network = |kind| Reply::Error(FetchError::new(kind, "synthetic"));
        match req.path.as_str() {
            "/404" => SimResponse::status(404).into(),
            "/401" => SimResponse::status(401).into(),
            "/429" => SimResponse::status(429).into(),
            "/html" => SimResponse::ok("<!doctype html><p>nope</p>").into(),
            "/tls" => network(FetchErrorKind::Tls),
            "/redirect" => network(FetchErrorKind::Redirect),
            "/deadline" => network(FetchErrorKind::Deadline),
            "/large" => network(FetchErrorKind::TooLarge),
            _ => network(FetchErrorKind::InvalidRequest),
        }
    });
    let paths = [
        "/404",
        "/401",
        "/429",
        "/html",
        "/tls",
        "/redirect",
        "/deadline",
        "/large",
        "/invalid",
    ];
    let sources: Vec<EpgSourceConfig> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| source(i as u32 + 1, path))
        .collect();
    let err = run(&fetch, &sources, &chans).err().expect("all failed");
    assert_eq!(
        err,
        ImportError::HttpStatus {
            step: Step::Guide(FeedId(1)),
            status: 404
        }
    );
    assert_eq!(fetch.requests().len(), paths.len(), "one request each");
    assert!(fetch.slept.borrow().is_empty());
}

#[test]
fn canceling_during_a_wait_ends_the_import() {
    let chans = channels(2);
    let fetch = FakeFetch::new(|_, _| common::connect_error());
    let cancel = CancelToken::new();
    *fetch.cancel_on_sleep.borrow_mut() = Some(cancel.clone());
    let err = import(
        &fetch,
        &[source(1, "/g.xml"), source(2, "/h.xml")],
        &chans,
        &Overrides::new(),
        &mut NoProgress,
        &cancel,
    );
    assert_eq!(err.err().expect("an error"), ImportError::Canceled);
    assert_eq!(fetch.requests().len(), 1);
    assert_eq!(*fetch.slept.borrow(), secs(&[2]));
}

#[test]
fn header_guide_planning_checks_identity_exhaustion_before_building_configs() {
    let header = iptv_providers::m3u::M3uHeader {
        epg_urls: vec![
            "https://guide.example.test/one.xml".into(),
            "https://guide.example.test/two.xml".into(),
        ],
        ..Default::default()
    };
    assert!(matches!(
        EpgSourceConfig::from_m3u_header(&header, u32::MAX),
        Err(iptv_library::guides::GuideFeedError::Exhausted)
    ));
    let at_end = EpgSourceConfig::from_m3u_header(&header, u32::MAX - 1).unwrap();
    assert_eq!(
        at_end.iter().map(|source| source.id.0).collect::<Vec<_>>(),
        [u32::MAX - 1, u32::MAX]
    );
    assert_eq!(
        at_end
            .iter()
            .map(|source| source.priority)
            .collect::<Vec<_>>(),
        [2, 1]
    );
}
