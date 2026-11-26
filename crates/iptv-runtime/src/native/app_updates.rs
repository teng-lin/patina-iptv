//! User-requested checks against the update repository embedded by the app build.

mod download;
mod selection;
mod trust;
#[cfg(windows)]
mod windows;
pub use selection::{InstalledPackage, ReleaseAsset, UpdateResult};

use std::time::Duration;

use iptv_net::{NetClient, NetConfig};
use iptv_providers::HttpRequest;
use tough::{
    IntoVec, RepositoryLoader, TargetName, Transport, TransportError, TransportErrorKind,
    TransportStream,
};
use url::Url;

const USER_AGENT: &str = "Patina";

#[derive(Clone, Debug)]
struct FeedTransport(NetClient);

#[tough::async_trait]
impl Transport for FeedTransport {
    async fn fetch(&self, url: Url) -> Result<TransportStream, TransportError> {
        let failure = |kind| TransportError::new(kind, url.as_str());
        if url.scheme() != "https" {
            return Err(failure(TransportErrorKind::UnsupportedUrlScheme));
        }
        let mut request = HttpRequest::get(url.as_str());
        request.public_only = true;
        let response = self
            .0
            .fetch(request)
            .await
            .map_err(|_| failure(TransportErrorKind::Other))?;
        match response.status {
            200 => Ok(Box::pin(futures_util::stream::once(std::future::ready(
                Ok(response.body.into()),
            )))),
            404 | 410 => Err(failure(TransportErrorKind::FileNotFound)),
            _ => Err(failure(TransportErrorKind::Other)),
        }
    }
}

impl crate::Runtime {
    /// Checks verified release records on this owner's worker. Provider settings,
    /// headers and cookies are never used on the update connection.
    pub async fn check_app_update(
        &self,
        feed: &'static str,
        root: &'static [u8],
        installed: InstalledPackage,
    ) -> UpdateResult {
        // Catalog restores swap the profile directory. Trust state must survive that swap.
        use sha2::{Digest, Sha256};
        let state = crate::backup::sibling(self.data_dir(), "app-update")
            .join(format!("{:x}", Sha256::digest(feed.as_bytes())));
        self.services()
            .spawn(async move {
                let check = async {
                    let base = Url::parse(feed).ok()?;
                    if base.scheme() != "https"
                        || base.host_str().is_none()
                        || !base.path().ends_with('/')
                        || !base.username().is_empty()
                        || base.password().is_some()
                        || base.query().is_some()
                        || base.fragment().is_some()
                    {
                        return None;
                    }
                    let client = NetClient::new(NetConfig {
                        user_agent: USER_AGENT.into(),
                        allow_files: false,
                        max_buffered_bytes: 1 << 20,
                        max_streamed_bytes: 1 << 20,
                        request_timeout: Duration::from_secs(15),
                        ..Default::default()
                    })
                    .ok()?;
                    verified_index(&state, root, &base, FeedTransport(client), installed).await
                };
                tokio::time::timeout(Duration::from_secs(45), check)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(UpdateResult::Unavailable)
            })
            .await
            .unwrap_or(UpdateResult::Unavailable)
    }
}

async fn verified_index(
    state: &std::path::Path,
    root: &[u8],
    base: &Url,
    transport: impl Transport + 'static,
    installed: InstalledPackage,
) -> Option<UpdateResult> {
    let directory = state.to_owned();
    let anchor = root.to_vec();
    let (session, installed) = tokio::task::spawn_blocking(move || {
        trust::Session::open(&directory, &anchor).map(|session| (session, installed.packaged()))
    })
    .await
    .ok()?
    .ok()?;
    let repository = RepositoryLoader::new(
        &session.root,
        base.join("metadata/").ok()?,
        base.join("targets/").ok()?,
    )
    .transport(transport)
    .limits(tough::Limits {
        max_root_updates: 32,
        ..Default::default()
    })
    .datastore(session.work.path())
    .load()
    .await
    .ok()?;
    // The signed repository metadata is already authenticated. Preserve its
    // rollback floor even if a mirror withholds the target bytes.
    let _session = tokio::task::spawn_blocking(move || {
        session.commit()?;
        Ok::<_, std::io::Error>(session)
    })
    .await
    .ok()?
    .ok()?;
    let name = TargetName::new("index.json").ok()?;
    let bytes = repository
        .read_target(&name)
        .await
        .ok()??
        .into_vec()
        .await
        .ok()?;
    selection::select(&bytes, installed)
}

#[cfg(test)]
mod tests;
