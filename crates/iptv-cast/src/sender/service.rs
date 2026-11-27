use std::{
    any::Any,
    fmt,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use tokio::{runtime::Handle, time::Instant};
use tokio_util::task::AbortOnDropHandle;

use super::{
    Availability, Backend, BackendConnect, Connection, CredentialKey, CredentialStore, Delivery,
    Error, ErrorKind, Feature, Features, Pairing, PeerTrust, Stage,
    credentials::{CredentialBook, Forget, MemoryStore},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Stable identifier for the backend of a route.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProtocolId(&'static str);

impl ProtocolId {
    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn new(value: &'static str) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        self.0
    }
}

/// An opaque, protocol-qualified route identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RouteId(String);

impl RouteId {
    /// An opaque key for callers that represent a route selection as text.
    /// Its format is private. Store credential bytes using `CredentialKey`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
pub struct Route {
    id: RouteId,
    protocol: ProtocolId,
    candidate: Candidate,
    owner: Arc<()>,
}

impl fmt::Debug for Route {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Route")
            .field("id", &self.id)
            .field("protocol", &self.protocol)
            .finish_non_exhaustive()
    }
}

impl Route {
    pub fn id(&self) -> &RouteId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.candidate.name
    }

    pub fn protocol(&self) -> &ProtocolId {
        &self.protocol
    }

    pub fn availability(&self, feature: Feature) -> Availability {
        self.candidate.hints.availability(feature)
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn payload<T: Any>(&self) -> Option<&T> {
        self.candidate.payload.downcast_ref::<T>()
    }

    fn credential_key(&self) -> Option<CredentialKey> {
        self.candidate
            .stable_id
            .as_ref()
            .map(|_| CredentialKey::new(self.id.0.clone()))
    }
}

#[derive(Clone)]
pub(crate) struct Candidate {
    name: String,
    stable_id: Option<String>,
    payload: Arc<dyn Any + Send + Sync>,
    hints: Features,
}

impl Candidate {
    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn new(
        name: String,
        stable_id: Option<String>,
        payload: impl Any + Send + Sync,
        hints: Features,
    ) -> Self {
        Self {
            name,
            stable_id: stable_id.filter(|identity| !identity.is_empty()),
            payload: Arc::new(payload),
            hints,
        }
    }

    fn into_route(self, protocol: ProtocolId, owner: Arc<()>) -> Route {
        static NEXT_EPHEMERAL: AtomicU64 = AtomicU64::new(0);
        let identity = match &self.stable_id {
            Some(identity) => format!("{}:stable:{}", protocol.as_str(), identity),
            None => format!(
                "{}:ephemeral:{}",
                protocol.as_str(),
                NEXT_EPHEMERAL.fetch_add(1, Ordering::Relaxed)
            ),
        };
        Route {
            id: RouteId(identity),
            protocol,
            candidate: self,
            owner,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ScanSink(Arc<Mutex<Option<Vec<Candidate>>>>);

impl ScanSink {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Some(Vec::new()))))
    }

    #[cfg_attr(
        not(any(test, feature = "googlecast")),
        expect(dead_code, reason = "requires a backend")
    )]
    pub(crate) fn push(&self, candidate: Candidate) {
        let mut guard = self.0.lock().unwrap();
        let Some(routes) = guard.as_mut() else { return };
        if let Some(stable_id) = &candidate.stable_id
            && let Some(existing) = routes
                .iter_mut()
                .find(|entry| entry.stable_id.as_ref() == Some(stable_id))
        {
            *existing = candidate;
        } else {
            routes.push(candidate);
        }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "backend integration follows this contract")
    )]
    pub(crate) fn remove(&self, stable_id: &str) {
        if let Some(routes) = self.0.lock().unwrap().as_mut() {
            routes.retain(|candidate| candidate.stable_id.as_deref() != Some(stable_id));
        }
    }

    fn finish(&self) -> Vec<Candidate> {
        self.0.lock().unwrap().take().unwrap_or_default()
    }
}

struct ScanWindows(Vec<(ProtocolId, ScanSink)>);

impl Drop for ScanWindows {
    fn drop(&mut self) {
        for (_, sink) in &self.0 {
            sink.finish();
        }
    }
}

#[derive(Debug)]
pub struct Discovery {
    routes: Vec<Route>,
    issues: Vec<DiscoveryIssue>,
}

impl Discovery {
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    pub fn issues(&self) -> &[DiscoveryIssue] {
        &self.issues
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveryIssue {
    protocol: ProtocolId,
    error: Error,
}

impl DiscoveryIssue {
    pub fn protocol(&self) -> &ProtocolId {
        &self.protocol
    }

    pub fn error(&self) -> &Error {
        &self.error
    }
}

#[non_exhaustive]
pub enum Connect {
    Ready(Connection),
    PinRequired(Pairing),
}

pub struct CastBuilder {
    credentials: CredentialBook,
}

impl CastBuilder {
    pub fn credential_store(mut self, store: impl CredentialStore + 'static) -> Self {
        self.credentials = CredentialBook::new(store);
        self
    }

    /// Create a sender while entered in a live Tokio runtime with I/O and
    /// time enabled. Until a production backend is registered, this returns
    /// an unavailable configuration error.
    pub fn build(self) -> Result<Cast, Error> {
        let backends: Vec<Arc<dyn Backend>> = vec![
            #[cfg(feature = "googlecast")]
            Arc::new(crate::googlecast::adapter::GoogleCast::default()),
        ];
        self.build_with(backends)
    }

    pub(crate) fn build_with(self, backends: Vec<Arc<dyn Backend>>) -> Result<Cast, Error> {
        let runtime = Handle::try_current().map_err(|_| {
            Error::new(
                ErrorKind::Unavailable,
                Stage::Configuration,
                Delivery::NotSent,
            )
        })?;
        if backends.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                Stage::Configuration,
                Delivery::NotSent,
            ));
        }
        let runtime = Runtime::capture(runtime);
        let service = Cast {
            inner: Arc::new(CastInner {
                backends,
                owner: Arc::new(()),
                credentials: self.credentials,
                runtime,
            }),
        };
        Ok(service)
    }
}

#[derive(Clone)]
pub struct Cast {
    inner: Arc<CastInner>,
}

struct CastInner {
    backends: Vec<Arc<dyn Backend>>,
    owner: Arc<()>,
    credentials: CredentialBook,
    runtime: Runtime,
}

struct RuntimeState {
    lifetime: Weak<()>,
    shutdown: tokio_util::sync::CancellationToken,
}

impl Drop for RuntimeState {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[derive(Clone)]
pub(crate) struct Runtime {
    pub(crate) handle: Handle,
    state: Arc<RuntimeState>,
}

impl Runtime {
    fn capture(handle: Handle) -> Self {
        let lifetime = Arc::new(());
        let shutdown = tokio_util::sync::CancellationToken::new();
        let state = Arc::new(RuntimeState {
            lifetime: Arc::downgrade(&lifetime),
            shutdown: shutdown.clone(),
        });
        // The runtime owns the strong marker, including when the task is
        // retired without ever being polled.
        handle.spawn(async move {
            let _lifetime = lifetime;
            shutdown.cancelled().await;
        });
        Self { handle, state }
    }

    pub(crate) fn check(&self, stage: Stage) -> Result<(), Error> {
        if self.state.lifetime.strong_count() == 0 {
            return Err(Error::new(ErrorKind::Unavailable, stage, Delivery::NotSent));
        }
        Ok(())
    }
}

impl Cast {
    pub fn builder() -> CastBuilder {
        CastBuilder {
            credentials: CredentialBook::new(MemoryStore::default()),
        }
    }

    pub async fn discover(&self, window: Duration) -> Result<Discovery, Error> {
        self.inner.runtime.check(Stage::Discovery)?;
        let now = self.inner.runtime.handle.enter();
        let deadline = Instant::now()
            .checked_add(window)
            .filter(|_| !window.is_zero());
        drop(now);
        let deadline = deadline.ok_or_else(|| discovery_error(ErrorKind::InvalidInput))?;
        let inner = self.inner.clone();
        let scan = self
            .inner
            .runtime
            .handle
            .spawn(async move { inner.scan(deadline).await });
        AbortOnDropHandle::new(scan)
            .await
            .map_err(|_| discovery_error(ErrorKind::Unavailable))?
    }

    pub async fn connect(&self, route: &Route) -> Result<Connect, Error> {
        self.verify_owner(route, Stage::Connect)?;
        self.inner.runtime.check(Stage::Connect)?;
        let inner = self.inner.clone();
        let route = route.clone();
        let started = Arc::new(AtomicBool::new(false));
        let worker_started = started.clone();
        let connect = self
            .inner
            .runtime
            .handle
            .spawn(async move { inner.connect(route, &worker_started).await });
        AbortOnDropHandle::new(connect)
            .await
            .map_err(|_| connect_interrupted(ErrorKind::Unavailable, &started))?
    }

    pub async fn forget(&self, route: &Route) -> Result<(), Error> {
        self.verify_owner(route, Stage::Store)?;
        if !self
            .inner
            .backend(route)
            .is_some_and(|backend| backend.uses_credentials())
        {
            return Ok(());
        }
        let Some(key) = route.credential_key() else {
            return Ok(());
        };
        self.inner.runtime.check(Stage::Store)?;
        // Once initiated, finish a forget even if its caller stops waiting.
        let credentials = self.inner.credentials.clone();
        let forgetting: Forget = credentials.begin_forget(&key);
        self.inner
            .runtime
            .handle
            .spawn(async move { credentials.forget(key, forgetting).await })
            .await
            .map_err(|_| Error::new(ErrorKind::Unavailable, Stage::Store, Delivery::NotSent))?
    }

    fn verify_owner(&self, route: &Route, stage: Stage) -> Result<(), Error> {
        if Arc::ptr_eq(&self.inner.owner, &route.owner) {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::InvalidInput,
                stage,
                Delivery::NotSent,
            ))
        }
    }
}

impl CastInner {
    fn backend(&self, route: &Route) -> Option<&dyn Backend> {
        self.backends
            .iter()
            .find(|backend| backend.protocol() == route.protocol)
            .map(Arc::as_ref)
    }

    async fn scan(&self, deadline: Instant) -> Result<Discovery, Error> {
        let windows = ScanWindows(
            self.backends
                .iter()
                .map(|backend| (backend.protocol(), ScanSink::new()))
                .collect(),
        );
        let mut scans = FuturesUnordered::new();
        for (i, backend) in self.backends.iter().enumerate() {
            let scan = backend.discover(deadline, windows.0[i].1.clone());
            scans.push(async move { (i, scan.await) });
        }
        let mut statuses: Vec<_> = (0..self.backends.len()).map(|_| None).collect();
        let timeout = tokio::time::sleep_until(deadline);
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                biased;
                result = scans.next() => match result {
                    Some((i, status)) => statuses[i] = Some(status),
                    None => break,
                },
                _ = &mut timeout => break,
            }
        }
        drop(scans);
        let mut routes = Vec::new();
        let mut issues = Vec::new();
        for ((protocol, sink), status) in windows.0.iter().zip(statuses) {
            for candidate in sink.finish() {
                routes.push(candidate.into_route(protocol.clone(), self.owner.clone()));
            }
            let status = status.unwrap_or_else(|| Err(discovery_error(ErrorKind::TimedOut)));
            if let Err(error) = status {
                issues.push(DiscoveryIssue {
                    protocol: protocol.clone(),
                    error,
                });
            }
        }
        if issues.len() == self.backends.len() && routes.is_empty() {
            return Err(Error::discovery(issues));
        }
        Ok(Discovery { routes, issues })
    }

    async fn connect(&self, route: Route, started: &AtomicBool) -> Result<Connect, Error> {
        let backend = self
            .backend(&route)
            .ok_or_else(|| connect_error(ErrorKind::Unsupported))?;
        let attempt = route
            .credential_key()
            .filter(|_| backend.uses_credentials())
            .map(|key| self.credentials.begin_pair(key))
            .transpose()?;
        let interrupted = |kind| connect_interrupted(kind, started);
        let connect = async {
            let credential = if let Some(attempt) = &attempt {
                self.credentials.load(attempt).await?
            } else {
                None
            };
            started.store(true, Ordering::Relaxed);
            let result = tokio::time::timeout(CONNECT_TIMEOUT, backend.connect(&route, credential))
                .await
                .map_err(|_| interrupted(ErrorKind::TimedOut))??;
            Ok::<_, Error>(result)
        };
        let result = tokio::select! {
            biased;
            _ = super::credentials::attempt_cancelled(attempt.as_ref()) => {
                return Err(interrupted(ErrorKind::Cancelled));
            }
            result = connect => result?,
        };
        if super::credentials::attempt_is_cancelled(attempt.as_ref()) {
            match result {
                BackendConnect::Ready(mut driver) => driver.close().await,
                BackendConnect::PinRequired(mut driver) => driver.cancel().await,
            }
            return Err(interrupted(ErrorKind::Cancelled));
        }
        let needs_verification = attempt
            .as_ref()
            .is_some_and(|attempt| attempt.has_credential());
        match result {
            BackendConnect::Ready(mut driver)
                if needs_verification && driver.peer_trust() != PeerTrust::VerifiedPairing =>
            {
                driver.close().await;
                Err(connect_error(ErrorKind::IdentityMismatch))
            }
            BackendConnect::Ready(driver) => Ok(Connect::Ready(Connection::new(
                driver,
                self.runtime.clone(),
            ))),
            BackendConnect::PinRequired(mut driver) if needs_verification => {
                driver.cancel().await;
                Err(connect_error(ErrorKind::IdentityMismatch))
            }
            BackendConnect::PinRequired(driver) => Ok(Connect::PinRequired(Pairing::start(
                driver,
                self.runtime.clone(),
                self.credentials.clone(),
                attempt,
            ))),
        }
    }
}

fn discovery_error(kind: ErrorKind) -> Error {
    Error::new(kind, Stage::Discovery, Delivery::NotSent)
}

fn connect_error(kind: ErrorKind) -> Error {
    Error::new(kind, Stage::Connect, Delivery::NotSent)
}

fn connect_interrupted(kind: ErrorKind, started: &AtomicBool) -> Error {
    let delivery = if started.load(Ordering::Relaxed) {
        Delivery::MayHaveReachedReceiver
    } else {
        Delivery::NotSent
    };
    Error::new(kind, Stage::Connect, delivery)
}
