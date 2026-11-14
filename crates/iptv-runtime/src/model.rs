//! Value types exposed by application commands. No connection or SQL API.
pub use crate::Error as StoreError;
pub use iptv_library::{
    ChannelScope, CustomGroup, CustomGroupId, GuideChannel, GuideSnapshot, GuideSource,
    ImportErrorKind, SourceId, SourcePolicy, StoredSource,
};
pub use iptv_store_sqlite::DatabaseKey;
pub(crate) use iptv_store_sqlite::StoredGuide;

#[cfg(test)]
pub use iptv_library::FeedResult;
