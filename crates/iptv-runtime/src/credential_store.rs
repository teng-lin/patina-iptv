//! Encrypted library open, migration and restore validation using a key port.
use iptv_store_sqlite::{DatabaseKey, Store, StoreError};
use std::path::Path;

/// The owner's key broker answers for its own library file.
fn database_key(ports: &crate::Ports, path: &Path) -> Result<DatabaseKey, StoreError> {
    (ports.database_key)(path).map_err(Into::into)
}

pub(crate) fn open_database(ports: &crate::Ports, path: &Path) -> Result<Store, StoreError> {
    let key = database_key(ports, path)?;
    let store = Store::open_encrypted(path, &key)?;
    seal_documents(store, path)
}

#[cfg(all(test, feature = "native"))]
mod test_support_tests {
    #[test]
    fn absent_synthetic_key_never_reaches_the_platform_broker() {
        // This unit-test process never installs the integration harness key.
        let directory = tempfile::tempdir().unwrap();
        let result = super::open_database(
            &crate::Ports::for_tests(),
            &directory.path().join("synthetic.sqlite"),
        );
        assert!(matches!(
            result,
            Err(iptv_store_sqlite::StoreError::Unavailable(
                "synthetic database key was not installed"
            ))
        ));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

/// Gives a validated, encrypted pending restore this host's recording rows.
#[cfg(not(test))]
pub(crate) fn adopt_host_recordings(
    ports: &crate::Ports,
    host: &Path,
    pending: &Path,
) -> Result<(), StoreError> {
    let mut store = Store::open_encrypted(pending, &database_key(ports, pending)?)?;
    store.adopt_host_recordings_encrypted(host, &database_key(ports, host)?)
}

fn seal_documents(store: Store, path: &Path) -> Result<Store, StoreError> {
    super::private_files::prepare_store(store, path)
}

/// Revalidate an encrypted pending restore; strictly validate and seal legacy
/// plaintext pending content before it can become active.
#[cfg(not(test))]
pub(crate) fn validate_staged_database(
    ports: &crate::Ports,
    path: &Path,
) -> Result<(), StoreError> {
    let key = database_key(ports, path)?;
    validate_staged_with_key(path, &key)
}

#[cfg_attr(all(test, not(unix)), allow(dead_code))]
fn validate_staged_with_key(path: &Path, key: &DatabaseKey) -> Result<(), StoreError> {
    use std::io::Read;
    let mut header = [0; 16];
    let mut file = std::fs::File::open(path)
        .map_err(|_| StoreError::Unavailable("pending restore cannot be read"))?;
    let plaintext = file.read_exact(&mut header).is_ok() && &header == b"SQLite format 3\0";
    drop(file);
    if plaintext {
        Store::validate_snapshot(path)?;
        // Commit this BEFORE encryption. A crash after database conversion but
        // before document sealing must still resume sealing on the next open,
        // even when the portable source device had already set this marker.
        let store = Store::open(path)?;
        store.mark_private_documents_unsealed()?;
        drop(store);
    } else {
        Store::validate_encrypted_snapshot(path, key)?;
    }
    let store = Store::open_encrypted(path, key)?;
    drop(seal_documents(store, path)?);
    Store::validate_encrypted_snapshot(path, key)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[cfg(unix)]
    use std::io::Read;

    #[test]
    #[cfg(unix)]
    fn legacy_pending_resumes_document_sealing_after_database_conversion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iptv.sqlite");
        let store = Store::open(&path).unwrap();
        store.mark_private_documents_sealed().unwrap();
        drop(store);
        let documents = dir.path().join("playlists");
        std::fs::create_dir(&documents).unwrap();
        let document = documents.join("synthetic.m3u");
        let bytes = b"#EXTM3U\nhttps://synthetic.example.test/private\n";
        std::fs::write(&document, bytes).unwrap();
        let blocked = documents.join("linked");
        std::os::unix::fs::symlink(&document, &blocked).unwrap();
        let key = DatabaseKey::from_bytes([0x62; 32]);
        // The database conversion succeeds, but a refused document prevents
        // publication. Restart must not trust the source device's old marker.
        assert!(validate_staged_with_key(&path, &key).is_err());
        Store::validate_encrypted_snapshot(&path, &key).unwrap();
        assert!(
            !Store::open_encrypted(&path, &key)
                .unwrap()
                .private_documents_sealed()
                .unwrap()
        );
        std::fs::remove_file(blocked).unwrap();
        validate_staged_with_key(&path, &key).unwrap();
        let store = Store::open_encrypted(&path, &key).unwrap();
        assert!(store.private_documents_sealed().unwrap());
        let mut actual = Vec::new();
        store
            .read_private_file(&document)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, bytes);
    }

    #[test]
    fn sealed_store_still_cleans_interrupted_new_document_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iptv.sqlite");
        let key = DatabaseKey::from_bytes([0x63; 32]);
        let store = Store::open_encrypted(&path, &key).unwrap();
        store.mark_private_documents_sealed().unwrap();
        let documents = dir.path().join("playlists");
        std::fs::create_dir(&documents).unwrap();
        for name in [".iptv-sealing-new", ".iptv-sealing-new-journal"] {
            std::fs::write(documents.join(name), b"interrupted encrypted write").unwrap();
        }
        drop(seal_documents(store, &path).unwrap());
        assert_eq!(std::fs::read_dir(documents).unwrap().count(), 0);
    }
}

#[cfg(all(test, feature = "native"))]
mod concurrent_services {
    use super::*;
    use std::io::{self, Read};
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    #[test]
    fn independent_service_opens_seal_the_managed_playlist_once() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("generated-app");
        let documents = root.join("playlists/generated");
        std::fs::create_dir_all(&documents).unwrap();
        let path = root.join("iptv.sqlite");
        let document = documents.join("generated.m3u");
        let key = DatabaseKey::from_bytes([0x67; 32]);
        drop(Store::open_encrypted(&path, &key).unwrap());
        let expected = b"#EXTM3U\nhttps://example.test/generated.ts\n".repeat(4096);
        std::fs::write(&document, &expected).unwrap();
        let ports = crate::Ports::new(root, |_| Ok(DatabaseKey::from_bytes([0x67; 32])));
        let barrier = Arc::new(Barrier::new(3));
        let tasks: Vec<_> = (0..2)
            .map(|_| {
                let ports = ports.clone();
                let path = path.clone();
                let document = document.clone();
                let expected = expected.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let store = open_database(&ports, &path).unwrap();
                    let mut actual = Vec::new();
                    store
                        .read_private_file(&document)
                        .unwrap()
                        .read_to_end(&mut actual)
                        .unwrap();
                    assert_eq!(actual, expected);
                })
            })
            .collect();
        barrier.wait();
        for task in tasks {
            task.join().unwrap();
        }
        assert!(std::fs::read(&document).unwrap() != expected);
    }

    #[test]
    fn service_recovery_cannot_remove_an_active_encrypted_publication() {
        struct HeldInput {
            ready: Option<mpsc::Sender<()>>,
            release: mpsc::Receiver<()>,
            bytes: &'static [u8],
        }
        impl Read for HeldInput {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if let Some(ready) = self.ready.take() {
                    ready.send(()).unwrap();
                    self.release
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(|_| io::ErrorKind::TimedOut)?;
                }
                self.bytes.read(buffer)
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("generated-app");
        let documents = root.join("playlists/generated/playlists");
        std::fs::create_dir_all(&documents).unwrap();
        let path = root.join("iptv.sqlite");
        let document = documents.join("generated.m3u");
        let store = Store::open_encrypted(&path, &DatabaseKey::from_bytes([0x68; 32])).unwrap();
        store.mark_private_documents_sealed().unwrap();
        drop(store);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer_path = path.clone();
        let writer_document = document.clone();
        let writer = std::thread::spawn(move || {
            let store =
                Store::open_encrypted(writer_path, &DatabaseKey::from_bytes([0x68; 32])).unwrap();
            super::super::private_files::write_atomic(
                &store,
                &writer_document,
                &mut HeldInput {
                    ready: Some(ready_tx),
                    release: release_rx,
                    bytes: b"#EXTM3U\nhttps://example.test/generated.ts\n",
                },
            )
            .unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let ports = crate::Ports::new(root, |_| Ok(DatabaseKey::from_bytes([0x68; 32])));
        let (done_tx, done_rx) = mpsc::channel();
        let opener = std::thread::spawn(move || {
            let store = open_database(&ports, &path).unwrap();
            done_tx.send(()).unwrap();
            store
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "recovery must wait for the active publisher"
        );
        assert!(documents.join(".iptv-sealing-generated.m3u").exists());
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        let store = opener.join().unwrap();
        let mut actual = Vec::new();
        store
            .read_private_file(&document)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, b"#EXTM3U\nhttps://example.test/generated.ts\n");
        assert!(!documents.join(".iptv-sealing-generated.m3u").exists());
    }
}
