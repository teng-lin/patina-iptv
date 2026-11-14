//! Shell entry points for favourite actions; state and persistence live in Runtime.
use crate::Ui;
use iptv_core::favorites::History;
use iptv_core::guide::Schedule;
use std::collections::BTreeSet;

pub fn catalog_changed(_ui: Ui, catalog: &dyn Schedule) {
    crate::runtime().favorites_catalog_changed(catalog);
}
pub fn edit(_ui: Ui, change: impl FnOnce(&mut BTreeSet<usize>)) {
    crate::runtime().edit_favorites(change);
}
pub fn toggle(_ui: Ui, channel: usize) {
    crate::runtime().toggle_favorite(channel);
}
pub fn reorder(_ui: Ui, key: &str, index: usize) {
    crate::runtime().reorder_favorite(key, index);
}
/// A new app root must reload persisted favourites, including an applied restore.
pub fn reset() {
    crate::runtime().reset_favorites();
}
/// The favourites on screen as catalog channels, in their order.
pub fn favorite_channels() -> Vec<usize> {
    crate::runtime().favorite_channels()
}
/// The recently watched channels on screen, newest first.
pub fn recent_channels() -> Vec<usize> {
    crate::runtime().recent_channels()
}
/// The player's History: the recently watched channels (newest first) and
/// the row of `playing` among them.
pub fn history(playing: Option<usize>) -> History {
    crate::runtime().favorites_history(playing)
}
/// The catalog channel with `key`.
pub fn channel_of(key: &str) -> Option<usize> {
    crate::runtime().favorites_channel_of(key)
}
/// A live channel started playing (by key).
pub fn played(key: &str) {
    crate::runtime().record_played(key);
}
/// Ordered keys of the displayed source.
pub fn ordered_keys() -> Vec<String> {
    crate::runtime().ordered_favorite_keys()
}
/// Resolves once the edits made so far are saved; false when they could not be.
pub fn flush() -> impl std::future::Future<Output = bool> {
    crate::runtime().flush_favorites()
}
/// Root teardown only: queued edits outlive their UI owner and drain first.
pub fn flush_on_shutdown() -> bool {
    crate::runtime().flush_favorites_on_shutdown()
}
