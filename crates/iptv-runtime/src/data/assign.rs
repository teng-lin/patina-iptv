//! The shell applies a guide choice and presents its result.
use super::stored;
pub use crate::assign::{Applied, Choice, Current, current, guide_channels};
use iptv_library::SourceId;

/// Saves `choice` for channel `key` of `source` (the playlist on screen)
/// and applies it; `done` hears when it shows.
pub fn choose(source: SourceId, key: String, choice: Choice, done: impl FnOnce(Applied) + 'static) {
    super::library::revoke_import_handoff(source);
    let generation = crate::current().generation.with(std::cell::Cell::get);
    let saved = crate::assign::save(source, key, choice);
    super::spawn_in_app(async move {
        let outcome = saved.await;
        let applied = match outcome {
            Err(error) => {
                log::warn!("iptv data: could not save a channel's guide: {error}");
                Applied::Failed
            }
            Ok(Some(imported)) => {
                super::show(imported, generation, false).await;
                Applied::Now
            }
            Ok(None) if stored::update_source(source, |_| {}) => Applied::Downloading,
            Ok(None) => Applied::Later,
        };
        log::info!("iptv data: a channel's guide was chosen ({applied:?})");
        done(applied);
    });
}
