use std::{
    collections::HashMap,
    fmt,
    future::{Future, pending},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    sync::{Mutex as AsyncMutex, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{
    BoxFuture, Connection, Delivery, Error, ErrorKind, PairDriver, PeerTrust, Stage,
    service::Runtime,
};

const PAIR_TIMEOUT: Duration = Duration::from_secs(30);

/// An opaque, collision-safe credential namespace key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CredentialKey(String);

impl CredentialKey {
    pub(super) fn new(storage_key: String) -> Self {
        Self(storage_key)
    }

    pub fn storage_key(&self) -> &str {
        &self.0
    }
}

/// Opaque backend-owned credential material. Debug output is redacted.
#[derive(Clone)]
pub struct Credential(Vec<u8>);

impl Credential {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Credential(..)")
    }
}

#[derive(Debug)]
pub struct StoreError {
    _private: (),
}

impl StoreError {
    /// Map a storage failure without retaining a potentially sensitive source.
    pub fn new(_source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self { _private: () }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("credential store operation failed")
    }
}

impl std::error::Error for StoreError {}

/// A host-owned store. Keep each operation bounded, including platform
/// prompts, and replace a record atomically. Forget waits for a committing
/// write before removing the pairing.
pub trait CredentialStore: Send + Sync {
    fn load(
        &self,
        key: &CredentialKey,
    ) -> impl Future<Output = Result<Option<Credential>, StoreError>> + Send;

    /// Atomically write the verified bytes for this key.
    fn save(
        &self,
        key: &CredentialKey,
        value: Credential,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Remove the record. An absent record is success.
    fn remove(&self, key: &CredentialKey) -> impl Future<Output = Result<(), StoreError>> + Send;
}

trait ErasedStore: Send + Sync {
    fn load<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> BoxFuture<'a, Result<Option<Credential>, StoreError>>;
    fn save<'a>(
        &'a self,
        key: &'a CredentialKey,
        value: Credential,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
    fn remove<'a>(&'a self, key: &'a CredentialKey) -> BoxFuture<'a, Result<(), StoreError>>;
}

impl<S: CredentialStore> ErasedStore for S {
    fn load<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> BoxFuture<'a, Result<Option<Credential>, StoreError>> {
        Box::pin(CredentialStore::load(self, key))
    }

    fn save<'a>(
        &'a self,
        key: &'a CredentialKey,
        value: Credential,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(CredentialStore::save(self, key, value))
    }

    fn remove<'a>(&'a self, key: &'a CredentialKey) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(CredentialStore::remove(self, key))
    }
}

#[derive(Default)]
pub(super) struct MemoryStore(Mutex<HashMap<CredentialKey, Credential>>);

impl CredentialStore for MemoryStore {
    async fn load(&self, key: &CredentialKey) -> Result<Option<Credential>, StoreError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    async fn save(&self, key: &CredentialKey, value: Credential) -> Result<(), StoreError> {
        self.0.lock().unwrap().insert(key.clone(), value);
        Ok(())
    }

    async fn remove(&self, key: &CredentialKey) -> Result<(), StoreError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct CredentialBook {
    store: Arc<dyn ErasedStore>,
    records: Arc<Mutex<HashMap<CredentialKey, Weak<Record>>>>,
}

impl CredentialBook {
    pub(super) fn new(store: impl CredentialStore + 'static) -> Self {
        Self {
            store: Arc::new(store),
            records: Arc::default(),
        }
    }

    fn record(&self, key: &CredentialKey) -> Arc<Record> {
        let mut records = self.records.lock().unwrap();
        records.retain(|_, record| record.strong_count() != 0);
        if let Some(record) = records.get(key).and_then(Weak::upgrade) {
            return record;
        }
        let record = Arc::new(Record::default());
        records.insert(key.clone(), Arc::downgrade(&record));
        record
    }

    pub(super) fn begin_pair(&self, key: CredentialKey) -> Result<Arc<Attempt>, Error> {
        let record = self.record(&key);
        let mut state = record.state.lock().unwrap();
        if state.attempt.strong_count() != 0 || state.forgetting != 0 {
            return Err(Error::new(ErrorKind::Busy, Stage::Pair, Delivery::NotSent));
        }
        let attempt = Arc::new(Attempt {
            key,
            record: record.clone(),
            cancelled: CancellationToken::new(),
            has_credential: AtomicBool::new(false),
        });
        state.attempt = Arc::downgrade(&attempt);
        Ok(attempt)
    }

    pub(super) fn begin_forget(&self, key: &CredentialKey) -> Forget {
        let record = self.record(key);
        {
            let mut state = record.state.lock().unwrap();
            state.forgetting += 1;
            if let Some(attempt) = state.attempt.upgrade() {
                attempt.cancelled.cancel();
            }
        }
        Forget(record)
    }

    pub(super) async fn load(&self, attempt: &Attempt) -> Result<Option<Credential>, Error> {
        let _guard = attempt.record.io.lock().await;
        let credential = self
            .store
            .load(&attempt.key)
            .await
            .map_err(|_| storage_error())?;
        attempt
            .has_credential
            .store(credential.is_some(), Ordering::Relaxed);
        Ok(credential)
    }

    async fn save(
        &self,
        attempt: &Attempt,
        credential: Credential,
        cancelled: &CancellationToken,
    ) -> Result<(), Error> {
        let _guard = attempt.record.io.lock().await;
        // Once the save begins, await its result. Forget takes the same I/O
        // lock and therefore removes anything committed by this attempt.
        if attempt.cancelled.is_cancelled() || cancelled.is_cancelled() {
            return Err(pair_error(ErrorKind::Cancelled, Delivery::Acknowledged));
        }
        self.store
            .save(&attempt.key, credential)
            .await
            .map_err(|_| storage_error())
    }

    pub(super) async fn forget(&self, key: CredentialKey, forgetting: Forget) -> Result<(), Error> {
        let _guard = forgetting.0.io.lock().await;
        self.store.remove(&key).await.map_err(|_| storage_error())
    }
}

fn storage_error() -> Error {
    Error::new(ErrorKind::Storage, Stage::Store, Delivery::NotSent)
}

#[derive(Default)]
struct Record {
    state: Mutex<RecordState>,
    io: AsyncMutex<()>,
}

#[derive(Default)]
struct RecordState {
    attempt: Weak<Attempt>,
    forgetting: usize,
}

pub(super) struct Forget(Arc<Record>);

impl Drop for Forget {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().forgetting -= 1;
    }
}

pub(super) struct Attempt {
    key: CredentialKey,
    record: Arc<Record>,
    cancelled: CancellationToken,
    has_credential: AtomicBool,
}

impl Attempt {
    pub(super) fn has_credential(&self) -> bool {
        self.has_credential.load(Ordering::Relaxed)
    }
}

pub(super) async fn attempt_cancelled(attempt: Option<&Arc<Attempt>>) {
    match attempt {
        Some(attempt) => attempt.cancelled.cancelled().await,
        None => pending().await,
    }
}

pub(super) fn attempt_is_cancelled(attempt: Option<&Arc<Attempt>>) -> bool {
    attempt.is_some_and(|attempt| attempt.cancelled.is_cancelled())
}

/// An ASCII PIN that preserves all digits, including leading zeroes.
pub struct PinCode(String);

impl PinCode {
    pub fn parse(digits: &str) -> Result<Self, Error> {
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(pair_error(ErrorKind::InvalidPin, Delivery::NotSent));
        }
        Ok(Self(digits.to_owned()))
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "AirPlay integration follows this contract")
    )]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PinCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PinCode(..)")
    }
}

/// One pending, receiver-approved PIN pairing attempt.
pub struct Pairing {
    expires_at: Option<Instant>,
    pin: Option<oneshot::Sender<PinCode>>,
    cancelled: CancellationToken,
    worker: Option<JoinHandle<Result<Connection, Error>>>,
}

impl Pairing {
    pub(super) fn start(
        driver: Box<dyn PairDriver>,
        runtime: Runtime,
        credentials: CredentialBook,
        attempt: Option<Arc<Attempt>>,
    ) -> Self {
        let expires_at = driver.expires_at();
        let (pin, receive) = oneshot::channel();
        let cancelled = CancellationToken::new();
        let worker = runtime.handle.clone().spawn(run_pairing(
            driver,
            runtime,
            credentials,
            attempt,
            receive,
            expires_at,
            cancelled.clone(),
        ));
        Self {
            expires_at,
            pin: Some(pin),
            cancelled,
            worker: Some(worker),
        }
    }

    pub fn expires_at(&self) -> Option<Instant> {
        self.expires_at
    }

    pub async fn submit(mut self, pin: PinCode) -> Result<Connection, Error> {
        // If the worker already retired, join it to recover the specific error.
        let sent = self.pin.take().unwrap().send(pin).is_ok();
        self.worker.take().unwrap().await.map_err(|_| {
            let delivery = if sent {
                Delivery::MayHaveReachedReceiver
            } else {
                Delivery::NotSent
            };
            pair_error(ErrorKind::Unavailable, delivery)
        })?
    }

    /// Retire this attempt. A save that has already committed can leave a
    /// verified credential in the store. Use `Cast::forget` to remove it.
    pub async fn cancel(mut self) {
        self.cancelled.cancel();
        let _ = self.worker.take().unwrap().await;
    }
}

impl Drop for Pairing {
    fn drop(&mut self) {
        self.cancelled.cancel();
    }
}

async fn run_pairing(
    mut driver: Box<dyn PairDriver>,
    runtime: Runtime,
    credentials: CredentialBook,
    attempt: Option<Arc<Attempt>>,
    receive: oneshot::Receiver<PinCode>,
    expires_at: Option<Instant>,
    cancelled: CancellationToken,
) -> Result<Connection, Error> {
    let cancellation = async {
        tokio::select! {
            _ = cancelled.cancelled() => {}
            _ = attempt_cancelled(attempt.as_ref()) => {}
        }
    };
    let expiration = async {
        match expires_at {
            Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
            None => pending().await,
        }
    };
    tokio::pin!(cancellation, expiration);
    let pin = tokio::select! {
        biased;
        _ = &mut cancellation => Err(pair_error(ErrorKind::Cancelled, Delivery::NotSent)),
        _ = &mut expiration => Err(pair_error(ErrorKind::Expired, Delivery::NotSent)),
        pin = receive => pin.map_err(|_| pair_error(ErrorKind::Cancelled, Delivery::NotSent)),
    };
    let pin = match pin {
        Ok(pin) => pin,
        Err(error) => {
            driver.cancel().await;
            return Err(error);
        }
    };
    let result = tokio::select! {
        biased;
        _ = &mut cancellation => Err(pair_error(ErrorKind::Cancelled, Delivery::MayHaveReachedReceiver)),
        _ = &mut expiration => Err(pair_error(ErrorKind::Expired, Delivery::MayHaveReachedReceiver)),
        result = tokio::time::timeout(PAIR_TIMEOUT, driver.submit(pin)) => {
            result.unwrap_or_else(|_| Err(pair_error(ErrorKind::TimedOut, Delivery::MayHaveReachedReceiver)))
        }
    };
    let (mut connection, credential) = match result {
        Ok(verified) => verified,
        Err(error) => {
            driver.cancel().await;
            return Err(error);
        }
    };
    let save = if connection.peer_trust() != PeerTrust::VerifiedPairing {
        Err(pair_error(
            ErrorKind::IdentityMismatch,
            Delivery::Acknowledged,
        ))
    } else if let Some(attempt) = &attempt {
        credentials.save(attempt, credential, &cancelled).await
    } else {
        Ok(())
    };
    if let Err(error) = save {
        connection.close().await;
        return Err(error);
    }
    if cancelled.is_cancelled()
        || attempt
            .as_ref()
            .is_some_and(|attempt| attempt.cancelled.is_cancelled())
    {
        connection.close().await;
        return Err(pair_error(ErrorKind::Cancelled, Delivery::Acknowledged));
    }
    Ok(Connection::new(connection, runtime))
}

fn pair_error(kind: ErrorKind, delivery: Delivery) -> Error {
    Error::new(kind, Stage::Pair, delivery)
}
