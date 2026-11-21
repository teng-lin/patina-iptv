//! Stalker/Ministra import: endpoint discovery → auth loop → genres →
//! paginated channels (and optionally VOD).
//!
//! Discovery probes each candidate endpoint with a token-less
//! `get_genres`: data proves a token-free panel; an auth refusal is
//! confirmed with a full handshake + profile. A connection failure aborts
//! (every candidate shares the host); timeouts and non-portal answers move
//! on. API calls rejected for the token re-run the auth loop and retry once.

use iptv_providers::model::{Channel, ContentKind, Series, StalkerCredentials, VodItem};
use iptv_providers::stalker::{
    ApiError, AuthStep, Page, PagedCrawl, PortalMode, ProbeResult, StalkerPortal, StalkerSession,
    classify_probe, endpoint_candidates, parse_channel_page, parse_genres, parse_series_page,
    parse_vod_page,
};
use iptv_providers::{HttpRequest, HttpResponse, ProviderError, StalkerError, StalkerErrorKind};

use crate::catalog::{Catalog, CatalogWarning, ImportOptions, SourceMeta};
use crate::ctx::Ctx;
use crate::error::{AuthProblem, ImportError, ParseProblem, Step};
use crate::fetch::Fetch;
use crate::progress::{Phase, Progress, ProgressSink};
use crate::xtream::{soften, soften_items};

pub(crate) async fn import_stalker<F: Fetch, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    creds: &StalkerCredentials,
    opts: &ImportOptions,
) -> Result<Catalog, ImportError> {
    let mut session = discover(ctx, creds).await?;
    let live = ContentKind::Live;

    ctx.phase(Phase::Groups(live));
    let genres = call(
        ctx,
        &mut session,
        Step::Categories(live),
        |s| s.genres(),
        |r| parse_genres(r, live),
    )
    .await;
    if let Err(e) = &genres
        && (e.is_fatal() || matches!(e, ImportError::Network { .. }))
    {
        return Err(e.clone());
    }
    let channels = crawl::<_, _, Channel>(
        ctx,
        &mut session,
        live,
        opts,
        opts.stalker_channel_cap,
        |s, p| s.channels_page("*", p),
        parse_channel_page,
    )
    .await?;

    let mut vod = None;
    if opts.include_vod {
        let kind = ContentKind::Movie;
        ctx.phase(Phase::Groups(kind));
        let cats = call(
            ctx,
            &mut session,
            Step::Categories(kind),
            |s| s.vod_categories(),
            |r| parse_genres(r, kind),
        )
        .await;
        let items = crawl::<_, _, VodItem>(
            ctx,
            &mut session,
            kind,
            opts,
            opts.stalker_vod_cap,
            |s, p| s.vod_page("*", p),
            parse_vod_page,
        )
        .await;
        vod = Some((cats, items));
    }

    let mut series = None;
    if opts.include_series {
        let kind = ContentKind::Series;
        let cats = call(
            ctx,
            &mut session,
            Step::Categories(kind),
            |s| s.series_categories(),
            |r| parse_genres(r, kind),
        )
        .await;
        let items = crawl::<_, _, Series>(
            ctx,
            &mut session,
            kind,
            opts,
            opts.stalker_vod_cap,
            |s, p| s.series_page("", None, p, true),
            parse_series_page,
        )
        .await;
        series = Some((cats, items));
    }
    let mut catalog = Catalog::new(SourceMeta::Stalker {
        session: Box::new(session),
    });
    if let Some(import) = soften(&mut catalog, genres)? {
        let groups = catalog.absorb(Step::Categories(live), None, import);
        catalog.groups.extend(groups);
    }
    catalog.channels = channels.into_catalog(&mut catalog);
    let referenced: Vec<_> = catalog
        .channels
        .iter()
        .flat_map(|c| c.groups.clone())
        .collect();
    catalog.add_missing_groups(live, referenced.iter());

    if let Some((cats, items)) = vod {
        let kind = ContentKind::Movie;
        if let Some(import) = soften(&mut catalog, cats)? {
            let groups = catalog.absorb(Step::Categories(kind), None, import);
            catalog.groups.extend(groups);
        }
        if let Some(items) = soften_items(&mut catalog, kind, items)? {
            catalog.vod = items.into_catalog(&mut catalog);
            let referenced: Vec<_> = catalog.vod.iter().flat_map(|v| v.groups.clone()).collect();
            catalog.add_missing_groups(kind, referenced.iter());
        }
    }

    if let Some((cats, items)) = series {
        let kind = ContentKind::Series;
        if let Some(import) = soften(&mut catalog, cats)? {
            let groups = catalog.absorb(Step::Categories(kind), None, import);
            catalog.groups.extend(groups);
        }
        if let Some(items) = soften_items(&mut catalog, kind, items)? {
            catalog.series = items.into_catalog(&mut catalog);
            let referenced: Vec<_> = catalog
                .series
                .iter()
                .flat_map(|v| v.groups.clone())
                .collect();
            catalog.add_missing_groups(kind, referenced.iter());
        }
    }
    if let Some(sink) = &mut ctx.catalog_sink {
        sink.begin_step()
            .map_err(|e| ImportError::Storage(e.to_string()))?;
        for group in std::mem::take(&mut catalog.groups) {
            sink.push(crate::CatalogRow::Group(group))
                .map_err(|e| ImportError::Storage(e.to_string()))?;
        }
        sink.end_step(true)
            .map_err(|e| ImportError::Storage(e.to_string()))?;
    }
    if catalog.is_empty() {
        return Err(ImportError::Empty {
            step: Step::Items(live),
        });
    }
    catalog.set_transport_stats(&ctx.stats);
    ctx.phase(Phase::Done);
    Ok(catalog)
}

/// Finds the API endpoint and mode, and authorizes a session on it.
pub(crate) async fn discover<F: Fetch, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    creds: &StalkerCredentials,
) -> Result<StalkerSession, ImportError> {
    let step = Step::PortalDiscovery;
    let candidates = endpoint_candidates(&creds.portal_url).map_err(config)?;
    let mut last = None;
    for endpoint in candidates {
        ctx.phase(Phase::Connecting);
        let portal =
            StalkerPortal::from_credentials(creds, &endpoint, PortalMode::Full).map_err(config)?;
        let resp = match ctx.send(step, portal.probe_request()).await {
            Ok(r) => r,
            Err(ImportError::Network { step, error }) if !error.kind.is_host_failure() => {
                last = Some(ImportError::Network { step, error });
                continue;
            }
            Err(e) => return Err(e),
        };
        match classify_probe(&resp) {
            ProbeResult::Data => {
                let portal = StalkerPortal::from_credentials(creds, &endpoint, PortalMode::Simple)
                    .map_err(config)?;
                return Ok(StalkerSession::new(portal));
            }
            ProbeResult::AuthRequired => {
                let mut session = StalkerSession::new(portal);
                match authenticate(ctx, &mut session).await {
                    Ok(()) => return Ok(session),
                    // Not a working token endpoint after all: try the next.
                    Err(
                        e @ ImportError::Auth {
                            problem:
                                AuthProblem::Stalker(StalkerError {
                                    kind: StalkerErrorKind::NoToken,
                                    ..
                                }),
                            ..
                        },
                    )
                    | Err(e @ ImportError::Portal { .. }) => last = Some(e),
                    Err(e) => return Err(e),
                }
            }
            ProbeResult::NotAPortal => {}
        }
    }
    Err(last.unwrap_or(ImportError::parse(step, ParseProblem::NotAPortal)))
}

fn config(e: ProviderError) -> ImportError {
    match e {
        ProviderError::InvalidConfig(why) => ImportError::InvalidConfig(why),
        other => ImportError::InvalidConfig(other.to_string()),
    }
}

/// Runs the session's auth loop to `Ready`.
pub(crate) async fn authenticate<F: Fetch, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    session: &mut StalkerSession,
) -> Result<(), ImportError> {
    let step = Step::Authentication;
    ctx.phase(Phase::Authenticating);
    loop {
        match session.poll() {
            AuthStep::Send(req) => {
                let resp = ctx.send(step, req).await?;
                session
                    .on_auth_response(&resp)
                    .map_err(|e| ImportError::stalker(step, e))?;
            }
            AuthStep::Ready => return Ok(()),
            AuthStep::Failed(e) => return Err(ImportError::stalker(step, e)),
            AuthStep::Waiting => {
                return Err(ImportError::stalker(
                    step,
                    StalkerError::new(StalkerErrorKind::NotAuthorized),
                ));
            }
        }
    }
}

/// One API call; a token rejection re-authenticates and retries once.
pub(crate) async fn call<F: Fetch, P: ProgressSink, T>(
    ctx: &mut Ctx<'_, F, P>,
    session: &mut StalkerSession,
    step: Step,
    build: impl Fn(&StalkerSession) -> Result<HttpRequest, StalkerError>,
    parse: impl Fn(&HttpResponse) -> Result<T, ProviderError>,
) -> Result<T, ImportError> {
    let mut retried = false;
    loop {
        let req = build(session).map_err(|e| ImportError::stalker(step, e))?;
        let resp = ctx.send(step, req).await?;
        match parse(&resp) {
            Ok(v) => {
                if retried {
                    // Resets the session's rejection counter after recovery.
                    let _ = session.check_api_response(&resp);
                }
                return Ok(v);
            }
            Err(ProviderError::Stalker(e))
                if !retried
                    && matches!(
                        e.kind,
                        StalkerErrorKind::AuthFailed | StalkerErrorKind::Blocked
                    ) =>
            {
                match session.check_api_response(&resp) {
                    Err(ApiError::Reauthenticate) => {
                        authenticate(ctx, session).await?;
                        retried = true;
                    }
                    Err(ApiError::Provider(pe)) => return Err(ImportError::provider(step, pe)),
                    Ok(_) => return Err(ImportError::stalker(step, e)),
                }
            }
            Err(e) => return Err(ImportError::provider(step, e)),
        }
    }
}

/// [`PagedCrawl`] for the row types it supports (its bound is private to
/// the providers crate).
trait Crawl: Sized {
    fn crawl(cap: usize) -> PagedCrawl<Self>;
    fn next_page(c: &PagedCrawl<Self>) -> Option<u32>;
    fn push(c: &mut PagedCrawl<Self>, page: Page<Self>);
    fn abort(c: &mut PagedCrawl<Self>);
    fn into_items(c: PagedCrawl<Self>) -> Vec<Self>;
    fn take_items(c: &mut PagedCrawl<Self>) -> Vec<Self>;
    fn row(self) -> crate::CatalogRow;
}

macro_rules! crawlable {
    ($t:ty, $variant:ident) => {
        impl Crawl for $t {
            fn crawl(cap: usize) -> PagedCrawl<Self> {
                PagedCrawl::new(cap)
            }
            fn next_page(c: &PagedCrawl<Self>) -> Option<u32> {
                c.next_page()
            }
            fn push(c: &mut PagedCrawl<Self>, page: Page<Self>) {
                c.push(page)
            }
            fn abort(c: &mut PagedCrawl<Self>) {
                c.abort()
            }
            fn take_items(c: &mut PagedCrawl<Self>) -> Vec<Self> {
                c.take_items()
            }
            fn row(self) -> crate::CatalogRow {
                crate::CatalogRow::$variant(self)
            }
            fn into_items(c: PagedCrawl<Self>) -> Vec<Self> {
                c.into_items()
            }
        }
    };
}
crawlable!(Channel, Channel);
crawlable!(VodItem, Vod);
crawlable!(Series, Series);

/// Items from a crawl plus what to report about them.
struct Crawled<T> {
    items: Vec<T>,
    skipped: usize,
    warnings: Vec<CatalogWarning>,
    dropped: usize,
    emitted: u64,
}

impl<T> Crawled<T> {
    fn push_warning(&mut self, warning: CatalogWarning) {
        if self.warnings.len() < iptv_providers::model::MAX_STORED_WARNINGS {
            self.warnings.push(warning);
        } else {
            self.dropped += 1;
        }
    }

    fn into_catalog(self, catalog: &mut Catalog) -> Vec<T> {
        catalog.stats.content_items += self.emitted;
        catalog.stats.skipped += self.skipped;
        catalog.warnings_dropped += self.dropped;
        for w in self.warnings {
            catalog.push_warning(w);
        }
        self.items
    }
}

/// Crawls `get_ordered_list` pages. A failed page is retried
/// `opts.page_retries` times; then the crawl stops with what it has (and a
/// warning), or fails when it has nothing.
async fn crawl<F: Fetch, P: ProgressSink, T: Crawl>(
    ctx: &mut Ctx<'_, F, P>,
    session: &mut StalkerSession,
    kind: ContentKind,
    opts: &ImportOptions,
    cap: usize,
    build: impl Fn(&StalkerSession, u32) -> Result<HttpRequest, StalkerError>,
    parse: impl Fn(&HttpResponse) -> Result<Page<T>, ProviderError>,
) -> Result<Crawled<T>, ImportError> {
    let step = Step::Items(kind);
    let mut crawl = T::crawl(cap);
    let mut out = Crawled {
        items: Vec::new(),
        skipped: 0,
        warnings: Vec::new(),
        dropped: 0,
        emitted: 0,
    };
    let mut rows = 0u64;
    let mut failures = 0u8;
    while let Some(page) = T::next_page(&crawl) {
        let mut p = Progress::phase(Phase::Items(kind));
        p.page = Some(page);
        p.items = rows;
        ctx.report(p);
        match call(ctx, session, step, |s| build(s, page), &parse).await {
            Ok(mut got) => {
                failures = 0;
                ctx.stats.pages += 1;
                rows += got.items.items.len() as u64;
                out.skipped += std::mem::take(&mut got.items.skipped);
                out.dropped += std::mem::take(&mut got.items.warnings_dropped);
                for w in std::mem::take(&mut got.items.warnings) {
                    out.push_warning(CatalogWarning {
                        step,
                        page: Some(page),
                        location: Some(w.location),
                        message: w.message,
                    });
                }
                let total = got.total_items;
                T::push(&mut crawl, got);
                if rows > opts.limits.items
                    || (rows >= cap as u64 && total.is_some_and(|t| t as usize > cap))
                {
                    return Err(ImportError::Limit {
                        step,
                        error: crate::LimitExceeded {
                            kind: crate::LimitKind::Items,
                            maximum: opts.limits.items.min(cap as u64),
                        },
                    });
                }
                if let Some(sink) = &mut ctx.catalog_sink {
                    sink.begin_step()
                        .map_err(|e| ImportError::Storage(e.to_string()))?;
                    let result: Result<(), ImportError> = (|| {
                        for item in T::take_items(&mut crawl) {
                            let row = item.row();
                            let stream = match &row {
                                crate::CatalogRow::Channel(v) => Some(&v.stream),
                                crate::CatalogRow::Vod(v) => Some(&v.stream),
                                _ => None,
                            };
                            #[cfg(not(debug_assertions))]
                            let allow_private = false;
                            #[cfg(debug_assertions)]
                            let allow_private = opts.allow_private_fixture;
                            if !allow_private && let Some(stream) = stream {
                                crate::validate_stream(stream)?;
                            }
                            sink.push(row)
                                .map_err(|e| ImportError::Storage(e.to_string()))?;
                            out.emitted += 1;
                        }
                        Ok(())
                    })();
                    sink.end_step(result.is_ok())
                        .map_err(|e| ImportError::Storage(e.to_string()))?;
                    result?;
                }
            }
            Err(e) if e.is_fatal() => return Err(e),
            Err(_) if failures < opts.page_retries => failures += 1,
            Err(e) if rows == 0 => return Err(e),
            Err(e) => {
                out.push_warning(CatalogWarning {
                    step,
                    page: Some(page),
                    location: None,
                    message: format!("stopped after {rows} rows: {e}"),
                });
                T::abort(&mut crawl);
            }
        }
    }
    out.items = T::into_items(crawl);
    out.emitted += out.items.len() as u64;
    let mut p = Progress::phase(Phase::Items(kind));
    p.items = out.items.len() as u64;
    ctx.report(p);
    Ok(out)
}

#[cfg(test)]
mod warning_tests {
    use super::*;
    use std::cell::Cell;
    use std::future::{Future, ready};
    use std::task::{Context, Poll, Waker};

    struct WarningPages {
        requests: Cell<usize>,
    }

    impl WarningPages {
        fn response(&self, request: HttpRequest) -> HttpResponse {
            let page = self.requests.get() + 1;
            self.requests.set(page);
            let expected_page = format!("p={page}");
            assert!(
                request
                    .url
                    .split(['?', '&'])
                    .any(|pair| pair == expected_page)
            );
            if page == 4 {
                return HttpResponse::new(503, b"Synthetic unavailable page".to_vec());
            }
            assert!(page <= 3, "crawl continued after the terminal failure");
            let mut data =
                vec![serde_json::Value::Null; iptv_providers::model::MAX_STORED_WARNINGS + 2];
            data.push(serde_json::json!({
                "id": page,
                "name": format!("Generated channel {page}"),
                "cmd": format!("http://media.example.test/channel/{page}"),
            }));
            HttpResponse::ok(
                serde_json::to_vec(&serde_json::json!({
                    "js": { "data": data, "total_items": 4, "max_page_items": 1 }
                }))
                .unwrap(),
            )
        }
    }

    impl Fetch for WarningPages {
        type Body = crate::MemoryBody;

        fn fetch(
            &self,
            request: HttpRequest,
        ) -> impl Future<Output = Result<HttpResponse, crate::FetchError>> {
            ready(Ok(self.response(request)))
        }

        fn fetch_stream(
            &self,
            request: HttpRequest,
        ) -> impl Future<Output = Result<crate::StreamingResponse<Self::Body>, crate::FetchError>>
        {
            let response = self.response(request);
            ready(Ok(crate::StreamingResponse {
                status: response.status,
                content_length: Some(response.body.len() as u64),
                body: crate::MemoryBody::new(&response.body, 83),
            }))
        }
    }

    #[test]
    fn real_pages_bound_retained_warnings_before_catalog_conversion() {
        let fetch = WarningPages {
            requests: Cell::new(0),
        };
        let cancel = crate::CancelToken::new();
        let mut progress = crate::NoProgress;
        let mut ctx = Ctx::new(&fetch, &mut progress, &cancel);
        let portal = StalkerPortal::new(
            "http://portal.example.test/portal.php",
            iptv_providers::model::MacAddress::parse("00:1A:79:00:00:42").unwrap(),
            PortalMode::Simple,
        )
        .unwrap();
        let mut session = StalkerSession::new(portal);
        let mut opts = ImportOptions::new(0);
        opts.page_retries = 0;
        let crawled = {
            let future = crawl::<_, _, Channel>(
                &mut ctx,
                &mut session,
                ContentKind::Live,
                &opts,
                opts.stalker_channel_cap,
                |session, page| session.channels_page("*", page),
                parse_channel_page,
            );
            let mut future = std::pin::pin!(future);
            // Every fake body and request is immediately ready; no runtime is needed.
            match future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Ready(result) => result.unwrap(),
                Poll::Pending => panic!("in-memory crawl unexpectedly waited for I/O"),
            }
        };
        let cap = iptv_providers::model::MAX_STORED_WARNINGS;
        assert_eq!(fetch.requests.get(), 4);
        assert_eq!(ctx.stats.pages, 3);
        assert_eq!(crawled.items.len(), 3);
        assert_eq!(crawled.skipped, 3 * (cap + 2));
        // This is the crawl's retained vector, before Catalog's separate cap runs.
        // Each parsed page has cap warnings plus two already dropped by its parser.
        assert_eq!(crawled.warnings.len(), cap);
        assert!(
            crawled
                .warnings
                .iter()
                .all(|warning| warning.page == Some(1))
        );
        assert_eq!(crawled.dropped, 2 * cap + 7);
        let mut catalog = Catalog::new(SourceMeta::Stalker {
            session: Box::new(session),
        });
        let channels = crawled.into_catalog(&mut catalog);
        assert_eq!(channels.len(), 3);
        assert_eq!(catalog.warning_count(), 3 * (cap + 2) + 1);
        assert_eq!(catalog.warnings_dropped, 2 * cap + 7);
    }

    #[test]
    fn page_and_terminal_warnings_are_bounded_while_accumulating() {
        let mut out = Crawled::<Channel> {
            items: Vec::new(),
            skipped: 0,
            warnings: Vec::new(),
            dropped: 0,
            emitted: 0,
        };
        let cap = iptv_providers::model::MAX_STORED_WARNINGS;
        for page in 1..=3 {
            out.dropped += 2; // warnings already omitted by the page parser
            for _ in 0..cap {
                out.push_warning(CatalogWarning {
                    step: Step::Items(ContentKind::Live),
                    page: Some(page),
                    location: None,
                    message: "Synthetic malformed row".into(),
                });
            }
            assert!(out.warnings.len() <= cap);
        }
        out.push_warning(CatalogWarning {
            step: Step::Items(ContentKind::Live),
            page: Some(4),
            location: None,
            message: "Synthetic terminal failure".into(),
        });
        assert_eq!(out.warnings.len(), cap);
        assert_eq!(out.dropped, 2 * cap + 7);
    }
}
