//! Typed EPG repository for guide configuration and read models.

use super::{Services, StoreError, catalog_in};
use crate::state::Matching;
use iptv_epg::{ChannelKey, GuideStore, UnixTime};
use iptv_import::EpgSourceConfig;
use iptv_library::{Choice, Current, SourceId};
use iptv_providers::ContentId;
use iptv_providers::model::{Channel, Group};
use iptv_search::{ChannelId, ProgramIndex, ProgramIndexBuilder};
use iptv_store_sqlite::{EpgOverride, EpgWindow, GuideChannel, GuideSource, Store};
use std::future::Future;

pub(crate) struct RebuiltSchedule {
    pub groups: Vec<Group>,
    pub channels: Vec<Channel>,
    pub guide: GuideStore,
    pub now: i64,
    pub source: SourceId,
    pub resume: bool,
}

fn held_window(window: iptv_guide::pages::PagedWindow) -> EpgWindow<'static> {
    EpgWindow {
        from: window.loaded_from,
        ..EpgWindow::from(0, 0)
    }
}

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
        Some(value @ EpgOverride::Xmltv { .. }) => match (value.feed(), value) {
            (Some(feed), EpgOverride::Xmltv { epg_channel, .. }) => {
                Choice::Channel { feed, epg_channel }
            }
            _ => Choice::Automatic,
        },
    }
}

pub(crate) struct EpgRepo {
    pub(super) services: Services,
}

impl EpgRepo {
    pub(crate) async fn guide_sources_with_legacy_configs(
        self,
        source: SourceId,
    ) -> Result<(Vec<EpgSourceConfig>, Vec<GuideSource>), StoreError> {
        self.services
            .call(move |store| {
                let stored = store.epg_guide(source)?;
                let sources = store.guide_sources(source)?;
                let fallback = if sources.is_empty() {
                    stored.map(|guide| guide.configs()).unwrap_or_default()
                } else {
                    Vec::new()
                };
                Ok((fallback, sources))
            })
            .await
    }

    pub(crate) fn listing_starts(
        self,
        source: SourceId,
        cutoff: UnixTime,
    ) -> impl Future<Output = Result<std::collections::HashMap<ChannelKey, UnixTime>, StoreError>> + use<>
    {
        self.services
            .read(move |store| store.epg_listing_starts(source, cutoff))
    }

    pub(crate) async fn clear_guides(
        self,
        sources: Vec<SourceId>,
    ) -> Result<(bool, Vec<RebuiltSchedule>), StoreError> {
        self.services
            .call(move |store| {
                let mut deleted = true;
                let mut rebuilt = Vec::new();
                for source in sources {
                    if let Err(error) = store.delete_epg(source) {
                        log::warn!("iptv data: could not delete stored guide: {error}");
                        deleted = false;
                        continue;
                    }
                    if let Ok((groups, channels)) = catalog_in(store, source) {
                        rebuilt.push(RebuiltSchedule {
                            groups,
                            channels,
                            guide: GuideStore::default(),
                            now: crate::time::now_unix(),
                            source,
                            resume: false,
                        });
                    }
                }
                Ok((deleted, rebuilt))
            })
            .await
    }

    pub(crate) async fn repair_publication(
        self,
        id: String,
        keep: u32,
    ) -> Result<Option<Box<RebuiltSchedule>>, StoreError> {
        self.services
            .call(move |store| repair_publication_in(store, &id, keep))
            .await
    }

    pub(crate) fn current(
        self,
        source: SourceId,
        key: String,
    ) -> impl Future<Output = Result<Current, StoreError>> + use<> {
        self.services.read(move |store| {
            let overlay = store
                .channel(source, &ContentId::from_stored(&key))?
                .and_then(|row| row.epg_override);
            let bound = store.epg_binding(source, &ChannelKey::new(&key))?;
            Ok(Current {
                choice: Some(choice_of(overlay)),
                bound,
            })
        })
    }

    pub(crate) fn guide_channels(
        self,
        source: SourceId,
    ) -> impl Future<Output = Result<Vec<GuideChannel>, StoreError>> + use<> {
        self.services
            .read(move |store| store.guide_channels(source))
    }

    pub(crate) fn save_choice(
        self,
        source: SourceId,
        key: String,
        choice: Choice,
        keep_past_days: u32,
    ) -> impl Future<Output = Result<Option<Box<RebuiltSchedule>>, StoreError>> + use<> {
        self.services
            .call(move |store| save_and_rebind(store, source, &key, &choice, keep_past_days))
    }

    pub(crate) async fn description(
        self,
        source: SourceId,
        key: ChannelKey,
        start: UnixTime,
    ) -> Result<Option<String>, StoreError> {
        self.services
            .read(move |store| store.epg_description(source, &key, start))
            .await
    }

    pub(crate) fn page_after_writes(
        &self,
        source: SourceId,
        window: iptv_guide::pages::PagedWindow,
    ) -> impl Future<Output = Result<GuideStore, StoreError>> + use<> {
        let epg_window = held_window(window);
        self.services
            .call(move |store| store.load_epg(source, &epg_window))
    }

    pub(crate) async fn authorized_page(
        &self,
        source: SourceId,
        incarnation: Option<String>,
        request: iptv_guide::pages::PageRequest,
    ) -> Result<GuideStore, StoreError> {
        self.services
            .read(move |store| {
                store.read_snapshot(|store| {
                    let Some(incarnation) = incarnation else {
                        return Err(StoreError::Unavailable(
                            "guide page has no source authority",
                        ));
                    };
                    if store.source(source)?.is_none()
                        || !store.source_policy(source)?.enabled
                        || store.source_incarnation(source)? != incarnation
                    {
                        return Err(StoreError::Unavailable("guide page source changed"));
                    }
                    store.load_epg(
                        source,
                        &EpgWindow {
                            from: request.from,
                            to: request.to,
                            ..EpgWindow::from(0, 0)
                        },
                    )
                })
            })
            .await
    }

    pub(crate) async fn program_index(
        self,
        sources: Vec<(SourceId, String, Vec<ChannelKey>, usize)>,
        rows: usize,
        from: UnixTime,
    ) -> Result<ProgramIndex, StoreError> {
        self.services
            .read(move |store| {
                store.read_snapshot(|store| {
                    let mut builder = ProgramIndexBuilder::new(rows);
                    for (source, incarnation, keys, offset) in sources {
                        if store.source_incarnation(source)? != incarnation {
                            return Err(StoreError::Unavailable(
                                "search source changed while indexing",
                            ));
                        }
                        for (channel, key) in keys.iter().enumerate() {
                            builder.set_identity(
                                channel + offset,
                                ChannelId::in_source(source.0, incarnation.clone(), key.as_str()),
                            );
                        }
                        store.for_each_epg_title(
                            source,
                            &keys,
                            from,
                            UnixTime::MAX,
                            |channel, start, stop, title| {
                                builder.push(channel + offset, start, stop, title)
                            },
                        )?;
                    }
                    Ok(builder.finish())
                })
            })
            .await
    }
}

fn save_and_rebind(
    store: &mut Store,
    source: SourceId,
    key: &str,
    choice: &Choice,
    keep_past_days: u32,
) -> Result<Option<Box<RebuiltSchedule>>, StoreError> {
    let content = ContentId::from_stored(key);
    let overlay = choice_overlay(choice);
    store.set_epg_override(source, &content, overlay.as_ref())?;
    let rebound = (|| -> Result<Option<Box<RebuiltSchedule>>, StoreError> {
        let to = match choice {
            Choice::Automatic => return Ok(None),
            Choice::NoGuide => None,
            Choice::Channel { feed, epg_channel } => Some((*feed, epg_channel.clone())),
        };
        let (groups, channels) = catalog_in(store, source)?;
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
        let now = crate::time::now_unix();
        let window = crate::paging::window(now, keep_past_days);
        let epg_window = held_window(window);
        Ok(Some(Box::new(RebuiltSchedule {
            guide: store.load_epg(source, &epg_window)?,
            groups,
            channels,
            now,
            source,
            resume: false,
        })))
    })();
    match rebound {
        Ok(rebuilt) => Ok(rebuilt),
        Err(error) => {
            log::warn!("iptv data: guide choice saved; cached guide could not be updated: {error}");
            Ok(None)
        }
    }
}

fn repair_publication_in(
    store: &mut Store,
    id: &str,
    keep: u32,
) -> Result<Option<Box<RebuiltSchedule>>, StoreError> {
    let Some(iptv_store_sqlite::RepairPublication {
        commit,
        choices,
        config_changed,
    }) = store.guide_repair_publication(id)?
    else {
        return Err(StoreError::Unavailable("guide repair commit is missing"));
    };
    let revisions = store.guide_revisions(commit.source)?;
    if revisions.mapping != commit.mapping_revision
        || revisions.configuration != commit.configuration_revision
    {
        return Err(StoreError::Unavailable("guide repair was superseded"));
    }
    if config_changed || choices.iter().any(|(_, choice)| choice.is_none()) {
        return Ok(None);
    }
    for (_, choice) in &choices {
        if let Some(value @ EpgOverride::Xmltv { .. }) = choice {
            let Some(feed) = value.feed() else {
                return Ok(None);
            };
            if store
                .inspection_stored_feed_incarnation(commit.source, feed)?
                .is_none()
            {
                return Ok(None);
            }
        }
    }
    let cached = store
        .epg_guide(commit.source)?
        .is_some_and(|guide| guide.replaced_at.is_some());
    let (groups, channels) = catalog_in(store, commit.source)?;
    let overrides = store
        .epg_overrides(commit.source)?
        .into_iter()
        .map(|(key, value)| (ChannelKey::new(key.as_str()), value.to_override()))
        .collect();
    let fingerprint = Matching {
        channels: &channels,
        overrides: &overrides,
    }
    .fingerprint();
    let mut all = true;
    for (channel, choice) in choices {
        let selected = match &choice {
            Some(EpgOverride::Disabled) if !cached => continue,
            Some(EpgOverride::Disabled) => None,
            Some(value @ EpgOverride::Xmltv { epg_channel, .. }) => {
                value.feed().map(|feed| (feed, epg_channel.as_str()))
            }
            None => return Ok(None),
        };
        all &= store.rebind_epg(
            commit.source,
            &ChannelKey::new(channel.as_str()),
            selected,
            fingerprint,
        )?;
    }
    if !all {
        return Ok(None);
    }
    let now = crate::time::now_unix();
    Ok(Some(Box::new(RebuiltSchedule {
        guide: store.load_epg(
            commit.source,
            &held_window(crate::paging::window(now, keep)),
        )?,
        groups,
        channels,
        now,
        source: commit.source,
        resume: false,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_window_reads_from_the_held_day_through_the_future() {
        let window = crate::paging::window(1_800_000_123, 7);
        assert_eq!(held_window(window).from, window.loaded_from);
        assert_eq!(held_window(window).to, UnixTime::MAX);
    }

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
            save_and_rebind(&mut store, source, "synthetic-channel", &Choice::NoGuide, 7)
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
