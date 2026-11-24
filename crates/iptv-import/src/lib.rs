#![forbid(unsafe_code)]
//! Import pipeline of iptv-rs.
//!
//! Drives the sans-IO provider clients (`iptv-providers`) and the XMLTV
//! parser (`iptv-epg`) through an abstract async [`Fetch`]:
//!
//! - [`import_source`]: M3U (URL or file via the adapter), Xtream (account
//!   → live categories + streams, optional VOD/series) or Stalker
//!   (discovery → auth loop → genres → paginated channels) into a
//!   provider-neutral [`Catalog`]. [`import_m3u_text`] parses a playlist
//!   already in memory.
//! - [`import_epg`]: chunked XMLTV download per source with priorities,
//!   programs streamed into a [`GuideSink`] as they are parsed (only the
//!   feeds' channel lists stay in memory), matched against the catalog's
//!   channels. Each source has its own time and size budget and is tried
//!   again after a passing failure. [`MemoryGuide`] is an in-memory sink.
//!   [`Catalog::epg_sources`] turns the M3U header's `url-tvg`/`x-tvg-url`
//!   (or the Xtream export) into [`EpgSourceConfig`]s.
//! - [`decide`] / [`plan_startup`]: when to download a stored guide again
//!   ([`RefreshPolicy`] from the `epg.update_*` settings).
//!
//! Failures are typed ([`ImportError`], [`FailureKind`]) so a failure never
//! looks like an empty source. Progress goes to a [`ProgressSink`];
//! [`CancelToken`] stops an import between requests, between chunks and
//! while a request is in flight.
//!
//! **Portable.** No runtime (no tokio), no NSH, no platform APIs, no clock
//! (callers pass `now`; elapsed time and the wait before a retry come from
//! the [`Fetch`] adapter); builds for `wasm32-unknown-unknown`. The native
//! adapter is `iptv-net`.

pub mod backup_manifest;
mod cancel;
mod catalog;
mod catalog_sink;
mod ctx;
mod epg;
mod error;
mod fetch;
mod link;
mod m3u;
mod progress;
mod refresh;
mod sink;
mod stalker;
mod xtream;

pub use cancel::{CancelToken, Canceled};
pub use catalog::{Catalog, CatalogWarning, ImportOptions, ImportStats, SourceMeta};
pub use catalog_sink::{CatalogRow, CatalogSink};
pub use epg::{
    EpgImport, EpgSourceConfig, EpgSourceOutcome, EpgSourceStats, GUIDE_RETRIES, GUIDE_RETRY_DELAY,
    GUIDE_RETRY_MAX_DELAY, GUIDE_SOURCE_BYTES, GUIDE_SOURCE_TIME, ListedChannel, import_epg,
};
pub use error::{AuthProblem, FailureKind, ImportError, ParseProblem, Step};
pub use fetch::{BodyStream, Fetch, FetchError, FetchErrorKind, MemoryBody, StreamingResponse};
pub use iptv_core::import_limits::{ImportLimits, LimitExceeded, LimitKind};
pub use link::{stalker_link, stalker_session};
pub use m3u::import_m3u_text;
pub use progress::{NoProgress, Phase, Progress, ProgressSink};
pub use refresh::{
    Decision, GuideState, KEEP_PAST_MAX_DAYS, KEEP_PAST_MIN_DAYS, RefreshPolicy, RefreshReason,
    StartupPlan, Trigger, decide, keep_past_days, matching_fingerprint, plan_startup,
    store_options, update_interval,
};
pub use sink::{GuideSink, MemoryGuide, SinkError};

use iptv_providers::model::SourceConfig;

/// Imports one configured source.
///
/// Network work happens only through `fetch`; `progress` receives events on
/// the calling task; `cancel` aborts with [`ImportError::Canceled`].
pub async fn import_source<F: Fetch, P: ProgressSink>(
    fetch: &F,
    config: &SourceConfig,
    options: &ImportOptions,
    progress: &mut P,
    cancel: &CancelToken,
) -> Result<Catalog, ImportError> {
    import_source_impl(fetch, config, options, progress, cancel, None).await
}

/// Streams typed catalog rows into a private destination. Only metadata and
/// bounded diagnostics remain in the returned Catalog; publish after success.
pub async fn import_source_into<F: Fetch, P: ProgressSink>(
    fetch: &F,
    config: &SourceConfig,
    options: &ImportOptions,
    progress: &mut P,
    cancel: &CancelToken,
    sink: &mut dyn CatalogSink,
) -> Result<Catalog, ImportError> {
    let result = import_source_impl(fetch, config, options, progress, cancel, Some(sink)).await?;
    sink.finish()
        .map_err(|e| ImportError::Storage(e.to_string()))?;
    Ok(result)
}

async fn import_source_impl<'a, F: Fetch, P: ProgressSink>(
    fetch: &'a F,
    config: &SourceConfig,
    options: &ImportOptions,
    progress: &'a mut P,
    cancel: &'a CancelToken,
    sink: Option<&'a mut dyn CatalogSink>,
) -> Result<Catalog, ImportError> {
    let mut ctx = ctx::Ctx::new(fetch, progress, cancel);
    ctx.limits = options.limits;
    ctx.catalog_sink = sink;
    #[cfg(debug_assertions)]
    {
        ctx.allow_private_fixture = options.allow_private_fixture;
    }
    ctx.check()?;
    let catalog = match config {
        SourceConfig::M3u(c) => m3u::import_m3u(&mut ctx, c).await,
        SourceConfig::Xtream(c) => xtream::import_xtream(&mut ctx, c, options).await,
        SourceConfig::Stalker(c) => stalker::import_stalker(&mut ctx, c, options).await,
    }?;
    #[cfg(not(debug_assertions))]
    let allow_private_fixture = false;
    #[cfg(debug_assertions)]
    let allow_private_fixture = options.allow_private_fixture;
    if !allow_private_fixture {
        for stream in catalog
            .channels
            .iter()
            .map(|c| &c.stream)
            .chain(catalog.vod.iter().map(|v| &v.stream))
        {
            validate_stream(stream)?;
        }
    }
    Ok(catalog)
}

pub(crate) fn validate_stream(
    stream: &iptv_providers::model::StreamLocator,
) -> Result<(), ImportError> {
    use iptv_providers::model::StreamLocator;
    let url = match stream {
        StreamLocator::Url { url, .. } => Some(url.clone()),
        StreamLocator::Stalker { cmd, link } => {
            iptv_providers::stalker::static_playback_url(cmd, *link)
        }
        StreamLocator::Xtream { .. } => None,
    };
    if let Some(url) = url {
        iptv_providers::content_url::validate(&url).map_err(|_| {
            ImportError::InvalidConfig("content stream URL must name a public HTTP(S) host".into())
        })?;
    }
    Ok(())
}

pub mod on_demand;
