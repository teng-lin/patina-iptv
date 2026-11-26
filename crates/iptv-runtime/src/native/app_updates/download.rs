//! Fetch the exact release asset selected from authenticated metadata.
use super::selection::ReleaseAsset;
use iptv_import::BodyStream;
use iptv_net::{NetClient, NetConfig};
use iptv_providers::HttpRequest;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

impl crate::Runtime {
    /// The caller owns the temporary file. Dropping it cancels the handoff.
    pub async fn download_update(
        &self,
        asset: ReleaseAsset,
        directory: PathBuf,
    ) -> Option<TempPath> {
        self.services()
            .spawn(async move {
                let client = NetClient::new(NetConfig {
                    user_agent: super::USER_AGENT.into(),
                    allow_files: false,
                    max_streamed_bytes: asset.length,
                    ..Default::default()
                })
                .ok()?;
                let mut request = HttpRequest::get(&asset.url);
                request.public_only = true;
                let response = client.fetch_stream(request).await.ok()?;
                if response.status != 200
                    || response
                        .content_length
                        .is_some_and(|len| len != asset.length)
                {
                    return None;
                }
                receive(response.body, directory, asset.length, &asset.sha256).await
            })
            .await
            .ok()
            .flatten()
    }
}

pub(super) async fn receive(
    mut body: impl BodyStream,
    directory: PathBuf,
    expected_length: u64,
    expected_digest: &str,
) -> Option<TempPath> {
    let (file, path) = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix("patina-update-")
            .suffix(".part")
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
    }
    if length != expected_length || format!("{:x}", hash.finalize()) != expected_digest {
        return None;
    }
    file.flush().await.ok()?;
    file.sync_all().await.ok()?;
    Some(path)
}
