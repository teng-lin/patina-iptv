//! Shared layout geometry at the current window width and text scale. The
//! window size class itself is portable (`iptv_presentation::layout`);
//! `crate::wiring::device::window_class` reads the current window's.

pub use iptv_presentation::layout::Class;

mod guide_header;
mod list_panel;
pub use guide_header::*;
pub use list_panel::*;
