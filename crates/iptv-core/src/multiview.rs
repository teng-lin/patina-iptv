//! Maximum number of simultaneously allocated player surfaces.

/// The most tiles (and player surfaces) multiview uses.
pub const MAX_SURFACES: usize = 4;

pub use crate::multiview_state::{
    Dialog, Key, Layout, Limits, Multiview, Outcome, Pick, Rect, THUMB, Tile, TileAction, VIDEO,
    WARNING_CHOICES, WarningChoice,
};
