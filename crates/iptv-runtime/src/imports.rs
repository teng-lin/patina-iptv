//! Native import and refresh orchestration. No UI toolkit dependency.
use super::paths::data_dir;
use super::private_files::Client as NetClient;
use super::{guides, opening, stored, wizard_map};
use iptv_core::wizard::{
    FailureKind, Field, ImportCounts, ImportFailure, ImportProgress, ImportStage,
};
use iptv_epg::GuideStore;
use iptv_guide::playback::PlaybackContext;
use iptv_import::{
    CancelToken, Catalog, Decision, EpgImport, EpgSourceConfig, ImportError, ImportOptions,
    NoProgress, Progress, ProgressSink, Trigger, import_epg, import_source_into,
};
use iptv_providers::{
    SecretUrl,
    model::{Channel, Group, Source},
};
use iptv_store_sqlite::{
    CatalogCounts, CatalogStage, EpgStage, GuideSource, ImportErrorKind, SourceId, Store,
    StoreError,
};
use std::{
    future::Future,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::UnboundedSender as Sender;
/// An import to run.
#[derive(Clone)]
pub struct Job {
    pub source: Source,
    /// A guide the user named, tried before the source's own guides. When
    /// set, it must load for the import to succeed.
    pub epg_url: Option<SecretUrl>,
    /// `None`: add the source to the store once it imported. `Some`:
    /// refresh that stored source.
    pub stored: Option<SourceId>,
    pub purpose: Purpose,
    /// `source` holds new details (link, login, User-Agent) for the stored
    /// source: they replace the stored ones once they imported; a failure
    /// leaves the stored source and its data as they were.
    pub edit: bool,
}

/// Why a source is imported, which decides what reaches the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A new source from the wizard: shown once it imported.
    Add,
    /// The stored source opening at startup: its stored channels and guide
    /// at once, then refreshed; its last channel may start.
    Startup,
    /// The user chose another stored source: as at startup, but nothing
    /// starts playing.
    Switch,
    /// "Refresh now" of the source on screen: the refreshed data replaces
    /// what is shown.
    Refresh,
    /// "Refresh now" of another stored source: kept in the store only; the
    /// screen and its guide timers stay as they are.
    Background,
    /// A constituent of the aggregated guide: cached data first, independent refresh.
    Combined,
}

impl Purpose {
    pub fn shows(self) -> bool {
        !matches!(self, Self::Background | Self::Combined)
    }

    /// What the store has is shown before the network.
    pub fn shows_stored_first(self) -> bool {
        matches!(self, Self::Startup | Self::Switch | Self::Combined)
    }

    /// The guide's refresh trigger: opening uses the stored guide while it
    /// is fresh; an added or refreshed playlist downloads it again.
    pub fn trigger(self) -> Trigger {
        match self {
            Self::Startup | Self::Switch | Self::Combined => Trigger::Startup,
            Self::Add | Self::Refresh | Self::Background => Trigger::PlaylistChanged,
        }
    }
}

/// What an import reports, on the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Progress(ImportProgress),
    /// The catalog is saved and on screen; its guide may still be loading.
    Done(Result<ImportCounts, ImportFailure>),
}

/// The lists the guide is built from.
pub struct Imported {
    pub groups: Vec<Group>,
    pub channels: Vec<Channel>,
    /// What the source's streams need (headers, account, session).
    pub playback: Streams,
    pub guide: GuideStore,
    pub now: i64,
    /// The stored source, whose descriptions are read on demand.
    pub source: SourceId,
    /// Whether the last channel may start (its stream can be resolved).
    pub resume: bool,
}

/// Where a new schedule gets its playback context.
pub enum Streams {
    /// Its own, with the streams the account allows at once (when it says).
    New(Option<PlaybackContext>, Option<u32>),
    /// The schedule on screen's (a refreshed guide of the same source).
    Current,
}

pub enum Msg {
    Progress(ImportProgress),
    /// When to look at `source`'s guide again (unix time).
    NextCheck {
        source: SourceId,
        at: i64,
    },
    Playlist(Result<(ImportCounts, Box<Imported>), ImportFailure>),
    /// What the store has (before the playlist), or the source's guide
    /// loaded after the playlist was reported.
    Guide(Box<Imported>),
    /// What the store has, with only the first screen's programmes
    /// (`opening.rs`); a `Guide` follows.
    FirstScreen(Box<Imported>, opening::Held),
}

#[derive(Clone, Copy)]
pub enum GuideRead {
    Check,
    RestoreAndCheck,
    CacheOnly,
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub fn with_runtime<F: Future<Output = ()>>(future: impl FnOnce() -> F) -> bool {
    super::executor::block_on(future());
    true
}

/// The import itself, on its own thread with a small tokio runtime.
pub fn run(job: Job, tx: &Sender<Msg>, cancel: &CancelToken) {
    if !with_runtime(|| pipeline(job, tx, cancel)) {
        let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(FailureKind::Network))));
    }
}

/// The timer's guide check: the stored feeds for the stored channels.
pub async fn guide_pipeline(
    source: SourceId,
    tx: &Sender<Msg>,
    read: GuideRead,
    cancel: &CancelToken,
) {
    if cancel.is_cancelled() {
        return;
    }
    let now = now_unix();
    let state = stored::guide_state(source);
    let guides = stored::current_configs(source, state.as_ref());
    let (groups, channels) = match stored::catalog(source) {
        Ok(catalog) => catalog,
        Err(e) => {
            log::warn!("iptv data: stored catalog: {e}");
            stored::retry_check(tx, source, &guides, now);
            return;
        }
    };
    let overrides = stored::overrides(source);
    let matching = stored::Matching {
        channels: &channels,
        overrides: &overrides,
    };
    if !matches!(read, GuideRead::Check) {
        let _ = tx.send(Msg::Guide(Box::new(Imported {
            groups: groups.clone(),
            channels: channels.clone(),
            playback: Streams::Current,
            guide: stored::load_guide(source, now).unwrap_or_default(),
            now,
            source,
            resume: false,
        })));
    }
    if matches!(read, GuideRead::CacheOnly) {
        return;
    }
    match stored::plan(Trigger::Timer, &guides, matching, state.as_ref(), now) {
        Decision::Refresh(reason) => {
            log::info!("iptv data: downloading the guide ({reason:?})");
            match NetClient::new(super::settings::net_config()) {
                Ok(net) => {
                    let result =
                        download_guide(&net, &guides, matching, now, &mut NoProgress, cancel, None)
                            .await;
                    match result {
                        Ok(import) => {
                            if cancel.is_cancelled() {
                                return;
                            }
                            let guide = keep_guide(source, import, &guides, matching, now);
                            let _ = tx.send(Msg::Guide(Box::new(Imported {
                                groups,
                                channels: channels.clone(),
                                playback: Streams::Current,
                                guide,
                                now,
                                source,
                                resume: false,
                            })));
                        }
                        Err(e) => {
                            if cancel.is_cancelled() {
                                return;
                            }
                            log::warn!("iptv data: guide: {e}");
                            stored::record_failure(source, &e, &guides);
                        }
                    }
                }
                Err(e) => log::warn!("iptv data: network client setup failed: {e}"),
            }
        }
        Decision::Discard => {
            stored::discard(source);
            // Its programmes went: the guide on screen shows none.
            let _ = tx.send(Msg::Guide(Box::new(Imported {
                groups,
                channels: channels.clone(),
                playback: Streams::Current,
                guide: GuideStore::default(),
                now,
                source,
                resume: false,
            })));
        }
        Decision::Keep { .. } | Decision::Backoff { .. } => {}
    }
    send_next_check(tx, source, &guides, matching);
}

/// Tells the UI when to look at the guide again, if ever.
pub(crate) fn send_next_check(
    tx: &Sender<Msg>,
    source: SourceId,
    guides: &[EpgSourceConfig],
    matching: stored::Matching<'_>,
) {
    if let Some(at) = stored::next_check(source, guides, matching, now_unix()) {
        let _ = tx.send(Msg::NextCheck { source, at });
    }
}

/// Reports to the UI as the wizard's progress.
struct Forward<'a>(&'a Sender<Msg>);

impl ProgressSink for Forward<'_> {
    fn report(&mut self, progress: &Progress) {
        let _ = self
            .0
            .send(Msg::Progress(wizard_map::progress_of(progress)));
    }
}

pub(crate) async fn pipeline(job: Job, tx: &Sender<Msg>, cancel: &CancelToken) {
    let fail = |failure: ImportFailure| {
        let _ = tx.send(Msg::Playlist(Err(failure)));
    };
    let now = now_unix();
    if let Some(id) = job.stored.filter(|id| !network_due(*id, job.purpose))
        && let Ok((groups, channels)) = stored::catalog(id)
        && !channels.is_empty()
    {
        let counts = ImportCounts {
            channels: channels.len() as u32,
            groups: groups.len() as u32,
            movies: 0,
            series: 0,
        };
        let guide = stored::load_guide(id, now).unwrap_or_default();
        let context = super::playback::stored_context(&job.source.config);
        let _ = tx.send(Msg::Playlist(Ok((
            counts,
            Box::new(Imported {
                groups: groups.clone(),
                channels: channels.clone(),
                guide,
                now,
                source: id,
                resume: context.is_some(),
                playback: Streams::New(context, None),
            }),
        ))));
        let overrides = stored::overrides(id);
        let matching = stored::Matching {
            channels: &channels,
            overrides: &overrides,
        };
        let guides = stored::current_configs(id, stored::guide_state(id).as_ref());
        let state = stored::guide_state(id);
        match stored::plan(
            job.purpose.trigger(),
            &guides,
            matching,
            state.as_ref(),
            now,
        ) {
            Decision::Refresh(_) => match NetClient::new(super::settings::net_config()) {
                Ok(net) => {
                    match download_guide(
                        &net,
                        &guides,
                        matching,
                        now,
                        &mut NoProgress,
                        cancel,
                        None,
                    )
                    .await
                    {
                        Ok(import) => {
                            let guide = keep_guide(id, import, &guides, matching, now);
                            let _ = tx.send(Msg::Guide(Box::new(Imported {
                                groups,
                                channels: channels.clone(),
                                guide,
                                now,
                                source: id,
                                resume: false,
                                playback: Streams::Current,
                            })));
                        }
                        Err(ImportError::Cancelled) => {}
                        Err(e) => stored::record_failure(id, &e, &guides),
                    }
                }
                // Without a client no guide source was tried. Noted as the
                // failed download it amounts to, so the guide's state shows
                // it and the refresh policy backs off instead of finding
                // the guide due again at every check.
                Err(e) => {
                    log::warn!("iptv data: network client setup failed: {e}");
                    stored::record_client_failure(id, &guides);
                }
            },
            Decision::Discard => {
                stored::discard(id);
                let _ = tx.send(Msg::Guide(Box::new(Imported {
                    groups,
                    channels: channels.clone(),
                    guide: GuideStore::default(),
                    now,
                    source: id,
                    resume: false,
                    playback: Streams::Current,
                })));
            }
            _ => {}
        }
        send_next_check(tx, id, &guides, matching);
        return;
    }
    let stored_guide = job.stored.and_then(stored::guide_state);
    let Ok(net) = NetClient::new(super::settings::net_config()) else {
        log::warn!("iptv data: network client setup failed");
        return fail(ImportFailure::new(FailureKind::Network));
    };
    let mut progress = Forward(tx);
    let mut options = ImportOptions::new(now);
    // A whole catalog import (live, movies and series of an Xtream account)
    // may take long on a slow box; a stalled download still ends at the
    // client's idle timeout. The budget is counted over the whole import
    // now that the client reports elapsed time.
    options.limits.elapsed = CATALOG_IMPORT_TIME;
    #[cfg(debug_assertions)]
    {
        options.allow_private_fixture = cfg!(feature = "test-support") || DEV_PLAYLIST.is_some();
    }
    let _ = std::fs::create_dir_all(data_dir());
    let mut stage = match CatalogStage::create_in_with(data_dir(), |path| {
        let path = path.to_owned();
        super::store_service::blocking(move |store| store.open_staging_connection(&path))
    }) {
        Ok(stage) => stage,
        Err(error) => {
            log::warn!("iptv data: catalog staging failed: {error}");
            return fail(ImportFailure::new(FailureKind::Storage));
        }
    };
    let mut catalog = match import_source_into(
        &net,
        &job.source.config,
        &options,
        &mut progress,
        cancel,
        &mut stage,
    )
    .await
    {
        Ok(catalog) => catalog,
        Err(e) => {
            log::warn!("iptv data: {:?} import failed: {e}", job.source.kind());
            // New details that failed say nothing about the stored ones.
            if !matches!(e, ImportError::Cancelled) && !job.edit {
                record_failure(job.stored, stored::error_kind(&e));
            }
            // The stored guide stays on screen: keep it current.
            if let (Some(id), Some(state)) = (job.stored, stored_guide.as_ref())
                && let Ok((_, channels)) = stored::catalog(id)
            {
                let overrides = stored::overrides(id);
                let matching = stored::Matching {
                    channels: &channels,
                    overrides: &overrides,
                };
                let guides = stored::current_configs(id, Some(state));
                send_next_check(tx, id, &guides, matching);
            }
            return fail(wizard_map::failure_of(&e));
        }
    };
    if catalog.is_empty() {
        if !job.edit {
            record_failure(job.stored, ImportErrorKind::Other);
        }
        return fail(ImportFailure::new(FailureKind::EmptyPlaylist));
    }
    // Materialize only the live guide's model; movies and series stay in SQLite.
    match stage
        .load_channels()
        .and_then(|channels| stage.load_groups().map(|groups| (channels, groups)))
    {
        Ok((channels, groups)) => {
            catalog.channels = channels;
            catalog.groups = groups;
        }
        Err(error) => {
            log::warn!("iptv data: staged catalog read failed: {error}");
            return fail(ImportFailure::new(FailureKind::Storage));
        }
    }
    // The guide sources: the stored source's as the user arranged them (a
    // new source: the guide just typed), with the provider's guides of now.
    let planned = guides::sync_provider(&planned_guides(&job), &catalog.guide_urls());
    let overrides = job.stored.map(stored::overrides).unwrap_or_default();
    let matching = stored::Matching {
        channels: &catalog.channels,
        overrides: &overrides,
    };
    // A guide the user typed is part of adding the source: it must load
    // before the source is kept, and its failure points at its field.
    let typed_guide = match job.epg_url {
        Some(_) => {
            match download_guide(
                &net,
                &stored::configs_for(&planned, &job.source.config),
                matching,
                now,
                &mut progress,
                cancel,
                Some(guides::FIRST_USER_FEED),
            )
            .await
            {
                Ok(import) => Some(import),
                Err(e) => {
                    log::warn!("iptv data: the typed guide failed: {e}");
                    let mut failure = wizard_map::failure_of(&e);
                    if failure.kind != FailureKind::Cancelled {
                        failure.field = Some(Field::EpgUrl);
                    }
                    return fail(failure);
                }
            }
        }
        None => None,
    };
    if cancel.is_cancelled() {
        return fail(ImportFailure::new(FailureKind::Cancelled));
    }
    let _ = tx.send(Msg::Progress(ImportProgress {
        stage: ImportStage::Saving,
        done: 0,
        total: None,
    }));
    let (id, counts, sources) = match save(&job, &catalog, &planned, stage) {
        Ok(saved) => saved,
        Err(e) => {
            log::warn!("iptv data: store: {e}");
            return fail(ImportFailure::new(FailureKind::Storage));
        }
    };
    let matching = stored::Matching {
        channels: &catalog.channels,
        overrides: &overrides,
    };
    let counts = ImportCounts {
        channels: counts.channels,
        groups: counts.groups,
        movies: counts.vod,
        series: counts.series,
    };
    let imported = |guide| {
        Box::new(Imported {
            groups: catalog.groups.clone(),
            channels: catalog.channels.clone(),
            playback: Streams::New(
                super::playback::context_for(&job.source.config, &catalog.source),
                super::playback::account_streams(&catalog.source),
            ),
            guide,
            now,
            source: id,
            resume: true,
        })
    };
    let report = |guide| {
        let _ = tx.send(Msg::Playlist(Ok((counts, imported(guide)))));
    };
    let guides = stored::configs_for(&sources, &job.source.config);
    if let Some(import) = typed_guide {
        report(keep_guide(id, import, &guides, matching, now));
        return send_next_check(tx, id, &guides, matching);
    }
    match stored::plan(
        job.purpose.trigger(),
        &guides,
        matching,
        stored_guide.as_ref(),
        now,
    ) {
        Decision::Refresh(reason) => {
            log::info!("iptv data: downloading the guide ({reason:?})");
            report(stored::load_guide(id, now).unwrap_or_default());
            // The guide loads after the channels are on screen.
            match download_guide(&net, &guides, matching, now, &mut NoProgress, cancel, None).await
            {
                Ok(import) => {
                    let guide = keep_guide(id, import, &guides, matching, now);
                    let _ = tx.send(Msg::Guide(imported(guide)));
                }
                Err(ImportError::Cancelled) => {}
                Err(e) => {
                    log::warn!("iptv data: guide: {e}");
                    stored::record_failure(id, &e, &guides);
                }
            }
        }
        Decision::Discard => {
            stored::discard(id);
            report(GuideStore::default());
        }
        decision => {
            log::info!("iptv data: using the stored guide ({decision:?})");
            report(stored::load_guide(id, now).unwrap_or_default());
        }
    }
    send_next_check(tx, id, &guides, matching);
}

/// The guide sources an import starts from: the stored source's, or the
/// guide typed for a new source.
pub(crate) fn planned_guides(job: &Job) -> Vec<GuideSource> {
    match (job.stored, &job.epg_url) {
        (Some(id), _) => stored::guide_sources(id),
        (None, Some(url)) => vec![guides::user_source(
            guides::FIRST_USER_FEED,
            url.clone(),
            None,
        )],
        (None, None) => Vec::new(),
    }
}

/// A downloaded guide not stored yet: how it matched, and its programmes
/// staged in a file next to the store (deleted when this is dropped, so a
/// guide that is never kept leaves the stored one as it was).
pub(crate) struct Downloaded {
    pub import: EpgImport,
    pub stage: EpgStage,
}

/// Stores a downloaded guide and reads it back without descriptions (the
/// stored one, if any, when the store fails).
pub(crate) fn keep_guide(
    id: SourceId,
    guide: Downloaded,
    guides: &[EpgSourceConfig],
    matching: stored::Matching<'_>,
    now: i64,
) -> GuideStore {
    if let Err(e) = stored::replace(id, guide, guides, matching, now) {
        log::warn!("iptv data: could not store the guide: {e}");
    }
    stored::load_guide(id, now).unwrap_or_default()
}

/// Downloads `guides` for the catalog's channels with the user's guide
/// choices, streaming the programmes to a stage file (memory stays small
/// whatever the guide's size). The guide `must_load` (the one the user
/// just typed), when given, must load.
pub(crate) async fn download_guide<P: ProgressSink>(
    net: &NetClient,
    guides: &[EpgSourceConfig],
    matching: stored::Matching<'_>,
    now: i64,
    progress: &mut P,
    cancel: &CancelToken,
    must_load: Option<iptv_epg::FeedId>,
) -> Result<Downloaded, ImportError> {
    let mut stage =
        super::store_service::call(|store| EpgStage::create_in_store(store, data_dir()))
            .await
            .map_err(|e| ImportError::Storage(e.to_string()))?;
    let epg = import_epg(
        net,
        guides,
        matching.channels,
        matching.overrides,
        now,
        &mut stage,
        progress,
        cancel,
    )
    .await?;
    if let Some(Err(e)) = must_load
        .and_then(|id| epg.sources.iter().find(|o| o.id == id))
        .map(|o| &o.result)
    {
        return Err(e.clone());
    }
    Ok(Downloaded { import: epg, stage })
}

/// Stores the catalog: under a new source (removed again if the catalog
/// cannot be written) or the refreshed one (with its new details when they
/// were edited), and the guide sources with the provider's guides of now
/// (`planned` for a new source; a stored one's are read again, as the user
/// may have changed them meanwhile). Returns the guide sources saved.
pub(crate) fn save(
    job: &Job,
    catalog: &Catalog,
    planned: &[GuideSource],
    stage: CatalogStage,
) -> Result<(SourceId, CatalogCounts, Vec<GuideSource>), StoreError> {
    let job = job.clone();
    let guide_urls = catalog.guide_urls();
    let planned = planned.to_vec();
    super::store_service::blocking(move |store| save_in(store, &job, guide_urls, planned, stage))
}

pub(crate) fn save_in(
    store: &mut Store,
    job: &Job,
    guide_urls: Vec<SecretUrl>,
    planned: Vec<GuideSource>,
    stage: CatalogStage,
) -> Result<(SourceId, CatalogCounts, Vec<GuideSource>), StoreError> {
    let (id, added) = match job.stored {
        Some(id) => (id, false),
        None => (store.add_source(&job.source)?, true),
    };
    let sources = if added {
        planned
    } else {
        guides::sync_provider(&store.guide_sources(id)?, &guide_urls)
    };
    let details = job.edit.then_some(&job.source);
    let written = store.publish_catalog_stage(id, stage, details, &sources);
    if written.is_err() && added {
        let _ = store.delete_source(id);
    }
    let counts = written?;
    log::info!(
        "iptv data: saved {} channels for source {}",
        counts.channels,
        id.0
    );
    Ok((id, counts, sources))
}

/// Notes a failed refresh on the stored source (kind only).
pub(crate) fn record_failure(stored: Option<SourceId>, kind: ImportErrorKind) {
    let Some(id) = stored else {
        return;
    };
    if let Err(e) =
        super::store_service::blocking(move |store| store.record_import_failure(id, kind))
    {
        log::warn!("iptv data: could not record the failure: {e}");
    }
}

/// Keeps a picked playlist file in the app's files (a picked document is
/// only readable now; a refresh reads the copy). Returns its path.
/// Total time of one catalog import ([`ImportOptions`] limits).
const CATALOG_IMPORT_TIME: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// A kept playlist file's name: letters, digits, `-`, `_` and `.` only, no
/// leading or trailing dot, and never a name Windows reserves (`con.m3u`),
/// which a whole-app backup would refuse to carry.
pub fn kept_file_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_matches('.');
    if name.is_empty() {
        "playlist.m3u".to_owned()
    } else if iptv_import::backup_manifest::portable_component(name) {
        name.to_owned()
    } else {
        format!("playlist-{name}")
    }
}

pub async fn keep_picked_file(name: &str, bytes: &[u8]) -> std::io::Result<String> {
    let name = kept_file_name(name);
    let mut random = [0u8; 8];
    getrandom::fill(&mut random)
        .map_err(|_| std::io::Error::other("cannot create document identity"))?;
    let dir = data_dir()
        .join("playlists")
        .join(format!("{:016x}", u64::from_le_bytes(random)));
    let path = dir.join(&name);
    let bytes = bytes.to_vec();
    super::store_service::call(move |store| {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).map_err(|_| {
            iptv_store_sqlite::StoreError::Unavailable("cannot create private document directory")
        })?;
        super::private_files::write_atomic(store, &path, &mut bytes.as_slice())?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|_| std::io::Error::other("private document cannot be saved"))
}

/// Cached startup/switch can avoid a playlist download independently of EPG policy.
pub(crate) fn network_due(id: SourceId, purpose: Purpose) -> bool {
    if !matches!(
        purpose,
        Purpose::Startup | Purpose::Switch | Purpose::Combined
    ) {
        return true;
    }
    super::store_service::blocking(move |store| {
        Ok((store.source_policy(id)?, store.import_status(id)?))
    })
    .map(|(p, status)| {
        status.last_success_at.is_none()
            || p.due(
                now_unix(),
                status.last_attempt_at,
                matches!(purpose, Purpose::Startup | Purpose::Combined),
            )
    })
    .unwrap_or(true)
}

pub const DEV_PLAYLIST: Option<&str> = option_env!("IPTV_RS_DEV_PLAYLIST");
