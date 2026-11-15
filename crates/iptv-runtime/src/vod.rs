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
    config: SourceConfig,
    locator: StreamLocator,
    parent: Option<Key>,
    episode: Option<u32>,
}
#[derive(Default)]
pub(crate) struct State {
    entries: Mutex<BTreeMap<Key, Entry>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playing {
    pub key: Key,
    pub parent: Option<Key>,
    pub title: String,
    pub progress: Progress,
    pub details: Details,
    pub last_saved_ms: Option<u64>,
    pub ended: bool,
}
fn unavailable() -> Error {
    Error::unavailable("On-demand content unavailable; refresh the playlist and try again")
}
impl Runtime {
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
        Ok(self
            .services()
            .read(move |s| {
                let (cards, more) = s.vod_page(&query, offset, 80)?;
                Ok((cards, more, s.vod_sources()?, s.vod_categories(&query)?))
            })
            .await?)
    }
    pub async fn vod_details(&self, key: Key) -> Result<Details, Error> {
        let services = self.services();
        let lookup = key.clone();
        let (source, mut item, locator, provider) = services
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
                ))
            })
            .await?;
        let config = source.ok_or_else(unavailable)?.source.config;
        let mut item = item.take().ok_or_else(unavailable)?;
        let state = self.vod.clone();
        let work = services.clone();
        let key2 = key.clone();
        let result=services.spawn(async move {
            let client=iptv_net::NetClient::new(crate::settings::net_config(&work.settings())).map_err(|_|unavailable())?;
            let cancel=iptv_import::CancelToken::new();
            let mut episodes=Vec::new();
            let mut entries=Vec::new();
            let mut movie_locator=locator.map(|(_,l)|l);
            match &config {
                SourceConfig::Xtream(credentials)=>{
                    let xt=XtreamClient::from_credentials(credentials).map_err(|_|unavailable())?;
                    if key2.kind==Kind::Series {
                        let id=provider.as_deref().and_then(|id|id.parse().ok()).ok_or_else(unavailable)?;
                        let response=iptv_import::on_demand::detail(&client,xt.series_info(id),&cancel).await.map_err(|_|unavailable())?;
                        let details=iptv_providers::xtream::parse_series_info(&response,id).map_err(|_|unavailable())?;
                        item.plot=details.series.plot.or(item.plot); item.poster=details.series.poster.or(item.poster);
                        for e in details.episodes.items.into_iter().take(20_000) {
                            let key=Key{source:key2.source,kind:Kind::Episode,id:format!("{}:{}",key2.id,e.id.as_str())};
                            entries.push((key.clone(),Entry{config:config.clone(),locator:e.stream,parent:Some(key2.clone()),episode:None}));
                            episodes.push(Episode{key,season:e.season,number:e.number,title:e.title,plot:e.plot,duration_secs:e.duration_secs,progress:Progress::default()});
                        }
                    } else if let Some(StreamLocator::Xtream{stream_id,..})=&movie_locator {
                        // Detail metadata can fail independently of a usable catalogue stream.
                        if let Ok(response)=iptv_import::on_demand::detail(&client,xt.vod_info(*stream_id),&cancel).await
                            && let Ok(details)=iptv_providers::xtream::parse_vod_info(&response,*stream_id) {
                            item.plot=details.item.plot.or(item.plot);item.poster=details.item.poster.or(item.poster);
                            item.duration_secs=details.item.duration_secs.or(item.duration_secs);
                            if matches!(&details.item.stream,StreamLocator::Url{url,..} if !url.is_empty()) || matches!(&details.item.stream,StreamLocator::Xtream{container_extension:Some(_),..}) {movie_locator=Some(details.item.stream)}
                        }
                    }
                }
                SourceConfig::Stalker(credentials) if key2.kind==Kind::Series=>{
                    let mut session=iptv_import::on_demand::stalker_session(&client,credentials,&cancel).await.map_err(|_|unavailable())?;
                    let id=provider.clone().or_else(||key2.id.rsplit(':').next().map(str::to_owned)).ok_or_else(unavailable)?;
                    let cmd=match &movie_locator {Some(StreamLocator::Stalker{cmd,..}) if provider.is_none()=>Some(cmd.clone()),_=>None};
                    let found=stalker_episodes(&client,&mut session,&id,cmd.as_deref(),&cancel).await?;
                    for (e,locator,episode) in found {
                        let key=Key{source:key2.source,kind:Kind::Episode,id:format!("{}:{}",key2.id,e.id.as_str())};
                        entries.push((key.clone(),Entry{config:config.clone(),locator,parent:Some(key2.clone()),episode}));
                        episodes.push(Episode{key,season:e.season,number:e.number,title:e.title,plot:e.plot,duration_secs:e.duration_secs,progress:Progress::default()});
                    }
                }
                _=>{}
            }
            if let Some(locator)=movie_locator {entries.push((key2.clone(),Entry{config,locator,parent:None,episode:None}));}
            let keys:Vec<_>=episodes.iter().map(|e|e.key.clone()).collect();
            let progress=work.read(move |s|keys.iter().map(|k|s.vod_progress(k)).collect::<iptv_store_sqlite::Result<Vec<_>>>()).await?;
            for (e,p) in episodes.iter_mut().zip(progress) {e.progress=p;}
            let mut cache=state.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.len()+entries.len()>30_000 {cache.clear();}
            cache.extend(entries);
            Ok::<_,Error>(Details{item,episodes})
        }).await.map_err(|_|unavailable())??;
        Ok(result)
    }
    pub async fn vod_play(
        &self,
        details: Details,
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
        let lookup = key.clone();
        let expected = entry.config.clone();
        let progress = self
            .services()
            .read(move |s| {
                let valid = s.source_policy(SourceId(lookup.source))?.enabled
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
                        let mut session =
                            iptv_import::on_demand::stalker_session(&client, c, &cancel)
                                .await
                                .map_err(|_| unavailable())?;
                        let url = iptv_import::on_demand::stalker_episode_link(
                            &client,
                            &mut session,
                            cmd,
                            entry.episode,
                            &cancel,
                        )
                        .await
                        .map_err(|_| unavailable())?;
                        iptv_guide::playback::StalkerLink::source(&session, url, &[])
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
        Ok((playing, source))
    }
    pub async fn vod_save_progress(
        &self,
        key: Key,
        parent: Option<Key>,
        progress: Progress,
    ) -> Result<(), Error> {
        Ok(self
            .services()
            .call(move |s| {
                s.save_vod_progress(&key, parent.as_ref().map(|p| p.id.as_str()), progress)
            })
            .await?)
    }
}

async fn stalker_episodes(
    client: &iptv_net::NetClient,
    session: &mut iptv_providers::stalker::StalkerSession,
    id: &str,
    parent_cmd: Option<&str>,
    cancel: &iptv_import::CancelToken,
) -> Result<Vec<(iptv_providers::model::Episode, StreamLocator, Option<u32>)>, Error> {
    let mut out = Vec::new();
    let native = parent_cmd.is_none();
    for page in 1..=100 {
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
            if row.season {
                let before_season = out.len();
                for ep_page in 1..=100 {
                    let response = iptv_import::on_demand::stalker_detail(
                        client,
                        session,
                        |s| s.series_page(id, Some(&row.id), ep_page, native),
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
    if row.season {
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
