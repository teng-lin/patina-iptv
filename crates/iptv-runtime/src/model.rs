//! Value types exposed by application commands. No connection or SQL API.
pub use iptv_library::{
    CustomGroupId, GuideChannel, GuideSource, ImportErrorKind, SourceId, SourcePolicy,
};
pub use iptv_store_sqlite::{
    ChannelScope, CustomGroup, DatabaseKey, StoreError, StoredGuide, StoredSource,
};

#[cfg(test)]
pub use iptv_library::FeedResult;
