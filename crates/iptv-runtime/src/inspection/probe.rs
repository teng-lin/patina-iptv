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
    fs::{File, OpenOptions},
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
const RESERVATION: u64 = iptv_store_sqlite::INSPECTION_STAGE_BYTES;

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
    nonce: Mutex<String>,
    key: Mutex<Option<DatabaseKey>>,
    stages: Arc<Mutex<Space>>,
    directory: Arc<Mutex<Option<Arc<Directory>>>>,
    running: Arc<AtomicUsize>,
    queries: Arc<AtomicUsize>,
}
impl Vault {
    pub(crate) fn new() -> Self {
        let key = random().ok().map(DatabaseKey::from_bytes);
        Self {
            nonce: Mutex::new(random::<16>().map(|b| hex(&b)).unwrap_or_default()),
            key: Mutex::new(key),
            stages: Arc::new(Mutex::new(Space::default())),
            directory: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicUsize::new(0)),
            queries: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub(crate) fn context(&self, epoch: u64) -> String {
        format!(
            "{}:{epoch}",
            self.nonce.lock().unwrap_or_else(|e| e.into_inner())
        )
    }
    pub(super) fn admit(&self) -> Option<QuerySlot> {
        let previous = self.queries.fetch_add(1, Ordering::SeqCst);
        if previous >= 16 {
            self.queries.fetch_sub(1, Ordering::SeqCst);
            None
        } else {
            Some(QuerySlot(self.queries.clone()))
        }
    }
    pub(crate) fn ready(&self) -> bool {
        self.key.lock().unwrap_or_else(|e| e.into_inner()).is_some()
            && !self
                .nonce
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
    }
    pub(crate) fn renew(&self) {
        self.clear();
        *self.nonce.lock().unwrap_or_else(|e| e.into_inner()) =
            random::<16>().map(|b| hex(&b)).unwrap_or_default();
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) =
            random().ok().map(DatabaseKey::from_bytes);
    }
    pub(crate) fn clear(&self) {
        self.stages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stages
            .clear();
        self.key.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.nonce.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.directory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}

pub(super) struct QuerySlot(Arc<AtomicUsize>);
impl Drop for QuerySlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Space {
    stages: BTreeMap<ProbeId, Arc<Snapshot>>,
    reserved: u64,
}
struct Reservation(Arc<Mutex<Space>>);
impl Reservation {
    fn acquire(space: Arc<Mutex<Space>>) -> InspectionResult<Self> {
        {
            let mut state = space.lock().unwrap_or_else(|e| e.into_inner());
            state
                .stages
                .retain(|_, s| s.evidence.expires_at > now() || Arc::strong_count(s) > 1);
            loop {
                let used: u64 = state.stages.values().map(|s| s.bytes).sum();
                if used
                    .saturating_add(state.reserved)
                    .saturating_add(RESERVATION)
                    <= STAGE_BUDGET
                {
                    break;
                }
                let evict = state
                    .stages
                    .iter()
                    .find(|(_, s)| Arc::strong_count(s) == 1)
                    .map(|(id, _)| id.clone());
                let Some(evict) = evict else {
                    return Err(InspectionError::LimitExceeded);
                };
                state.stages.remove(&evict);
            }
            state.reserved += RESERVATION;
        }
        Ok(Self(space))
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).reserved -= RESERVATION;
    }
}
struct Directory {
    path: PathBuf,
    lock: Option<File>,
}
impl Directory {
    fn create(services: &Services, nonce: &str) -> InspectionResult<Arc<Self>> {
        let base = services.directory().join("guide-probes");
        std::fs::create_dir_all(&base).map_err(|_| InspectionError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| InspectionError::Storage)?;
        }
        (services.ports().exclude_from_backup)(&base).map_err(|_| InspectionError::Storage)?;
        // A random disposable key is never persisted. Remove unreadable crash
        // remnants only after obtaining their owner lock; active owners survive.
        for entry in std::fs::read_dir(&base).map_err(|_| InspectionError::Storage)? {
            let entry = entry.map_err(|_| InspectionError::Storage)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.len() != 32 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|_| InspectionError::Storage)?
                .is_dir()
            {
                continue;
            }
            if let Ok(file) = OpenOptions::new()
                .read(true)
                .write(true)
                .open(entry.path().join("owner.lock"))
                && file.try_lock().is_ok()
            {
                drop(file);
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let path = base.join(nonce);
        std::fs::create_dir(&path).map_err(|_| InspectionError::Storage)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(path.join("owner.lock"))
            .map_err(|_| InspectionError::Storage)?;
        lock.try_lock().map_err(|_| InspectionError::Busy)?;
        Ok(Arc::new(Self {
            path,
            lock: Some(lock),
        }))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        self.lock.take();
        let _ = std::fs::remove_dir_all(&self.path);
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
    _directory: Arc<Directory>,
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
    fn integrity(&self) -> InspectionResult<()> {
        self.check()?;
        let stage = storage(InspectionStage::open(&self.0.path, &self.0.key))?;
        let mut hash = Sha256::new();
        let deadline = Instant::now() + super::QUERY_DEADLINE;
        storage(stage.for_each_chunk(|bytes| {
            if Instant::now() >= deadline {
                return Err(iptv_store_sqlite::StoreError::Unavailable(
                    "probe integrity deadline",
                ));
            }
            hash.update(bytes);
            Ok(())
        }))?;
        let actual: [u8; 32] = hash.finalize().into();
        if actual != self.0.evidence.digest {
            return Err(InspectionError::StaleEvidence);
        }
        Ok(())
    }

    fn current_configuration(&self) -> InspectionResult<()> {
        self.check()?;
        let source = self.0.evidence.source;
        let source_incarnation = self.0.source_incarnation.clone();
        let feed = self.0.evidence.feed;
        let incarnation = self.0.evidence.incarnation.clone();
        let current = self.0.services.analyze(move |store| {
            Ok(store.source_incarnation(source)? == source_incarnation
                && match feed {
                    Some(feed) => {
                        store.feed_incarnation(source, feed)?.as_ref() == Some(&incarnation)
                    }
                    None => true,
                })
        });
        if !self
            .0
            .services
            .block_on(current)
            .map_err(|_| InspectionError::Storage)?
        {
            return Err(InspectionError::StaleEvidence);
        }
        Ok(())
    }

    pub fn validate_candidate(&self, candidate: &CandidateRef) -> InspectionResult<()> {
        self.integrity()?;
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
    public_only: bool,
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
        let (url, feed, incarnation, time, public_only) = match request.target {
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
                    config.url.clone(),
                    Some(feed),
                    incarnation,
                    TimePolicy {
                        shift_minutes: config.shift_minutes,
                        ..TimePolicy::UTC
                    },
                    config.is_provider(),
                )
            }
            ProbeTarget::Supplied(url) => {
                let incarnation = hex(&random::<16>().map_err(|_| {
                    iptv_store_sqlite::StoreError::Unavailable("probe identity unavailable")
                })?);
                (url, None, incarnation, TimePolicy::UTC, true)
            }
        };
        let source = store.source(request.source)?.expect("source checked");
        let user_agent = if feed.is_none() {
            None
        } else {
            match source.source.config {
                iptv_providers::model::SourceConfig::M3u(c) => c.user_agent,
                iptv_providers::model::SourceConfig::Xtream(c) => c.user_agent,
                _ => None,
            }
        };
        Ok(Ok(Configuration {
            url,
            feed,
            incarnation,
            source_incarnation,
            time,
            net,
            user_agent,
            public_only,
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

fn await_worker<T: Send + 'static>(
    services: Services,
    control: InspectionControl,
    deadline: Instant,
    task: tokio::task::JoinHandle<InspectionResult<T>>,
    cancel: CancelToken,
) -> impl Future<Output = InspectionResult<T>> + use<T> {
    let guard = super::CancelOnDrop(cancel.clone());
    let accepted = services.clone();
    let reply = services.spawn(async move {
        let result = guarded(&accepted, &control, deadline, async move {
            task.await.map_err(|_| InspectionError::OwnerClosed)?
        })
        .await;
        if result.is_err() {
            cancel.cancel();
        }
        result
    });
    async move {
        let _guard = guard;
        reply.await.map_err(|_| InspectionError::OwnerClosed)?
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
        let key = self
            .inspection
            .key
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let directory = self.inspection.directory.clone();
        let nonce = self
            .inspection
            .nonce
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let running = self.inspection.running.clone();
        let deadline = Instant::now() + control.timeout.min(PROBE_DEADLINE);
        let accepted = services.clone();

        let mut worker_control = control.clone();
        worker_control.cancel = CancelToken::new();
        let operation_cancel = worker_control.cancel.clone();
        let cancel = operation_cancel.clone();
        let task = services.spawn_blocking(move || {
            if running.fetch_add(1, Ordering::SeqCst) >= 2 {
                running.fetch_sub(1, Ordering::SeqCst);
                return Err(InspectionError::Busy);
            }
            let _running = Running(running);
            let key = key.ok_or(InspectionError::Storage)?;
            let reservation = Reservation::acquire(stages.clone())?;
            check(&accepted, &cancel, deadline)?;
            let configuration =
                accepted.block_on(guarded(&accepted, &worker_control, deadline, config))?;
            if configuration.url.expose().len() > 4096 {
                return Err(InspectionError::InvalidRequest);
            }
            let id = ProbeId(hex(&random::<16>()?));
            let directory = {
                let mut directory = directory.lock().unwrap_or_else(|e| e.into_inner());
                if directory.is_none() {
                    *directory = Some(Directory::create(&accepted, &nonce)?);
                }
                directory.as_ref().expect("initialized directory").clone()
            };
            let path = directory.path.join(format!("{}.sqlite", id.0));
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
                _directory: directory,
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
            request.public_only = configuration.public_only;
            request.preserve_gzip = true;
            request.timeout = Some(deadline.saturating_duration_since(Instant::now()));
            if let Some(agent) = configuration.user_agent {
                request = request.with_header("User-Agent", agent);
            }
            let mut response =
                accepted.block_on(guarded(&accepted, &worker_control, deadline, async {
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
                let chunk =
                    accepted.block_on(guarded(&accepted, &worker_control, deadline, async {
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
            if channels > 100_000 {
                return Err(InspectionError::LimitExceeded);
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
            if snapshot.bytes > RESERVATION {
                return Err(InspectionError::LimitExceeded);
            }
            let current_source = configuration.source_incarnation.clone();
            let current_feed = configuration.feed;
            let current_incarnation = configuration.incarnation.clone();
            let current = accepted.analyze(move |store| {
                Ok(store.source_incarnation(source)? == current_source
                    && match current_feed {
                        Some(feed) => {
                            store.feed_incarnation(source, feed)?.as_ref()
                                == Some(&current_incarnation)
                        }
                        None => true,
                    })
            });
            if !accepted
                .block_on(current)
                .map_err(|_| InspectionError::Storage)?
            {
                return Err(InspectionError::StaleEvidence);
            }
            {
                let mut state = stages.lock().unwrap_or_else(|e| e.into_inner());
                check(&accepted, &cancel, deadline)?;
                state.stages.insert(id, Arc::new(snapshot));
            }
            drop(reservation);
            Ok(summary)
        });
        await_worker(services, control, deadline, task, operation_cancel)
    }

    pub fn lease_probe(&self, evidence: &ProbeEvidence) -> InspectionResult<ProbeLease> {
        let stages = self
            .inspection
            .stages
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let stage = stages
            .stages
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
        after: Option<Cursor>,
        limit: u32,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<Candidate>>> + use<> {
        let lease = self.lease_probe(&evidence);
        let services = self.services();
        let deadline = Instant::now() + control.timeout.min(super::QUERY_DEADLINE);
        let mut worker_control = control.clone();
        worker_control.cancel = CancelToken::new();
        let operation_cancel = worker_control.cancel.clone();
        let task = services.spawn_blocking(move || {
            let lease = lease?;
            lease.current_configuration()?;
            check(&lease.0.services, &worker_control.cancel, deadline)?;
            if limit == 0 || limit > MAX_PAGE || text.len() > MAX_QUERY_BYTES {
                return Err(InspectionError::InvalidRequest);
            }
            let context = hex(&Sha256::digest(
                format!("{}:{text}", evidence.stage.0).as_bytes(),
            ));
            let snapshot = hex(&evidence.digest);
            if after.as_ref().is_some_and(|c| {
                c.context != context
                    || c.snapshot != snapshot
                    || c.after.len() != 1
                    || c.after[0].len() > 4096
            }) {
                return Err(InspectionError::StaleCursor);
            }
            let mut stage = storage(InspectionStage::open(&lease.0.path, &lease.0.key))?;
            let mut channels =
                storage(
                    stage.bounded(worker_control.cancel.clone(), deadline, |stage| {
                        stage.channels(
                            after.as_ref().map(|c| c.after[0].as_str()).unwrap_or(""),
                            &text,
                            limit + 1,
                        )
                    }),
                )?;
            check(&lease.0.services, &worker_control.cancel, deadline)?;
            let more = channels.len() > limit as usize;
            channels.truncate(limit as usize);
            let next = more.then(|| Cursor {
                context,
                snapshot,
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
        await_worker(services, control, deadline, task, operation_cancel)
    }

    pub fn probe_schedule(
        &self,
        evidence: ProbeEvidence,
        candidate: CandidateRef,
        window: TimeWindow,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Vec<CandidateProgramme>>> + use<> {
        let lease = self.lease_probe(&evidence);
        let services = self.services();
        let deadline = Instant::now() + control.timeout.min(PROBE_DEADLINE);
        let mut worker_control = control.clone();
        worker_control.cancel = CancelToken::new();
        let operation_cancel = worker_control.cancel.clone();
        let task = services.spawn_blocking(move || {
            if !window.valid() {
                return Err(InspectionError::InvalidRequest);
            }
            let lease = lease?;
            lease.current_configuration()?;
            check(&lease.0.services, &worker_control.cancel, deadline)?;
            lease.validate_candidate(&candidate)?;
            let stage = storage(InspectionStage::open(&lease.0.path, &lease.0.key))?;
            let mut hash = Sha256::new();
            // The probe parser already applied this feed's configured shift.
            // Reparse with the same policy, captured independently of current config.
            let mut parser = StrictXmltvParser::new(parse_options(lease.0.time));
            let mut raw = BTreeMap::<i64, iptv_epg::XmltvProgramme>::new();
            let mut overflow = false;
            let mut failure = None;
            let mut sink = |item| {
                if overflow {
                    return;
                }
                let XmltvItem::Programme(programme) = item else {
                    return;
                };
                if programme.channel != candidate.epg_channel
                    || programme
                        .stop
                        .is_some_and(|stop| stop <= programme.start || stop <= window.from)
                {
                    return;
                }
                // Retain one look-ahead event to infer the final visible stop.
                if programme.start >= window.to {
                    if raw
                        .range(window.to..)
                        .next()
                        .is_some_and(|(&start, _)| start <= programme.start)
                    {
                        return;
                    }
                    if let Some((&start, _)) = raw.range(window.to..).next() {
                        raw.remove(&start);
                    }
                }
                if programme.start < window.from && programme.stop.is_none() {
                    if raw
                        .range(..window.from)
                        .next_back()
                        .is_some_and(|(&start, _)| start > programme.start)
                    {
                        return;
                    }
                    if let Some((&start, entry)) = raw.range(..window.from).next_back()
                        && entry.stop.is_none()
                    {
                        raw.remove(&start);
                    }
                }
                match raw.entry(programme.start) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(programme);
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if iptv_epg::ScheduleRules::replaces(programme.stop, entry.get().stop) {
                            entry.insert(programme);
                        }
                    }
                }
                if raw.len() > MAX_PAGE as usize + 2 {
                    overflow = true;
                }
            };
            storage(stage.for_each_chunk(|bytes| {
                if let Err(error) = check(&lease.0.services, &worker_control.cancel, deadline) {
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
            .map_err(|error| failure.take().unwrap_or(error))?;
            parser.finish(&mut sink).map_err(strict_error)?;
            let actual: [u8; 32] = hash.finalize().into();
            if actual != evidence.digest {
                return Err(InspectionError::StaleEvidence);
            }
            check(&lease.0.services, &worker_control.cancel, deadline)?;
            if overflow {
                return Err(InspectionError::LimitExceeded);
            }
            let raw: Vec<_> = raw.into_values().collect();
            let intervals: Vec<_> = raw.iter().map(|p| (p.start, p.stop)).collect();
            let mut programmes = Vec::new();
            iptv_epg::ScheduleRules::new(
                iptv_epg::StoreOptions {
                    past_days_to_keep: None,
                    ..Default::default()
                },
                window.from,
            )
            .settle(
                &intervals,
                &mut iptv_epg::IngestReport::default(),
                |at, fate| {
                    let p = &raw[at];
                    if let iptv_epg::Settled::Keep { stop } = fate
                        && p.start < window.to
                        && stop > window.from
                    {
                        programmes.push(CandidateProgramme {
                            candidate: candidate.clone(),
                            evidence_revision: hex(&evidence.digest),
                            provider_event_id: None,
                            start: p.start,
                            stop,
                            title: p.title.clone(),
                        });
                    }
                },
            );
            if programmes.len() > MAX_PAGE as usize {
                return Err(InspectionError::LimitExceeded);
            }
            Ok(programmes)
        });
        await_worker(services, control, deadline, task, operation_cancel)
    }
}
