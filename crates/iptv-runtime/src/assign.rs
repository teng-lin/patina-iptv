//! Choosing a channel's guide by hand (Long OK menu → "Choose guide
//! data…"): automatic matching, no guide, or one channel of the stored
//! guides. The choice is a channel overlay in the store (by channel key,
//! kept across refreshes) that every later match applies
//! (`stored::overrides`).
//!
//! It shows at once when the stored guide allows it: no guide, or a guide
//! channel whose programmes are stored (another playlist channel uses it).
//! Otherwise (a guide channel nothing used yet, or back to automatic,
//! which needs the full channel lists) the playlist's guide is downloaded
//! right away and matched with the choice.

use iptv_epg::ChannelKey;
use iptv_providers::ContentId;
use iptv_store_sqlite::{EpgOverride, GuideChannel, SourceId, Store, StoreError};

use super::imports::{Imported, Streams};
use super::stored::{self, Matching};

pub use iptv_library::Choice;
fn choice_overlay(choice: &Choice) -> Option<EpgOverride> {
    match choice {
        Choice::Automatic => None,
        Choice::NoGuide => Some(EpgOverride::Disabled),
        Choice::Channel { feed, epg_channel } => {
            Some(EpgOverride::for_feed(*feed, epg_channel.clone()))
        }
    }
}

fn choice_of(overlay: Option<EpgOverride>) -> Choice {
    match overlay {
        None => Choice::Automatic,
        Some(EpgOverride::Disabled) => Choice::NoGuide,
        Some(o @ EpgOverride::Xmltv { .. }) => match (o.feed(), o) {
            (Some(feed), EpgOverride::Xmltv { epg_channel, .. }) => {
                Choice::Channel { feed, epg_channel }
            }
            _ => Choice::Automatic,
        },
    }
}

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
pub async fn current(source: SourceId, key: &str) -> Current {
    let key = key.to_owned();
    let read = super::store_service::read(move |s| {
        let overlay = s
            .channel(source, &ContentId::from_stored(&key))?
            .and_then(|row| row.epg_override);
        let bound = s.epg_binding(source, &ChannelKey::new(&key))?;
        Ok(Current {
            choice: Some(choice_of(overlay)),
            bound,
        })
    })
    .await;
    read.unwrap_or_else(|e: StoreError| {
        log::warn!("iptv data: a channel's guide: {e}");
        Current::default()
    })
}

/// The channels the stored guides of `source` list.
pub async fn guide_channels(source: SourceId) -> Vec<GuideChannel> {
    super::store_service::read(move |store| store.guide_channels(source))
        .await
        .unwrap_or_else(|e| {
            log::warn!("iptv data: guide channels: {e}");
            Vec::new()
        })
}

/// On a worker thread: saves the choice, and when the stored guide can
/// show it, rebinds it and reads the guide back (`Some`); `None`: it needs
/// a download.
fn save_and_rebind(
    store: &mut Store,
    source: SourceId,
    key: &str,
    choice: &Choice,
) -> Result<Option<Box<Imported>>, StoreError> {
    let content = ContentId::from_stored(key);
    let overlay = choice_overlay(choice);
    store.set_epg_override(source, &content, overlay.as_ref())?;
    let rebound = (|| -> Result<Option<Box<Imported>>, StoreError> {
        let to = match choice {
            Choice::Automatic => return Ok(None),
            Choice::NoGuide => None,
            Choice::Channel { feed, epg_channel } => Some((*feed, epg_channel.clone())),
        };
        let (groups, channels) = stored::catalog_in(store, source)?;
        let overrides = store
            .epg_overrides(source)?
            .into_iter()
            .map(|(id, epg)| (ChannelKey::new(id.as_str()), epg.to_override()))
            .collect();
        let matching = Matching {
            channels: &channels,
            overrides: &overrides,
        };
        let fingerprint = matching.fingerprint();
        let key = ChannelKey::new(key);
        let rebound = store.rebind_epg(
            source,
            &key,
            to.as_ref().map(|(feed, channel)| (*feed, channel.as_str())),
            fingerprint,
        )?;
        if !rebound {
            return Ok(None);
        }
        let now = super::imports::now_unix();
        Ok(Some(Box::new(Imported {
            guide: store.load_epg(
                source,
                &super::paging::store_window(super::paging::window(
                    now,
                    stored::keep_past_days(&super::settings::get()),
                )),
            )?,
            groups,
            channels,
            playback: Streams::Current,
            now,
            source,
            resume: false,
        })))
    })();
    match rebound {
        Ok(imported) => Ok(imported),
        Err(error) => {
            log::warn!("iptv data: guide choice saved; cached guide could not be updated: {error}");
            Ok(None)
        }
    }
}

pub fn save(
    source: SourceId,
    key: String,
    choice: Choice,
) -> impl std::future::Future<Output = Result<Option<Box<Imported>>, StoreError>> {
    super::store_service::call(move |store| save_and_rebind(store, source, &key, &choice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_choice_survives_a_cached_guide_read_failure() {
        use iptv_providers::{
            SecretUrl,
            model::{M3uSourceConfig, Source, SourceConfig},
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.sqlite");
        let mut store = Store::open(&path).unwrap();
        let source = store
            .add_source(&Source {
                name: "Synthetic guide choice".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://synthetic.invalid/list.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        // The preference can commit even when the cached guide is unreadable.
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("DROP TABLE epg_bindings").unwrap();
        assert!(
            save_and_rebind(&mut store, source, "synthetic-channel", &Choice::NoGuide)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .epg_overrides(source)
                .unwrap()
                .iter()
                .any(|(key, value)| key.as_str() == "synthetic-channel"
                    && *value == EpgOverride::Disabled)
        );
    }

    #[test]
    fn choices_round_trip_through_the_overlay() {
        let feed = iptv_epg::FeedId(1001);
        for choice in [
            Choice::Automatic,
            Choice::NoGuide,
            Choice::Channel {
                feed,
                epg_channel: "news.test".into(),
            },
        ] {
            assert_eq!(choice_of(choice_overlay(&choice)), choice);
        }
        // A key that names no feed (not written by this app) reads as
        // automatic.
        let other = EpgOverride::Xmltv {
            epg_source: "elsewhere".into(),
            epg_channel: "x".into(),
        };
        assert_eq!(choice_of(Some(other)), Choice::Automatic);
    }
}
