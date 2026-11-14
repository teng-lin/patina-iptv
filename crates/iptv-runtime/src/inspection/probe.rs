//! Disposable encrypted snapshots with leases that pin evidence during repair.
use super::{InspectionControl, guarded};
use crate::{Runtime, store_service::Services};
use iptv_epg::{FeedId, ParseOptions, StrictXmltvParser, TimePolicy, XmltvItem};
use iptv_import::{BodyStream, CancelToken};
use iptv_library::inspection::*;
use iptv_providers::{HttpRequest, SecretUrl};
use iptv_store_sqlite::{DatabaseKey, InspectionStage};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub const PROBE_BYTES: u64 = 32 << 20;
pub const PROBE_EXPANDED_BYTES: u64 = 256 << 20;
pub const PROBE_DEADLINE: Duration = Duration::from_secs(60);
pub const STAGE_BUDGET: u64 = 512 << 20;
pub const STAGE_TTL: i64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
pub enum ProbeTarget {
    Configured(FeedId),
    Supplied(SecretUrl),
}
#[derive(Clone, Debug)]
pub struct ProbeRequest {
    pub source: iptv_library::SourceId,
    pub target: ProbeTarget,
}

fn now() -> i64 {
    crate::imports::now_unix()
}
fn random<const N: usize>() -> InspectionResult<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| InspectionError::Storage)?;
    Ok(bytes)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn storage<T>(result: iptv_store_sqlite::Result<T>) -> InspectionResult<T> {
    result.map_err(|_| InspectionError::Storage)
}
fn check(services: &Services, cancel: &CancelToken, deadline: Instant) -> InspectionResult<()> {
    if !services.is_current() {
        return Err(InspectionError::OwnerClosed);
    }
    if cancel.is_cancelled() {
        return Err(InspectionError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(InspectionError::Deadline);
    }
    Ok(())
}

pub(crate) struct Vault {
    nonce: String,
    key: Option<DatabaseKey>,
    stages: Arc<Mutex<BTreeMap<ProbeId, Arc<Snapshot>>>>,
    running: Arc<AtomicUsize>,
}
impl Vault {
    pub(crate) fn new() -> Self {
        let key = random().ok().map(DatabaseKey::from_bytes);
        Self {
            nonce: random::<16>().map(|b| hex(&b)).unwrap_or_default(),
            key,
            stages: Arc::new(Mutex::new(BTreeMap::new())),
            running: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub(crate) fn context(&self, epoch: u64) -> String {
        format!("{}:{epoch}", self.nonce)
    }
    pub(crate) fn clear(&self) {
        self.stages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

struct Running(Arc<AtomicUsize>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Snapshot {
    path: PathBuf,
    key: DatabaseKey,
    evidence: ProbeEvidence,
    source_incarnation: String,
    services: Services,
    bytes: u64,
    time: TimePolicy,
    location: SecretUrl,
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Pins the exact snapshot against eviction. The writer must additionally
/// compare source/feed incarnations with its transaction's current metadata.
#[derive(Clone)]
pub struct ProbeLease(Arc<Snapshot>);
impl ProbeLease {
    /// Private configuration value; callers must preserve SecretUrl redaction.
    pub fn location(&self) -> SecretUrl {
        self.0.location.clone()
    }
    pub fn validate_alive(&self) -> InspectionResult<()> {
        self.check()
    }
    pub fn listed_channels(&self) -> InspectionResult<Vec<iptv_epg::XmltvChannel>> {
        self.check()?;
        let stage = storage(InspectionStage::open(&self.0.path, &self.0.key))?;
        let mut channels = Vec::new();
        let mut after = String::new();
        loop {
            let page = storage(stage.channels(&after, "", MAX_PAGE))?;
            if page.is_empty() {
                break;
            }
            after = page.last().expect("nonempty page").id.clone();
            channels.extend(page);
            if channels.len() > 100_000 {
                return Err(InspectionError::LimitExceeded);
            }
        }
        Ok(channels)
    }
    pub fn evidence(&self) -> &ProbeEvidence {
        &self.0.evidence
    }
    pub fn source_incarnation(&self) -> &str {
        &self.0.source_incarnation
    }
    pub fn validate_candidate(&self, candidate: &CandidateRef) -> InspectionResult<()> {
        self.check()?;
        if candidate.source != self.0.evidence.source
            || self.0.evidence.feed.is_some_and(|f| f != candidate.feed)
            || candidate.incarnation != self.0.evidence.incarnation
        {
            return Err(InspectionError::StaleEvidence);
        }
        let stage = storage(InspectionStage::open(&self.0.path, &self.0.key))?;
        if !storage(stage.has_channel(&candidate.epg_channel))? {
            return Err(InspectionError::StaleEvidence);
        }
        Ok(())
    }
    pub fn check(&self) -> InspectionResult<()> {
        if !self.0.services.is_current() {
            return Err(InspectionError::OwnerClosed);
        }
        if now() >= self.0.evidence.expires_at {
            return Err(InspectionError::StaleEvidence);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Configuration {
    url: SecretUrl,
    feed: Option<FeedId>,
    incarnation: String,
    source_incarnation: String,
    time: TimePolicy,
    net: iptv_net::NetConfig,
    user_agent: Option<String>,
}
fn configuration(
    owner: &Runtime,
    request: ProbeRequest,
) -> impl Future<Output = InspectionResult<Configuration>> + use<> {
    let services = owner.services();
    let net = crate::settings::net_config(&services.settings());
    let read = services.analyze(move |store| {
        if store.source(request.source)?.is_none() {
            return Ok(Err(InspectionError::SourceMissing(request.source)));
        }
        if !store.source_policy(request.source)?.enabled {
            return Ok(Err(InspectionError::SourceDisabled(request.source)));
        }
        let source_incarnation = store.source_incarnation(request.source)?;
        let (url, feed, incarnation, time) = match request.target {
            ProbeTarget::Configured(feed) => {
                let Some(config) = store
                    .guide_sources(request.source)?
                    .into_iter()
                    .find(|f| f.feed == feed && f.enabled)
                else {
                    return Ok(Err(InspectionError::StaleEvidence));
                };
                let Some(incarnation) = store.feed_incarnation(request.source, feed)? else {
                    return Ok(Err(InspectionError::StaleEvidence));
                };
                (
                    config.url,
                    Some(feed),
                    incarnation,
                    TimePolicy {
                        shift_minutes: config.shift_minutes,
                        ..TimePolicy::UTC
                    },
                )
            }
            ProbeTarget::Supplied(url) => {
                let incarnation = hex(&random::<16>().map_err(|_| {
                    iptv_store_sqlite::StoreError::Unavailable("probe identity unavailable")
                })?);
                (url, None, incarnation, TimePolicy::UTC)
            }
        };
        let source = store.source(request.source)?.expect("source checked");
        let user_agent = match source.source.config {
            iptv_providers::model::SourceConfig::M3u(c) => c.user_agent,
            iptv_providers::model::SourceConfig::Xtream(c) => c.user_agent,
            _ => None,
        };
        Ok(Ok(Configuration {
            url,
            feed,
            incarnation,
            source_incarnation,
            time,
            net,
            user_agent,
        }))
    });
    async move { read.await.map_err(|_| InspectionError::Storage)? }
}

fn parse_options(time: TimePolicy) -> ParseOptions {
    let mut options = ParseOptions {
        time,
        descriptions: false,
        ..ParseOptions::default()
    };
    options.limits.input_bytes = PROBE_BYTES;
    options.limits.decoded_bytes = PROBE_EXPANDED_BYTES;
    options.limits.record_bytes = 1 << 20;
    options.limits.items = 1_000_000;
    options.limits.elapsed = PROBE_DEADLINE;
    options
}
fn strict_error(error: iptv_epg::StrictError) -> InspectionError {
    match error {
        iptv_epg::StrictError::Decode(iptv_epg::DecodeError::Limit(_)) => {
            InspectionError::LimitExceeded
        }
        _ => InspectionError::InvalidFeed,
    }
}
fn network_error(error: iptv_import::FetchError) -> InspectionError {
    use iptv_import::FetchErrorKind;
    match error.kind {
        FetchErrorKind::TooLarge => InspectionError::LimitExceeded,
        FetchErrorKind::Deadline => InspectionError::Deadline,
        FetchErrorKind::InvalidRequest => InspectionError::AddressPolicy,
        _ => InspectionError::Network,
    }
}

impl Runtime {
    pub fn probe_feed(
        &self,
        request: ProbeRequest,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<ProbeSummary>> + use<> {
        let source = request.source;
        let config = configuration(self, request);
        let services = self.services();
        let stages = self.inspection.stages.clone();
        let key = self.inspection.key.clone();
        let running = self.inspection.running.clone();
        let deadline = Instant::now() + control.timeout.min(PROBE_DEADLINE);
        let accepted = services.clone();
        let cancel = control.cancel.clone();
        let task = services.spawn_blocking(move || {
            if running.fetch_add(1, Ordering::SeqCst) >= 2 {
                running.fetch_sub(1, Ordering::SeqCst);
                return Err(InspectionError::Busy);
            }
            let _running = Running(running);
            let key = key.ok_or(InspectionError::Storage)?;
            check(&accepted, &cancel, deadline)?;
            let configuration =
                accepted.block_on(guarded(&accepted, &control, deadline, config))?;
            if configuration.url.expose().len() > 4096 {
                return Err(InspectionError::InvalidRequest);
            }
            let id = ProbeId(hex(&random::<16>()?));
            let directory = accepted.directory().join("guide-probes");
            std::fs::create_dir_all(&directory).map_err(|_| InspectionError::Storage)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| InspectionError::Storage)?;
            }
            (accepted.ports().exclude_from_backup)(&directory)
                .map_err(|_| InspectionError::Storage)?;
            let path = directory.join(format!("{}.sqlite", id.0));
            let evidence = ProbeEvidence {
                stage: id.clone(),
                source,
                feed: configuration.feed,
                incarnation: configuration.incarnation.clone(),
                digest: [0; 32],
                expires_at: now() + STAGE_TTL,
            };
            let mut snapshot = Snapshot {
                path,
                key,
                evidence,
                source_incarnation: configuration.source_incarnation.clone(),
                services: accepted.clone(),
                bytes: 0,
                time: configuration.time,
                location: configuration.url.clone(),
            };
            let mut stage = storage(InspectionStage::create(&snapshot.path, &snapshot.key))?;
            let mut net = configuration.net;
            net.max_redirects = 3;
            net.request_timeout = PROBE_DEADLINE;
            net.max_streamed_bytes = PROBE_BYTES;
            net.allow_files = false;
            net.read_timeout = Duration::from_secs(15);
            let client = iptv_net::NetClient::new(net).map_err(network_error)?;
            let mut request = HttpRequest::get(configuration.url.expose());
            request.public_only = true;
            request.preserve_gzip = true;
            request.timeout = Some(deadline.saturating_duration_since(Instant::now()));
            if let Some(agent) = configuration.user_agent {
                request = request.with_header("User-Agent", agent);
            }
            let mut response =
                accepted.block_on(guarded(&accepted, &control, deadline, async {
                    client.fetch_stream(request).await.map_err(network_error)
                }))?;
            if !response.is_success() {
                return Err(InspectionError::Network);
            }
            let mut parser = StrictXmltvParser::new(parse_options(configuration.time));
            let mut hash = Sha256::new();
            let mut channels = 0u64;
            let mut error = None;
            loop {
                check(&accepted, &cancel, deadline)?;
                let chunk = accepted.block_on(guarded(&accepted, &control, deadline, async {
                    response.body.next_chunk().await.map_err(network_error)
                }))?;
                let Some(chunk) = chunk else { break };
                for part in chunk.chunks(64 * 1024) {
                    check(&accepted, &cancel, deadline)?;
                    snapshot.bytes += part.len() as u64;
                    if snapshot.bytes > PROBE_BYTES {
                        return Err(InspectionError::LimitExceeded);
                    }
                    storage(stage.chunk(part))?;
                    hash.update(part);
                    parser
                        .feed(part, &mut |item| {
                            if let XmltvItem::Channel(channel) = item {
                                channels += 1;
                                if channels > 100_000 {
                                    error = Some(InspectionError::LimitExceeded);
                                } else if stage.channel(&channel).is_err() {
                                    error = Some(InspectionError::InvalidFeed);
                                }
                            }
                        })
                        .map_err(strict_error)?;
                    if let Some(error) = error.take() {
                        return Err(error);
                    }
                }
            }
            let stats = parser
                .finish(&mut |item| {
                    if let XmltvItem::Channel(channel) = item {
                        channels += 1;
                        if stage.channel(&channel).is_err() {
                            error = Some(InspectionError::InvalidFeed);
                        }
                    }
                })
                .map_err(strict_error)?;
            if let Some(error) = error {
                return Err(error);
            }
            if channels == 0 {
                return Err(InspectionError::InvalidFeed);
            }
            storage(stage.finish())?;
            snapshot.evidence.digest = hash.finalize().into();
            snapshot.bytes = std::fs::metadata(&snapshot.path)
                .map_err(|_| InspectionError::Storage)?
                .len();
            let summary = ProbeSummary {
                evidence: snapshot.evidence.clone(),
                channels,
                programmes: stats.programmes,
                bytes: stats.bytes_in,
                warnings: stats.warnings.total(),
            };
            check(&accepted, &cancel, deadline)?;
            let mut stages = stages.lock().unwrap_or_else(|e| e.into_inner());
            stages.retain(|_, stage| {
                stage.evidence.expires_at > now() || Arc::strong_count(stage) > 1
            });
            let used: u64 = stages.values().map(|s| s.bytes).sum();
            if used.saturating_add(snapshot.bytes) > STAGE_BUDGET {
                return Err(InspectionError::LimitExceeded);
            }
            stages.insert(id, Arc::new(snapshot));
            Ok(summary)
        });
        async move { task.await.map_err(|_| InspectionError::OwnerClosed)? }
    }

    pub fn lease_probe(&self, evidence: &ProbeEvidence) -> InspectionResult<ProbeLease> {
        let stages = self
            .inspection
            .stages
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let stage = stages
            .get(&evidence.stage)
            .ok_or(InspectionError::StaleEvidence)?;
        if stage.evidence != *evidence {
            return Err(InspectionError::StaleEvidence);
        }
        let lease = ProbeLease(stage.clone());
        lease.check()?;
        Ok(lease)
    }

    pub fn probe_candidates(
        &self,
        evidence: ProbeEvidence,
        text: String,
        after: Option<String>,
        limit: u32,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<Candidate>>> + use<> {
        let lease = self.lease_probe(&evidence);
        let services = self.services();
        let task = services.spawn_blocking(move || {
            let lease = lease?;
            lease.check()?;
            if limit == 0 || limit > MAX_PAGE || text.len() > MAX_QUERY_BYTES {
                return Err(InspectionError::InvalidRequest);
            }
            let stage = storage(InspectionStage::open(&lease.0.path, &lease.0.key))?;
            let mut channels =
                storage(stage.channels(after.as_deref().unwrap_or(""), &text, limit + 1))?;
            if control.cancel.is_cancelled() {
                return Err(InspectionError::Cancelled);
            }
            let more = channels.len() > limit as usize;
            channels.truncate(limit as usize);
            let next = more.then(|| Cursor {
                context: evidence.stage.0.clone(),
                snapshot: hex(&evidence.digest),
                after: vec![channels.last().expect("nonzero page").id.clone()],
            });
            Ok(Page {
                items: channels
                    .into_iter()
                    .map(|c| Candidate {
                        key: CandidateRef {
                            source: evidence.source,
                            feed: evidence.feed.unwrap_or(FeedId(0)),
                            incarnation: evidence.incarnation.clone(),
                            epg_channel: c.id,
                        },
                        name: c.display_names.first().cloned(),
                        names: c.display_names,
                        metadata: Vec::new(),
                        assigned: false,
                        evidence: vec![Evidence::Alternative],
                        binding: None,
                    })
                    .collect(),
                next,
            })
        });
        async move { task.await.map_err(|_| InspectionError::OwnerClosed)? }
    }

    pub fn probe_schedule(
        &self,
        evidence: ProbeEvidence,
        candidate: CandidateRef,
        window: TimeWindow,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Vec<ProgrammeInfo>>> + use<> {
        let lease = self.lease_probe(&evidence);
        let services = self.services();
        let deadline = Instant::now() + control.timeout.min(PROBE_DEADLINE);
        let task = services.spawn_blocking(move || {
            if !window.valid() {
                return Err(InspectionError::InvalidRequest);
            }
            let lease = lease?;
            lease.validate_candidate(&candidate)?;
            let stage = storage(InspectionStage::open(&lease.0.path, &lease.0.key))?;
            let mut hash = Sha256::new();
            // The probe parser already applied this feed's configured shift.
            // Reparse with the same policy, captured independently of current config.
            let mut parser = StrictXmltvParser::new(parse_options(lease.0.time));
            let mut programmes = Vec::new();
            let mut failure = None;
            let mut sink = |item| {
                if let XmltvItem::Programme(p) = item {
                    if p.channel == candidate.epg_channel && p.start < window.to {
                        if let Some(stop) =
                            p.stop.filter(|stop| *stop > p.start && *stop > window.from)
                        {
                            if programmes.len() < MAX_PAGE as usize {
                                programmes.push(ProgrammeInfo {
                                    channel: ChannelRef {
                                        source: evidence.source,
                                        content: iptv_providers::ContentId::from_stored(
                                            candidate.epg_channel.clone(),
                                        ),
                                    },
                                    candidate: candidate.clone(),
                                    source_incarnation: lease.0.source_incarnation.clone(),
                                    evidence_revision: hex(&evidence.digest),
                                    provider_event_id: None,
                                    start: p.start,
                                    stop,
                                    title: p.title,
                                });
                            }
                        }
                    }
                }
            };
            storage(stage.for_each_chunk(|bytes| {
                if let Err(error) = check(&lease.0.services, &control.cancel, deadline) {
                    failure = Some(error);
                    return Err(iptv_store_sqlite::StoreError::Unavailable(
                        "probe inspection stopped",
                    ));
                }
                hash.update(bytes);
                if let Err(error) = parser.feed(bytes, &mut sink) {
                    failure = Some(strict_error(error));
                    return Err(iptv_store_sqlite::StoreError::Unavailable(
                        "probe snapshot cannot be parsed",
                    ));
                }
                Ok(())
            }))
            .or_else(|error| Err(failure.take().unwrap_or(error)))?;
            parser.finish(&mut sink).map_err(strict_error)?;
            let actual: [u8; 32] = hash.finalize().into();
            if actual != evidence.digest {
                return Err(InspectionError::StaleEvidence);
            }
            check(&lease.0.services, &control.cancel, deadline)?;
            programmes.sort_by_key(|p| p.start);
            Ok(programmes)
        });
        async move { task.await.map_err(|_| InspectionError::OwnerClosed)? }
    }
}
