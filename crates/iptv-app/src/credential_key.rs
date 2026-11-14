//! The database key never lives in application preferences or the database tree.
//! Blocking platform credential access runs exclusively on the store worker.
use std::path::Path;

use crate::runtime::model::{DatabaseKey, StoreError};
#[cfg(all(target_os = "linux", not(feature = "test-support")))]
mod linux;
#[cfg(all(
    any(target_os = "macos", target_os = "ios", windows),
    not(feature = "test-support")
))]
use keyring::Entry;
#[cfg(all(target_os = "linux", not(feature = "test-support")))]
use linux::Entry;

#[cfg(all(target_os = "android", not(feature = "test-support")))]
mod android;

#[cfg(feature = "test-support")]
static TEST_KEY: std::sync::OnceLock<DatabaseKey> = std::sync::OnceLock::new();

/// Explicit synthetic key injection, absent from production builds.
#[cfg(feature = "test-support")]
pub fn install_test_key(bytes: [u8; 32]) {
    let _ = TEST_KEY.set(DatabaseKey::from_bytes(bytes));
}

/// Platform credential port. The runtime owns opening and migrating the store.
pub(crate) fn database_key(path: &Path) -> Result<DatabaseKey, StoreError> {
    #[cfg(feature = "test-support")]
    {
        let _ = path;
        TEST_KEY.get().cloned().ok_or(StoreError::Unavailable(
            "synthetic database key was not installed",
        ))
    }
    #[cfg(not(feature = "test-support"))]
    {
        platform_key(path)
    }
}

#[cfg(all(target_os = "android", not(feature = "test-support")))]
fn platform_key(_: &Path) -> Result<DatabaseKey, StoreError> {
    android::database_key()
}

#[cfg(all(
    not(feature = "test-support"),
    any(target_os = "linux", target_os = "macos", target_os = "ios", windows)
))]
fn platform_key(path: &Path) -> Result<DatabaseKey, StoreError> {
    use std::io::Read;
    use zeroize::Zeroizing;
    // Serialize first use across processes, before creating the keyring entry.
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".key-lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(Path::new(&lock_name))
        .map_err(|_| StoreError::Unavailable("cannot lock the credential broker"))?;
    lock.lock()
        .map_err(|_| StoreError::Unavailable("cannot lock the credential broker"))?;
    let entry = Entry::new("org.iptv-rs.database", "catalog-v1")
        .map_err(|_| StoreError::Unavailable("platform credential broker could not be opened"))?;
    let secret = match entry.get_secret() {
        Ok(secret) => Zeroizing::new(secret),
        Err(keyring::Error::NoEntry) => {
            let mut header = [0; 16];
            match std::fs::File::open(path) {
                Ok(mut file) => {
                    let count = file
                        .read(&mut header)
                        .map_err(|_| StoreError::Unavailable("cannot inspect existing database"))?;
                    if count != 0 && &header != b"SQLite format 3\0" {
                        return Err(StoreError::Unavailable(
                            "database key is missing from the platform credential broker",
                        ));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(StoreError::Unavailable("cannot inspect existing database")),
            }
            let mut bytes = Zeroizing::new(vec![0; 32]);
            getrandom::fill(&mut bytes)
                .map_err(|_| StoreError::Unavailable("secure random generator failed"))?;
            // UTF-8 encoding also supports KDE Wallet's Secret Service backend.
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let encoded = Zeroizing::new(
                bytes
                    .iter()
                    .flat_map(|byte| [HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]])
                    .collect::<Vec<_>>(),
            );
            entry.set_secret(&encoded).map_err(|_| {
                StoreError::Unavailable(
                    "platform credential broker could not save the database key",
                )
            })?;
            encoded
        }
        Err(_) => {
            return Err(StoreError::Unavailable(
                "platform credential broker is locked or unavailable",
            ));
        }
    };
    let mut bytes = [0; 32];
    if secret.len() != bytes.len() * 2 {
        return Err(StoreError::Unavailable(
            "platform database key has an invalid length",
        ));
    }
    for (byte, pair) in bytes.iter_mut().zip(secret.as_chunks::<2>().0.iter()) {
        let digit = |value: u8| match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            _ => None,
        };
        let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
            zeroize::Zeroize::zeroize(&mut bytes);
            return Err(StoreError::Unavailable(
                "platform database key has an invalid encoding",
            ));
        };
        *byte = (high << 4) | low;
    }
    let key = DatabaseKey::from_bytes(bytes);
    zeroize::Zeroize::zeroize(&mut bytes);
    Ok(key)
}

#[cfg(all(
    not(feature = "test-support"),
    not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        windows
    ))
))]
fn platform_key(_: &Path) -> Result<DatabaseKey, StoreError> {
    Err(StoreError::Unavailable(
        "no platform credential broker for this target",
    ))
}

#[cfg(all(test, target_os = "linux", not(feature = "test-support")))]
mod tests {
    use super::*;

    /// Run only in a new dbus-run-session with an isolated unlocked daemon.
    #[test]
    #[ignore = "requires an isolated Secret Service daemon"]
    fn isolated_secret_service_persists_key_and_reopens_catalog() {
        assert_eq!(
            std::env::var("IPTV_TEST_ISOLATED_BROKER").as_deref(),
            Ok("1")
        );
        let directory = tempfile::tempdir().unwrap();
        let owner = || {
            std::rc::Rc::new(crate::runtime::Runtime::new(crate::runtime::Ports::new(
                directory.path().to_owned(),
                database_key,
            )))
        };
        let first = owner();
        crate::with_runtime(first.clone(), || {
            first.start_services();
            crate::executor::block_on(crate::runtime::commands::remember_combined_channel(
                "synthetic-secret".into(),
            ))
            .unwrap();
            assert!(first.drain_services());
        });
        drop(first);
        let second = owner();
        crate::with_runtime(second.clone(), || {
            second.start_services();
            let saved =
                crate::executor::block_on(crate::runtime::commands::combined_resume_key()).unwrap();
            assert_eq!(saved.as_deref(), Some("synthetic-secret"));
            assert!(second.drain_services());
        });
        assert!(
            !std::fs::read(directory.path().join("iptv.sqlite"))
                .unwrap()
                .windows(16)
                .any(|bytes| bytes == b"synthetic-secret")
        );
    }
}
