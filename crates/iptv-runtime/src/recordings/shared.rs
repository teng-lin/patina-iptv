//! Owned incoming streams shared across native playback and recording. Every
//! committed provider owner remains registered and budgeted until its upstream
//! shutdown is acknowledged, independently of UI/capture lease removal.
use super::*;
use iptv_core::{i18n::key, live::Identity};
use iptv_net::live::{LiveInput, Validated};
use iptv_net::recording::Failure;
use std::time::Instant;

/// A stream: source, its incarnation, channel. Not the provider URL, which
/// may carry a token that changes on every resolve.
type Stream = (i64, String, String);
/// Streams remembered at most; the oldest is forgotten first.
const UNSHAREABLE: usize = 256;
/// Inputs whose first segment is still watched at most.
const WATCHED: usize = 64;
/// How long a stream stays on its provider's own transport.
const UNSHAREABLE_FOR: Duration = Duration::from_secs(30 * 60);
/// Validation failures that send a stream to its provider's own transport:
/// one segment over the limit, or two that failed their checks (a single
/// torn final packet may be a connection cut short).
const STRIKES: u8 = 2;

/// Streams whose first HLS segment failed validation after a shared input
/// had offered its first packets (`LiveInput::first_segment`). Such an input
/// ends; once a stream is pinned, its next preparation plays the provider's
/// own transport, as one refused before its first packets does at once.
#[derive(Default)]
pub(super) struct Unshareable {
    failures: HashMap<Stream, (u8, Instant)>,
    /// Shared inputs whose first segment is watched: where each stands
    /// (`LiveInput::validation`).
    watching: Vec<(Stream, Box<dyn Fn() -> Validated + Send>)>,
}
impl Unshareable {
    fn watch(&mut self, stream: Stream, validation: impl Fn() -> Validated + Send + 'static) {
        // Decided and abandoned inputs first; only then the oldest pending.
        self.settle();
        if self.watching.len() >= WATCHED {
            drop(self.watching.remove(0));
        }
        self.watching.push((stream, Box::new(validation)));
    }
    /// Records the watched inputs' outcomes. An input's outcome is set
    /// before its player sees it end, so the player's retry finds it here.
    /// A stream whose first segment validated starts its strikes afresh.
    fn settle(&mut self) {
        for (stream, validation) in std::mem::take(&mut self.watching) {
            match validation() {
                Validated::Pending => self.watching.push((stream, validation)),
                Validated::Abandoned => {}
                Validated::Settled(Ok(()), _) => {
                    self.failures.remove(&stream);
                }
                Validated::Settled(Err(failure), at) => self.fail(stream, failure, at),
            }
        }
    }
    /// `stream`'s first segment failed with `failure` at `at`.
    fn fail(&mut self, stream: Stream, failure: Failure, at: Instant) {
        let strikes = match failure {
            Failure::Limit => STRIKES,
            Failure::Unsupported => 1,
            _ => return,
        };
        if !self.failures.contains_key(&stream) && self.failures.len() >= UNSHAREABLE {
            let oldest = self
                .failures
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(stream, _)| stream.clone());
            if let Some(oldest) = oldest {
                self.failures.remove(&oldest);
            }
        }
        let entry = self.failures.entry(stream).or_insert((0, at));
        if at.saturating_duration_since(entry.1) >= UNSHAREABLE_FOR {
            entry.0 = 0;
        }
        *entry = (entry.0.saturating_add(strikes), entry.1.max(at));
    }
    fn pinned(&self, stream: &Stream, now: Instant) -> bool {
        self.failures.get(stream).is_some_and(|(strikes, at)| {
            *strikes >= STRIKES && now.saturating_duration_since(*at) < UNSHAREABLE_FOR
        })
    }
    /// The source changed (incarnation or configuration): start afresh.
    fn forget_source(&mut self, source: i64) {
        self.failures.retain(|(id, _, _), _| *id != source);
        self.watching.retain(|((id, _, _), _)| *id != source);
    }
}
pub(super) struct Incoming {
    pub id: u64,
    pub identity: Identity,
    pub config: SourceConfig,
    pub source: PlaySource,
    pub policy: PlaybackPolicy,
    pub input: Transport,
}
#[derive(Clone)]
pub(super) enum Transport {
    Shared(LiveInput),
    Passthrough(Arc<Passthrough>),
}
pub(super) struct Passthrough {
    url: String,
    relay: Mutex<Option<iptv_net::playback::PlaybackRelay>>,
    closing: AtomicBool,
    closed: AtomicBool,
    changed: tokio::sync::Notify,
    executor: tokio::runtime::Handle,
}
impl Transport {
    fn passthrough(
        source: &PlaySource,
        policy: PlaybackPolicy,
    ) -> Result<Self, iptv_net::FetchError> {
        let relay = iptv_net::playback::PlaybackRelay::start_with_policy(
            &source.url,
            &source.headers,
            policy,
        )?;
        Ok(Self::Passthrough(Arc::new(Passthrough {
            url: relay.url().to_owned(),
            relay: Mutex::new(Some(relay)),
            closing: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            changed: tokio::sync::Notify::new(),
            executor: tokio::runtime::Handle::current(),
        })))
    }
    fn url(&self) -> &str {
        match self {
            Self::Shared(input) => input.url(),
            Self::Passthrough(input) => &input.url,
        }
    }
    pub(super) fn shared(&self) -> Option<&LiveInput> {
        match self {
            Self::Shared(input) => Some(input),
            Self::Passthrough(_) => None,
        }
    }
    fn is_ready(&self) -> bool {
        match self {
            Self::Shared(input) => input.is_ready(),
            Self::Passthrough(_) => true,
        }
    }
    pub(super) fn is_running(&self) -> bool {
        match self {
            Self::Shared(input) => input.is_running(),
            Self::Passthrough(input) => !input.closing.load(Ordering::Acquire),
        }
    }
    async fn shutdown(self) {
        match self {
            Self::Shared(input) => input.shutdown().await,
            Self::Passthrough(input) => {
                let relay = input
                    .relay
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(relay) = relay {
                    input.closing.store(true, Ordering::Release);
                    let closing = input.clone();
                    // Closure remains owned even if this retirement waiter is
                    // canceled by UI teardown or another replacement.
                    input.executor.spawn(async move {
                        relay.shutdown().await;
                        closing.closed.store(true, Ordering::Release);
                        closing.changed.notify_waiters();
                    });
                }
                loop {
                    let changed = input.changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if input.closed.load(Ordering::Acquire) {
                        break;
                    }
                    changed.await;
                }
            }
        }
    }
}
fn register_transport(
    state: &State,
    id: u64,
    identity: &Identity,
    config: &SourceConfig,
    source: &PlaySource,
    policy: &PlaybackPolicy,
    input: Transport,
) -> Arc<Incoming> {
    let incoming = Arc::new(Incoming {
        id,
        identity: identity.clone(),
        config: config.clone(),
        source: source.clone(),
        policy: policy.clone(),
        input,
    });
    if let Some(registered) = state.lock().inputs.get_mut(&id) {
        registered.lease.input = Some(incoming.clone());
        registered.lease.pending = None;
    }
    state.inputs_changed.notify_waiters();
    incoming
}
pub(super) struct Registered {
    pub lease: Lease,
    pub closing: bool,
}
impl Incoming {
    fn local_source(&self) -> PlaySource {
        let mut source = PlaySource::new(self.input.url());
        source.public_only = false;
        source
    }
}
impl Data {
    fn retained(&self, id: u64) -> bool {
        self.viewers
            .values()
            .chain(self.suspended_viewers.values())
            .chain(self.active.values().map(|(lease, _)| lease))
            .any(|lease| lease.connection() == Some(id))
    }
    fn available(&self, id: u64) -> bool {
        self.inputs
            .get(&id)
            .is_some_and(|registered| !registered.closing)
    }
    pub(super) fn shared_for(&self, source: i64, channel: &str) -> Option<Arc<Incoming>> {
        self.shared_matching(source, channel, |_| true)
    }
    pub(super) fn shared_resolved(
        &self,
        identity: &Identity,
        config: &SourceConfig,
        source: &PlaySource,
        policy: &PlaybackPolicy,
    ) -> Option<Arc<Incoming>> {
        self.shared_matching(identity.source, &identity.channel, |incoming| {
            incoming.identity == *identity
                && incoming.config == *config
                && incoming.source == *source
                && incoming.policy == *policy
        })
    }
    fn shared_matching(
        &self,
        source: i64,
        channel: &str,
        matches: impl Fn(&Incoming) -> bool,
    ) -> Option<Arc<Incoming>> {
        let (incarnation, config) = self.scopes.get(&source)?;
        self.viewers
            .values()
            .chain(self.suspended_viewers.values())
            .chain(self.active.values().map(|(lease, _)| lease))
            .filter_map(|lease| lease.input.as_ref())
            .find(|incoming| {
                incoming.identity.source == source
                    && incoming.identity.channel == channel
                    && &incoming.identity.incarnation == incarnation
                    && &incoming.config == config
                    && self.available(incoming.id)
                    && incoming.input.shared().is_some()
                    && incoming.input.is_ready()
                    && incoming.input.is_running()
                    && matches(incoming)
            })
            .cloned()
    }
    pub(super) fn recording_capacity(&self, request: &Request) -> bool {
        let input = self.shared_for(request.source, &request.channel);
        self.capacity_with(request.source, None, input.as_ref().map(|input| input.id))
    }
}
impl State {
    #[cfg(feature = "test-support")]
    pub(super) fn original_player_source(&self, url: &str) -> Option<PlaySource> {
        self.lock()
            .inputs
            .values()
            .filter_map(|registered| registered.lease.input.as_ref())
            .find(|incoming| incoming.input.url() == url)
            .map(|incoming| incoming.source.clone())
    }

    fn preparation_gate(&self, surface: usize) -> Arc<tokio::sync::Mutex<()>> {
        self.preparations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(surface)
            .or_insert_with(|| (CancelToken::new(), Arc::default()))
            .1
            .clone()
    }
    pub(super) fn detach_capture(&self, id: JobId) {
        if let Some((lease, _)) = self.lock().active.get_mut(&id) {
            lease.input = None;
        }
    }
    /// Registry entries remain counted while closure is in progress. Concurrent
    /// callers may join the same idempotent closure; no new consumer can attach
    /// after its closing fence is set under the accounting lock.
    pub(super) async fn retire_unused(&self) {
        loop {
            let notified = self.inputs_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let next = {
                let mut data = self.lock();
                let capturing: Vec<u64> = data
                    .active
                    .values()
                    .filter_map(|(lease, _)| lease.connection())
                    .collect();
                for lease in data.suspended_viewers.values_mut() {
                    lease.pending = None;
                    if lease
                        .input
                        .as_ref()
                        .is_some_and(|input| !capturing.contains(&input.id))
                    {
                        lease.input = None;
                    }
                }
                let id = data.inputs.keys().copied().find(|id| !data.retained(*id));
                id.map(|id| {
                    let registered = data.inputs.get_mut(&id).expect("registered input");
                    registered.closing = true;
                    registered.lease.input.clone()
                })
            };
            match next {
                None => break,
                Some(None) => notified.await,
                Some(Some(incoming)) => {
                    incoming.input.clone().shutdown().await;
                    self.lock().inputs.remove(&incoming.id);
                    self.inputs_changed.notify_waiters();
                }
            }
        }
    }
    pub(super) fn cancel_preparation(&self, surface: usize) {
        if let Some((cancel, _)) = self
            .preparations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&surface)
        {
            cancel.cancel();
        }
    }
    pub(super) fn cancel_all_preparations(&self) {
        for (cancel, _) in self
            .preparations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            cancel.cancel();
        }
    }
}
impl Runtime {
    pub(super) fn retire_previous_viewing(&self, surface: usize) {
        let state = self.recordings.clone();
        let gate = state.preparation_gate(surface);
        self.services().spawn(async move {
            let _gate = gate.lock().await;
            state.retire_unused().await;
        });
    }
    /// Join superseded incoming owners before an independent native source
    /// opens, including VOD/catch-up following a shared live stream.
    pub fn retire_viewing_inputs(
        &self,
        surface: usize,
    ) -> impl std::future::Future<Output = ()> + use<> {
        let state = self.recordings.clone();
        let gate = state.preparation_gate(surface);
        let services = self.services();
        #[cfg(feature = "test-support")]
        let pause = services.take_source_preparation();
        async move {
            let work = services.clone();
            let _ = services
                .spawn(async move {
                    let _gate = gate.lock().await;
                    #[cfg(feature = "test-support")]
                    if let Some(pause) = pause {
                        let _ = work.spawn_blocking(move || pause.pause()).await;
                    }
                    #[cfg(not(feature = "test-support"))]
                    let _ = work;
                    state.retire_unused().await;
                })
                .await;
        }
    }
    /// A returning viewer can retain the incoming owner still held by capture.
    /// A different channel needs its own permitted connection.
    pub fn reserve_live_viewing(&self, surface: usize, source: SourceId, channel: &str) -> bool {
        if self.recordings_detached.get() {
            return false;
        }
        let mut data = self.recordings.lock();
        let input = data.shared_for(source.0, channel);
        if !data.active.is_empty()
            && !data.capacity_with(
                source.0,
                Some(surface),
                input.as_ref().map(|input| input.id),
            )
        {
            return false;
        }
        self.recordings.cancel_preparation(surface);
        let mut lease = data.lease(source.0);
        lease.input = input;
        data.suspended_viewers.remove(&surface);
        *data.viewer_revision.entry(surface).or_default() += 1;
        data.viewers.insert(surface, lease);
        drop(data);
        self.retire_previous_viewing(surface);
        true
    }
    pub fn owns_viewing_source(&self, surface: usize, source: &PlaySource) -> bool {
        self.recordings
            .lock()
            .viewers
            .get(&surface)
            .and_then(|lease| lease.input.as_ref())
            .is_some_and(|input| input.input.url() == source.url)
    }
    pub fn prepare_live_source(
        &self,
        surface: usize,
        source: SourceId,
        channel: String,
        stream: PlaySource,
        authority: iptv_guide::playback::ResolutionAuthority,
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        self.prepare_provider_source(surface, source, channel, stream, true, Some(authority))
    }
    /// Own HTTP VOD/catch-up/native-fallback transport without selecting HLS
    /// variants or reading ahead. Local recordings have no provider reservation.
    pub fn prepare_independent_source(
        &self,
        surface: usize,
        provider: Option<(SourceId, String)>,
        source: PlaySource,
        authority: Option<iptv_guide::playback::ResolutionAuthority>,
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        let retired = self.retire_viewing_inputs(surface);
        let prepared = provider.map(|(provider, key)| {
            self.prepare_provider_source(surface, provider, key, source.clone(), false, authority)
        });
        async move {
            retired.await;
            match prepared {
                Some(prepared) => prepared.await,
                None => Ok(source),
            }
        }
    }
    /// Prepare live transport on the recording host's owned I/O executor before
    /// publishing a native source. Original provider leases cover preparations
    /// as well as committed inputs, through acknowledged cancellation/closure.
    fn prepare_provider_source(
        &self,
        surface: usize,
        source_id: SourceId,
        channel: String,
        mut source: PlaySource,
        share: bool,
        expected: Option<iptv_guide::playback::ResolutionAuthority>,
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        let disabled = self.recordings_detached.get()
            || self.shutdown_requested.get()
            || self.recordings.is_stopping();
        if !disabled {
            self.start_recordings();
        }
        #[cfg(feature = "test-support")]
        let pause = share
            .then(|| self.services().take_source_preparation())
            .flatten();
        let state = self.recordings.clone();
        let services = state
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|host| host.accept())
            .unwrap_or_else(|| self.services());
        let revision = state.lock().viewer_revision.get(&surface).copied();
        let cancel = CancelToken::new();
        let gate = {
            let mut slots = state
                .preparations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let slot = slots
                .entry(surface)
                .or_insert_with(|| (CancelToken::new(), Arc::default()));
            slot.0.cancel();
            slot.0 = cancel.clone();
            slot.1.clone()
        };
        async move {
            if disabled {
                return Err(Error::unavailable(key("Channel unavailable")));
            }
            let work = services.clone();
            let stopping = cancel.clone();
            let task = services.spawn(async move {
                #[cfg(test)]
                if let Some(arrived) = state.preparation_arrivals.lock().unwrap().remove(&surface) {
                    let _ = arrived.send(());
                }
                let _gate = gate.lock().await;
                #[cfg(feature = "test-support")]
                if let Some(pause) = pause {
                    let _ = work.spawn_blocking(move || pause.pause()).await;
                }
                let result = async {
                    state.retire_unused().await;
                    if stopping.is_canceled() || revision.is_none() {
                        return Err(Error::unavailable(key("Channel unavailable")));
                    }
                    let member = ContentId::from_stored(channel.clone());
                    let expected_member = member.clone();
                    let (incarnation, config, authority) = work
                        .read(move |store| {
                            if !store.source_policy(source_id)?.enabled {
                                return Err(iptv_store_sqlite::StoreError::Unavailable(key(
                                    "Channel unavailable",
                                )));
                            }
                            let authority = if share || expected.is_some() {
                                let stored = store.channel(source_id, &member)?.ok_or(
                                    iptv_store_sqlite::StoreError::Unavailable(key(
                                        "Channel unavailable",
                                    )),
                                )?;
                                Some((stored.channel, stored.blocked))
                            } else {
                                None
                            };
                            let stored = store
                                .source(source_id)?
                                .ok_or(iptv_store_sqlite::StoreError::SourceNotFound(source_id))?;
                            if expected.as_ref().is_some_and(|expected| {
                                expected.source() != source_id.0
                                    || expected.channel().id != member
                                    || expected.incarnation() != stored.incarnation
                                    || expected.config() != &stored.source.config
                                    || authority
                                        .as_ref()
                                        .is_none_or(|(channel, _)| channel != expected.channel())
                            }) {
                                return Err(iptv_store_sqlite::StoreError::Unavailable(key(
                                    "Channel unavailable",
                                )));
                            }
                            Ok((stored.incarnation, stored.source.config, authority))
                        })
                        .await?;
                    let identity = Identity {
                        source: source_id.0,
                        incarnation: incarnation.clone(),
                        channel,
                    };
                    let net = crate::settings::net_config(&work.settings());
                    source.default_header("User-Agent", &net.user_agent);
                    let policy = PlaybackPolicy {
                        public_only: source.public_only,
                        extra_roots_der: net.extra_roots_der,
                        #[cfg(debug_assertions)]
                        allow_private_fixture: net.allow_private_fixture,
                    };
                    {
                        let mut data = state.lock();
                        if data
                            .source_incarnations
                            .get(&source_id.0)
                            .is_some_and(|old| old != &incarnation)
                            || data.scopes.get(&source_id.0).is_some_and(|scope| {
                                scope != &(incarnation.clone(), config.clone())
                            })
                        {
                            data.provider_limits.remove(&source_id.0);
                            data.unshareable.forget_source(source_id.0);
                        }
                        data.source_incarnations
                            .insert(source_id.0, incarnation.clone());
                        let limit = connection_limit(
                            &work.settings(),
                            source_id.0,
                            data.provider_limits.get(&source_id.0).copied(),
                        );
                        data.limits.insert(source_id.0, limit);
                        data.accounts.insert(source_id.0, account(&config));
                        data.scopes
                            .insert(source_id.0, (incarnation.clone(), config.clone()));
                    }
                    let existing = {
                        let data = state.lock();
                        share
                            .then(|| data.shared_resolved(&identity, &config, &source, &policy))
                            .flatten()
                    };
                    let mut created = None;
                    let incoming = if let Some(existing) = existing {
                        Some(existing)
                    } else {
                        {
                            let mut data = state.lock();
                            if data.viewer_revision.get(&surface).copied() != revision
                                || stopping.is_canceled()
                            {
                                return Err(Error::unavailable(key("Channel unavailable")));
                            }
                            if let Some(lease) = data.viewers.get_mut(&surface) {
                                lease.input = None;
                                lease.pending = None;
                            }
                        }
                        state.retire_unused().await;
                        let id = state.next_input.fetch_add(1, Ordering::Relaxed) + 1;
                        {
                            let mut data = state.lock();
                            let mut wanted = data.lease(source_id.0);
                            wanted.account = account(&config);
                            if !data.capacity_for(&wanted, Some(surface), None) {
                                return Err(Error::unavailable(key(
                                    "No provider connection available; check Streams at once",
                                )));
                            }
                            if data.viewer_revision.get(&surface).copied() != revision
                                || stopping.is_canceled()
                            {
                                return Err(Error::unavailable(key("Channel unavailable")));
                            }
                            if !data.viewers.contains_key(&surface) {
                                return Err(Error::unavailable(key("Channel unavailable")));
                            }
                            let mut lease = data.lease(source_id.0);
                            lease.account = account(&config);
                            lease.pending = Some(id);
                            data.viewers.insert(surface, lease.clone());
                            data.inputs.insert(
                                id,
                                Registered {
                                    lease,
                                    closing: false,
                                },
                            );
                        }
                        created = Some(id);
                        let mut incoming = None;
                        let asked = std::time::Instant::now();
                        let stream = (
                            source_id.0,
                            identity.incarnation.clone(),
                            identity.channel.clone(),
                        );
                        let shareable = {
                            let mut data = state.lock();
                            data.unshareable.settle();
                            !data.unshareable.pinned(&stream, asked)
                        };
                        if share
                            && shareable
                            && let Ok(input) = LiveInput::begin_with_cancel(
                                &source.url,
                                &source.headers,
                                policy.clone(),
                                stopping.clone(),
                            )
                        {
                            let shared = register_transport(
                                &state,
                                id,
                                &identity,
                                &config,
                                &source,
                                &policy,
                                Transport::Shared(input),
                            );
                            if shared
                                .input
                                .shared()
                                .expect("shared probe")
                                .ready()
                                .await
                                .is_ok()
                            {
                                // The provider's first packets (a cold
                                // start's first stream; `startup.rs`).
                                work.mark(
                                    "live input ready",
                                    format_args!("{} ms", asked.elapsed().as_millis()),
                                );
                                // The player opens with the first packets; a
                                // first segment that then fails validation
                                // ends this input, and the player's retry
                                // may fall back to the provider's own
                                // transport (`Unshareable`).
                                let validation =
                                    shared.input.shared().expect("shared probe").validation();
                                state
                                    .lock()
                                    .unshareable
                                    .watch(stream, move || validation.state());
                                incoming = Some(shared);
                            } else {
                                shared.input.clone().shutdown().await;
                            }
                        }
                        if incoming.is_none()
                            && !stopping.is_canceled()
                            && let Ok(input) = Transport::passthrough(&source, policy.clone())
                        {
                            incoming = Some(register_transport(
                                &state, id, &identity, &config, &source, &policy, input,
                            ));
                        }
                        if incoming.is_none() {
                            // A failed/canceled actual owner stays registered until
                            // retire_unused joins it; a protocol with no owner can
                            // release its provisional reservation immediately.
                            let empty = state
                                .lock()
                                .inputs
                                .get(&id)
                                .is_some_and(|registered| registered.lease.input.is_none());
                            if empty {
                                state.lock().inputs.remove(&id);
                                state.inputs_changed.notify_waiters();
                            }
                        }
                        incoming
                    };
                    let valid = work
                        .read(move |store| {
                            Ok(store.source_policy(source_id)?.enabled
                                && store.source_incarnation(source_id)? == incarnation
                                && store
                                    .source(source_id)?
                                    .is_some_and(|stored| stored.source.config == config)
                                && authority.is_none_or(|expected| {
                                    store
                                        .channel(source_id, &expected_member)
                                        .ok()
                                        .flatten()
                                        .is_some_and(|stored| {
                                            (stored.channel, stored.blocked) == expected
                                        })
                                }))
                        })
                        .await
                        .unwrap_or(false);
                    let installed = {
                        let mut data = state.lock();
                        let available = incoming
                            .as_ref()
                            .is_none_or(|input| data.available(input.id));
                        let current = valid
                            && available
                            && !stopping.is_canceled()
                            && data.viewer_revision.get(&surface).copied() == revision;
                        match data.viewers.get_mut(&surface).filter(|lease| {
                            lease.source == source_id.0
                                && (created.is_none() || lease.pending == created)
                        }) {
                            Some(lease) if current => {
                                lease.input = incoming.clone();
                                lease.pending = None;
                                true
                            }
                            _ => false,
                        }
                    };
                    // Unsuccessful/stale preparations remain in the registry until
                    // this join, even when another surface now uses the old account.
                    if !installed || incoming.is_none() {
                        state.retire_unused().await;
                    }
                    if !installed {
                        return Err(Error::unavailable(key("Channel unavailable")));
                    }
                    Ok(incoming.map_or(source, |incoming| incoming.local_source()))
                }
                .await;
                if result.is_err() {
                    {
                        let mut data = state.lock();
                        // A failed preparation relinquishes only its own lease.
                        // A newer reservation and any capture retain ownership.
                        if data.viewer_revision.get(&surface).copied() == revision
                            && let Some(lease) = data.viewers.get_mut(&surface)
                        {
                            lease.input = None;
                            lease.pending = None;
                        }
                    }
                    state.retire_unused().await;
                }
                result
            });
            struct Cancel(CancelToken);
            impl Drop for Cancel {
                fn drop(&mut self) {
                    self.0.cancel();
                }
            }
            let _cancel = Cancel(cancel);
            task.await
                .map_err(|_| Error::unavailable(key("Channel unavailable")))?
        }
    }
}

#[cfg(test)]
mod unshareable_tests {
    use super::*;
    #[test]
    fn unshareable_streams_need_two_strikes_and_expire() {
        let stream = |channel: &str| (1, "incarnation".to_owned(), channel.to_owned());
        let at = Instant::now();
        let mut set = Unshareable::default();
        set.fail(stream("a"), Failure::Unsupported, at);
        assert!(
            !set.pinned(&stream("a"), at),
            "one torn segment is not enough"
        );
        set.fail(stream("a"), Failure::Unsupported, at);
        assert!(set.pinned(&stream("a"), at));
        assert!(
            !set.pinned(&stream("a"), at + UNSHAREABLE_FOR),
            "it expires"
        );
        set.fail(stream("a"), Failure::Unsupported, at + UNSHAREABLE_FOR);
        assert!(
            !set.pinned(&stream("a"), at + UNSHAREABLE_FOR),
            "an expired stream starts its strikes afresh"
        );
        set.fail(stream("b"), Failure::Limit, at);
        assert!(
            set.pinned(&stream("b"), at),
            "one oversized segment pins it"
        );
        set.fail(stream("c"), Failure::Network, at);
        set.fail(stream("c"), Failure::Network, at);
        assert!(!set.pinned(&stream("c"), at), "network failures never pin");
        set.forget_source(1);
        assert!(
            !set.pinned(&stream("b"), at),
            "a changed source starts afresh"
        );
        for n in 0..=UNSHAREABLE {
            set.fail(
                stream(&n.to_string()),
                Failure::Limit,
                at + Duration::from_secs(n as u64),
            );
        }
        assert_eq!(set.failures.len(), UNSHAREABLE, "bounded");
        assert!(
            !set.pinned(&stream("0"), at),
            "the oldest is forgotten first"
        );
        assert!(set.pinned(&stream("1"), at + Duration::from_secs(1)));
    }

    /// A watched input's state, as a test sets it.
    fn input(state: Validated) -> (Arc<Mutex<Validated>>, impl Fn() -> Validated + Send) {
        let shared = Arc::new(Mutex::new(state));
        let read = shared.clone();
        (shared, move || *read.lock().unwrap())
    }

    #[test]
    fn a_validated_first_segment_clears_earlier_strikes() {
        let stream = || (1, "incarnation".to_owned(), "a".to_owned());
        let at = Instant::now();
        let mut set = Unshareable::default();
        let (_, torn) = input(Validated::Settled(Err(Failure::Unsupported), at));
        set.watch(stream(), torn);
        let (_, healthy) = input(Validated::Settled(Ok(()), at));
        set.watch(stream(), healthy);
        set.settle();
        set.fail(stream(), Failure::Unsupported, at);
        assert!(
            !set.pinned(&stream(), at),
            "a strike before a good segment does not count"
        );
    }

    #[test]
    fn abandoned_inputs_leave_the_watch_and_never_evict_a_live_one() {
        let stream = |n: usize| (1, "incarnation".to_owned(), n.to_string());
        let at = Instant::now();
        let mut set = Unshareable::default();
        let (live, pending) = input(Validated::Pending);
        set.watch(stream(0), pending);
        for n in 1..WATCHED * 2 {
            let (_, abandoned) = input(Validated::Abandoned);
            set.watch(stream(n), abandoned);
        }
        set.settle();
        assert_eq!(set.watching.len(), 1, "only the live input is watched");
        *live.lock().unwrap() = Validated::Settled(Err(Failure::Limit), at);
        set.settle();
        assert!(set.pinned(&stream(0), at), "its later failure still counts");
    }

    #[test]
    fn a_failure_expires_from_when_it_happened() {
        let stream = || (1, "incarnation".to_owned(), "a".to_owned());
        let at = Instant::now();
        let mut set = Unshareable::default();
        let (_, oversized) = input(Validated::Settled(Err(Failure::Limit), at));
        set.watch(stream(), oversized);
        // Settled at the next preparation, two hours on.
        let later = at + Duration::from_secs(2 * 3600);
        set.settle();
        assert!(set.pinned(&stream(), at));
        assert!(
            !set.pinned(&stream(), later),
            "expired 30 minutes after the failure, not after it was read"
        );
        // An older failure read late does not move a newer one's time.
        set.fail(stream(), Failure::Limit, at + Duration::from_secs(60));
        set.fail(stream(), Failure::Unsupported, at);
        assert!(set.pinned(&stream(), at + UNSHAREABLE_FOR + Duration::from_secs(30)));
    }
}
