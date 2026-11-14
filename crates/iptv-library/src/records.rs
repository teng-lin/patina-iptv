//! Portable records shared by storage and application presentation.
use crate::{CustomGroupId, ImportErrorKind, SourceId};
use iptv_epg::UnixTime;
use iptv_providers::{GroupId, model::Source};

/// A source read back with its secrets. `Debug` is safe: every secret in
/// [`Source`] is a redacting type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSource {
    pub id: SourceId,
    pub source: Source,
    /// Order in the source list (insertion order).
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomGroup {
    pub id: CustomGroupId,
    pub source_id: SourceId,
    pub name: String,
    /// Members present in the current catalog (hidden ones included).
    pub channel_count: u32,
}

/// Which channels to list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ChannelScope {
    #[default]
    All,
    /// One provider group.
    Group(GroupId),
    /// One custom group, in its own member order.
    Custom(CustomGroupId),
}

/// View-facing guide metadata, independent of stored feed and SQL representations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideSnapshot {
    pub replaced_at: Option<UnixTime>,
    pub attempted_at: Option<UnixTime>,
    pub last_error: Option<ImportErrorKind>,
    pub bound_channels: u64,
}
