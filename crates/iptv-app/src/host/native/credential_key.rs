//! The database key never lives in application preferences or the database tree.
//! Blocking platform credential access runs exclusively on the store worker.
use std::path::Path;

use crate::runtime::{Error, model::DatabaseKey};
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
pub(crate) fn database_key(path: &Path) -> Result<DatabaseKey, Error> {
    #[cfg(feature = "test-support")]
    {
        let _ = path;
        TEST_KEY.get().cloned().ok_or(Error::unavailable(
            "synthetic database key was not installed",
        ))
    }
    #[cfg(not(feature = "test-support"))]
    {
        platform_key(path)
    }
}

#[cfg(all(target_os = "android", not(feature = "test-support")))]
fn platform_key(_: &Path) -> Result<DatabaseKey, Error> {
    android::database_key()
}

#[cfg(all(
    not(feature = "test-support"),
    any(target_os = "linux", target_os = "macos", target_os = "ios", windows)
))]
fn platform_key(path: &Path) -> Result<DatabaseKey, Error> {
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
        .map_err(|_| Error::unavailable("cannot lock the credential broker"))?;
    lock.lock()
        .map_err(|_| Error::unavailable("cannot lock the credential broker"))?;
    let entry = Entry::new("org.iptv-rs.database", "catalog-v1")
        .map_err(|_| Error::unavailable("platform credential broker could not be opened"))?;
    let secret = match entry.get_secret() {
        Ok(secret) => Zeroizing::new(secret),
        Err(keyring::Error::NoEntry) => {
            let mut header = [0; 16];
            match std::fs::File::open(path) {
                Ok(mut file) => {
                    let count = file
                        .read(&mut header)
                        .map_err(|_| Error::unavailable("cannot inspect existing database"))?;
                    if count != 0 && &header != b"SQLite format 3\0" {
                        return Err(Error::unavailable(
                            "database key is missing from the platform credential broker",
                        ));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Error::unavailable("cannot inspect existing database")),
            }
            let mut bytes = Zeroizing::new(vec![0; 32]);
            getrandom::fill(&mut bytes)
                .map_err(|_| Error::unavailable("secure random generator failed"))?;
            // UTF-8 encoding also supports KDE Wallet's Secret Service backend.
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let encoded = Zeroizing::new(
                bytes
                    .iter()
                    .flat_map(|byte| [HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]])
                    .collect::<Vec<_>>(),
            );
            entry.set_secret(&encoded).map_err(|_| {
                Error::unavailable("platform credential broker could not save the database key")
            })?;
            encoded
        }
        Err(_) => {
            return Err(Error::unavailable(
                "platform credential broker is locked or unavailable",
            ));
        }
    };
    let mut bytes = [0; 32];
    if secret.len() != bytes.len() * 2 {
        return Err(Error::unavailable(
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
            return Err(Error::unavailable(
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
fn platform_key(_: &Path) -> Result<DatabaseKey, Error> {
    Err(Error::unavailable(
        "no platform credential broker for this target",
    ))
}

#[cfg(all(test, target_os = "linux", not(feature = "test-support")))]
mod tests {
    use super::*;

    /// Drives a store reply on this thread; the store's worker wakes it.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        struct Unpark(std::thread::Thread);
        impl std::task::Wake for Unpark {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::sync::Arc::new(Unpark(std::thread::current())).into();
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::park();
        }
    }

    /// Run only in a new dbus-run-session with an isolated unlocked daemon.
    #[test]
    #[ignore = "requires an isolated Secret Service daemon"]
    fn isolated_secret_service_persists_key_and_reopens_catalog() {
        assert_eq!(
            std::env::var("IPTV_TEST_ISOLATED_BROKER").as_deref(),
            Ok("1")
        );
        if std::env::var_os("IPTV_TEST_BROKER_UNAVAILABLE_CHILD").is_some() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("iptv.sqlite");
            assert!(database_key(&path).is_err());
            assert!(
                !path.exists(),
                "broker failure must not create a plaintext catalog"
            );
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let owner = || {
            std::rc::Rc::new(crate::runtime::Runtime::new(crate::runtime::Ports::new(
                directory.path().to_owned(),
                database_key,
            )))
        };
        let first = owner();
        first.start_services();
        block_on(crate::runtime::commands::remember_combined_channel(
            &first,
            "synthetic-secret".into(),
        ))
        .unwrap();
        assert!(first.drain_services());
        drop(first);
        let second = owner();
        second.start_services();
        let saved = block_on(crate::runtime::commands::combined_resume_key(&second)).unwrap();
        assert_eq!(saved.as_deref(), Some("synthetic-secret"));
        assert!(second.drain_services());
        drop(second);
        let path = directory.path().join("iptv.sqlite");
        let encrypted = std::fs::read(&path).unwrap();
        assert!(
            !encrypted
                .windows(16)
                .any(|bytes| bytes == b"synthetic-secret")
        );

        let entry = linux::Entry::new("org.iptv-rs.database", "catalog-v1").unwrap();
        let original = zeroize::Zeroizing::new(entry.get_secret().unwrap());
        let assert_closed = || {
            let failed = owner();
            failed.start_services();
            assert!(block_on(crate::runtime::commands::combined_resume_key(&failed)).is_err());
            assert!(failed.drain_services());
            drop(failed);
            assert_eq!(std::fs::read(&path).unwrap(), encrypted);
        };
        // These replacements are synthetic and occur only in the disposable broker.
        entry.set_secret(&[b'0'; 64]).unwrap();
        assert_closed();
        entry.set_secret(b"invalid-key-encoding").unwrap();
        assert_closed();
        entry.delete_secret().unwrap();
        assert_closed();
        assert!(matches!(entry.get_secret(), Err(keyring::Error::NoEntry)));
        entry.set_secret(&original).unwrap();
        let restored = owner();
        restored.start_services();
        assert_eq!(
            block_on(crate::runtime::commands::combined_resume_key(&restored))
                .unwrap()
                .as_deref(),
            Some("synthetic-secret")
        );
        assert!(restored.drain_services());
        drop(restored);

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "host::native::credential_key::tests::isolated_secret_service_persists_key_and_reopens_catalog",
                "--ignored",
            ])
            .env("IPTV_TEST_BROKER_UNAVAILABLE_CHILD", "1")
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/absent-bus", directory.path().display()),
            )
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("missing-broker child exceeded its deadline");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(
            status.success(),
            "missing broker must fail closed in a fresh process"
        );
        // No unlock UI is available in the disposable session. The existing
        // catalog must stay intact when its broker collection is locked.
        entry.lock_collection().unwrap();
        assert_closed();
    }
}
