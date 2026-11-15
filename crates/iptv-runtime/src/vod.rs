//! On-demand catalogue and playback. Credentials and ephemeral provider links stay here.
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

#[derive(Default)]
pub(crate) struct State {
    entries: Mutex<BTreeMap<Key, Entry>>,
    sessions: Mutex<BTreeMap<i64, (SourceConfig, iptv_providers::stalker::StalkerSession)>>,
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
        self.parts
            .borrow()
            .get(&SourceId(source))
            .and_then(|s| s.shared_playback())
            .or_else(|| {
                self.single
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
    pub async fn vod_catalogue(
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
        self.parental_refresh().await.map_err(|_| unavailable())?;
        let mut result = self
            .services()
            .read(move |s| {
                let (cards, more) = s.vod_page(&query, offset, 80)?;
                Ok((cards, more, s.vod_sources()?, s.vod_categories(&query)?))
            })
            .await?;
        for card in &mut result.0 {
            if !self.parental_allowed(&target(&card.key)) {
                card.plot = None;
                card.poster = None;
            }
        }
        Ok(result)
    }
    pub async fn vod_details(&self, key: Key) -> Result<Details, Error> {
        self.parental_check(&target(&key))
            .await
            .map_err(|_| unavailable())?;
        let services = self.services();
        let lookup = key.clone();
        let (source, mut item, locator, provider, embedded, incarnation) = services
            .read(move |s| {
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
                    s.source_incarnation(SourceId(lookup.source))?,
                ))
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
                let client =
                    iptv_net::NetClient::new(crate::settings::net_config(&work.settings()))
                        .map_err(|_| unavailable())?;
                let cancel = iptv_import::CancelToken::new();
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
                            // Detail metadata can fail independently of a usable catalogue stream.
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
                let keys: Vec<_> = episodes.iter().map(|e| e.key.clone()).collect();
                let resume_key = key2.clone();
                let (progress, resume_episode) = work
                    .read(move |s| {
                        let progress = keys
                            .iter()
                            .map(|k| s.vod_progress(k))
                            .collect::<iptv_store_sqlite::Result<Vec<_>>>()?;
                        Ok((progress, s.vod_resume_episode(&resume_key)?))
                    })
                    .await?;
                for (e, p) in episodes.iter_mut().zip(progress) {
                    e.progress = p;
                }
                let mut cache = state
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if cache.len() + entries.len() > 30_000 {
                    cache.clear();
                }
                cache.extend(entries);
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
        if !current {
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
                        source.public_only = false;
                        if let Some(ua) = &c.user_agent {
                            source.default_header("User-Agent", ua);
                        }
                        source
                    }
                    (SourceConfig::Stalker(c), StreamLocator::Stalker { cmd, .. }) => {
                        let client =
                            iptv_net::NetClient::new(crate::settings::net_config(&work.settings()))
                                .map_err(|_| unavailable())?;
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
                    &crate::settings::net_config(&work.settings()).user_agent,
                );
                Ok::<_, Error>(source)
            })
            .await
            .map_err(|_| unavailable())??;
        // Portal authentication and link creation can outlive a playlist edit,
        // disable or deletion. The old catalogue's result is no longer authority.
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
        if !current {
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
    client: &iptv_net::NetClient,
    session: &mut iptv_providers::stalker::StalkerSession,
    id: &str,
    parent_cmd: Option<&str>,
    cancel: &iptv_import::CancelToken,
) -> Result<Vec<(iptv_providers::model::Episode, StreamLocator, Option<u32>)>, Error> {
    let started = std::time::Instant::now();
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

#[cfg(test)]
mod tests {
    use super::*;
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
