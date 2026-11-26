//! Data operations exposed to the app's library and guide views.
//! Import, library, combined-browser and guide work live in `iptv_runtime::data`.

mod library;
pub(crate) mod pages;
pub(crate) mod stored;
pub use crate::runtime::data::{
    Activity, Job, Playlist, Purpose, Update, delete, edit_guides, order_changed,
    organization_changed, refresh, refresh_all, rename, reorder, set_user_agent, source_from_draft,
    start_import, switch_all, switch_to, toggle_playlist, try_details, update_policy,
};
pub use crate::runtime::data::{assign, combined};
pub use crate::runtime::guides;
pub use library::Library;
use std::path::PathBuf;

/// Where the app keeps its files: the Android app's files directory,
/// `~/Library/Application Support/iptv-rs` on macOS and in the iOS app's
/// container, or
/// `~/.local/share/iptv-rs` on the other desktops.
pub fn data_dir() -> PathBuf {
    crate::runtime_owner::runtime().data_dir().to_owned()
}
