//! The shell applies a guide choice and presents its result.
use super::stored;
pub use crate::assign::{Applied, Choice, Current, current, guide_channels};
use iptv_library::SourceId;

/// Saves `choice` for channel `key` of `source` (the playlist on screen)
/// and applies it; `done` hears when it shows.
pub fn choose(source: SourceId, key: String, choice: Choice, done: impl FnOnce(Applied) + 'static) {
    // Work under way shows or stores the guide as it was matched before
    // this choice: it is replaced, whatever it is.
    let superseded = stored::supersede(source);
    let generation = crate::current().generation().with(std::cell::Cell::get);
    let saved = crate::assign::save(source, key, choice);
    super::spawn_in_app(async move {
        // The choice is applied as soon as it is saved, in the order of the
        // store's writer: a guide action after this one (the guide data
        // deleted) then also shows after it. Nothing here waits for the
        // work the choice cancelled, which may sit behind busy workers.
        let applied = match saved.await {
            Err(error) => {
                log::warn!("iptv data: could not save a channel's guide: {error}");
                Applied::Failed
            }
            Ok(Some(imported)) => {
                super::show(imported, generation, false).await;
                Applied::Now
            }
            // The download that applies the choice is an update of its
            // own, which starts once the cancelled work has unwound.
            Ok(None) if stored::restart_update(source, superseded.cancelled.clone(), |_| {}) => {
                Applied::Downloading
            }
            Ok(None) => Applied::Later,
        };
        // A check the choice stopped comes due again shortly, unless the
        // download answers for it.
        superseded.keep_check(source, generation, super::now_unix());
        log::info!("iptv data: a channel's guide was chosen ({applied:?})");
        done(applied);
    });
}
