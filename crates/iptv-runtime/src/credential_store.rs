//! Encrypted library open, migration and restore validation using a key port.
use iptv_store_sqlite::{DatabaseKey, Store, StoreError};
use std::path::Path;

pub(crate) fn open_database(path: &Path) -> Result<Store, StoreError> {
    let key = crate::ports::database_key(path)?;
    let store = Store::open_encrypted(path, &key)?;
    seal_documents(store, path)
}

#[cfg(test)]
mod test_support_tests {
    #[test]
    fn absent_synthetic_key_never_reaches_the_platform_broker() {
        // This unit-test process never installs the integration harness key.
        let directory = tempfile::tempdir().unwrap();
        let result = super::open_database(&directory.path().join("synthetic.sqlite"));
        assert!(matches!(
            result,
            Err(iptv_store_sqlite::StoreError::Unavailable(
                "synthetic database key was not installed"
            ))
        ));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

fn seal_documents(store: Store, path: &Path) -> Result<Store, StoreError> {
    if let Some(root) = path.parent() {
        super::private_files::cleanup_temporary(&root.join("playlists"))?;
    }
    if !store.private_documents_sealed()? {
        if let Some(root) = path.parent() {
            super::private_files::seal_tree(&store, &root.join("playlists"))?;
        }
        store.mark_private_documents_sealed()?;
    }
    Ok(store)
}

/// Revalidate an encrypted pending restore; strictly validate and seal legacy
/// plaintext pending content before it can become active.
#[cfg(not(test))]
pub(crate) fn validate_staged_database(path: &Path) -> Result<(), StoreError> {
    let key = crate::ports::database_key(path)?;
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
