//! Xtream lists stream through typed row parsing into a private catalog sink.
use crate::catalog::{Catalog, ImportOptions, SourceMeta};
use crate::ctx::Ctx;
use crate::error::{AuthProblem, ImportError, Step};
use crate::fetch::Fetch;
use crate::progress::{Phase, ProgressSink};
use crate::{CatalogRow, CatalogSink, ImportLimits, LimitExceeded, LimitKind};
use iptv_providers::model::{ContentKind, XtreamCredentials};
use iptv_providers::xtream::{
    AccountInfo, AccountStatus, ListKind, XtreamClient, XtreamListParser, parse_account_info,
};
use iptv_providers::{HttpRequest, SecretUrl};

pub(crate) async fn import_xtream<F: Fetch, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    creds: &XtreamCredentials,
    opts: &ImportOptions,
) -> Result<Catalog, ImportError> {
    let client = XtreamClient::from_credentials(creds)
        .map_err(|e| ImportError::InvalidConfig(e.to_string()))?;
    ctx.phase(Phase::Account);
    let account = account(ctx, &client, opts.now).await?;
    let mut catalog = Catalog::new(SourceMeta::Xtream {
        base_url: client.base_url().into(),
        xmltv_url: SecretUrl::new(client.xmltv_url()),
        account: account.clone(),
    });
    if account.is_none() {
        catalog.warn(
            Step::AccountInfo,
            "the panel did not report account details",
        );
    }
    for (kind, enabled, categories, items, list_kind) in [
        (
            ContentKind::Live,
            true,
            client.live_categories(),
            client.live_streams(None),
            ListKind::Live,
        ),
        (
            ContentKind::Movie,
            opts.include_vod,
            client.vod_categories(),
            client.vod_streams(None),
            ListKind::Vod,
        ),
        (
            ContentKind::Series,
            opts.include_series,
            client.series_categories(),
            client.series(None),
            ListKind::Series,
        ),
    ] {
        if !enabled {
            continue;
        }
        let groups = stream_list(
            ctx,
            &mut catalog,
            Step::Categories(kind),
            Phase::Groups(kind),
            categories,
            ListKind::Categories(kind),
        )
        .await;
        if let Err(e) = groups {
            if e.is_fatal()
                || (kind == ContentKind::Live
                    && matches!(e, ImportError::Network { .. } | ImportError::Auth { .. }))
            {
                return Err(e);
            }
            catalog.warn(Step::Categories(kind), e.to_string());
        }
        let result = stream_list(
            ctx,
            &mut catalog,
            Step::Items(kind),
            Phase::Items(kind),
            items,
            list_kind,
        )
        .await;
        if kind == ContentKind::Live {
            result?;
        } else {
            soften_items(&mut catalog, kind, result)?;
        }
        // A staged catalog synthesizes missing groups in SQL. The in-memory
        // compatibility API keeps its existing placeholder-group behaviour.
        let referenced: Vec<_> = match kind {
            ContentKind::Live => catalog
                .channels
                .iter()
                .flat_map(|c| c.groups.clone())
                .collect(),
            ContentKind::Movie => catalog.vod.iter().flat_map(|c| c.groups.clone()).collect(),
            ContentKind::Series => catalog
                .series
                .iter()
                .flat_map(|c| c.groups.clone())
                .collect(),
            _ => Vec::new(),
        };
        catalog.add_missing_groups(kind, referenced.iter());
    }
    if catalog.is_empty() {
        return Err(ImportError::Empty {
            step: Step::Items(ContentKind::Live),
        });
    }
    catalog.set_transport_stats(&ctx.stats);
    ctx.phase(Phase::Done);
    Ok(catalog)
}

pub(crate) fn accept_row(
    catalog: &mut Catalog,
    sink: &mut Option<&mut dyn CatalogSink>,
    row: CatalogRow,
    limits: ImportLimits,
    allow_private: bool,
) -> Result<(), ImportError> {
    let stream = match &row {
        CatalogRow::Channel(c) => Some(&c.stream),
        CatalogRow::Vod(v) => Some(&v.stream),
        _ => None,
    };
    if !allow_private && let Some(stream) = stream {
        crate::validate_stream(stream)?;
    }
    if !matches!(row, CatalogRow::Group(_)) {
        catalog.stats.content_items = catalog.stats.content_items.saturating_add(1);
        LimitExceeded::check(LimitKind::Items, catalog.stats.content_items, limits.items).map_err(
            |error| ImportError::Limit {
                step: Step::Playlist,
                error,
            },
        )?;
    }
    if let Some(sink) = sink {
        sink.push(row)
            .map_err(|e| ImportError::Storage(e.to_string()))?;
    } else {
        match row {
            CatalogRow::Group(v) => catalog.groups.push(v),
            CatalogRow::Channel(v) => catalog.channels.push(v),
            CatalogRow::Vod(v) => catalog.vod.push(v),
            CatalogRow::Series(v) => catalog.series.push(v),
        }
    }
    Ok(())
}
async fn stream_list<F: Fetch, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    catalog: &mut Catalog,
    step: Step,
    phase: Phase,
    req: HttpRequest,
    kind: ListKind,
) -> Result<(), ImportError> {
    let mut sink = ctx.catalog_sink.take();
    if let Some(s) = &mut sink {
        s.begin_step()
            .map_err(|e| ImportError::Storage(e.to_string()))?;
    }
    let before = (
        catalog.groups.len(),
        catalog.channels.len(),
        catalog.vod.len(),
        catalog.series.len(),
        catalog.stats.content_items,
    );
    let limits = ctx.limits;
    #[cfg(not(debug_assertions))]
    let allow_private = false;
    #[cfg(debug_assertions)]
    let allow_private = ctx.allow_private_fixture;
    let mut parser = XtreamListParser::new(kind, limits);
    let mut failure = None;
    let mut count = 0;
    let result = ctx
        .stream(step, phase, req, |bytes| {
            parser
                .feed(bytes, &mut |row| {
                    if failure.is_none() {
                        match accept_row(catalog, &mut sink, row, limits, allow_private) {
                            Ok(()) => count += 1,
                            Err(error) => failure = Some(error),
                        }
                    }
                })
                .map_err(|e| ImportError::provider(step, e))?;
            if let Some(error) = failure.take() {
                return Err(error);
            }
            Ok(count)
        })
        .await
        .and_then(|_| {
            parser
                .finish(&mut |row| {
                    if failure.is_none() {
                        match accept_row(catalog, &mut sink, row, limits, allow_private) {
                            Ok(()) => count += 1,
                            Err(error) => failure = Some(error),
                        }
                    }
                })
                .map_err(|e| ImportError::provider(step, e))?;
            failure.map_or(Ok(()), Err)
        });
    if let Some(s) = &mut sink {
        s.end_step(result.is_ok())
            .map_err(|e| ImportError::Storage(e.to_string()))?;
    }
    ctx.catalog_sink = sink;
    if result.is_err() {
        catalog.groups.truncate(before.0);
        catalog.channels.truncate(before.1);
        catalog.vod.truncate(before.2);
        catalog.series.truncate(before.3);
        catalog.stats.content_items = before.4;
    }
    let _ = catalog.absorb(step, None, parser.take_warnings());
    result?;
    let mut progress = crate::Progress::phase(phase);
    progress.items = count;
    ctx.report(progress);
    Ok(())
}

/// The result of an optional step: non-fatal failures become catalog warnings.
/// [`soften`] for a movie or series list: its failure is also kept by
/// kind ([`Catalog::on_demand_failures`]), so an import that kept its live
/// channels is not taken for one whose provider has no titles.
pub(crate) fn soften_items<T>(
    catalog: &mut Catalog,
    kind: ContentKind,
    result: Result<T, ImportError>,
) -> Result<Option<T>, ImportError> {
    if let Err(e) = &result
        && !e.is_fatal()
        && !catalog.on_demand_failures.iter().any(|(k, _)| *k == kind)
    {
        catalog.on_demand_failures.push((kind, e.clone()));
    }
    soften(catalog, result)
}

pub(crate) fn soften<T>(
    catalog: &mut Catalog,
    result: Result<T, ImportError>,
) -> Result<Option<T>, ImportError> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_fatal() => Err(e),
        Err(e) => {
            let at = e.step().unwrap_or(Step::AccountInfo);
            catalog.warn(at, e.to_string());
            Ok(None)
        }
    }
}

/// Probes the account endpoints in order. Inactive/expired accounts fail;
/// a panel that never reports `user_info` but answers is let through (the
/// catalog requests will tell).
async fn account<F: Fetch, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    client: &XtreamClient,
    now: i64,
) -> Result<Option<AccountInfo>, ImportError> {
    let step = Step::AccountInfo;
    let mut answered = false;
    let mut last: Option<ImportError> = None;
    for req in client.account_info_probes() {
        let resp = match ctx.send(step, req).await {
            Ok(r) => r,
            Err(ImportError::Network { step, error }) if !error.kind.is_host_failure() => {
                last = Some(ImportError::Network { step, error });
                continue;
            }
            Err(e) => return Err(e),
        };
        if !resp.is_success() {
            last = Some(ImportError::http(step, resp.status));
            continue;
        }
        answered = true;
        match parse_account_info(&resp, now) {
            Ok(info) => match info.status {
                AccountStatus::Active => return Ok(Some(info)),
                AccountStatus::Inactive | AccountStatus::Expired => {
                    return Err(ImportError::Auth {
                        step,
                        problem: AuthProblem::XtreamAccount {
                            status: info.status,
                            message: info.message,
                        },
                    });
                }
                AccountStatus::Unavailable => {}
            },
            Err(e) => last = Some(ImportError::provider(step, e)),
        }
    }
    if answered {
        return Ok(None);
    }
    Err(last.unwrap_or(ImportError::HttpStatus { step, status: 0 }))
}
