//! Stored channel and group organization values.

use std::collections::HashSet;

/// An explicit sort choice for a playlist or guide list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    Provider,
    Name,
    NameDescending,
    LastWatched,
    OldestWatched,
    MostWatched,
    LeastWatched,
    #[default]
    Manual,
}

impl Sort {
    pub fn uses_watch_history(self) -> bool {
        matches!(
            self,
            Self::LastWatched | Self::OldestWatched | Self::MostWatched | Self::LeastWatched
        )
    }
}

/// Preferences scoped to one named live channel group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupPreferences {
    pub sort: Sort,
    pub favorites_only: bool,
    pub external_player: bool,
    pub epg_offset_minutes: i32,
}

pub const GROUP_EPG_OFFSET_MIN: i32 = -12 * 60;
pub const GROUP_EPG_OFFSET_MAX: i32 = 14 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupPreference {
    FavoritesOnly(bool),
    ExternalPlayer(bool),
    EpgOffsetMinutes(i32),
}

/// How a channel's playback is blocked by guide preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Block {
    #[default]
    None,
    Channel,
    Group,
}

/// Explicit guide blocks and the channels affected by blocked groups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blocks {
    pub channels: HashSet<String>,
    pub groups: HashSet<String>,
    pub members: HashSet<String>,
}

impl Blocks {
    /// Direct blocks take precedence when a channel has both kinds of block.
    pub fn channel(&self, id: &str) -> Block {
        if self.channels.contains(id) {
            Block::Channel
        } else if self.members.contains(id) {
            Block::Group
        } else {
            Block::None
        }
    }
}

/// Which type of guide entry is directly blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Channel,
    Group,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub hidden: bool,
}
/// An edit that can be applied to the stored channel or group organization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Visibility,
    ResetName,
    Earlier,
    Later,
    First,
    Last,
    ResetOrder,
    HideSelected,
    ShowSelected,
    HideAll,
    ShowAll,
}
