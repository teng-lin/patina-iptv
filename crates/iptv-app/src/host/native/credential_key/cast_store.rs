//! Persistent AirPlay pairings stay in the Linux session's Secret Service.
use super::linux::Entry;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iptv_cast::{Credential, CredentialKey, CredentialStore, StoreError};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

pub(in crate::host::native) struct CastStore {
    directory: PathBuf,
    label: String,
    lease: tokio::sync::OnceCell<Lease>,
}

struct Lease {
    namespace: String,
    lock: Arc<File>,
}

impl Lease {
    fn open(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join(".airplay-pairing-lock"))?;
        lock.try_lock()?;
        Ok(Self {
            namespace: URL_SAFE_NO_PAD.encode(directory.as_os_str().as_encoded_bytes()),
            lock: Arc::new(lock),
        })
    }
}

impl CastStore {
    pub(in crate::host::native) fn new(directory: &Path, label: String) -> Self {
        Self {
            directory: directory.to_owned(),
            label,
            lease: tokio::sync::OnceCell::new(),
        }
    }

    async fn run<T: Send + 'static>(
        &self,
        key: &str,
        work: impl FnOnce(Entry) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let lease = self
            .lease
            .get_or_try_init(|| async {
                let directory = self.directory.clone();
                tokio::task::spawn_blocking(move || Lease::open(&directory))
                    .await
                    .map_err(StoreError::new)?
                    .map_err(StoreError::new)
            })
            .await?;
        let account = format!("{}:{}", lease.namespace, key);
        let lock = lease.lock.clone();
        let label = self.label.clone();
        tokio::task::spawn_blocking(move || {
            // A cancelled waiter can detach from an OS call. Hold the lease
            // until the blocking operation actually returns.
            let _lease = lock;
            work(Entry::new("org.iptv-rs.airplay", &account, label).map_err(StoreError::new)?)
        })
        .await
        .map_err(StoreError::new)?
    }

    async fn load_record(&self, key: &str) -> Result<Option<Credential>, StoreError> {
        self.run(key, |entry| match entry.get_secret() {
            Ok(secret) => {
                let secret = Zeroizing::new(secret);
                let bytes = URL_SAFE_NO_PAD
                    .decode(secret.as_slice())
                    .map_err(|_| StoreError::invalid_credential())?;
                Ok(Some(Credential::from_bytes(bytes)))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(keyring::Error::Ambiguous(_)) => Err(StoreError::invalid_credential()),
            Err(error) => Err(StoreError::new(error)),
        })
        .await
    }

    async fn save_record(&self, key: &str, credential: Credential) -> Result<(), StoreError> {
        self.run(key, move |entry| {
            let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(credential.as_bytes()));
            entry
                .set_secret(encoded.as_bytes())
                .map_err(StoreError::new)
        })
        .await
    }

    async fn remove_record(&self, key: &str) -> Result<(), StoreError> {
        self.run(key, |entry| entry.delete_secret().map_err(StoreError::new))
            .await
    }
}

impl CredentialStore for CastStore {
    async fn load(&self, key: &CredentialKey) -> Result<Option<Credential>, StoreError> {
        self.load_record(key.storage_key()).await
    }

    async fn save(&self, key: &CredentialKey, credential: Credential) -> Result<(), StoreError> {
        self.save_record(key.storage_key(), credential).await
    }

    async fn remove(&self, key: &CredentialKey) -> Result<(), StoreError> {
        self.remove_record(key.storage_key()).await
    }
}

#[cfg(test)]
pub(super) fn verify_disposable_pairing_store() {
    assert_eq!(
        std::env::var("IPTV_TEST_ISOLATED_BROKER").as_deref(),
        Ok("1")
    );
    let profile = tempfile::tempdir().unwrap();
    let tokio = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    tokio.block_on(async {
        let key = "airplay:stable:generated-receiver";
        let open = || CastStore::new(profile.path(), "Generated pairing".to_owned());
        let first = open();
        assert!(first.load_record(key).await.unwrap().is_none());
        let reopened = open();
        assert!(reopened.load_record(key).await.is_err());
        first
            .save_record(key, Credential::from_bytes(vec![0, 255, 8]))
            .await
            .unwrap();
        drop(first);
        assert_eq!(
            reopened.load_record(key).await.unwrap().unwrap().as_bytes(),
            [0, 255, 8]
        );
        reopened
            .save_record(key, Credential::from_bytes(vec![1, 128]))
            .await
            .unwrap();
        assert_eq!(
            reopened.load_record(key).await.unwrap().unwrap().as_bytes(),
            [1, 128]
        );
        reopened.remove_record(key).await.unwrap();
        reopened.remove_record(key).await.unwrap();
        assert!(reopened.load_record(key).await.unwrap().is_none());
        reopened
            .run(key, |entry| {
                entry
                    .set_secret(b"generated%bad-encoding")
                    .map_err(StoreError::new)
            })
            .await
            .unwrap();
        assert!(reopened.load_record(key).await.is_err());
        reopened.remove_record(key).await.unwrap();
        assert!(reopened.load_record(key).await.unwrap().is_none());
        reopened
            .run(key, |entry| {
                entry.set_secret(b"AQ").map_err(StoreError::new)?;
                entry.add_session_duplicate(b"Ag").map_err(StoreError::new)
            })
            .await
            .unwrap();
        assert!(reopened.load_record(key).await.is_err());
        reopened.remove_record(key).await.unwrap();
        assert!(reopened.load_record(key).await.unwrap().is_none());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_client_owns_each_directory_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let store = Lease::open(dir.path()).unwrap();
        assert!(Lease::open(&dir.path().join(".")).is_err());
        let second = Lease::open(&dir.path().join("another-profile")).unwrap();
        assert_ne!(store.namespace, second.namespace);
        drop(store);
        assert!(Lease::open(dir.path()).is_ok());
    }
}
