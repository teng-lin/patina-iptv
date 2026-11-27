//! Choosing a channel's guide by hand (Long OK menu → "Choose guide
//! data…"): automatic matching, no guide, or one channel of the stored
//! guides. The choice is a channel overlay in the store (by channel key,
//! kept across refreshes) that every later match applies
//! (`stored::overrides`).
//!
//! It shows at once when the stored guide allows it: no guide, or a guide
//! channel whose programs are stored (another playlist channel uses it).
//! Otherwise (a guide channel nothing used yet, or back to automatic,
//! which needs the full channel lists) the playlist's guide is downloaded
//! right away and matched with the choice.

use crate::imports::Imported;
use crate::stored;
use iptv_library::{GuideChannel, SourceId};

pub use iptv_library::Choice;

/// When a choice shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The guide on screen shows it already.
    Now,
    /// The guide is downloading to apply it.
    Downloading,
    /// At the next guide update (nothing to download now).
    Later,
    /// It could not be saved.
    Failed,
}

pub use iptv_library::Current;

/// The saved choice and the binding of channel `key` of `source`.
pub fn current(
    owner: &crate::Runtime,
    source: SourceId,
    key: &str,
) -> impl std::future::Future<Output = Current> + use<> {
    let key = key.to_owned();
    let read = owner.services().epg().current(source, key);
    async move {
        read.await.unwrap_or_else(|e: crate::error::AdapterError| {
            log::warn!("iptv data: a channel's guide: {e}");
            Current::default()
        })
    }
}

/// The channels the stored guides of `source` list.
pub fn guide_channels(
    owner: &crate::Runtime,
    source: SourceId,
) -> impl std::future::Future<Output = Vec<GuideChannel>> + use<> {
    let read = owner.services().epg().guide_channels(source);
    async move {
        read.await.unwrap_or_else(|e| {
            log::warn!("iptv data: guide channels: {e}");
            Vec::new()
        })
    }
}

pub(crate) fn save(
    owner: &crate::Runtime,
    source: SourceId,
    key: String,
    choice: Choice,
) -> impl std::future::Future<Output = Result<Option<Box<Imported>>, crate::Error>> + use<> {
    let keep = stored::stored_past_days();
    let saved = owner
        .services()
        .epg()
        .save_choice(source, key, choice, keep);
    async move { saved.await.map_err(crate::Error::from) }
}
