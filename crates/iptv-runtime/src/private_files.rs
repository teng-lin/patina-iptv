//! App-owned playlist copies are encrypted chunk databases, decrypted only
//! while streaming into the importer or a user-requested portable backup.
use iptv_import::{BodyStream, Fetch, FetchError, FetchErrorKind, StreamingResponse};
use iptv_providers::{HttpRequest, HttpResponse};
use iptv_store_sqlite::{PrivateFileReader, Store, StoreError};
use std::{io::Read, path::Path};

fn failure() -> FetchError {
    FetchError::new(FetchErrorKind::Io, "Private document is unavailable")
}

pub(crate) fn temporary_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with(".iptv-sealing-")
}

fn remove_temporary(path: &Path) -> Result<(), StoreError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StoreError::Unavailable("cannot recover private document")),
    }
}

/// Run on every open, even after the one-time legacy conversion completed.
pub(crate) fn cleanup_temporary(directory: &Path) -> Result<(), StoreError> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)
        .map_err(|_| StoreError::Unavailable("cannot inspect private documents"))?
    {
        let entry =
            entry.map_err(|_| StoreError::Unavailable("cannot inspect private documents"))?;
        if temporary_name(&entry.file_name()) {
            remove_temporary(&entry.path())?;
        } else if entry
            .file_type()
            .map_err(|_| StoreError::Unavailable("cannot inspect private documents"))?
            .is_dir()
        {
            cleanup_temporary(&entry.path())?;
        }
    }
    Ok(())
}

pub(crate) fn seal_tree(store: &Store, directory: &Path) -> Result<(), StoreError> {
    cleanup_temporary(directory)?;
    seal_clean_tree(store, directory)
}

fn seal_clean_tree(store: &Store, directory: &Path) -> Result<(), StoreError> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)
        .map_err(|_| StoreError::Unavailable("cannot inspect private documents"))?
    {
        let entry =
            entry.map_err(|_| StoreError::Unavailable("cannot inspect private documents"))?;
        let kind = entry
            .file_type()
            .map_err(|_| StoreError::Unavailable("cannot inspect private documents"))?;
        if kind.is_symlink() {
            return Err(StoreError::Unavailable(
                "linked private documents are refused",
            ));
        }
        if kind.is_dir() {
            seal_clean_tree(store, &entry.path())?;
        } else if kind.is_file() {
            seal_file(store, &entry.path())?;
        }
    }
    Ok(())
}

pub(crate) fn seal_file(store: &Store, path: &Path) -> Result<(), StoreError> {
    seal_file_after_probe(store, path, store.read_private_file(path).map(|_| ()))
}

fn seal_file_after_probe(
    store: &Store,
    path: &Path,
    probe: Result<(), StoreError>,
) -> Result<(), StoreError> {
    match probe {
        Ok(()) => return Ok(()),
        Err(error) if error.is_not_database() => {}
        Err(error) => {
            // An empty legacy picked document has no SQLite schema. Any
            // nonempty I/O, busy, or schema failure must leave bytes untouched.
            if !std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == 0) {
                return Err(error);
            }
        }
    }
    // Only a one-time legacy conversion or validated portable restore may
    // reinterpret bytes as plaintext; routine reads always fail closed.
    let mut input = std::fs::File::open(path)
        .map_err(|_| StoreError::Unavailable("cannot read legacy document"))?;
    write_atomic(store, path, &mut input)
}

pub(crate) fn write_atomic(
    store: &Store,
    path: &Path,
    input: &mut impl Read,
) -> Result<(), StoreError> {
    let temporary = path.with_file_name(format!(
        ".iptv-sealing-{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    // All producers run on the single store worker. Recovery occurs before a
    // new write, so these names cannot belong to a concurrent publication.
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = temporary.as_os_str().to_owned();
        name.push(suffix);
        remove_temporary(Path::new(&name))?;
    }
    store.write_private_file(&temporary, input)?;
    std::fs::rename(&temporary, path)
        .map_err(|_| StoreError::Unavailable("cannot seal private document"))?;
    #[cfg(unix)]
    std::fs::File::open(
        path.parent()
            .ok_or(StoreError::Unavailable("invalid private document path"))?,
    )
    .and_then(|file| file.sync_all())
    .map_err(|_| StoreError::Unavailable("cannot finish private document"))?;
    Ok(())
}

fn local_allowed(config: &iptv_net::NetConfig, request: &HttpRequest) -> bool {
    #[cfg(debug_assertions)]
    let fixture = config.allow_private_fixture;
    #[cfg(not(debug_assertions))]
    let fixture = false;
    config.allow_files && (fixture || (request.picked_file && !request.public_only))
}

#[derive(Clone)]
pub(crate) struct Client {
    net: iptv_net::NetClient,
}
impl Client {
    pub(crate) fn new(config: iptv_net::NetConfig) -> Result<Self, FetchError> {
        Ok(Self {
            net: iptv_net::NetClient::new(config)?,
        })
    }
    pub(crate) async fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> Result<StreamingResponse<Body>, FetchError> {
        let deadline = tokio::time::Instant::now()
            + request
                .timeout
                .unwrap_or(self.net.config().request_timeout)
                .min(self.net.config().request_timeout);
        tokio::time::timeout_at(deadline, self.fetch_stream_inner(request, deadline))
            .await
            .map_err(|_| {
                FetchError::new(FetchErrorKind::Deadline, "total document deadline exceeded")
            })?
    }
    async fn fetch_stream_inner(
        &self,
        mut request: HttpRequest,
        deadline: tokio::time::Instant,
    ) -> Result<StreamingResponse<Body>, FetchError> {
        let path = iptv_providers::content_url::local_path(&request.url).map_err(|_| failure())?;
        if let Some(path) = path {
            let root = super::paths::data_dir().join("playlists");
            if path.starts_with(&root) {
                if !local_allowed(self.net.config(), &request) {
                    return Err(failure());
                }
                let actual = path.canonicalize().map_err(|_| failure())?;
                let root = root.canonicalize().map_err(|_| failure())?;
                if !actual.starts_with(root) {
                    return Err(failure());
                }
                let reader =
                    super::store_service::call(move |store| store.read_private_file(&actual))
                        .await
                        .map_err(|_| failure())?;
                let remaining = self.net.config().max_streamed_bytes;
                if reader.len() > remaining {
                    return Err(FetchError::new(
                        FetchErrorKind::TooLarge,
                        "document byte budget exceeded",
                    ));
                }

                return Ok(StreamingResponse {
                    status: 200,
                    content_length: Some(reader.len()),
                    body: Body::Private {
                        reader: Some(reader),
                        remaining,
                        deadline,
                    },
                });
            }
        }
        request.timeout = Some(deadline.saturating_duration_since(tokio::time::Instant::now()));
        let response = self.net.fetch_stream(request).await?;
        Ok(StreamingResponse {
            status: response.status,
            content_length: response.content_length,
            body: Body::Network(response.body),
        })
    }
    pub(crate) async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, FetchError> {
        let mut response = self.fetch_stream(request).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.body.next_chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > self.net.config().max_buffered_bytes {
                return Err(FetchError::new(
                    FetchErrorKind::TooLarge,
                    "Document exceeds buffering limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(HttpResponse::new(response.status, bytes))
    }
}
pub(crate) enum Body {
    Network(iptv_net::NetBody),
    Private {
        reader: Option<PrivateFileReader>,
        remaining: u64,
        deadline: tokio::time::Instant,
    },
}
impl BodyStream for Body {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, FetchError> {
        match self {
            Self::Network(body) => body.next_chunk().await,
            Self::Private {
                reader,
                remaining,
                deadline,
            } => {
                let Some(mut owned) = reader.take() else {
                    return Ok(None);
                };
                let (owned, result) = tokio::time::timeout_at(
                    *deadline,
                    tokio::task::spawn_blocking(move || {
                        let mut bytes = vec![0; 64 * 1024];
                        let result = owned.read(&mut bytes).map(|count| {
                            bytes.truncate(count);
                            (count > 0).then_some(bytes)
                        });
                        (owned, result)
                    }),
                )
                .await
                .map_err(|_| {
                    FetchError::new(FetchErrorKind::Deadline, "total document deadline exceeded")
                })?
                .map_err(|_| failure())?;
                let chunk = result.map_err(|_| failure())?;
                if let Some(bytes) = &chunk {
                    *remaining = remaining.checked_sub(bytes.len() as u64).ok_or_else(|| {
                        FetchError::new(FetchErrorKind::TooLarge, "document byte budget exceeded")
                    })?;
                }
                *reader = Some(owned);
                Ok(chunk)
            }
        }
    }
}
impl Fetch for Client {
    type Body = Body;
    // The import's clock: budgets count the whole import (a guide source's
    // retries and waits included), not each request afresh.
    fn elapsed(&self) -> std::time::Duration {
        self.net.elapsed()
    }
    // The wait before a guide download is tried again; the trait's default
    // does not wait.
    async fn sleep(&self, duration: std::time::Duration) {
        self.net.sleep(duration).await;
    }
    async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, FetchError> {
        self.fetch(request).await
    }
    async fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> Result<StreamingResponse<Body>, FetchError> {
        self.fetch_stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_encrypted_probe_failure_never_reencrypts_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_encrypted(
            dir.path().join("catalog"),
            &iptv_store_sqlite::DatabaseKey::from_bytes([0x64; 32]),
        )
        .unwrap();
        let document = dir.path().join("synthetic.m3u");
        write_atomic(&store, &document, &mut b"original".as_slice()).unwrap();
        let before = std::fs::read(&document).unwrap();
        let error = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            None,
        ));
        assert!(seal_file_after_probe(&store, &document, Err(error)).is_err());
        assert_eq!(std::fs::read(&document).unwrap(), before);
        let empty = dir.path().join("empty");
        std::fs::write(&empty, []).unwrap();
        seal_file(&store, &empty).unwrap();
        assert!(store.read_private_file(&empty).unwrap().is_empty());
    }

    #[test]
    fn failed_atomic_write_preserves_published_document_and_recovers_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_encrypted(
            dir.path().join("catalog"),
            &iptv_store_sqlite::DatabaseKey::from_bytes([0x61; 32]),
        )
        .unwrap();
        let document = dir.path().join("synthetic.m3u");
        write_atomic(&store, &document, &mut b"original".as_slice()).unwrap();
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("synthetic interrupted input"))
            }
        }
        assert!(write_atomic(&store, &document, &mut Failed).is_err());
        let mut actual = Vec::new();
        store
            .read_private_file(&document)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, b"original");
        for suffix in ["", "-journal"] {
            std::fs::write(
                dir.path()
                    .join(format!(".iptv-sealing-synthetic.m3u{suffix}")),
                b"crash remainder",
            )
            .unwrap();
        }
        write_atomic(&store, &document, &mut b"replacement".as_slice()).unwrap();
        let mut actual = Vec::new();
        store
            .read_private_file(&document)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, b"replacement");
        remove_temporary(&dir.path().join("already-gone")).unwrap();
    }

    #[test]
    fn local_documents_require_explicit_pick_and_never_content_provenance() {
        let config = iptv_net::NetConfig::default();
        let mut request = HttpRequest::get("file:/synthetic/private.m3u");
        assert!(!local_allowed(&config, &request));
        request.picked_file = true;
        assert!(local_allowed(&config, &request));
        request.public_only = true;
        assert!(!local_allowed(&config, &request));
        request.public_only = false;
        assert!(!local_allowed(
            &iptv_net::NetConfig {
                allow_files: false,
                ..config
            },
            &request
        ));
    }
    #[test]
    fn legacy_documents_are_sealed_and_reopen_without_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_encrypted(
            directory.path().join("catalog"),
            &iptv_store_sqlite::DatabaseKey::from_bytes([0x33; 32]),
        )
        .unwrap();
        let documents = directory.path().join("playlists");
        std::fs::create_dir(&documents).unwrap();
        let path = documents.join("synthetic.m3u");
        let original = b"#EXTM3U\n#EXTINF:-1,Synthetic\nhttps://synthetic.test/private-password\n";
        std::fs::write(&path, original).unwrap();
        std::fs::write(
            documents.join(".iptv-sealing-synthetic.m3u"),
            b"partial encrypted write",
        )
        .unwrap();
        seal_tree(&store, &documents).unwrap();
        assert!(!documents.join(".iptv-sealing-synthetic.m3u").exists());
        assert!(
            !std::fs::read(&path)
                .unwrap()
                .windows(16)
                .any(|bytes| bytes == b"private-password")
        );
        seal_tree(&store, &documents).unwrap();
        let mut actual = Vec::new();
        store
            .read_private_file(&path)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, original);
        std::fs::write(&path, b"corrupted ciphertext").unwrap();
        assert!(store.read_private_file(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupted ciphertext");
    }
}
