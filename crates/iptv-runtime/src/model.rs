//! Value types exposed by application commands. No connection or SQL API.
pub use crate::Error as StoreError;
pub use iptv_library::{
    ChannelScope, CustomGroup, CustomGroupId, GuideChannel, GuideSnapshot, GuideSource,
    ImportErrorKind, SourceId, SourcePolicy, StoredSource,
};
/// The sole adapter-owned boundary value: the credential broker supplies an
/// opaque, redacting key; commands never return storage records or errors.
pub use iptv_store_sqlite::DatabaseKey;
pub(crate) use iptv_store_sqlite::StoredGuide;

/// What early playback needs before the full catalogue has been loaded.
/// Storage overlays other than the parental block stay inside the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackChannel {
    pub channel: iptv_providers::model::Channel,
    pub blocked: bool,
}

#[cfg(test)]
pub use iptv_library::FeedResult;
