//! Playback metadata and per-source resume persistence.
use iptv_guide::playback::PlaybackContext;
use iptv_import::SourceMeta;
use iptv_library::SourceId;
use iptv_providers::model::SourceConfig;
use iptv_providers::xtream::StreamFormat;
pub fn context_for(config: &SourceConfig, meta: &SourceMeta) -> Option<PlaybackContext> {
    match (config, meta) {
        (SourceConfig::M3u(c), _) => Some(PlaybackContext::m3u(c)),
        (SourceConfig::Xtream(c), SourceMeta::Xtream { account, .. }) => {
            PlaybackContext::xtream(c, account.as_ref(), StreamFormat::Auto).ok()
        }
        (SourceConfig::Stalker(_), SourceMeta::Stalker { session }) => {
            Some(PlaybackContext::stalker((**session).clone()))
        }
        _ => None,
    }
}

/// What a stored source's streams need before its refresh ran: no account
/// details yet (HLS for Xtream) and no portal session, so a Stalker
/// source plays only once it imported.
pub fn stored_context(config: &SourceConfig) -> Option<PlaybackContext> {
    match config {
        SourceConfig::M3u(c) => Some(PlaybackContext::m3u(c)),
        SourceConfig::Xtream(c) => PlaybackContext::xtream(c, None, StreamFormat::Auto).ok(),
        _ => None,
    }
}

pub fn account_streams(meta: &SourceMeta) -> Option<u32> {
    match meta {
        SourceMeta::Xtream {
            account: Some(account),
            ..
        } => account.max_connections.filter(|n| *n > 0),
        _ => None,
    }
}

const LAST_CHANNEL_FILE: &str = "last_channel";
pub fn last_channel_path(source: Option<SourceId>) -> std::path::PathBuf {
    super::paths::data_dir().join(match source {
        Some(id) => format!("{LAST_CHANNEL_FILE}-{}", id.0),
        None => LAST_CHANNEL_FILE.to_owned(),
    })
}

/// Queues the write: a zap never waits for the disk (`file_writer`).
pub fn save_last_channel(source: Option<SourceId>, key: &str) {
    drop(super::file_writer::write(
        last_channel_path(source),
        key.as_bytes().to_vec(),
    ));
}

/// Includes a write still queued, so a zap is remembered at once.
pub fn load_last_channel(source: Option<SourceId>) -> Option<String> {
    let read = |path: std::path::PathBuf| {
        let key = String::from_utf8(super::file_writer::read(&path)?).ok()?;
        let key = key.trim();
        (!key.is_empty()).then(|| key.to_owned())
    };
    read(last_channel_path(source)).or_else(|| read(last_channel_path(None)))
}

/// Playlist `source` was deleted: its last channel goes too (after any
/// write still queued for it).
pub fn forget_source(source: SourceId) {
    drop(super::file_writer::remove(last_channel_path(Some(source))));
}

/// Reuse connections across Stalker zaps; settings changes replace the client.
pub fn link_client(
    config: iptv_net::NetConfig,
) -> impl std::future::Future<Output = Result<iptv_net::NetClient, iptv_net::FetchError>> + Send {
    let cache = crate::current().link_client.clone();
    async move {
        let mut cached = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((previous, client)) = cached.as_ref()
            && previous == &config
        {
            return Ok(client.clone());
        }
        let client = iptv_net::NetClient::new(config.clone())?;
        *cached = Some((config, client.clone()));
        Ok(client)
    }
}

pub fn save_combined_channel(key: &str) {
    drop(super::file_writer::write(
        super::paths::data_dir().join("last_channel_combined"),
        key.as_bytes().to_vec(),
    ));
}

pub fn load_combined_channel() -> Option<String> {
    let bytes = super::file_writer::read(&super::paths::data_dir().join("last_channel_combined"))?;
    let key = String::from_utf8(bytes).ok()?;
    (!key.trim().is_empty()).then(|| key.trim().to_owned())
}
