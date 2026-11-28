//! Data operations exposed to the app's library and guide views.
//! Import, library, combined-browser and guide work live in `iptv_runtime::data`.

mod library;
pub(crate) mod pages;
pub(crate) mod stored;
pub use iptv_library::guides;
pub use iptv_runtime::data::{
    Activity, Job, Playlist, Purpose, Update, delete, edit_guides, organization_changed, refresh,
    refresh_all, rename, reorder, set_user_agent, source_from_draft, switch_all, switch_to,
    toggle_playlist, try_details, update_policy,
};
pub use iptv_runtime::data::{assign, combined};
pub use library::Library;
use std::path::PathBuf;

/// Where the app keeps its files: the Android app's files directory,
/// `~/Library/Application Support/iptv-rs` on macOS and in the iOS app's
/// container, or
/// `~/.local/share/iptv-rs` on the other desktops.
pub fn data_dir() -> PathBuf {
    crate::wiring::runtime_owner::runtime()
        .data_dir()
        .to_owned()
}
