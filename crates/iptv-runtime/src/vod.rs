//! On-demand catalog and playback. Credentials and ephemeral provider links stay here.
#[cfg(all(test, feature = "native"))]
mod recovery_tests;
use crate::{Error, Runtime};
use iptv_core::{
    playback::PlaySource,
    vod::{Card, Details, Episode, Key, Kind, Progress, Query},
};
use iptv_library::SourceId;
use iptv_providers::{
    model::{SourceConfig, StreamLocator},
    xtream::XtreamClient,
};
use std::{collections::BTreeMap, sync::Mutex};
#[derive(Clone)]
struct Entry {
    incarnation: String,
    config: SourceConfig,
    locator: StreamLocator,
    parent: Option<Key>,
    episode: Option<u32>,
}

#[cfg(test)]
type PlaybackCompletion = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[derive(Default)]
pub(crate) struct State {
    entries: Mutex<BTreeMap<Key, Entry>>,
    sessions: Mutex<BTreeMap<i64, (SourceConfig, iptv_providers::stalker::StalkerSession)>>,
    /// The discovery read in progress: one shelf is shown at a time, so a
    /// newer read, a closed shelf or a closed application cancels it.
    discovery: Mutex<Option<iptv_import::CancelToken>>,
    #[cfg(test)]
    playback_completion: Mutex<Option<PlaybackCompletion>>,
}
impl State {
    /// A token for a new discovery read, canceling the one before.
    fn begin_discovery(&self) -> iptv_import::CancelToken {
        let token = iptv_import::CancelToken::new();
        if let Some(previous) = self
            .discovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(token.clone())
        {
            previous.cancel();
        }
        token
    }
    pub(crate) fn cancel_discovery(&self) {
        if let Some(token) = self
            .discovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            token.cancel();
        }
    }
}
pub use iptv_core::vod::Playing;
fn session_matches(
    config: &SourceConfig,
    session: &iptv_providers::stalker::StalkerSession,
) -> bool {
    let SourceConfig::Stalker(credentials) = config else {
        return false;
    };
    iptv_providers::stalker::endpoint_candidates(&credentials.portal_url).is_ok_and(|endpoints| {
        endpoints.iter().any(|endpoint| {
            iptv_providers::stalker::StalkerPortal::from_credentials(
                credentials,
                endpoint,
                session.portal().mode(),
            )
            .is_ok_and(|expected| expected == *session.portal())
        })
    })
}
fn unavailable() -> Error {
    Error::unavailable("On-demand content unavailable; refresh the playlist and try again")
}

impl Runtime {
    fn vod_context(
        &self,
        source: i64,
    ) -> Option<std::rc::Rc<iptv_guide::playback::PlaybackContext>> {
        self.catalog
            .schedule_parts()
            .borrow()
            .get(&SourceId(source))
            .and_then(|s| s.shared_playback())
            .or_else(|| {
                self.catalog
                    .single_schedule()
                    .borrow()
                    .as_ref()
                    .filter(|s| s.source.0 == source)
                    .and_then(|s| s.schedule.shared_playback())
            })
    }
    fn vod_session(
        &self,
        source: i64,
        config: &SourceConfig,
    ) -> Option<iptv_providers::stalker::StalkerSession> {
        self.vod_context(source)
            .and_then(|c| c.stalker_session())
            .filter(|session| session_matches(config, session))
            .or_else(|| {
                self.vod
                    .sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&source)
                    .filter(|(c, _)| c == config)
                    .map(|(_, s)| s.clone())
            })
    }
    fn vod_update_session(&self, source: i64) {
        if let Some(context) = self.vod_context(source)
            && let Some((config, session)) = self
                .vod
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&source)
            && context
                .stalker_session()
                .is_some_and(|current| session_matches(config, &current))
        {
            context.update_stalker_session(session.clone());
        }
    }
    pub async fn vod_catalog(
        &self,
        query: Query,
        offset: u32,
    ) -> Result<
        (
            Vec<Card>,
            bool,
            Vec<iptv_core::vod::Source>,
            Vec<iptv_core::vod::Category>,
        ),
        Error,
    > {
        self.vod_catalog_pages(query, offset, 1).await
    }
    /// The catalog from its top, a page at a time until it holds at
    /// least `through` titles (or ends), with its filter choices. A list
    /// read again after the player (its progress changed) keeps the titles
    /// the viewer had loaded, so its focused title is found again by
    /// identity however many pages down it was. The pages come from one
    /// read snapshot, so a write between them cannot shift a title out.
    pub async fn vod_catalog_through(
        &self,
        query: Query,
        through: u32,
    ) -> Result<
        (
            Vec<Card>,
            bool,
            Vec<iptv_core::vod::Source>,
            Vec<iptv_core::vod::Category>,
        ),
        Error,
    > {
        self.vod_catalog_pages(query, 0, through.max(1)).await
    }
    async fn vod_catalog_pages(
        &self,
        query: Query,
        offset: u32,
        at_least: u32,
    ) -> Result<
        (
            Vec<Card>,
            bool,
            Vec<iptv_core::vod::Source>,
            Vec<iptv_core::vod::Category>,
        ),
        Error,
    > {
        let mut result = self
            .services()
            .vod()
            .catalog_pages(query, offset, at_least)
            .await?;
        self.vod_redact_protected(&mut result.0).await?;
        Ok(result)
    }
    /// One bounded page of a cross-source discovery shelf (Continue
    /// watching, Recently added). The page size is clamped by the store.
    ///
    /// Continue watching follows the progress records and Recently added
    /// sorts the enabled library. Both use the query-only analysis lane:
    /// even a large watch history cannot queue guide reads behind a sort.
    /// A newer discovery read, [`Runtime::vod_discovery_cancel`] (the shelf
    /// closed) or closing the application cancels a read still running.
    ///
    /// Protected titles keep their identity and name, as in the catalog,
    /// but lose poster and plot until unlocked; opening or playing one goes
    /// through [`Runtime::vod_details`] and [`Runtime::vod_play`], which
    /// check the parental lock, the source's current incarnation and policy,
    /// and resolve a fresh provider link. No playback URL is part of a card.
    pub fn vod_discovery(
        &self,
        query: iptv_core::vod::DiscoveryQuery,
    ) -> impl std::future::Future<Output = Result<iptv_core::vod::DiscoveryPage, Error>> + '_ {
        let token = self.vod.begin_discovery();
        let services = self.services();
        async move {
            let mut page = services.vod().discovery(query, token).await?;
            self.vod_redact_protected(&mut page.cards).await?;
            Ok(page)
        }
    }
    /// The bounded page of `shelf` holding `key` at its current place, or
    /// the first page when the shelf no longer lists it
    /// ([`iptv_core::vod::DiscoveryQuery::around`]). A reopened shelf
    /// focuses its title again by identity this way, however deep it is.
    /// The place and the page come from one read transaction on the
    /// analysis lane, canceled like the shelf's other pages; a failure is
    /// reported, never mistaken for a title that has gone.
    pub fn vod_discovery_around(
        &self,
        shelf: iptv_core::vod::Shelf,
        key: Key,
    ) -> impl std::future::Future<Output = Result<iptv_core::vod::DiscoveryPage, Error>> + '_ {
        let token = self.vod.begin_discovery();
        let services = self.services();
        async move {
            let mut page = services.vod().discovery_around(shelf, key, token).await?;
            self.vod_redact_protected(&mut page.cards).await?;
            Ok(page)
        }
    }
    /// Whether playlist `source` is still the incarnation `incarnation` (an
    /// opaque value from [`Details::incarnation`]): false once it was
    /// deleted and added again under the same id, or its provider settings
    /// were edited. A retried playback checks this before contacting the
    /// provider, so a stale failure is never rebound to another playlist.
    pub async fn vod_is_incarnation(
        &self,
        source: i64,
        incarnation: String,
    ) -> Result<bool, Error> {
        Ok(self
            .services()
            .read(move |s| {
                Ok(s.source_incarnation(SourceId(source))
                    .is_ok_and(|current| current == incarnation))
            })
            .await?)
    }
    /// The shelf closed: stop its discovery read if one is still running.
    pub fn vod_discovery_cancel(&self) {
        self.vod.cancel_discovery();
    }
    /// Removes poster and plot from every card the parental policy hides
    /// now (its own block or a protected category or source).
    async fn vod_redact_protected(&self, cards: &mut [Card]) -> Result<(), Error> {
        self.parental_refresh_targets(cards.iter().map(|card| target(&card.key)).collect())
            .await
            .map_err(|_| unavailable())?;
        for card in cards.iter_mut() {
            if !self.parental_allowed(&target(&card.key)) {
                card.plot = None;
                card.poster = None;
            }
        }
        Ok(())
    }
    pub async fn vod_details(&self, key: Key) -> Result<Details, Error> {
        self.vod_details_for(key, None, true, iptv_import::CancelToken::new())
            .await
    }
    /// Focused-poster metadata uses the same parental/source checks, without
    /// registering playback locators or displacing an active title's cache.
    pub async fn vod_preview(
        &self,
        key: Key,
        cancel: iptv_import::CancelToken,
    ) -> Result<Details, Error> {
        self.vod_details_for(key, None, false, cancel).await
    }
    /// Retry remains bound to the playlist whose original details failed.
    /// Check its incarnation in the same snapshot that captures credentials,
    /// before any provider request; a replacement cannot inherit this action.
    pub async fn vod_retry_details(&self, key: Key, incarnation: String) -> Result<Details, Error> {
        self.vod_details_for(
            key,
            Some(incarnation),
            true,
            iptv_import::CancelToken::new(),
        )
        .await
    }
    async fn vod_details_for(
        &self,
        key: Key,
        expected: Option<String>,
        retain_playback: bool,
        cancel: iptv_import::CancelToken,
    ) -> Result<Details, Error> {
        if cancel.is_canceled() {
            return Err(unavailable());
        }
        self.parental_check(&target(&key))
            .await
            .map_err(|_| unavailable())?;
        let services = self.services();
        let lookup = key.clone();
        let (source, mut item, locator, provider, embedded, incarnation) = services
            .read(move |s| {
                s.vod_read_snapshot(|s| {
                    let incarnation = s.source_incarnation(SourceId(lookup.source))?;
                    if expected.as_ref().is_some_and(|value| *value != incarnation) {
                        return Err(iptv_store_sqlite::StoreError::Unavailable(
                            "Playlist changed",
                        ));
                    }
                    if !s.source_policy(SourceId(lookup.source))?.enabled {
                        return Err(iptv_store_sqlite::StoreError::Unavailable(
                            "Playlist disabled",
                        ));
                    }
                    Ok((
                        s.source(SourceId(lookup.source))?,
                        s.vod_card(&lookup)?,
                        s.vod_locator(&lookup)?,
                        s.vod_series_provider_id(&lookup)?,
                        s.vod_episode_numbers(&lookup)?,
                        incarnation,
                    ))
                })
            })
            .await?;
        let config = source.ok_or_else(unavailable)?.source.config;
        let mut item = item.take().ok_or_else(unavailable)?;
        let existing_session = self.vod_session(key.source, &config);
        let state = self.vod.clone();
        let work = services.clone();
        let key2 = key.clone();
        let result = services
            .spawn(async move {
                let client = work.network_client().map_err(|_| unavailable())?;
                if cancel.is_canceled() {
                    return Err(unavailable());
                }
                let mut episodes = Vec::new();
                let mut entries = Vec::new();
                let mut movie_locator = locator.map(|(_, l)| l);
                match &config {
                    SourceConfig::Xtream(credentials) => {
                        let xt = XtreamClient::from_credentials(credentials)
                            .map_err(|_| unavailable())?;
                        if key2.kind == Kind::Series {
                            let id = provider
                                .as_deref()
                                .and_then(|id| id.parse().ok())
                                .ok_or_else(unavailable)?;
                            let response = iptv_import::on_demand::detail(
                                &client,
                                xt.series_info(id),
                                &cancel,
                            )
                            .await
                            .map_err(|_| unavailable())?;
                            let details = iptv_providers::xtream::parse_series_info(&response, id)
                                .map_err(|_| unavailable())?;
                            item.plot = details.series.plot.or(item.plot);
                            item.poster = details.series.poster.or(item.poster);
                            item.year = details.series.year.or(item.year);
                            item.rating_x10 = details.series.rating_x10.or(item.rating_x10);
                            for e in details.episodes.items.into_iter().take(20_000) {
                                let key = Key {
                                    source: key2.source,
                                    kind: Kind::Episode,
                                    id: format!("{}:{}", key2.id, e.id.as_str()),
                                };
                                entries.push((
                                    key.clone(),
                                    Entry {
                                        incarnation: incarnation.clone(),
                                        config: config.clone(),
                                        locator: e.stream,
                                        parent: Some(key2.clone()),
                                        episode: None,
                                    },
                                ));
                                episodes.push(Episode {
                                    key,
                                    season: e.season,
                                    number: e.number,
                                    title: e.title,
                                    plot: e.plot,
                                    duration_secs: e.duration_secs,
                                    progress: Progress::default(),
                                });
                            }
                        } else if let Some(StreamLocator::Xtream { stream_id, .. }) = &movie_locator
                        {
                            // Detail metadata can fail independently of a usable catalog stream.
                            if let Ok(response) = iptv_import::on_demand::detail(
                                &client,
                                xt.vod_info(*stream_id),
                                &cancel,
                            )
                            .await
                                && let Ok(details) =
                                    iptv_providers::xtream::parse_vod_info(&response, *stream_id)
                            {
                                item.plot = details.item.plot.or(item.plot);
                                item.poster = details.item.poster.or(item.poster);
                                item.year = details.item.year.or(item.year);
                                item.rating_x10 = details.item.rating_x10.or(item.rating_x10);
                                item.duration_secs =
                                    details.item.duration_secs.or(item.duration_secs);
                                if matches!(&details.item.stream,
StreamLocator::Url{
url,
..}
 if !url.is_empty()) || matches!(
                                    &details.item.stream,
                                    StreamLocator::Xtream {
                                        container_extension: Some(_),
                                        ..
                                    }
                                ) {
                                    movie_locator = Some(details.item.stream)
                                }
                            }
                        }
                    }
                    SourceConfig::Stalker(credentials) if key2.kind == Kind::Series => {
                        let found = if !embedded.is_empty() {
                            let mut found = Vec::new();
                            let cmd = match &movie_locator {
                                Some(StreamLocator::Stalker { cmd, .. }) => Some(cmd.clone()),
                                _ => None,
                            };
                            append_stalker(
                                &mut found,
                                iptv_providers::stalker::SeriesRow {
                                    id: key2.id.clone(),
                                    title: item.name.clone(),
                                    video_id: None,
                                    episode: false,
                                    cmd,
                                    season: false,
                                    number: 1,
                                    episodes: embedded,
                                    plot: None,
                                    duration_secs: None,
                                },
                                1,
                                None,
                            );
                            found
                        } else {
                            let mut session = match existing_session {
                                Some(s) => s,
                                None => iptv_import::on_demand::stalker_session(
                                    &client,
                                    credentials,
                                    &cancel,
                                )
                                .await
                                .map_err(|_| unavailable())?,
                            };
                            let id = provider
                                .clone()
                                .or_else(|| key2.id.rsplit(':').next().map(str::to_owned))
                                .ok_or_else(unavailable)?;
                            let cmd = match &movie_locator {
                                Some(StreamLocator::Stalker { cmd, .. }) if provider.is_none() => {
                                    Some(cmd.clone())
                                }
                                _ => None,
                            };
                            let found = stalker_episodes(
                                &client,
                                &mut session,
                                &id,
                                cmd.as_deref(),
                                &cancel,
                            )
                            .await?;
                            state
                                .sessions
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .insert(key2.source, (config.clone(), session));
                            found
                        };
                        for (e, locator, episode) in found {
                            let key = Key {
                                source: key2.source,
                                kind: Kind::Episode,
                                id: format!("{}:{}", key2.id, e.id.as_str()),
                            };
                            entries.push((
                                key.clone(),
                                Entry {
                                    incarnation: incarnation.clone(),
                                    config: config.clone(),
                                    locator,
                                    parent: Some(key2.clone()),
                                    episode,
                                },
                            ));
                            episodes.push(Episode {
                                key,
                                season: e.season,
                                number: e.number,
                                title: e.title,
                                plot: e.plot,
                                duration_secs: e.duration_secs,
                                progress: Progress::default(),
                            });
                        }
                    }
                    _ => {}
                }
                if let Some(locator) = movie_locator {
                    entries.push((
                        key2.clone(),
                        Entry {
                            incarnation: incarnation.clone(),
                            config,
                            locator,
                            parent: None,
                            episode: None,
                        },
                    ));
                }
                if cancel.is_canceled() {
                    return Err(unavailable());
                }
                let keys: Vec<_> = episodes.iter().map(|e| e.key.clone()).collect();
                let resume_key = key2.clone();
                let (progress, resume_episode) = if retain_playback {
                    work.read(move |s| {
                        let progress = keys
                            .iter()
                            .map(|k| s.vod_progress(k))
                            .collect::<iptv_store_sqlite::Result<Vec<_>>>()?;
                        Ok((progress, s.vod_resume_episode(&resume_key)?))
                    })
                    .await?
                } else {
                    (Vec::new(), None)
                };
                for (e, p) in episodes.iter_mut().zip(progress) {
                    e.progress = p;
                }
                if retain_playback {
                    let mut cache = state
                        .entries
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if cache.len() + entries.len() > 30_000 {
                        cache.clear();
                    }
                    cache.extend(entries);
                }
                Ok::<_, Error>(Details {
                    item,
                    episodes,
                    resume_episode,
                    incarnation,
                })
            })
            .await
            .map_err(|_| unavailable())??;
        self.vod_update_session(key.source);
        self.parental_check(&target(&key))
            .await
            .map_err(|_| unavailable())?;
        let lookup = key.clone();
        let incarnation = result.incarnation.clone();
        let current = self
            .services()
            .read(move |s| {
                Ok(s.source_incarnation(SourceId(lookup.source))
                    .is_ok_and(|v| v == incarnation)
                    && s.source_policy(SourceId(lookup.source))?.enabled)
            })
            .await?;
        // Keep the source read last, then check access without yielding:
        // an expired or revoked grant cannot survive that final store await.
        if !current || !self.parental_allowed(&target(&key)) {
            return Err(unavailable());
        }
        Ok(result)
    }
    pub async fn vod_play(
        &self,
        mut details: Details,
        key: Key,
        restart: bool,
    ) -> Result<(Playing, PlaySource), Error> {
        let entry = self
            .vod
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned()
            .ok_or_else(unavailable)?;
        if let Some(parent) = &entry.parent {
            self.parental_check(&target(parent))
                .await
                .map_err(|_| unavailable())?;
        }
        self.parental_check(&target(&key))
            .await
            .map_err(|_| unavailable())?;
        let expected_item = entry.parent.as_ref().unwrap_or(&key);
        if details.item.key != *expected_item || details.incarnation != entry.incarnation {
            return Err(unavailable());
        }
        let lookup = key.clone();
        let parent = entry.parent.clone();
        let expected = entry.config.clone();
        let incarnation = entry.incarnation.clone();
        let progress = self
            .services()
            .read(move |s| {
                s.vod_read_snapshot(|s| {
                    let valid = s
                        .source_incarnation(SourceId(lookup.source))
                        .is_ok_and(|v| v == incarnation)
                        && s.vod_card(parent.as_ref().unwrap_or(&lookup))?.is_some()
                        && s.source_policy(SourceId(lookup.source))?.enabled
                        && s.source(SourceId(lookup.source))?
                            .is_some_and(|s| s.source.config == expected);
                    if !valid {
                        return Err(iptv_store_sqlite::StoreError::Unavailable(
                            "Playlist changed",
                        ));
                    }
                    s.vod_progress(&lookup)
                })
            })
            .await?;
        if key.kind == Kind::Episode {
            details.resume_episode = Some(key.clone());
        }
        let title = details.episodes.iter().find(|e| e.key == key).map_or_else(
            || details.item.name.clone(),
            |e| format!("{} · {}", details.item.name, e.title),
        );
        let playing = Playing {
            key: key.clone(),
            parent: entry.parent.clone(),
            title,
            progress: if restart {
                Progress::default()
            } else {
                progress
            },
            details,
            last_saved_ms: None,
            ended: false,
        };
        let services = self.services();
        let work = services.clone();
        let existing_session = self.vod_session(key.source, &entry.config);
        let state = self.vod.clone();
        let playing_source = key.source;
        let expected_config = entry.config.clone();
        let expected_incarnation = entry.incarnation.clone();
        let source = services
            .spawn(async move {
                let mut source = match (&entry.config, &entry.locator) {
                    (_, StreamLocator::Url { url, headers, .. }) => {
                        PlaySource::new(url).with_headers(headers)
                    }
                    (
                        SourceConfig::Xtream(c),
                        StreamLocator::Xtream {
                            stream_id,
                            container_extension,
                        },
                    ) => {
                        let xt = XtreamClient::from_credentials(c).map_err(|_| unavailable())?;
                        let url = if key.kind == Kind::Episode {
                            xt.episode_url(*stream_id, container_extension.as_deref())
                        } else {
                            xt.movie_url(*stream_id, container_extension.as_deref())
                        }
                        .ok_or_else(unavailable)?;
                        let mut source = PlaySource::new(url);
                        source.public_only = xt.stream_must_be_public(&source.url);
                        source
                    }
                    (SourceConfig::Stalker(c), StreamLocator::Stalker { cmd, .. }) => {
                        let client = work.network_client().map_err(|_| unavailable())?;
                        let cancel = iptv_import::CancelToken::new();
                        let mut session = match existing_session {
                            Some(s) => s,
                            None => iptv_import::on_demand::stalker_session(&client, c, &cancel)
                                .await
                                .map_err(|_| unavailable())?,
                        };
                        let direct = if entry.episode.is_none() {
                            match session.resolve_stream(&entry.locator, true) {
                                Ok(iptv_providers::stalker::Resolve::Play(url)) => Some(url),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        let url = match direct {
                            Some(url) => url,
                            None => iptv_import::on_demand::stalker_episode_link(
                                &client,
                                &mut session,
                                cmd,
                                entry.episode,
                                &cancel,
                            )
                            .await
                            .map_err(|_| unavailable())?,
                        };
                        let source = iptv_guide::playback::StalkerLink::source(&session, url, &[]);
                        state
                            .sessions
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(key.source, (entry.config.clone(), session));
                        source
                    }
                    _ => return Err(unavailable()),
                };
                if let SourceConfig::Xtream(c) = &entry.config
                    && let Some(ua) = &c.user_agent
                {
                    source.default_header("User-Agent", ua);
                }
                if let SourceConfig::M3u(c) = &entry.config {
                    if let Some(ua) = &c.user_agent {
                        source.default_header("User-Agent", ua);
                    }
                    if let Some(r) = &c.referrer {
                        source.default_header("Referer", r);
                    }
                }
                source.default_header(
                    "User-Agent",
                    &crate::settings::network_config(&work.settings()).user_agent,
                );
                Ok::<_, Error>(source)
            })
            .await
            .map_err(|_| unavailable())??;
        // Portal authentication and link creation can outlive a playlist edit,
        // disable or deletion. The old catalog's result is no longer authority.
        if let Some(parent) = &playing.parent {
            self.parental_check(&target(parent))
                .await
                .map_err(|_| unavailable())?;
        }
        self.parental_check(&target(&playing.key))
            .await
            .map_err(|_| unavailable())?;
        let lookup = playing.key.clone();
        let parent = playing.parent.clone();
        let current = self
            .services()
            .read(move |s| {
                Ok(s.source_incarnation(SourceId(lookup.source))
                    .is_ok_and(|v| v == expected_incarnation)
                    && s.source_policy(SourceId(lookup.source))?.enabled
                    && s.vod_card(parent.as_ref().unwrap_or(&lookup))?.is_some()
                    && s.source(SourceId(lookup.source))?
                        .is_some_and(|s| s.source.config == expected_config))
            })
            .await?;
        #[cfg(test)]
        {
            let completion = self.vod.playback_completion.lock().unwrap().take();
            if let Some((arrived, release)) = completion {
                let _ = arrived.send(());
                let _ = release.await;
            }
        }
        if !current
            || !services.is_current()
            || !self.parental_allowed(&target(&playing.key))
            || playing
                .parent
                .as_ref()
                .is_some_and(|parent| !self.parental_allowed(&target(parent)))
        {
            return Err(unavailable());
        }
        self.vod_update_session(playing_source);
        Ok((playing, source))
    }
    pub fn queue_vod_progress(
        &self,
        key: Key,
        parent: Option<Key>,
        progress: Progress,
        incarnation: String,
    ) {
        let _queued = self
            .services()
            .call(move |s| save_progress(s, key, parent, progress, incarnation));
    }
    pub async fn vod_save_progress(
        &self,
        key: Key,
        parent: Option<Key>,
        progress: Progress,
        incarnation: String,
    ) -> Result<(), Error> {
        Ok(self
            .services()
            .call(move |s| save_progress(s, key, parent, progress, incarnation))
            .await?)
    }
}

fn save_progress(
    store: &mut iptv_store_sqlite::Store,
    key: Key,
    parent: Option<Key>,
    progress: Progress,
    incarnation: String,
) -> iptv_store_sqlite::Result<()> {
    let valid = store
        .source_incarnation(SourceId(key.source))
        .is_ok_and(|v| v == incarnation)
        && store.source_policy(SourceId(key.source))?.enabled
        && parent
            .as_ref()
            .is_none_or(|p| p.source == key.source && p.kind == Kind::Series)
        && (key.kind != Kind::Episode || parent.is_some())
        && store.vod_card(parent.as_ref().unwrap_or(&key))?.is_some();
    if !valid {
        return Err(iptv_store_sqlite::StoreError::Unavailable(
            "Playlist changed",
        ));
    }
    store.save_vod_progress(&key, parent.as_ref().map(|p| p.id.as_str()), progress)
}

async fn stalker_episodes(
    client: &crate::network::Client,
    session: &mut iptv_providers::stalker::StalkerSession,
    id: &str,
    parent_cmd: Option<&str>,
    cancel: &iptv_import::CancelToken,
) -> Result<Vec<(iptv_providers::model::Episode, StreamLocator, Option<u32>)>, Error> {
    let started = crate::time::Instant::now();
    let mut requests = 0;
    let mut out = Vec::new();
    let native = parent_cmd.is_none();
    for page in 1..=100 {
        requests += 1;
        if requests > 200 || started.elapsed() > std::time::Duration::from_secs(30) {
            return Err(unavailable());
        }
        let response = iptv_import::on_demand::stalker_detail(
            client,
            session,
            |s| s.series_page(id, None, page, native),
            cancel,
        )
        .await
        .map_err(|_| unavailable())?;
        let rows =
            iptv_providers::stalker::parse_series_rows(&response).map_err(|_| unavailable())?;
        if rows.is_empty() {
            break;
        }
        let mut changed = false;
        for row in rows {
            if !row.episodes.is_empty() {
                let before = out.len();
                let season = if native { row.number } else { 1 };
                append_stalker(&mut out, row, season, parent_cmd);
                changed |= out.len() > before;
            } else if row.season {
                let before_season = out.len();
                for ep_page in 1..=100 {
                    requests += 1;
                    if requests > 200 || started.elapsed() > std::time::Duration::from_secs(30) {
                        return Err(unavailable());
                    }
                    let response = iptv_import::on_demand::stalker_detail(
                        client,
                        session,
                        |s| {
                            s.series_page(
                                row.video_id.as_deref().unwrap_or(id),
                                Some(&row.id),
                                ep_page,
                                native,
                            )
                        },
                        cancel,
                    )
                    .await
                    .map_err(|_| unavailable())?;
                    let episodes = iptv_providers::stalker::parse_series_rows(&response)
                        .map_err(|_| unavailable())?;
                    if episodes.is_empty() {
                        break;
                    }
                    let before = out.len();
                    for ep in episodes {
                        append_stalker(&mut out, ep, row.number, parent_cmd);
                    }
                    if out.len() == before {
                        break;
                    }
                }
                changed |= out.len() > before_season;
            } else {
                let before = out.len();
                append_stalker(&mut out, row, 1, parent_cmd);
                changed |= out.len() > before;
            }
        }
        if out.len() > 20_000 {
            return Err(unavailable());
        }
        if !changed {
            break;
        }
    }
    out.sort_by_key(|(e, _, _)| (e.season, e.number));
    Ok(out)
}

fn append_stalker(
    out: &mut Vec<(iptv_providers::model::Episode, StreamLocator, Option<u32>)>,
    row: iptv_providers::stalker::SeriesRow,
    season: u32,
    parent_cmd: Option<&str>,
) {
    if row.episodes.is_empty() && (row.season || !row.episode) {
        return;
    }
    let numbers = if row.episodes.is_empty() {
        vec![row.number]
    } else {
        row.episodes.clone()
    };
    for number in numbers {
        let Some(cmd) = row.cmd.as_deref().or(parent_cmd) else {
            continue;
        };
        let id = iptv_providers::ContentId::from_stored(format!(
            "st:episode:{season}:{}:{number}",
            row.id
        ));
        if out.iter().any(|(e, _, _)| e.id == id) {
            continue;
        }
        let locator = StreamLocator::Stalker {
            cmd: cmd.into(),
            link: Default::default(),
        };
        let title = if row.episodes.is_empty() {
            row.title.clone()
        } else {
            format!("{} · {number}", row.title)
        };
        let e = iptv_providers::model::Episode {
            id,
            provider_id: Some(row.id.clone()),
            season,
            number,
            title,
            plot: row.plot.clone(),
            image: None,
            duration_secs: row.duration_secs,
            stream: locator.clone(),
        };
        out.push((e, locator, Some(number)));
    }
}

pub fn target(key: &Key) -> iptv_core::parental::Target {
    use iptv_core::parental::ContentKind;
    iptv_core::parental::Target {
        source: key.source,
        id: key.id.clone(),
        kind: match key.kind {
            Kind::Movie => ContentKind::Movie,
            Kind::Series => ContentKind::Series,
            Kind::Episode => ContentKind::Episode,
        },
    }
}

impl Runtime {
    /// Derives the parental authorization target of an on-demand identity.
    pub fn vod_target(key: &Key) -> iptv_core::parental::Target {
        target(key)
    }
}

/// Opaque authority captured for one already-resolved on-demand selection.
/// It retains the original credentials/incarnation rather than rebinding an
/// edited source while a native handoff is pending.
#[derive(Clone)]
pub struct PublicationAuthority {
    key: Key,
    parent: Option<Key>,
    incarnation: String,
    config: SourceConfig,
}
impl Runtime {
    pub fn vod_publication_authority(
        &self,
        playing: &Playing,
    ) -> Result<PublicationAuthority, Error> {
        let entries = self
            .vod
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries
            .get(&playing.key)
            .filter(|entry| {
                entry.incarnation == playing.details.incarnation && entry.parent == playing.parent
            })
            .ok_or_else(unavailable)?;
        Ok(PublicationAuthority {
            key: playing.key.clone(),
            parent: entry.parent.clone(),
            incarnation: entry.incarnation.clone(),
            config: entry.config.clone(),
        })
    }
    pub fn validate_vod_publication(
        &self,
        authority: PublicationAuthority,
    ) -> impl std::future::Future<Output = bool> + Send + use<> {
        let services = self.services();
        async move {
            services
                .read(move |store| {
                    store.vod_read_snapshot(|store| {
                        let source = SourceId(authority.key.source);
                        Ok(store
                            .source_incarnation(source)
                            .is_ok_and(|incarnation| incarnation == authority.incarnation)
                            && store.source_policy(source)?.enabled
                            && store
                                .source(source)?
                                .is_some_and(|source| source.source.config == authority.config)
                            && store
                                .vod_card(authority.parent.as_ref().unwrap_or(&authority.key))?
                                .is_some())
                    })
                })
                .await
                .unwrap_or(false)
        }
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn playback_rechecks_movie_and_episode_parent_grants_after_the_final_read() {
        use iptv_core::parental::Pin;
        use iptv_providers::{
            ContentId, SecretUrl,
            model::{M3uSourceConfig, Source, VodItem},
        };
        for episode in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let owner = Runtime::new(crate::Ports::new(directory.path().to_owned(), |_| {
                Ok(crate::model::DatabaseKey::from_bytes([0x68; 32]))
            }));
            owner.start_services();
            let config = SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            });
            let stored = config.clone();
            let source = owner
                .services()
                .call(move |store| {
                    let source = store.add_source(&Source {
                        name: "Generated completion fence".into(),
                        config: stored,
                    })?;
                    let item = VodItem {
                        id: ContentId::from_stored("generated-parent"),
                        provider_id: None,
                        name: "Generated title".into(),
                        groups: vec![],
                        poster: None,
                        plot: None,
                        year: None,
                        rating_x10: None,
                        added: None,
                        duration_secs: None,
                        stream: StreamLocator::Url {
                            url: "https://example.test/generated.mp4".into(),
                            headers: vec![],
                            properties: vec![],
                        },
                        is_series: episode,
                        episode_numbers: vec![],
                    };
                    if episode {
                        let series = iptv_providers::model::Series {
                            id: item.id,
                            provider_id: item.provider_id,
                            name: item.name,
                            groups: item.groups,
                            poster: item.poster,
                            plot: item.plot,
                            year: item.year,
                            rating_x10: item.rating_x10,
                            last_modified: None,
                        };
                        store.replace_catalog(source, &[], &[], &[], &[series])?;
                    } else {
                        store.replace_catalog(source, &[], &[], &[item], &[])?;
                    }
                    Ok(source)
                })
                .await
                .unwrap();
            let parent = Key {
                source: source.0,
                kind: if episode { Kind::Series } else { Kind::Movie },
                id: "generated-parent".into(),
            };
            let mut settings = (*owner.settings()).clone();
            settings
                .set("parental.vod_enabled", iptv_settings::Value::Bool(true))
                .unwrap();
            *owner.services().shared_settings().write().unwrap() =
                Some(std::sync::Arc::new(settings));
            owner
                .parental_change_pin(None, Pin::parse("2468").unwrap())
                .await
                .unwrap();
            let protected = target(&parent);
            owner
                .parental_set_blocked(protected.clone(), true)
                .await
                .unwrap();
            owner
                .parental_authorize(protected, Pin::parse("2468").unwrap())
                .await
                .unwrap();
            let details = owner.vod_details(parent.clone()).await.unwrap();
            let key = if episode {
                let key = Key {
                    kind: Kind::Episode,
                    id: "generated-episode".into(),
                    ..parent.clone()
                };
                let incarnation = details.incarnation.clone();
                owner.vod.entries.lock().unwrap().insert(
                    key.clone(),
                    Entry {
                        incarnation,
                        config: config.clone(),
                        locator: StreamLocator::Url {
                            url: "https://example.test/generated-episode.mp4".into(),
                            headers: vec![],
                            properties: vec![],
                        },
                        parent: Some(parent),
                        episode: None,
                    },
                );
                key
            } else {
                parent
            };
            let (arrived, waiting) = tokio::sync::oneshot::channel();
            let (release, held) = tokio::sync::oneshot::channel();
            *owner.vod.playback_completion.lock().unwrap() = Some((arrived, held));
            let playing = owner.vod_play(details, key, false);
            tokio::pin!(playing);
            tokio::select! {
                result = &mut playing => panic!("playback passed its final completion hold: {}", result.is_ok()),
                arrived = tokio::time::timeout(std::time::Duration::from_secs(5), waiting) => arrived.unwrap().unwrap(),
            }
            owner.parental_lock();
            release.send(()).unwrap();
            assert!(
                playing.await.is_err(),
                "revocation must fence the final movie or parent capability"
            );
            assert!(owner.drain_services());
        }
    }

    #[test]
    fn catalog_authorization_only_loads_the_returned_movie_identities() {
        use iptv_providers::{
            HttpResponse, SecretUrl,
            model::{M3uSourceConfig, Source},
        };
        use std::rc::Rc;
        let directory = tempfile::tempdir().unwrap();
        let database_key = crate::model::DatabaseKey::from_bytes([0x76; 32]);
        let mut store = iptv_store_sqlite::Store::open_encrypted(
            directory.path().join("iptv.sqlite"),
            &database_key,
        )
        .unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated bounded catalog".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let mut playlist = String::from("#EXTM3U\n");
        for channel in 0..5000 {
            playlist.push_str(&format!("#EXTINF:-1 group-title=\"Generated live\",Generated channel {channel}\nhttps://example.test/generated/{channel}.ts\n"));
        }
        let parsed = iptv_providers::m3u::parse_m3u(&playlist);
        let movies=iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
            r#"[{"stream_id":1,"name":"Generated movie one"},{"stream_id":2,"name":"Generated movie two"}]"#,
        )).unwrap();
        store
            .replace_catalog(
                source,
                &parsed.groups,
                &parsed.channels.items,
                &movies.items,
                &[],
            )
            .unwrap();
        let live =
            iptv_core::parental::Target::live(source.0, parsed.channels.items[0].id.as_str());
        drop(store);
        let mut settings = iptv_settings::Settings::new();
        settings
            .set("parental.live_enabled", iptv_settings::Value::Bool(true))
            .unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            settings.to_json_string(),
        )
        .unwrap();
        let owner = Rc::new(Runtime::new(crate::Ports::new(
            directory.path().to_owned(),
            move |_| Ok(database_key.clone()),
        )));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        crate::settings::load(&owner);
        let (cards, _, _, _) = owner
            .services()
            .block_on(owner.vod_catalog(Query::default(), 0))
            .unwrap();
        assert_eq!(cards.len(), 2);
        assert!(
            !owner.parental_allowed(&live),
            "a VOD page must not populate unrelated live-channel ancestry"
        );
        assert!(owner.drain_services());
    }
    /// Holds the analysis lane, then checks that `read` (a discovery page
    /// or window) waits behind it while the guide's reader still answers,
    /// and that closing the shelf cancels it before it runs.
    fn waits_off_the_guide_reader(
        owner: &Runtime,
        read: impl std::future::Future<Output = Result<iptv_core::vod::DiscoveryPage, Error>>,
    ) {
        use std::{
            task::{Context, Waker},
            time::Duration,
        };
        // A long analysis (another sort, an inspection) holds its lane.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (started, running) = std::sync::mpsc::channel::<()>();
        let _occupied = owner.services().analyze(move |_| {
            let _ = started.send(());
            let _ = held.recv_timeout(Duration::from_secs(20));
            Ok(())
        });
        running.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut read = std::pin::pin!(read);
        let mut context = Context::from_waker(Waker::noop());
        assert!(
            std::future::Future::poll(read.as_mut(), &mut context).is_pending(),
            "the read waits behind the analysis lane"
        );
        // The guide's reader is not queued behind it.
        let sources = owner
            .services()
            .block_on(owner.services().read(|s| s.vod_sources()))
            .unwrap();
        assert_eq!(sources.len(), 1);
        // The shelf closes before the read ran: it is canceled, not run.
        owner.vod_discovery_cancel();
        release.send(()).unwrap();
        assert!(owner.services().block_on(read).is_err());
    }
    /// Both shelves can be expensive (a large library to sort, a long
    /// watch history to rank): their pages and their windows around a
    /// focused title wait on the analysis lane, so the reader that serves
    /// guide pages stays free, and closing the shelf cancels a read that
    /// has not finished.
    #[test]
    fn discovery_leaves_the_guide_reader_free_and_is_canceled_with_its_shelf() {
        use iptv_core::vod::{DiscoveryQuery, Shelf};
        use iptv_providers::{
            HttpResponse, SecretUrl,
            model::{M3uSourceConfig, Source},
        };
        use std::rc::Rc;
        let directory = tempfile::tempdir().unwrap();
        let database_key = crate::model::DatabaseKey::from_bytes([0x72; 32]);
        let mut store = iptv_store_sqlite::Store::open_encrypted(
            directory.path().join("iptv.sqlite"),
            &database_key,
        )
        .unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated discovery playlist".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://example.test/generated-discovery.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
            r#"[{"stream_id":1,"name":"Generated newer","added":"1700000200"},{"stream_id":2,"name":"Generated older","added":"1700000100"}]"#,
        ))
        .unwrap();
        store
            .replace_catalog(source, &[], &[], &movies.items, &[])
            .unwrap();
        let older = Key {
            source: source.0,
            kind: Kind::Movie,
            id: movies.items[1].id.to_string(),
        };
        store
            .save_vod_progress(
                &older,
                None,
                Progress::observed(30_000, Some(120_000), false),
            )
            .unwrap();
        drop(store);
        let owner = Rc::new(Runtime::new(crate::Ports::new(
            directory.path().to_owned(),
            move |_| Ok(database_key.clone()),
        )));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        crate::settings::load(&owner);
        let names = |page: iptv_core::vod::DiscoveryPage| {
            page.cards.into_iter().map(|c| c.name).collect::<Vec<_>>()
        };
        for (shelf, expected) in [
            (Shelf::ContinueWatching, vec!["Generated older"]),
            (
                Shelf::RecentlyAdded,
                vec!["Generated newer", "Generated older"],
            ),
        ] {
            let first = DiscoveryQuery::first(shelf);
            let page = owner
                .services()
                .block_on(owner.vod_discovery(first))
                .unwrap();
            assert_eq!(names(page), expected, "{shelf:?}");
            waits_off_the_guide_reader(&owner, owner.vod_discovery(first));
            waits_off_the_guide_reader(&owner, owner.vod_discovery_around(shelf, older.clone()));
            // Closing before a host polls the future must not start a new,
            // uncanceled query when the queued task eventually runs.
            let pending = owner.vod_discovery(first);
            owner.vod_discovery_cancel();
            assert!(owner.services().block_on(pending).is_err(), "{shelf:?}");
            let pending = owner.vod_discovery_around(shelf, older.clone());
            owner.vod_discovery_cancel();
            assert!(owner.services().block_on(pending).is_err(), "{shelf:?}");
            // A later shelf reads normally, its window around the title too.
            let window = owner
                .services()
                .block_on(owner.vod_discovery_around(shelf, older.clone()))
                .unwrap();
            assert_eq!(window.start, 0);
            assert!(window.cards.iter().any(|c| c.key == older), "{shelf:?}");
            assert_eq!(
                names(
                    owner
                        .services()
                        .block_on(owner.vod_discovery(first))
                        .unwrap()
                ),
                expected
            );
        }
        assert!(owner.drain_services());
    }
    #[test]
    fn dropping_the_owner_cancels_abandoned_discovery_before_its_fallback_drain() {
        use std::sync::mpsc;
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let key = crate::model::DatabaseKey::from_bytes([0x75; 32]);
        let owner = Runtime::new(crate::Ports::new(directory.path().into(), move |_| {
            Ok(key.clone())
        }));
        owner.start_services();
        let services = owner.services();
        // Make the writer and its shutdown barrier real, not an unopened lane.
        services.block_on(services.call(|_| Ok(()))).unwrap();
        let cancel = owner.vod.begin_discovery();
        let (started, running) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let waiting = services.clone();
        let abandoned = services.analyze(move |_| {
            started.send(()).unwrap();
            // An abandoned query still occupies the real analysis worker.
            // Owner closure wakes it at the start of fallback draining; the
            // query cancellation must already have happened by that boundary.
            let closed = waiting.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), waiting.closed()).await
            });
            finished
                .send(closed.is_ok() && cancel.is_canceled())
                .unwrap();
            Ok(())
        });
        running.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(abandoned);
        drop(owner);
        assert!(
            result.recv_timeout(Duration::from_secs(10)).unwrap(),
            "owner disposal must cancel discovery before awaiting its analysis barrier"
        );
        assert!(!services.is_current());
    }

    #[test]
    fn retry_cannot_contact_credentials_changed_after_its_preflight() {
        use iptv_providers::{
            HttpResponse, Secret,
            model::{Source, XtreamCredentials},
        };
        use std::rc::Rc;
        use std::{
            io::{Read, Write},
            net::TcpListener,
            time::{Duration, Instant},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let database_key = crate::model::DatabaseKey::from_bytes([0x72; 32]);
        let mut store = iptv_store_sqlite::Store::open_encrypted(
            directory.path().join("iptv.sqlite"),
            &database_key,
        )
        .unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated retry authority".into(),
                config: SourceConfig::Xtream(XtreamCredentials {
                    server_url: format!("http://{address}"),
                    username: Secret::new("generated-original"),
                    password: Secret::new("generated-password"),
                    user_agent: None,
                }),
            })
            .unwrap();
        let series = iptv_providers::xtream::parse_series_list(&HttpResponse::ok(
            r#"[{"series_id":41,"name":"Generated retry series"}]"#,
        ))
        .unwrap();
        store
            .replace_catalog(source, &[], &[], &[], &series.items)
            .unwrap();
        let incarnation = store.source_incarnation(source).unwrap();
        let key = Key {
            source: source.0,
            kind: Kind::Series,
            id: series.items[0].id.to_string(),
        };
        drop(store);
        let owner = Rc::new(Runtime::new(crate::Ports::new(
            directory.path().to_owned(),
            move |_| Ok(database_key.clone()),
        )));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        assert!(
            owner
                .services()
                .block_on(owner.vod_is_incarnation(source.0, incarnation.clone()))
                .unwrap()
        );
        // The edit commits after the UI preflight but before its details lookup.
        owner
            .services()
            .block_on(owner.services().call(move |store| {
                let mut changed = store.source(source)?.unwrap().source;
                let SourceConfig::Xtream(credentials) = &mut changed.config else {
                    unreachable!()
                };
                credentials.username = Secret::new("generated-replacement");
                store.update_source(source, &changed)
            }))
            .unwrap();
        assert!(
            owner
                .services()
                .block_on(owner.vod_retry_details(key.clone(), incarnation))
                .is_err()
        );
        assert!(
            matches!(listener.accept(), Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock),
            "a stale retry must not even connect to the replacement provider"
        );
        // Prove the replacement fixture is reachable for an explicit fresh selection.
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "fresh selection did not contact fixture"
                        );
                        std::thread::park_timeout(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0 && request.len() + count < 16_384);
                request.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.contains("username=generated-replacement"));
            assert!(request.contains("action=get_series_info"));
            let body = r#"{"info":{"name":"Generated retry series"},"episodes":{}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let selected = owner.services().block_on(owner.vod_details(key));
        server.join().unwrap();
        assert!(
            selected.is_ok(),
            "an explicit fresh selection remains usable"
        );
        assert!(owner.drain_services());
    }
    #[test]
    fn a_portal_link_cannot_survive_a_concurrent_source_change() {
        use iptv_providers::{
            ContentId,
            model::{MacAddress, Source, StalkerCredentials, VodItem},
            stalker::{PortalMode, StalkerPortal, StalkerSession},
        };
        use std::{
            io::{Read, Write},
            net::TcpListener,
            rc::Rc,
        };

        for change in ["unchanged", "disable", "credentials", "delete"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("iptv.sqlite");
            let database_key = crate::model::DatabaseKey::from_bytes([0x74; 32]);
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let credentials = StalkerCredentials {
                portal_url: format!("http://{address}/c/"),
                mac: MacAddress::parse("00:1A:79:00:00:74").unwrap(),
                serial: None,
                device_id: None,
                device_id2: None,
                signature: None,
                signature2: None,
                login: None,
                password: None,
            };
            let config = SourceConfig::Stalker(credentials.clone());
            let mut store = iptv_store_sqlite::Store::open_encrypted(&path, &database_key).unwrap();
            let source = store
                .add_source(&Source {
                    name: "Generated delayed portal".into(),
                    config: config.clone(),
                })
                .unwrap();
            let movie = VodItem {
                id: ContentId::from_stored("generated-movie"),
                provider_id: Some("74".into()),
                name: "Generated delayed film".into(),
                groups: vec![],
                poster: None,
                plot: None,
                year: None,
                rating_x10: None,
                added: None,
                duration_secs: None,
                stream: StreamLocator::Stalker {
                    cmd: "ffmpeg /media/generated-movie".into(),
                    link: Default::default(),
                },
                is_series: false,
                episode_numbers: vec![],
            };
            store
                .replace_catalog(source, &[], &[], &[movie], &[])
                .unwrap();
            drop(store);
            let key = Key {
                source: source.0,
                kind: Kind::Movie,
                id: "generated-movie".into(),
            };
            let owner = Rc::new(Runtime::new(crate::Ports::new(
                directory.path().to_owned(),
                {
                    let database_key = database_key.clone();
                    move |_| Ok(database_key.clone())
                },
            )));
            owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
            owner.start_services();
            let portal = StalkerPortal::from_credentials(
                &credentials,
                &iptv_providers::stalker::endpoint_candidates(&credentials.portal_url).unwrap()[0],
                PortalMode::Simple,
            )
            .unwrap();
            owner
                .vod
                .sessions
                .lock()
                .unwrap()
                .insert(source.0, (config, StalkerSession::new(portal)));
            let details = owner.services().block_on(owner.vod_details(key.clone()));
            let details = details.unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() < 16_384);
                }
                assert!(
                    String::from_utf8(request)
                        .unwrap()
                        .contains("action=create_link")
                );
                let mut store =
                    iptv_store_sqlite::Store::open_encrypted(path, &database_key).unwrap();
                match change {
                    "disable" => {
                        let mut policy = store.source_policy(source).unwrap();
                        policy.enabled = false;
                        store.set_source_policy(source, policy).unwrap();
                    }
                    "credentials" => {
                        let mut source_data = store.source(source).unwrap().unwrap().source;
                        let SourceConfig::Stalker(credentials) = &mut source_data.config else {
                            unreachable!()
                        };
                        credentials.mac = MacAddress::parse("00:1A:79:00:00:75").unwrap();
                        store.update_source(source, &source_data).unwrap();
                    }
                    "delete" => {
                        store.delete_source(source).unwrap();
                    }
                    "unchanged" => {}
                    _ => unreachable!(),
                }
                let body =
                    format!(r#"{{"js":{{"cmd":"ffmpeg http://{address}/media/generated.ts"}}}}"#);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let result = owner
                .services()
                .block_on(owner.vod_play(details, key, false));
            server.join().unwrap();
            assert_eq!(
                result.is_err(),
                change != "unchanged",
                "source change during link resolution: {change}"
            );
            assert!(owner.drain_services());
        }
    }
    #[test]
    fn native_seasons_keep_opaque_file_commands_and_episode_coordinates() {
        let response = iptv_providers::HttpResponse::ok(
            r#"{"js":[{"id":"file-season-3","name":"Season 3","is_season":true,"cmd":"ffmpeg /media/generated-file","series":[2,4]}]}"#,
        );
        let row = iptv_providers::stalker::parse_series_rows(&response)
            .unwrap()
            .remove(0);
        let mut out = Vec::new();
        let season = row.number;
        append_stalker(&mut out, row, season, None);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0.season, 3);
        assert_eq!(out[0].0.number, 2);
        assert_eq!(out[1].2, Some(4));
        assert!(matches!(&out[1].1,
StreamLocator::Stalker{
cmd,
..}
 if cmd=="ffmpeg /media/generated-file"));
        assert_ne!(out[0].0.id, out[1].0.id);
    }
    #[test]
    fn unrecognized_lazy_rows_never_play_the_parent_command() {
        let response = iptv_providers::HttpResponse::ok(
            r#"{"js":{"data":[{"id":"unknown","name":"Generated non-episode"}]}}"#,
        );
        let row = iptv_providers::stalker::parse_series_rows(&response)
            .unwrap()
            .remove(0);
        let mut out = Vec::new();
        append_stalker(&mut out, row, 2, Some("/media/generated-parent"));
        assert!(out.is_empty());
    }
    #[test]
    fn imported_sessions_never_cross_changed_provider_credentials() {
        use iptv_providers::{
            model::{MacAddress, StalkerCredentials},
            stalker::{PortalMode, StalkerPortal, StalkerSession},
        };
        let mut credentials = StalkerCredentials {
            portal_url: "http://portal.example.test/c/".into(),
            mac: MacAddress::parse("00:1A:79:00:00:01").unwrap(),
            serial: None,
            device_id: None,
            device_id2: None,
            signature: None,
            signature2: None,
            login: None,
            password: None,
        };
        let portal = StalkerPortal::from_credentials(
            &credentials,
            "http://portal.example.test/portal.php",
            PortalMode::Simple,
        )
        .unwrap();
        let session = StalkerSession::new(portal);
        assert!(session_matches(
            &SourceConfig::Stalker(credentials.clone()),
            &session
        ));
        credentials.mac = MacAddress::parse("00:1A:79:00:00:02").unwrap();
        assert!(!session_matches(
            &SourceConfig::Stalker(credentials.clone()),
            &session
        ));
        credentials.mac = MacAddress::parse("00:1A:79:00:00:01").unwrap();
        credentials.portal_url = "http://other.example.test/c/".into();
        assert!(!session_matches(
            &SourceConfig::Stalker(credentials),
            &session
        ));
    }
}
