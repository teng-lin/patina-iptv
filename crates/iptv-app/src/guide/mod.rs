//! Guide views, schedule navigation, and channel lists.

pub(crate) mod clock;
pub(crate) mod fav_editor;
pub(crate) mod favorites;
pub(crate) mod grid;
pub(crate) mod guide_clock;
pub(crate) mod header;
pub(crate) mod list_rows;
pub(crate) mod listing_labels;
pub(crate) mod lists;
pub(crate) mod numbers;

pub use iptv_core::guide::{
    Airing, CHANNELS, DAY, GROUPS, GroupName, HISTORY, HORIZON, HOUR, Listing, Program,
    SYNTHETIC_EPOCH_UNIX, Schedule, SyntheticSchedule, VISIBLE, catchup_days, floor, floor_of,
    group_start, now_minute, program_at, secs_to_next_minute, unix_of,
};
pub use iptv_presentation::guide::{
    Focus, Move, PAST, Pane, ProgramSelection, STEP, home_window, moves, page_row, timeline,
    view_range,
};
