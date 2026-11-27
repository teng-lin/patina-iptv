//! Typed EPG repository for guide configuration and read models.

use super::{Services, StoreError};
use iptv_epg::{ChannelKey, GuideStore, UnixTime};
use iptv_library::{Choice, Current, SourceId};
use iptv_providers::ContentId;
use iptv_search::{ChannelId, ProgramIndex, ProgramIndexBuilder};
use iptv_store_sqlite::{EpgOverride, EpgWindow, GuideChannel, GuideSource, Store, StoredGuide};

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
    pub(crate) async fn guide_sources_snapshot(
        self,
        source: SourceId,
    ) -> Result<(Option<StoredGuide>, Vec<GuideSource>), StoreError> {
        self.services
            .read(move |store| Ok((store.epg_guide(source)?, store.guide_sources(source)?)))
            .await
    }

    pub(crate) async fn listing_starts(
        self,
        source: SourceId,
        cutoff: UnixTime,
    ) -> Result<std::collections::HashMap<ChannelKey, UnixTime>, StoreError> {
        self.services
            .read(move |store| store.epg_listing_starts(source, cutoff))
            .await
    }

    pub(crate) async fn clear_guides(
        self,
        sources: Vec<SourceId>,
    ) -> Result<(bool, Vec<crate::imports::Imported>), StoreError> {
        self.services
            .call(move |store| {
                let mut deleted = true;
                let mut imports = Vec::new();
                for source in sources {
                    if let Err(error) = store.delete_epg(source) {
                        log::warn!("iptv data: could not delete stored guide: {error}");
                        deleted = false;
                        continue;
                    }
                    if let Ok((groups, channels)) = crate::stored::catalog_in(store, source) {
                        imports.push(crate::imports::Imported {
                            groups,
                            channels,
                            playback: crate::imports::Streams::Current,
                            guide: GuideStore::default(),
                            now: crate::time::now_unix(),
                            source,
                            resume: false,
                        });
                    }
                }
                Ok((deleted, imports))
            })
            .await
    }

    pub(crate) async fn repair_publication(
        self,
        id: String,
        keep: u32,
        now: UnixTime,
    ) -> Result<Option<Box<crate::imports::Imported>>, StoreError> {
        self.services
            .call(move |store| repair_publication_in(store, &id, keep, now))
            .await
    }

    pub(crate) async fn current(
        self,
        source: SourceId,
        key: String,
    ) -> Result<Current, StoreError> {
        self.services
            .read(move |store| {
                let overlay = store
                    .channel(source, &ContentId::from_stored(&key))?
                    .and_then(|row| row.epg_override);
                let bound = store.epg_binding(source, &ChannelKey::new(&key))?;
                Ok(Current {
                    choice: Some(choice_of(overlay)),
                    bound,
                })
            })
            .await
    }

    pub(crate) async fn guide_channels(
        self,
        source: SourceId,
    ) -> Result<Vec<GuideChannel>, StoreError> {
        self.services
            .read(move |store| store.guide_channels(source))
            .await
    }

    pub(crate) async fn save_choice(
        self,
        source: SourceId,
        key: String,
        choice: Choice,
        keep_past_days: u32,
    ) -> Result<Option<Box<crate::imports::Imported>>, StoreError> {
        self.services
            .call(move |store| {
                let content = ContentId::from_stored(&key);
                let overlay = choice_overlay(&choice);
                store.set_epg_override(source, &content, overlay.as_ref())?;
                let rebound = (|| -> Result<Option<Box<crate::imports::Imported>>, StoreError> {
                    let to = match &choice {
                        Choice::Automatic => return Ok(None),
                        Choice::NoGuide => None,
                        Choice::Channel { feed, epg_channel } => {
                            Some((*feed, epg_channel.clone()))
                        }
                    };
                    let (groups, channels) = crate::stored::catalog_in(store, source)?;
                    let overrides = store
                        .epg_overrides(source)?
                        .into_iter()
                        .map(|(id, epg)| {
                            (ChannelKey::new(id.as_str()), epg.to_override())
                        })
                        .collect();
                    let matching = crate::stored::Matching {
                        channels: &channels,
                        overrides: &overrides,
                    };
                    let fingerprint = matching.fingerprint();
                    let key = ChannelKey::new(&key);
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
                    let epg_window = EpgWindow {
                        from: window.loaded_from,
                        ..EpgWindow::from(0, 0)
                    };
                    Ok(Some(Box::new(crate::imports::Imported {
                        guide: store.load_epg(source, &epg_window)?,
                        groups,
                        channels,
                        playback: crate::imports::Streams::Current,
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
            })
            .await
    }

    pub(crate) async fn guide(self, source: SourceId) -> Result<Option<StoredGuide>, StoreError> {
        self.services
            .read(move |store| store.epg_guide(source))
            .await
    }

    pub(crate) async fn guide_sources(
        self,
        source: SourceId,
    ) -> Result<Vec<GuideSource>, StoreError> {
        self.services
            .read(move |store| store.guide_sources(source))
            .await
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

    pub(crate) async fn page(
        self,
        source: SourceId,
        window: EpgWindow<'static>,
    ) -> Result<GuideStore, StoreError> {
        self.services
            .read(move |store| store.load_epg(source, &window))
            .await
    }

    pub(crate) async fn page_from(
        self,
        source: SourceId,
        window: iptv_guide::pages::PagedWindow,
    ) -> Result<GuideStore, StoreError> {
        let epg_window = EpgWindow {
            from: window.loaded_from,
            ..EpgWindow::from(0, 0)
        };
        self.page(source, epg_window).await
    }

    pub(crate) async fn page_after_writes(
        &self,
        source: SourceId,
        window: iptv_guide::pages::PagedWindow,
    ) -> Result<GuideStore, StoreError> {
        let epg_window = EpgWindow {
            from: window.loaded_from,
            ..EpgWindow::from(0, 0)
        };
        self.services
            .call(move |store| store.load_epg(source, &epg_window))
            .await
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

fn repair_publication_in(
    store: &mut Store,
    id: &str,
    keep: u32,
    now: UnixTime,
) -> Result<Option<Box<crate::imports::Imported>>, StoreError> {
    use crate::imports::Streams;
    use crate::stored::Matching;
    use iptv_epg::ChannelKey;

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
    let (groups, channels) = crate::stored::catalog_in(store, commit.source)?;
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
    Ok(Some(Box::new(crate::imports::Imported {
        guide: store.load_epg(
            commit.source,
            &EpgWindow {
                from: crate::paging::window(now, keep).loaded_from,
                ..EpgWindow::from(0, 0)
            },
        )?,
        groups,
        channels,
        playback: Streams::Current,
        now,
        source: commit.source,
        resume: false,
    })))
}
