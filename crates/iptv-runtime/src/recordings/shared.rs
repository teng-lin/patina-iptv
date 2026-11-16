//! Owned incoming streams shared across native playback and recording. Every
//! committed provider owner remains registered and budgeted until its upstream
//! shutdown is acknowledged, independently of UI/capture lease removal.
use super::*;
use iptv_core::{i18n::key, live::Identity};
use iptv_net::live::LiveInput;
use std::sync::atomic::AtomicU64;

static NEXT_INPUT: AtomicU64 = AtomicU64::new(1);
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
                    // cancelled by UI teardown or another replacement.
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
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        self.prepare_provider_source(surface, source, channel, stream, true)
    }
    /// Own HTTP VOD/catch-up/native-fallback transport without selecting HLS
    /// variants or reading ahead. Local recordings have no provider reservation.
    pub fn prepare_independent_source(
        &self,
        surface: usize,
        provider: Option<(SourceId, String)>,
        source: PlaySource,
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        let retired = self.retire_viewing_inputs(surface);
        let prepared = provider.map(|(provider, key)| {
            self.prepare_provider_source(surface, provider, key, source.clone(), false)
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
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        let disabled = self.recordings_detached.get()
            || self.shutdown_requested.get()
            || self.recordings.is_stopping();
        if !disabled {
            self.start_recordings();
        }
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
                let _gate = gate.lock().await;
                let result = async {
                    state.retire_unused().await;
                    if stopping.is_cancelled() || revision.is_none() {
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
                            let authority = if share {
                                let stored = store.channel(source_id, &member)?.ok_or(
                                    iptv_store_sqlite::StoreError::Unavailable(key(
                                        "Channel unavailable",
                                    )),
                                )?;
                                Some((stored.channel, stored.blocked))
                            } else {
                                None
                            };
                            let config = store
                                .source(source_id)?
                                .ok_or(iptv_store_sqlite::StoreError::SourceNotFound(source_id))?
                                .source
                                .config;
                            Ok((store.source_incarnation(source_id)?, config, authority))
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
                            .then(|| data.shared_for(source_id.0, &identity.channel))
                            .flatten()
                            .filter(|incoming| {
                                incoming.identity == identity
                                    && incoming.config == config
                                    && incoming.source == source
                                    && incoming.policy == policy
                            })
                    };
                    let mut created = None;
                    let incoming = if let Some(existing) = existing {
                        Some(existing)
                    } else {
                        {
                            let mut data = state.lock();
                            if data.viewer_revision.get(&surface).copied() != revision
                                || stopping.is_cancelled()
                            {
                                return Err(Error::unavailable(key("Channel unavailable")));
                            }
                            if let Some(lease) = data.viewers.get_mut(&surface) {
                                lease.input = None;
                                lease.pending = None;
                            }
                        }
                        state.retire_unused().await;
                        let id = NEXT_INPUT.fetch_add(1, Ordering::Relaxed);
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
                                || stopping.is_cancelled()
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
                        if share
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
                                incoming = Some(shared);
                            } else {
                                shared.input.clone().shutdown().await;
                            }
                        }
                        if incoming.is_none()
                            && !stopping.is_cancelled()
                            && let Ok(input) = Transport::passthrough(&source, policy.clone())
                        {
                            incoming = Some(register_transport(
                                &state, id, &identity, &config, &source, &policy, input,
                            ));
                        }
                        if incoming.is_none() {
                            // A failed/cancelled actual owner stays registered until
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
                            && !stopping.is_cancelled()
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
