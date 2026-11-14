//! EPG import, streamed: per source, a chunked download into
//! [`XmltvParser`] (gzip detected automatically) whose programmes go
//! straight to a [`GuideSink`] (the app's on-disk stage), then the
//! catalog's channels are matched against every source's channel list by
//! priority and bound to the best source with programmes.
//!
//! Only the feeds' channel lists stay in memory. Programmes of an EPG
//! channel no playlist channel can match ([`MatchFilter`]) are not passed
//! on; a feed declares its channels before its programmes (XMLTV DTD
//! order), and a programme of a channel not declared yet is passed on in
//! case it is declared later. Matching and binding give the same result as
//! matching the complete in-memory guide.
//!
//! A failing source does not discard the others: its error is kept in
//! [`EpgSourceOutcome`] and its programmes are dropped from the sink. The
//! import fails only when every source failed, on cancellation, or when
//! the sink fails ([`ImportError::Storage`]).
//!
//! Each source has a budget of its own ([`GUIDE_SOURCE_BYTES`],
//! [`GUIDE_SOURCE_TIME`]): a large or slow feed cannot use up the time of
//! the feeds after it, and running out ends that source only. The time is
//! generous because the adapter ends a stalled download by its idle
//! timeout; the total only bounds a download that never stops.
//!
//! A source that fails for a passing reason (no connection, a stalled or
//! interrupted download, HTTP 5xx) is downloaded again from the start, up
//! to [`GUIDE_RETRIES`] times, after waits that double from
//! [`GUIDE_RETRY_DELAY`]. The wait is [`Fetch::sleep`], the adapter's
//! timer; this crate has none. A refusal (4xx), content that is not a
//! guide, an exhausted budget and cancellation are not retried.

use std::collections::HashMap;
use std::time::Duration;

use iptv_core::import_limits::ImportLimits;
use iptv_epg::{
    ChannelKey, Compression, EpgRef, FeedId, IngestReport, MatchFilter, MatchIndex, MatchReport,
    Overrides, ParseOptions, ParseStats, PlaylistChannel, ProgrammeData, StoreOptions, TimePolicy,
    UnixTime, XmltvChannel, XmltvItem, XmltvParser,
};
use iptv_providers::m3u::M3uHeader;
use iptv_providers::model::Channel;
use iptv_providers::{HttpRequest, SecretUrl};

use crate::cancel::{CancelToken, check, or_cancel};
use crate::ctx::Ctx;
use crate::error::{ImportError, ParseProblem, Step};
use crate::fetch::Fetch;
use crate::progress::{Phase, Progress, ProgressSink};
use crate::sink::{GuideSink, SinkError};

/// Bytes one guide source may send, as downloaded (a `.gz` file before it
/// is unpacked), counted per attempt.
pub const GUIDE_SOURCE_BYTES: u64 = 512 << 20;
/// Time one guide source may take on the adapter's clock
/// ([`Fetch::elapsed`]), retries and their waits included. The adapter
/// also gets it as each request's deadline.
pub const GUIDE_SOURCE_TIME: Duration = Duration::from_secs(30 * 60);
/// Further attempts at a source after a passing failure.
pub const GUIDE_RETRIES: u32 = 3;
/// The wait before the first retry; each later one waits twice as long
/// (2 s, 4 s, 8 s), at most [`GUIDE_RETRY_MAX_DELAY`].
pub const GUIDE_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Upper bound of one wait, should the number of retries be raised.
pub const GUIDE_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

/// The wait before retry number `retry` (from 1).
fn retry_delay(retry: u32) -> Duration {
    let doublings = retry.saturating_sub(1).min(16);
    GUIDE_RETRY_DELAY
        .saturating_mul(1 << doublings)
        .min(GUIDE_RETRY_MAX_DELAY)
}

/// One XMLTV source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpgSourceConfig {
    /// False only for a guide/source endpoint explicitly configured by the user.
    pub content_supplied: bool,
    pub id: FeedId,
    /// May carry credentials (Xtream `xmltv.php`).
    pub url: SecretUrl,
    /// Higher wins when several sources have a channel.
    pub priority: i32,
    pub user_agent: Option<String>,
    /// Offset for times without a zone, and a per-source shift.
    pub time: TimePolicy,
    pub store: StoreOptions,
}

impl EpgSourceConfig {
    pub fn new(id: FeedId, url: SecretUrl) -> Self {
        Self {
            content_supplied: false,
            id,
            url,
            priority: 0,
            user_agent: None,
            time: TimePolicy::UTC,
            store: StoreOptions::default(),
        }
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// The playlist header's `url-tvg`/`x-tvg-url`/`tvg-url` entries, ids
    /// from `first_id`, the first URL with the highest priority.
    pub fn from_m3u_header(header: &M3uHeader, first_id: u32) -> Vec<Self> {
        let n = header.epg_urls.len() as i32;
        header
            .epg_urls
            .iter()
            .enumerate()
            .map(|(i, url)| {
                let mut config =
                    Self::new(FeedId(first_id + i as u32), SecretUrl::new(url.clone()))
                        .with_priority(n - i as i32);
                config.content_supplied = true;
                config
            })
            .collect()
    }
}

/// Counters for one source that loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpgSourceStats {
    pub parse: ParseStats,
    /// What the sink kept of the programmes passed on.
    pub ingest: IngestReport,
    /// Programmes not passed on: no playlist channel can match their
    /// channel.
    pub skipped: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpgSourceOutcome {
    pub id: FeedId,
    pub result: Result<EpgSourceStats, ImportError>,
}

/// A channel a feed lists (for choosing a channel's guide by hand).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedChannel {
    pub id: String,
    /// Its first display name.
    pub name: Option<String>,
}

/// How the guide matched; the programmes are in the [`GuideSink`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpgImport {
    /// Each playlist channel with guide data and the EPG channel it comes
    /// from (the highest-priority candidate with programmes), by key.
    pub bindings: Vec<(ChannelKey, EpgRef)>,
    pub matches: MatchReport,
    /// One entry per configured source, in input order.
    pub sources: Vec<EpgSourceOutcome>,
    /// Every channel each loaded feed lists, in document order.
    pub listed: Vec<(FeedId, Vec<ListedChannel>)>,
}

impl EpgImport {
    /// Number of playlist channels that have guide data.
    pub fn bound_count(&self) -> usize {
        self.bindings.len()
    }

    /// The EPG channel playlist channel `key` takes its guide from.
    pub fn binding(&self, key: &ChannelKey) -> Option<&EpgRef> {
        let i = self.bindings.binary_search_by(|(k, _)| k.cmp(key)).ok()?;
        Some(&self.bindings[i].1)
    }

    /// The channels feed `feed` lists (empty when it did not load).
    pub fn listed(&self, feed: FeedId) -> &[ListedChannel] {
        self.listed
            .iter()
            .find(|(id, _)| *id == feed)
            .map_or(&[], |(_, l)| l.as_slice())
    }
}

/// Downloads and parses every source into `sink`, then matches `channels`
/// (tvg-id, then names; `overrides` win) and binds each to the best source
/// with programmes. `now` drives retention.
#[allow(clippy::too_many_arguments)]
pub async fn import_epg<F: Fetch, S: GuideSink, P: ProgressSink>(
    fetch: &F,
    sources: &[EpgSourceConfig],
    channels: &[Channel],
    overrides: &Overrides,
    now: UnixTime,
    sink: &mut S,
    progress: &mut P,
    cancel: &CancelToken,
) -> Result<EpgImport, ImportError> {
    let keys: Vec<ChannelKey> = channels
        .iter()
        .map(|c| ChannelKey::new(c.id.as_str()))
        .collect();
    let playlist = || {
        channels.iter().zip(&keys).map(|(c, key)| PlaylistChannel {
            key,
            tvg_id: c.epg_id.as_deref(),
            name: &c.name,
            tvg_name: c.epg_name.as_deref(),
        })
    };
    let filter = MatchFilter::new(playlist(), overrides);
    let mut index = MatchIndex::new();
    let mut outcomes = Vec::with_capacity(sources.len());
    // Loaded feeds in load order (a later one replaces the same id).
    let mut loaded: Vec<(FeedId, i32)> = Vec::new();
    let mut listed: Vec<(FeedId, Vec<ListedChannel>)> = Vec::new();
    for cfg in sources {
        // Only the user ends the run between sources. Time and bytes are
        // counted per source, so one that used up its budget is recorded
        // like any other failure and the next one starts fresh.
        check(cancel)?;
        let mut ctx = Ctx::new(fetch, &mut *progress, cancel);
        ctx.limits = ImportLimits {
            input_bytes: GUIDE_SOURCE_BYTES,
            elapsed: GUIDE_SOURCE_TIME,
            ..ImportLimits::default()
        };
        ctx.limit_step = Step::Guide(cfg.id);
        let mut retries = 0;
        let result = loop {
            sink.begin_feed(cfg.id, cfg.priority, cfg.store, now)
                .map_err(storage)?;
            let error = match load_source(&mut ctx, cfg, &filter, sink).await {
                Ok((feed_channels, mut stats)) => {
                    stats.ingest = sink.finish_feed().map_err(storage)?;
                    index.add_source(cfg.id, cfg.priority, &feed_channels);
                    loaded.retain(|(id, _)| *id != cfg.id);
                    loaded.push((cfg.id, cfg.priority));
                    listed.retain(|(id, _)| *id != cfg.id);
                    listed.push((cfg.id, feed_channels.into_iter().map(listed_of).collect()));
                    break Ok(stats);
                }
                Err(e) => e,
            };
            // Whatever the attempt stored is incomplete, also before a retry.
            sink.discard_feed().map_err(storage)?;
            // A failed feed leaves the others; these end the import.
            if matches!(error, ImportError::Cancelled | ImportError::Storage(_)) {
                return Err(error);
            }
            if retries == GUIDE_RETRIES || !error.retry_now() {
                break Err(error);
            }
            retries += 1;
            or_cancel(cancel, fetch.sleep(retry_delay(retries))).await?;
            ctx.restart_bytes();
        };
        outcomes.push(EpgSourceOutcome { id: cfg.id, result });
    }
    if let Some(Err(first)) = outcomes.first().map(|o| &o.result)
        && outcomes.iter().all(|o| o.result.is_err())
    {
        return Err(first.clone());
    }

    progress.report(&Progress::phase(Phase::Matching));
    let result = index.match_channels(playlist(), overrides);
    // Source rank as the guide store orders sources: priority, then load
    // order.
    let mut by_rank = loaded;
    by_rank.sort_by_key(|&(_, priority)| std::cmp::Reverse(priority));
    let rank: HashMap<FeedId, usize> = by_rank
        .iter()
        .enumerate()
        .map(|(i, &(id, _))| (id, i))
        .collect();
    let mut bindings: Vec<(ChannelKey, EpgRef)> = result
        .matches
        .into_iter()
        .filter_map(|(key, cands)| {
            let best = cands
                .into_iter()
                .filter_map(|c| {
                    let r = *rank.get(&c.source)?;
                    sink.has_programmes(c.source, &c.epg_id).then_some((r, c))
                })
                .min_by_key(|(r, _)| *r)?;
            Some((key, best.1))
        })
        .collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    let mut done = Progress::phase(Phase::Done);
    done.items = bindings.len() as u64;
    progress.report(&done);
    Ok(EpgImport {
        bindings,
        matches: result.report,
        sources: outcomes,
        listed,
    })
}

fn storage(e: SinkError) -> ImportError {
    ImportError::Storage(e.0)
}

fn listed_of(c: XmltvChannel) -> ListedChannel {
    ListedChannel {
        name: c.display_names.into_iter().next(),
        id: c.id,
    }
}

/// What one feed's parse keeps in memory: its channels, and which ids a
/// playlist channel may match (first declaration wins, as in matching).
struct FeedState<'a, S> {
    feed: FeedId,
    filter: &'a MatchFilter,
    sink: &'a mut S,
    channels: Vec<XmltvChannel>,
    wanted: HashMap<String, bool>,
    items: u64,
    skipped: u64,
    failed: Option<SinkError>,
}

impl<S: GuideSink> FeedState<'_, S> {
    fn push(&mut self, item: XmltvItem) {
        self.items += 1;
        match item {
            XmltvItem::Channel(c) => {
                if !self.wanted.contains_key(&c.id) {
                    let wanted = self.filter.wants_channel(self.feed, &c);
                    self.wanted.insert(c.id.clone(), wanted);
                }
                self.channels.push(c);
            }
            XmltvItem::Programme(p) => {
                // Not declared (yet): kept, it may be declared later.
                let wanted = self.filter.wants_id(self.feed, &p.channel)
                    || self.wanted.get(&p.channel).copied().unwrap_or(true);
                if !wanted {
                    self.skipped += 1;
                } else if self.failed.is_none()
                    && let Err(e) = self
                        .sink
                        .programme(&p.channel, &ProgrammeData::from_xmltv(&p))
                {
                    self.failed = Some(e);
                }
            }
        }
    }

    fn check(&mut self) -> Result<(), ImportError> {
        match self.failed.take() {
            Some(e) => Err(storage(e)),
            None => Ok(()),
        }
    }
}

async fn load_source<F: Fetch, S: GuideSink, P: ProgressSink>(
    ctx: &mut Ctx<'_, F, P>,
    cfg: &EpgSourceConfig,
    filter: &MatchFilter,
    sink: &mut S,
) -> Result<(Vec<XmltvChannel>, EpgSourceStats), ImportError> {
    let step = Step::Guide(cfg.id);
    let url = cfg.url.expose().trim();
    if url.is_empty() {
        return Err(ImportError::InvalidConfig("EPG address is required".into()));
    }
    let mut req = HttpRequest::get(url).with_header("Accept-Encoding", "gzip");
    req.preserve_gzip = true;
    req.public_only = cfg.content_supplied;
    req.picked_file = !cfg.content_supplied;
    if let Some(ua) = cfg
        .user_agent
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        req = req.with_header("User-Agent", ua);
    }
    let mut parser = XmltvParser::new(ParseOptions {
        time: cfg.time,
        compression: Compression::Auto,
        descriptions: cfg.store.store_descriptions,
        ..ParseOptions::default()
    });
    let mut state = FeedState {
        feed: cfg.id,
        filter,
        sink,
        channels: Vec::new(),
        wanted: HashMap::new(),
        items: 0,
        skipped: 0,
        failed: None,
    };
    let decode = |e: iptv_epg::DecodeError| match e {
        iptv_epg::DecodeError::Limit(error) => ImportError::Limit { step, error },
        e => ImportError::parse(step, ParseProblem::Decode(e.to_string())),
    };
    ctx.phase(Phase::Guide(cfg.id));
    let bytes = ctx
        .stream(step, Phase::Guide(cfg.id), req, |chunk| {
            parser
                .feed(chunk, &mut |item| state.push(item))
                .map_err(decode)?;
            state.check()?;
            Ok(state.items)
        })
        .await?;
    let parse = parser
        .finish(&mut |item| state.push(item))
        .map_err(decode)?;
    state.check()?;
    if parse.channels == 0 && parse.programmes == 0 {
        return Err(if bytes == 0 {
            ImportError::Empty { step }
        } else {
            ImportError::parse(step, ParseProblem::NotXmltv)
        });
    }
    let stats = EpgSourceStats {
        parse,
        ingest: IngestReport::default(),
        skipped: state.skipped,
        bytes,
    };
    let mut p = Progress::phase(Phase::Guide(cfg.id));
    p.items = stats.parse.programmes;
    p.bytes = bytes;
    ctx.report(p);
    Ok((state.channels, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_double_from_the_first_and_stay_bounded() {
        let waits: Vec<u64> = (1..=GUIDE_RETRIES)
            .map(|n| retry_delay(n).as_secs())
            .collect();
        assert_eq!(waits, [2, 4, 8]);
        assert_eq!(retry_delay(0), GUIDE_RETRY_DELAY);
        for retry in [5, 6, 40, u32::MAX] {
            assert_eq!(retry_delay(retry), GUIDE_RETRY_MAX_DELAY, "{retry}");
        }
    }
}
