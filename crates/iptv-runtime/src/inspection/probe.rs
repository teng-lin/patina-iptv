//! Disposable encrypted snapshots with leases that pin evidence during repair.
use super::{InspectionControl, guarded};
use crate::model::{ProbeLease, ProbeRequest, ProbeTarget};
use crate::time::Instant;
use crate::{Runtime, store_service::Services};
use iptv_epg::{FeedId, ParseOptions, StrictXmltvParser, TimePolicy, XmltvItem};
use iptv_import::{BodyStream, CancelToken, Fetch};
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
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

pub const PROBE_BYTES: u64 = 32 << 20;
pub const PROBE_EXPANDED_BYTES: u64 = 256 << 20;
pub const PROBE_DEADLINE: Duration = Duration::from_secs(60);
pub const STAGE_BUDGET: u64 = 512 << 20;
pub const STAGE_TTL: i64 = 24 * 60 * 60;
const RESERVATION: u64 = iptv_store_sqlite::INSPECTION_STAGE_BYTES;

fn now() -> i64 {
    crate::time::now_unix()
}
fn random<const N: usize>(entropy: crate::ports::Entropy) -> InspectionResult<[u8; N]> {
    let mut bytes = [0; N];
    entropy(&mut bytes).map_err(|_| InspectionError::Storage)?;
    Ok(bytes)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
/// `operation` on a probe's stage, within `deadline` and `cancel`. Stages
/// are files of the native store; a browser build has none (its staged
/// probes are refused before they start).
#[cfg(feature = "native")]
fn bounded<T>(
    stage: &mut InspectionStage,
    cancel: CancelToken,
    deadline: Instant,
    operation: impl FnOnce(&InspectionStage) -> iptv_store_sqlite::Result<T>,
) -> iptv_store_sqlite::Result<T> {
    stage.bounded_until(cancel, crate::time::expiry(deadline), operation)
}

#[cfg(not(feature = "native"))]
fn bounded<T>(
    _: &mut InspectionStage,
    _: CancelToken,
    _: Instant,
    _: impl FnOnce(&InspectionStage) -> iptv_store_sqlite::Result<T>,
) -> iptv_store_sqlite::Result<T> {
    Err(iptv_store_sqlite::StoreError::Unavailable(
        "staged probes need a native store",
    ))
}

fn storage<T>(result: iptv_store_sqlite::Result<T>) -> InspectionResult<T> {
    result.map_err(|_| InspectionError::Storage)
}
fn check(services: &Services, cancel: &CancelToken, deadline: Instant) -> InspectionResult<()> {
    if !services.is_current() {
        return Err(InspectionError::OwnerClosed);
    }
    if cancel.is_canceled() {
        return Err(InspectionError::Canceled);
    }
    if Instant::now() >= deadline {
        return Err(InspectionError::Deadline);
    }
    Ok(())
}

pub(crate) struct Vault {
    entropy: crate::ports::Entropy,
    nonce: Mutex<String>,
    key: Mutex<Option<DatabaseKey>>,
    stages: Arc<Mutex<Space>>,
    directory: Arc<Mutex<Option<Arc<Directory>>>>,
    running: Arc<AtomicUsize>,
    queries: Arc<AtomicUsize>,
    calls: Arc<Mutex<BTreeMap<u64, CancelToken>>>,
    next_call: AtomicU64,
    workers: Arc<Workers>,
    #[cfg(feature = "test-support")]
    scan_hook: std::cell::RefCell<Option<crate::inspection::ScanHook>>,
    #[cfg(feature = "test-support")]
    stage_hook: Arc<Mutex<Option<Arc<crate::testing::GateState>>>>,
}
impl Vault {
    pub(crate) fn new(entropy: crate::ports::Entropy) -> Self {
        let key = random(entropy).ok().map(DatabaseKey::from_bytes);
        Self {
            entropy,
            nonce: Mutex::new(random::<16>(entropy).map(|b| hex(&b)).unwrap_or_default()),
            key: Mutex::new(key),
            stages: Arc::new(Mutex::new(Space::default())),
            directory: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicUsize::new(0)),
            queries: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(BTreeMap::new())),
            next_call: AtomicU64::new(0),
            workers: Arc::default(),
            #[cfg(feature = "test-support")]
            scan_hook: std::cell::RefCell::default(),
            #[cfg(feature = "test-support")]
            stage_hook: Arc::default(),
        }
    }
    #[cfg(feature = "test-support")]
    pub(crate) fn set_scan_hook(&self, hook: crate::inspection::ScanHook) {
        *self.scan_hook.borrow_mut() = Some(hook);
    }
    #[cfg(feature = "test-support")]
    pub(crate) fn take_scan_hook(&self) -> Option<crate::inspection::ScanHook> {
        self.scan_hook.borrow_mut().take()
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
    pub(super) fn register(&self, cancel: CancelToken) -> CallGuard {
        let id = self.next_call.fetch_add(1, Ordering::SeqCst);
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, cancel.clone());
        CallGuard {
            calls: self.calls.clone(),
            id,
            cancel,
        }
    }
    pub(crate) fn cancel_requests(&self) {
        let calls = std::mem::take(&mut *self.calls.lock().unwrap_or_else(|e| e.into_inner()));
        for cancel in calls.into_values() {
            cancel.cancel();
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
        *self.nonce.lock().unwrap_or_else(|e| e.into_inner()) = random::<16>(self.entropy)
            .map(|b| hex(&b))
            .unwrap_or_default();
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) =
            random(self.entropy).ok().map(DatabaseKey::from_bytes);
        self.workers
            .state
            .lock()
            .expect("probe worker state poisoned")
            .accepting = true;
    }
    pub(crate) fn close_workers(&self) {
        self.workers
            .state
            .lock()
            .expect("probe worker state poisoned")
            .accepting = false;
    }
    pub(crate) fn drain_workers(&self, limit: Duration) -> bool {
        self.workers.drain(limit)
    }
    pub(crate) fn clear(&self) {
        self.close_workers();
        self.cancel_requests();
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

#[derive(Default)]
struct WorkerState {
    accepting: bool,
    active: usize,
}

#[derive(Default)]
struct Workers {
    state: Mutex<WorkerState>,
    finished: Condvar,
    #[cfg(feature = "test-support")]
    draining: Mutex<Option<Arc<crate::testing::GateState>>>,
}
impl Workers {
    fn enter(self: &Arc<Self>) -> Option<Completion> {
        let mut state = self.state.lock().expect("probe worker state poisoned");
        if !state.accepting {
            return None;
        }
        state.active += 1;
        Some(Completion(self.clone()))
    }

    fn drain(&self, limit: Duration) -> bool {
        let state = self.state.lock().expect("probe worker state poisoned");
        #[cfg(feature = "test-support")]
        if state.active != 0
            && let Some(gate) = self.draining.lock().unwrap().take()
        {
            gate.acknowledge();
        }
        let (state, _) = self
            .finished
            .wait_timeout_while(state, limit, |state| state.active != 0)
            .expect("probe worker state poisoned");
        state.active == 0
    }
}

struct Completion(Arc<Workers>);
impl Drop for Completion {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("probe worker state poisoned");
        state.active -= 1;
        self.0.finished.notify_all();
    }
}

// Fields drop in declaration order, including if a queued task never starts.
// Completion must follow the operation's captured snapshots and capacity permit.
struct WorkerOperation<F, Permit> {
    operation: F,
    _permit: Permit,
    _completion: Completion,
}
impl<F, Permit> WorkerOperation<F, Permit> {
    fn run<T>(self) -> T
    where
        F: FnOnce() -> T,
    {
        (self.operation)()
    }
}

pub(super) struct CallGuard {
    calls: Arc<Mutex<BTreeMap<u64, CancelToken>>>,
    id: u64,
    cancel: CancelToken,
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
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
impl Running {
    fn acquire(running: Arc<AtomicUsize>) -> Option<Self> {
        if running.fetch_add(1, Ordering::SeqCst) >= 2 {
            running.fetch_sub(1, Ordering::SeqCst);
            None
        } else {
            Some(Self(running))
        }
    }
}
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
impl crate::model::ProbeSnapshot for Snapshot {}

impl ProbeLease {
    fn snapshot(&self) -> &Snapshot {
        self.downcast_ref::<Snapshot>()
    }

    /// Private configuration value; callers must preserve SecretUrl redaction.
    pub fn location(&self) -> SecretUrl {
        self.snapshot().location.clone()
    }
    /// Confirms the stage is still in its owning runtime epoch and within its lifetime.
    pub fn validate_alive(&self) -> InspectionResult<()> {
        self.check()
    }
    /// Reads the staged feed's listed channels within the default probe budget.
    pub fn listed_channels(&self) -> InspectionResult<Vec<iptv_epg::XmltvChannel>> {
        self.listed_channels_with_control(&CancelToken::new(), Instant::now() + PROBE_DEADLINE)
    }
    /// Reads the staged feed's listed channels under the caller's deadline and cancellation.
    pub fn listed_channels_with_control(
        &self,
        cancel: &CancelToken,
        deadline: Instant,
    ) -> InspectionResult<Vec<iptv_epg::XmltvChannel>> {
        let snapshot = self.snapshot();
        check(&snapshot.services, cancel, deadline)?;
        self.check()?;
        let mut stage = storage(InspectionStage::open(&snapshot.path, &snapshot.key))?;
        let result = bounded(&mut stage, cancel.clone(), deadline, |stage| {
            let mut channels = Vec::new();
            let mut after = String::new();
            loop {
                check(&snapshot.services, cancel, deadline).map_err(|_| {
                    iptv_store_sqlite::StoreError::Unavailable("probe channel inspection stopped")
                })?;
                let page = stage.channels(&after, "", MAX_PAGE)?;
                if page.is_empty() {
                    break;
                }
                after = page.last().expect("nonempty page").id.clone();
                channels.extend(page);
                if channels.len() > 100_000 {
                    return Err(iptv_store_sqlite::StoreError::Unavailable(
                        "probe channel limit exceeded",
                    ));
                }
            }
            Ok(channels)
        });
        check(&snapshot.services, cancel, deadline)?;
        storage(result)
    }
    /// Evidence identifying the exact leased stage.
    pub fn evidence(&self) -> &ProbeEvidence {
        &self.snapshot().evidence
    }
    /// The playlist incarnation that admitted the probe.
    pub fn source_incarnation(&self) -> &str {
        &self.snapshot().source_incarnation
    }
    fn integrity(
        &self,
        stage: &InspectionStage,
        cancel: &CancelToken,
        deadline: Instant,
    ) -> InspectionResult<()> {
        let snapshot = self.snapshot();
        self.check()?;
        let mut hash = Sha256::new();
        let result = stage.for_each_chunk(|bytes| {
            check(&snapshot.services, cancel, deadline).map_err(|_| {
                iptv_store_sqlite::StoreError::Unavailable("probe integrity inspection stopped")
            })?;
            hash.update(bytes);
            Ok(())
        });
        check(&snapshot.services, cancel, deadline)?;
        storage(result)?;
        let actual: [u8; 32] = hash.finalize().into();
        if actual != snapshot.evidence.digest {
            return Err(InspectionError::StaleEvidence);
        }
        Ok(())
    }

    fn current_configuration(
        &self,
        cancel: &CancelToken,
        deadline: Instant,
    ) -> InspectionResult<()> {
        let snapshot = self.snapshot();
        self.check()?;
        let source = snapshot.evidence.source;
        let source_incarnation = snapshot.source_incarnation.clone();
        let feed = snapshot.evidence.feed;
        let incarnation = snapshot.evidence.incarnation.clone();
        let query_cancel = cancel.clone();
        let current = snapshot.services.analyze(move |store| {
            store.inspection_read_until(
                query_cancel,
                crate::time::expiry(deadline),
                || {},
                |store| {
                    store
                        .read_snapshot(|store| {
                            if store.source(source)?.is_none()
                                || !store.source_policy(source)?.enabled
                            {
                                return Ok(false);
                            }
                            Ok(store.source_incarnation(source)? == source_incarnation
                                && match feed {
                                    Some(feed) => {
                                        store.feed_incarnation(source, feed)?.as_ref()
                                            == Some(&incarnation)
                                    }
                                    None => true,
                                })
                        })
                        .map(Ok)
                },
            )
        });
        let control = InspectionControl {
            cancel: cancel.clone(),
            timeout: deadline.saturating_duration_since(Instant::now()),
        };
        let valid = snapshot
            .services
            .try_block_on(guarded(
                &snapshot.services,
                &control,
                deadline,
                async move { current.await.map_err(|_| InspectionError::Storage)? },
            ))
            .ok_or(InspectionError::Storage)??;
        if !valid {
            return Err(InspectionError::StaleEvidence);
        }
        Ok(())
    }

    /// Confirms the candidate occurs in this staged snapshot.
    pub fn validate_candidate(&self, candidate: &CandidateRef) -> InspectionResult<()> {
        self.validate_candidates_with_control(
            std::slice::from_ref(candidate),
            &CancelToken::new(),
            Instant::now() + PROBE_DEADLINE,
        )
    }
    /// Validate a repair batch with one encrypted open and one integrity scan.
    pub fn validate_candidates_with_control(
        &self,
        candidates: &[CandidateRef],
        cancel: &CancelToken,
        deadline: Instant,
    ) -> InspectionResult<()> {
        let snapshot = self.snapshot();
        self.check()?;
        check(&snapshot.services, cancel, deadline)?;
        if candidates.len() > 500 {
            return Err(InspectionError::LimitExceeded);
        }
        for candidate in candidates {
            if candidate.source != snapshot.evidence.source
                || snapshot
                    .evidence
                    .feed
                    .is_some_and(|feed| feed != candidate.feed)
                || candidate.incarnation != snapshot.evidence.incarnation
            {
                return Err(InspectionError::StaleEvidence);
            }
        }
        let mut stage = storage(InspectionStage::open(&snapshot.path, &snapshot.key))?;
        let mut failure = None;
        let result = bounded(&mut stage, cancel.clone(), deadline, |stage| {
            self.integrity(stage, cancel, deadline).map_err(|error| {
                failure = Some(error);
                iptv_store_sqlite::StoreError::Unavailable("probe integrity failed")
            })?;
            for candidate in candidates {
                if !stage.has_channel(&candidate.epg_channel)? {
                    return Ok(false);
                }
            }
            Ok(true)
        });
        check(&snapshot.services, cancel, deadline)?;
        if !storage(result).map_err(|error| failure.unwrap_or(error))? {
            return Err(InspectionError::StaleEvidence);
        }
        self.check()
    }
    /// Checks the owner epoch and expiry without opening the staged database.
    pub fn check(&self) -> InspectionResult<()> {
        let snapshot = self.snapshot();
        if !snapshot.services.is_current() {
            return Err(InspectionError::OwnerClosed);
        }
        if now() >= snapshot.evidence.expires_at {
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
    net: crate::network::NetworkConfig,
    user_agent: Option<String>,
    public_only: bool,
}
fn configuration(
    owner: &Runtime,
    request: ProbeRequest,
) -> impl Future<Output = InspectionResult<Configuration>> + use<> {
    let services = owner.services();
    let net = crate::settings::network_config(&services.settings());
    let entropy = services.ports().entropy;
    let read = services.analyze(move |store| {
        store.read_snapshot(|store| {
            let Some(source) = store.source(request.source)? else {
                return Ok(Err(InspectionError::SourceMissing(request.source)));
            };
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
                    if iptv_providers::content_url::validate_uncredentialed_http(url.expose())
                        .is_err()
                    {
                        return Ok(Err(InspectionError::AddressPolicy));
                    }
                    let incarnation = hex(&random::<16>(entropy).map_err(|_| {
                        iptv_store_sqlite::StoreError::Unavailable("probe identity unavailable")
                    })?);
                    (url, None, incarnation, TimePolicy::UTC, true)
                }
            };
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
        })
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
    task: crate::execution::Join<InspectionResult<T>>,
    cancel: CancelToken,
    guard: CallGuard,
) -> impl Future<Output = InspectionResult<T>> + use<T> {
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

/// Submit blocking work only for reserved capacity. Rejected requests finish
/// on the async executor and never enter its blocking queue.
fn admitted_worker<T: Send + 'static, Permit: Send + 'static>(
    services: &Services,
    workers: &Arc<Workers>,
    admission: Option<Permit>,
    operation: impl FnOnce() -> InspectionResult<T> + Send + 'static,
) -> crate::execution::Join<InspectionResult<T>> {
    match workers.enter() {
        Some(completion) => {
            let work = WorkerOperation {
                operation,
                _permit: admission,
                _completion: completion,
            };
            // Probe workers block on their downloads: a host whose tasks
            // cannot block has no capacity for them.
            if work._permit.is_some() && services.has_workers() {
                services.spawn_blocking(move || work.run())
            } else {
                drop(work);
                services.spawn(async { Err(InspectionError::Busy) })
            }
        }
        None => {
            drop(operation);
            drop(admission);
            services.spawn(async { Err(InspectionError::OwnerClosed) })
        }
    }
}

impl Runtime {
    /// Number of admitted probe/read workers whose actual blocking work has not exited.
    #[cfg(feature = "test-support")]
    pub fn active_inspection_workers(&self) -> usize {
        self.inspection
            .workers
            .state
            .lock()
            .expect("probe worker state poisoned")
            .active
    }

    /// Hold a probe after its encrypted partial stage has been created.
    #[cfg(feature = "test-support")]
    pub fn hold_probe_stage(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        *self.inspection.stage_hook.lock().unwrap() = Some(gate.0.clone());
        gate
    }

    /// Observe teardown waiting for actual blocking probe workers to finish.
    #[cfg(feature = "test-support")]
    pub fn observe_probe_drain(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        *self.inspection.workers.draining.lock().unwrap() = Some(gate.0.clone());
        gate
    }

    /// Fetches and parses a guide into bounded, disposable private staging.
    pub fn probe_feed(
        &self,
        request: ProbeRequest,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<ProbeSummary>> + use<> {
        // Staged probes need blocking workers: a host without them refuses
        // before any clock is read or store work is queued.
        let admitted = self
            .services()
            .has_workers()
            .then(|| self.probe_feed_admitted(request, control));
        async move {
            match admitted {
                Some(admitted) => admitted.await,
                None => Err(InspectionError::Busy),
            }
        }
    }

    fn probe_feed_admitted(
        &self,
        request: ProbeRequest,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<ProbeSummary>> + use<> {
        let source = request.source;
        let admission = Running::acquire(self.inspection.running.clone());
        let config = admission.as_ref().map(|_| configuration(self, request));
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
        let deadline = Instant::now() + control.timeout.min(PROBE_DEADLINE);
        let accepted = services.clone();

        let mut worker_control = control.clone();
        worker_control.cancel = CancelToken::new();
        let operation_cancel = worker_control.cancel.clone();
        let guard = self.inspection.register(operation_cancel.clone());
        let cancel = operation_cancel.clone();
        #[cfg(feature = "test-support")]
        let stage_hook = self.inspection.stage_hook.clone();
        let task = admitted_worker(&services, &self.inspection.workers, admission, move || {
            let config = config.ok_or(InspectionError::Busy)?;
            let key = key.ok_or(InspectionError::Storage)?;
            let reservation = Reservation::acquire(stages.clone())?;
            check(&accepted, &cancel, deadline)?;
            let configuration = accepted
                .try_block_on(guarded(&accepted, &worker_control, deadline, config))
                .ok_or(InspectionError::Storage)??;
            if configuration.url.expose().len() > 4096 {
                return Err(InspectionError::InvalidRequest);
            }
            let id = ProbeId(hex(&random::<16>(accepted.ports().entropy)?));
            let directory = {
                let mut directory = directory.lock().unwrap_or_else(|e| e.into_inner());
                // A timed-out worker may resume after shutdown or a restart.
                // Fence initialization under the same lock clear() uses.
                check(&accepted, &cancel, deadline)?;
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
            #[cfg(feature = "test-support")]
            {
                let hook = stage_hook.lock().unwrap().take();
                if let Some(hook) = hook {
                    hook.pause();
                }
            }
            let mut net = configuration.net;
            net.max_redirects = 3;
            net.request_timeout = PROBE_DEADLINE;
            net.max_streamed_bytes = PROBE_BYTES;
            net.allow_files = false;
            net.read_timeout = Duration::from_secs(15);
            let client = accepted
                .ports()
                .network
                .client(&net)
                .map_err(network_error)?;
            let mut request = HttpRequest::get(configuration.url.expose());
            request.public_only = configuration.public_only;
            request.preserve_gzip = true;
            request.timeout = Some(deadline.saturating_duration_since(Instant::now()));
            if let Some(agent) = configuration.user_agent {
                request = request.with_header("User-Agent", agent);
            }
            let mut response = accepted
                .try_block_on(guarded(&accepted, &worker_control, deadline, async {
                    client.fetch_stream(request).await.map_err(network_error)
                }))
                .ok_or(InspectionError::Storage)??;
            if !response.is_success() {
                return Err(InspectionError::Network);
            }
            let mut parser = StrictXmltvParser::new(parse_options(configuration.time));
            let mut hash = Sha256::new();
            let mut channels = 0u64;
            let mut error = None;
            loop {
                check(&accepted, &cancel, deadline)?;
                let chunk = accepted
                    .try_block_on(guarded(&accepted, &worker_control, deadline, async {
                        response.body.next_chunk().await.map_err(network_error)
                    }))
                    .ok_or(InspectionError::Storage)??;
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
                            if error.is_some() {
                                return;
                            }
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
                    if error.is_some() {
                        return;
                    }
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
                programs: stats.programs,
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
                if store.source(source)?.is_none() || !store.source_policy(source)?.enabled {
                    return Ok(false);
                }
                Ok(store.source_incarnation(source)? == current_source
                    && match current_feed {
                        Some(feed) => {
                            store.feed_incarnation(source, feed)?.as_ref()
                                == Some(&current_incarnation)
                        }
                        None => true,
                    })
            });
            let current = accepted
                .try_block_on(current)
                .ok_or(InspectionError::Storage)?
                .map_err(|_| InspectionError::Storage)?;
            if !current {
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
        await_worker(services, control, deadline, task, operation_cancel, guard)
    }

    /// Pins the exact retained snapshot for the supplied evidence.
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
        let lease = ProbeLease::new(stage.clone());
        lease.check()?;
        Ok(lease)
    }

    /// Queries a bounded page of guide channels from the exact retained snapshot.
    pub fn probe_candidates(
        &self,
        evidence: ProbeEvidence,
        text: String,
        after: Option<Cursor>,
        limit: u32,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<Candidate>>> + use<> {
        // Staged probes need blocking workers: a host without them refuses
        // before any clock is read or store work is queued.
        let admitted = self
            .services()
            .has_workers()
            .then(|| self.probe_candidates_admitted(evidence, text, after, limit, control));
        async move {
            match admitted {
                Some(admitted) => admitted.await,
                None => Err(InspectionError::Busy),
            }
        }
    }

    fn probe_candidates_admitted(
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
        let guard = self.inspection.register(operation_cancel.clone());
        let workers = &self.inspection.workers;
        let admission = self.inspection.admit();
        let task = admitted_worker(&services, workers, admission, move || {
            let lease = lease?;
            let leased = lease.snapshot();
            lease.current_configuration(&worker_control.cancel, deadline)?;
            check(&leased.services, &worker_control.cancel, deadline)?;
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
            let mut stage = storage(InspectionStage::open(&leased.path, &leased.key))?;
            let mut channels = storage(bounded(
                &mut stage,
                worker_control.cancel.clone(),
                deadline,
                |stage| {
                    stage.channels(
                        after.as_ref().map(|c| c.after[0].as_str()).unwrap_or(""),
                        &text,
                        limit + 1,
                    )
                },
            ))?;
            check(&leased.services, &worker_control.cancel, deadline)?;
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
        await_worker(services, control, deadline, task, operation_cancel, guard)
    }

    /// Samples programs for a candidate in the exact retained snapshot.
    pub fn probe_schedule(
        &self,
        evidence: ProbeEvidence,
        candidate: CandidateRef,
        window: TimeWindow,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Vec<CandidateProgram>>> + use<> {
        // Staged probes need blocking workers: a host without them refuses
        // before any clock is read or store work is queued.
        let admitted = self
            .services()
            .has_workers()
            .then(|| self.probe_schedule_admitted(evidence, candidate, window, control));
        async move {
            match admitted {
                Some(admitted) => admitted.await,
                None => Err(InspectionError::Busy),
            }
        }
    }

    fn probe_schedule_admitted(
        &self,
        evidence: ProbeEvidence,
        candidate: CandidateRef,
        window: TimeWindow,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Vec<CandidateProgram>>> + use<> {
        let lease = self.lease_probe(&evidence);
        let services = self.services();
        let deadline = Instant::now() + control.timeout.min(PROBE_DEADLINE);
        let mut worker_control = control.clone();
        worker_control.cancel = CancelToken::new();
        let operation_cancel = worker_control.cancel.clone();
        let guard = self.inspection.register(operation_cancel.clone());
        let workers = &self.inspection.workers;
        let admission = self.inspection.admit();
        let task = admitted_worker(&services, workers, admission, move || {
            if !window.valid() {
                return Err(InspectionError::InvalidRequest);
            }
            let lease = lease?;
            let leased = lease.snapshot();
            lease.current_configuration(&worker_control.cancel, deadline)?;
            check(&leased.services, &worker_control.cancel, deadline)?;
            lease.validate_candidates_with_control(
                std::slice::from_ref(&candidate),
                &worker_control.cancel,
                deadline,
            )?;
            let stage = storage(InspectionStage::open(&leased.path, &leased.key))?;
            let mut hash = Sha256::new();
            // The probe parser already applied this feed's configured shift.
            // Reparse with the same policy, captured independently of current config.
            let mut parser = StrictXmltvParser::new(parse_options(leased.time));
            let mut raw = BTreeMap::<i64, iptv_epg::XmltvProgram>::new();
            let mut overflow = false;
            let mut failure = None;
            let mut sink = |item| {
                if overflow {
                    return;
                }
                let XmltvItem::Program(program) = item else {
                    return;
                };
                if program.channel != candidate.epg_channel
                    || program.stop.is_some_and(|stop| stop <= program.start)
                {
                    return;
                }
                // Retain one look-ahead event to infer the final visible stop.
                if program.start >= window.to {
                    if raw
                        .range(window.to..)
                        .next()
                        .is_some_and(|(&start, _)| start <= program.start)
                    {
                        return;
                    }
                    if let Some((&start, _)) = raw.range(window.to..).next() {
                        raw.remove(&start);
                    }
                }
                // The latest predecessor is also boundary evidence when it
                // ended before the window: it cuts off any earlier no-stop row.
                // Keep just that predecessor even when the feed is out of order.
                if program.start < window.from {
                    if raw
                        .range(..window.from)
                        .next_back()
                        .is_some_and(|(&start, _)| start > program.start)
                    {
                        return;
                    }
                    if let Some((&start, _)) = raw.range(..window.from).next_back()
                        && start < program.start
                    {
                        raw.remove(&start);
                    }
                }
                match raw.entry(program.start) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(program);
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if iptv_epg::ScheduleRules::replaces(program.stop, entry.get().stop) {
                            entry.insert(program);
                        }
                    }
                }
                if raw.len() > MAX_PAGE as usize + 2 {
                    overflow = true;
                }
            };
            storage(stage.for_each_chunk(|bytes| {
                if let Err(error) = check(&leased.services, &worker_control.cancel, deadline) {
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
            check(&leased.services, &worker_control.cancel, deadline)?;
            if overflow {
                return Err(InspectionError::LimitExceeded);
            }
            let raw: Vec<_> = raw.into_values().collect();
            let intervals: Vec<_> = raw.iter().map(|p| (p.start, p.stop)).collect();
            let mut programs = Vec::new();
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
                        programs.push(CandidateProgram {
                            candidate: candidate.clone(),
                            source_incarnation: leased.source_incarnation.clone(),
                            evidence_revision: hex(&evidence.digest),
                            provider_event_id: None,
                            start: p.start,
                            stop,
                            title: p.title.clone(),
                        });
                    }
                },
            );
            if programs.len() > MAX_PAGE as usize {
                return Err(InspectionError::LimitExceeded);
            }
            Ok(programs)
        });
        await_worker(services, control, deadline, task, operation_cancel, guard)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod worker_tests {
    use super::*;

    struct Cleanup(crate::TestGate);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.0.pause();
        }
    }

    #[test]
    fn completion_follows_capture_cleanup_for_run_panic_and_queued_drop() {
        for mode in 0..3 {
            let vault = Vault::new(crate::native::entropy);
            vault.renew();
            let gate = crate::TestGate::new();
            let cleanup = Cleanup(crate::TestGate(gate.0.clone()));
            let work = WorkerOperation {
                operation: move || {
                    let _cleanup = cleanup;
                    assert_ne!(mode, 1, "synthetic worker panic");
                },
                _permit: Running::acquire(vault.running.clone()).unwrap(),
                _completion: vault.workers.enter().unwrap(),
            };
            let worker = std::thread::spawn(move || {
                if mode == 2 {
                    drop(work);
                } else {
                    work.run();
                }
            });
            assert!(gate.wait_entered(Duration::from_secs(5)));
            vault.close_workers();
            assert!(vault.workers.enter().is_none());
            assert!(!vault.drain_workers(Duration::ZERO));
            assert_eq!(vault.running.load(Ordering::SeqCst), 1);
            gate.release();
            assert_eq!(worker.join().is_err(), mode == 1);
            assert!(vault.drain_workers(Duration::ZERO));
            assert_eq!(vault.running.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn timed_out_shutdown_keeps_old_workers_tracked_across_restart() {
        let owner = Runtime::default();
        owner.start_services();
        let old = owner.services();
        let gate = crate::TestGate::new();
        let worker_gate = gate.0.clone();
        let accepted = old.clone();
        let task = admitted_worker(
            &old,
            &owner.inspection.workers,
            Running::acquire(owner.inspection.running.clone()),
            move || {
                worker_gate.pause();
                check(
                    &accepted,
                    &CancelToken::new(),
                    Instant::now() + PROBE_DEADLINE,
                )
            },
        );
        assert!(gate.wait_entered(Duration::from_secs(5)));
        owner.store.hooks.set_drain_limit(Some(Duration::ZERO));
        assert!(!owner.drain_services());
        let refused = admitted_worker(
            &old,
            &owner.inspection.workers,
            Running::acquire(owner.inspection.running.clone()),
            || -> InspectionResult<()> { panic!("closed workers must refuse late work") },
        );
        assert_eq!(
            old.block_on(refused).unwrap(),
            Err(InspectionError::OwnerClosed)
        );
        assert_eq!(owner.inspection.running.load(Ordering::SeqCst), 1);
        owner.start_services();
        assert!(!owner.inspection.drain_workers(Duration::ZERO));
        gate.release();
        assert_eq!(
            old.block_on(task).unwrap(),
            Err(InspectionError::OwnerClosed)
        );
        owner.store.hooks.set_drain_limit(None);
        assert!(owner.drain_services());
    }
}

#[cfg(all(test, feature = "test-support", feature = "native"))]
mod host_tests {
    use super::*;

    struct Unpolled;
    impl crate::execution::CooperativeHost for Unpolled {
        fn spawn(&self, _: crate::execution::Task) {}
        fn delay(&self, _: Duration) -> crate::execution::Task {
            Box::pin(std::future::pending())
        }
        fn now(&self) -> Duration {
            Duration::ZERO
        }
    }

    #[test]
    fn a_host_without_workers_refuses_a_probe_before_queueing_anything() {
        let directory = tempfile::tempdir().unwrap();
        let ports = crate::Ports::with_adapters(
            directory.path().to_owned(),
            |_| Ok(DatabaseKey::from_bytes([0x75; 32])),
            Arc::new(crate::native::network::NativeNetwork),
            Arc::new(crate::execution::Cooperative::new(Arc::new(Unpolled))),
            crate::native::entropy,
        );
        let owner = Runtime::new(ports);
        owner.start_services();
        let refused = owner.probe_feed(
            ProbeRequest {
                source: iptv_library::SourceId(1),
                target: ProbeTarget::Configured(FeedId(1)),
            },
            InspectionControl::default(),
        );
        let mut refused = std::pin::pin!(refused);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            refused.as_mut().poll(&mut context),
            std::task::Poll::Ready(Err(InspectionError::Busy))
        ));
        assert!(
            owner
                .store_queue_stats()
                .iter()
                .all(|lane| lane.pending == 0)
        );
        assert_eq!(owner.active_inspection_workers(), 0);
    }
}
