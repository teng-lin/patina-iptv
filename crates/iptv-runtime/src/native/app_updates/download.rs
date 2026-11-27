//! Fetch the exact release asset selected from authenticated metadata.
use super::selection::ReleaseAsset;
use iptv_import::BodyStream;
use iptv_net::{NetClient, NetConfig};
use iptv_providers::HttpRequest;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, atomic::Ordering};
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

impl crate::Runtime {
    /// The caller owns the temporary file. Dropping it cancels the handoff.
    pub async fn download_update(
        &self,
        asset: ReleaseAsset,
        directory: PathBuf,
    ) -> Option<TempPath> {
        // Dropping the caller while its dialog closes must stop the background
        // request and let the private partial file's guard clean up.
        let (_cancel_on_drop, cancel) = tokio::sync::oneshot::channel::<()>();
        let (owned_download, progress, owner_cancel) = self.begin_update_download(asset.length);
        let result = self
            .services()
            .spawn(async move {
                tokio::select! {
                    biased;
                    _ = cancel => None,
                    _ = owner_cancel => None,
                    path = fetch(asset, directory, progress) => path,
                }
            })
            .await
            .ok()
            .flatten();
        // Cancellation or replacement can win after the worker sent its
        // result but before this caller resumed. Never publish that stale file.
        if owned_download.is_current() {
            result
        } else {
            None
        }
    }
}

async fn fetch(
    asset: ReleaseAsset,
    directory: PathBuf,
    progress: Arc<super::pending::Progress>,
) -> Option<TempPath> {
    let client = NetClient::new(NetConfig {
        user_agent: super::USER_AGENT.into(),
        allow_files: false,
        max_streamed_bytes: asset.length,
        ..Default::default()
    })
    .ok()?;
    let (url, public_only) = endpoint(&asset)?;
    let mut request = HttpRequest::get(url);
    request.public_only = public_only;
    let response = client.fetch_stream(request).await.ok()?;
    if response.status != 200
        || response
            .content_length
            .is_some_and(|len| len != asset.length)
    {
        return None;
    }
    receive_tracked(
        response.body,
        directory,
        asset.length,
        &asset.sha256,
        Some(progress),
    )
    .await
}

fn endpoint(asset: &ReleaseAsset) -> Option<(String, bool)> {
    // Test the real streaming/hash/cleanup path without changing the immutable
    // asset selected from authenticated metadata. Never present in a package.
    #[cfg(all(feature = "test-support", debug_assertions))]
    if let Some(value) = std::env::var_os("IPTV_TEST_UPDATE_DOWNLOAD_API") {
        let value = value.into_string().ok()?;
        return super::github::fixture_endpoint(&value).then_some((value, false));
    }
    Some((asset.url.clone(), true))
}

#[cfg(test)]
pub(super) async fn receive(
    body: impl BodyStream,
    directory: PathBuf,
    expected_length: u64,
    expected_digest: &str,
) -> Option<TempPath> {
    receive_tracked(body, directory, expected_length, expected_digest, None).await
}

async fn receive_tracked(
    mut body: impl BodyStream,
    directory: PathBuf,
    expected_length: u64,
    expected_digest: &str,
    progress: Option<Arc<super::pending::Progress>>,
) -> Option<TempPath> {
    let (file, path) = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix("patina-update-")
            .suffix(if cfg!(target_os = "android") {
                ".apk"
            } else if cfg!(windows) {
                ".exe"
            } else {
                ".part"
            })
            .tempfile_in(directory)
            .map(|temp| temp.into_parts())
    })
    .await
    .ok()?
    .ok()?;
    let mut file = tokio::fs::File::from_std(file);
    let mut hash = Sha256::new();
    let mut length = 0u64;
    while let Some(chunk) = body.next_chunk().await.ok()? {
        length = length.checked_add(chunk.len() as u64)?;
        if length > expected_length {
            return None;
        }
        hash.update(&chunk);
        file.write_all(&chunk).await.ok()?;
        if let Some(progress) = &progress {
            progress.received.store(length, Ordering::Relaxed);
        }
    }
    if length != expected_length || format!("{:x}", hash.finalize()) != expected_digest {
        return None;
    }
    file.flush().await.ok()?;
    file.sync_all().await.ok()?;
    Some(path)
}
