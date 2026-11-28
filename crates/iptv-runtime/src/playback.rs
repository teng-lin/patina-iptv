//! Playback metadata and per-source resume persistence.
mod resolve;
pub use resolve::{ResolveFailure, STALKER_LINK_TIMEOUT, StreamPurpose};

/// Attach the exact stored source snapshot without another store job.
pub fn source_authority(
    stored: &iptv_library::StoredSource,
) -> iptv_guide::playback::SourceAuthority {
    iptv_guide::playback::SourceAuthority::new(
        stored.id.0,
        stored.incarnation.clone(),
        stored.source.config.clone(),
    )
}

use iptv_guide::playback::PlaybackContext;
use iptv_import::SourceMeta;
use iptv_providers::model::SourceConfig;
use iptv_providers::xtream::StreamFormat;

impl crate::Runtime {
    /// Builds a playback context for a stored source before the first refresh.
    pub fn stored_playback_context(config: &SourceConfig) -> Option<PlaybackContext> {
        stored_context(config)
    }

    /// Captures the exact stored source identity and configuration for early playback.
    pub fn stored_source_authority(
        stored: &iptv_library::StoredSource,
    ) -> iptv_guide::playback::SourceAuthority {
        source_authority(stored)
    }
}

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

pub(crate) use super::store_service::{
    forget_source, load_combined_channel, load_last_channel, save_combined_channel,
    save_last_channel,
};

/// Reuse connections across Stalker zaps; settings changes replace the client.
pub(crate) fn link_client(
    owner: &crate::Runtime,
    config: crate::network::NetworkConfig,
) -> impl std::future::Future<Output = Result<crate::network::Client, iptv_import::FetchError>>
+ crate::MaybeSend
+ use<> {
    let cache = owner.catalog.link_client_cache();
    let network = owner.services().ports().network.clone();
    async move {
        let mut cached = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((previous, client)) = cached.as_ref()
            && previous == &config
        {
            return Ok(client.clone());
        }
        let client = network.client(&config)?;
        *cached = Some((config, client.clone()));
        Ok(client)
    }
}

pub use crate::time::playback_clock as clock;
