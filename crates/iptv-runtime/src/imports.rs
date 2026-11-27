//! Native import and refresh orchestration. No UI toolkit dependency.
use super::private_files::Client as NetClient;
use super::store_service::Services;
use super::{guides, opening, stored, wizard_map};
use crate::time::now_unix;
use iptv_core::add_playlist::{
    FailureKind, ImportCounts, ImportFailure, ImportInput, ImportProgress, ImportStage,
};
use iptv_epg::GuideStore;
use iptv_guide::playback::PlaybackContext;
use iptv_import::{
    CancelToken, Catalog, Decision, EpgImport, EpgSourceConfig, ImportError, ImportOptions,
    NoProgress, Progress, ProgressSink, Trigger, import_epg, import_source_into,
};
use iptv_providers::{
    SecretUrl,
    model::{Channel, ContentKind, Group, Source},
};
use iptv_store_sqlite::{
    CatalogCounts, CatalogStage, EpgStage, GuideSource, ImportErrorKind, SourceId, Store,
    StoreError, StoredGuide,
};
use tokio::sync::mpsc::UnboundedSender as Sender;
#[cfg(all(test, feature = "test-support"))]
mod recovery_tests;

/// Authority captured before I/O, checked again on the writer before mutation.
/// A reused numeric source ID or an edit cannot inherit an old import's result.
#[derive(Clone)]
pub(crate) struct Publication {
    cancel: CancelToken,
    source: Option<(SourceId, String, iptv_providers::model::SourceConfig)>,
    authorization: crate::parental::Snapshot,
}

#[derive(Debug)]
pub(crate) enum PublicationError {
    Superseded,
    Denied,
    Store(StoreError),
}
impl From<StoreError> for PublicationError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
impl std::fmt::Display for PublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Superseded => f.write_str("import superseded"),
            Self::Denied => f.write_str("import blocked by parental controls"),
            Self::Store(error) => error.fmt(f),
        }
    }
}
impl PublicationError {
    fn failure(&self) -> ImportFailure {
        ImportFailure::new(match self {
            Self::Superseded | Self::Store(StoreError::SourceNotFound(_)) => FailureKind::Canceled,
            Self::Denied => FailureKind::Auth,
            Self::Store(_) => FailureKind::Storage,
        })
    }
}

impl Publication {
    pub(crate) fn capture(
        services: &Services,
        job: &Job,
        cancel: &CancelToken,
        authorization: &crate::parental::Snapshot,
    ) -> Result<Self, PublicationError> {
        if cancel.is_canceled() {
            return Err(PublicationError::Superseded);
        }
        let job = job.clone();
        let auth = authorization.clone();
        let source = services.blocking(move |store| {
            if !crate::parental::import_allowed(store, &job, &auth)? {
                return Ok(Err(PublicationError::Denied));
            }
            let source = match job.stored {
                Some(id) => {
                    let stored = store.source(id)?.ok_or(StoreError::SourceNotFound(id))?;
                    // A refresh can wait in the bounded pool while source details
                    // change. It must not adopt the newer incarnation and then
                    // fetch using the older job's connection configuration.
                    if !job.edit && stored.source.config != job.source.config {
                        return Ok(Err(PublicationError::Superseded));
                    }
                    Some((id, store.source_incarnation(id)?, stored.source.config))
                }
                None => None,
            };
            Ok(Ok(source))
        })??;
        Ok(Self {
            cancel: cancel.clone(),
            source,
            authorization: authorization.clone(),
        })
    }

    fn current(&self, store: &Store) -> Result<bool, StoreError> {
        if self.cancel.is_canceled() {
            return Ok(false);
        }
        if let Some((id, incarnation, config)) = &self.source {
            let Some(current) = store.source(*id)? else {
                return Ok(false);
            };
            if store.source_incarnation(*id)? != *incarnation || current.source.config != *config {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
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
    New(
        Option<PlaybackContext>,
        Option<u32>,
        std::sync::Arc<iptv_guide::playback::SourceAuthority>,
    ),
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
    /// The refresh policy asks for no further check of `source`'s guide:
    /// only a request or a restart looks at it again. Work that cannot
    /// tell (it failed, or could not read the store) sends neither.
    NoCheck {
        source: SourceId,
    },
    Playlist(Result<(ImportCounts, Box<Imported>), ImportFailure>),
    /// What the store has (before the playlist), or the source's guide
    /// loaded after the playlist was reported.
    Guide(Box<Imported>),
    /// What the store has, with only the first screen's programs
    /// (`opening.rs`); a `Guide` follows. The window read waits until the
    /// [`opening::Shown`] is dropped.
    FirstScreen(Box<Imported>, opening::Held, opening::Shown),
}

#[derive(Clone, Copy)]
pub enum GuideRead {
    Check,
    RestoreAndCheck,
    CacheOnly,
}

/// The import on the calling worker, to its end (tests drive it so).
#[cfg(all(test, feature = "test-support"))]
pub(crate) fn run(
    services: &Services,
    job: Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    revoked: &CancelToken,
    authorization: crate::parental::Snapshot,
) {
    services.block_on(pipeline(services, job, tx, cancel, revoked, &authorization));
}

/// The timer's guide check: the stored feeds for the stored channels.
pub(crate) async fn guide_pipeline(
    services: &Services,
    source: SourceId,
    tx: &Sender<Msg>,
    read: GuideRead,
    cancel: &CancelToken,
) {
    if cancel.is_canceled() {
        return;
    }
    let now = now_unix();
    let stored::GuideInputs {
        state,
        guides,
        groups,
        channels,
        overrides,
    } = match stored::guide_inputs(services, source) {
        Ok(inputs) => inputs,
        Err(e) => {
            stored::unreadable(services, tx, source, now, &e);
            return;
        }
    };
    let matching = stored::Matching {
        channels: &channels,
        overrides: &overrides,
    };
    if !matches!(read, GuideRead::Check) {
        let guide = match stored::try_load_guide(services, source, now) {
            Ok(guide) => guide,
            Err(e) => {
                stored::unreadable(services, tx, source, now, &e);
                return;
            }
        };
        let _ = tx.send(Msg::Guide(Box::new(Imported {
            groups: groups.clone(),
            channels: channels.clone(),
            playback: Streams::Current,
            guide,
            now,
            source,
            resume: false,
        })));
    }
    if matches!(read, GuideRead::CacheOnly) {
        return;
    }
    match stored::plan_with(
        &services.settings(),
        Trigger::Timer,
        &guides,
        matching,
        state.as_ref(),
        now,
    ) {
        Decision::Refresh(reason) => {
            log::info!("iptv data: downloading the guide ({reason:?})");
            match NetClient::new(services) {
                Ok(net) => {
                    let result =
                        download_guide(&net, &guides, matching, now, &mut NoProgress, cancel, None)
                            .await;
                    match result {
                        Ok(import) => {
                            if cancel.is_canceled() {
                                return;
                            }
                            let guide = keep_guide(
                                services,
                                source,
                                import,
                                &guides,
                                matching,
                                now,
                                &[cancel],
                            )
                            .await;
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
                            if cancel.is_canceled() {
                                return;
                            }
                            log::warn!("iptv data: guide: {e}");
                            stored::record_failure(services, source, &e, &guides, &[cancel]);
                        }
                    }
                }
                Err(e) => log::warn!("iptv data: network client setup failed: {e}"),
            }
        }
        Decision::Discard => {
            stored::discard(services, source, &[cancel]);
            // Its programs went: the guide on screen shows none.
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
    send_next_check(services, tx, source, &guides, matching);
}

/// Tells the UI when to look at the guide again, or that the policy asks
/// for no check; when the store cannot say, see [`stored::unreadable`].
pub(crate) fn send_next_check(
    services: &Services,
    tx: &Sender<Msg>,
    source: SourceId,
    guides: &[EpgSourceConfig],
    matching: stored::Matching<'_>,
) {
    let now = now_unix();
    let _ = match stored::next_check(services, source, guides, matching, now) {
        Ok(Some(at)) => tx.send(Msg::NextCheck { source, at }),
        Ok(None) => tx.send(Msg::NoCheck { source }),
        Err(e) => return stored::unreadable(services, tx, source, now, &e),
    };
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

pub(crate) async fn pipeline(
    services: &Services,
    job: Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    revoked: &CancelToken,
    authorization: &crate::parental::Snapshot,
) {
    // What keeps a downloaded guide out of the store.
    let stops = [cancel, revoked];
    let fail = |failure: ImportFailure| {
        let _ = tx.send(Msg::Playlist(Err(failure)));
    };
    let publication = match Publication::capture(services, &job, cancel, authorization) {
        Ok(publication) => publication,
        Err(error) => {
            if !services.is_current() {
                return fail(ImportFailure::new(FailureKind::Canceled));
            }
            log::warn!("iptv data: import could not start: {error}");
            return fail(error.failure());
        }
    };
    let now = now_unix();
    if let Some((id, groups, channels)) = cached_catalog(services, &job) {
        report_cached_catalog(
            services,
            tx,
            &publication,
            &job.source.config,
            id,
            groups.clone(),
            channels.clone(),
            now,
        );
        refresh_cached_guide(services, &job, tx, cancel, revoked, id, now, &stops).await;
        return;
    }
    run_fresh_catalog(
        services,
        &job,
        tx,
        cancel,
        revoked,
        &publication,
        now,
        &stops,
    )
    .await;
}

fn cached_catalog(services: &Services, job: &Job) -> Option<(SourceId, Vec<Group>, Vec<Channel>)> {
    let id = job
        .stored
        .filter(|id| !job.edit && !network_due(services, *id, job.purpose))?;
    let (groups, channels) = stored::catalog(services, id).ok()?;
    (!channels.is_empty()).then_some((id, groups, channels))
}

fn report_cached_catalog(
    services: &Services,
    tx: &Sender<Msg>,
    publication: &Publication,
    source_config: &iptv_providers::model::SourceConfig,
    id: SourceId,
    groups: Vec<Group>,
    channels: Vec<Channel>,
    now: i64,
) {
    let counts = ImportCounts {
        channels: channels.len() as u32,
        groups: groups.len() as u32,
        movies: 0,
        series: 0,
    };
    let guide = stored::load_guide(services, id, now).unwrap_or_default();
    let context = super::playback::stored_context(source_config);
    let (source, incarnation, config) = publication.source.as_ref().expect("stored publication");
    let authority = std::sync::Arc::new(iptv_guide::playback::SourceAuthority::new(
        source.0,
        incarnation.clone(),
        config.clone(),
    ));
    let _ = tx.send(Msg::Playlist(Ok((
        counts,
        Box::new(Imported {
            groups,
            channels,
            guide,
            now,
            source: id,
            resume: context.is_some(),
            playback: Streams::New(context, None, authority),
        }),
    ))));
}

async fn refresh_cached_guide(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    revoked: &CancelToken,
    id: SourceId,
    now: i64,
    stops: &[&CancelToken; 2],
) {
    let stored::GuideInputs {
        state,
        guides,
        groups,
        channels,
        overrides,
    } = match stored::guide_inputs(services, id) {
        Ok(inputs) => inputs,
        Err(e) => return stored::unreadable(services, tx, id, now, &e),
    };
    let matching = stored::Matching {
        channels: &channels,
        overrides: &overrides,
    };
    match stored::plan_with(
        &services.settings(),
        job.purpose.trigger(),
        &guides,
        matching,
        state.as_ref(),
        now,
    ) {
        Decision::Refresh(_) if revoked.is_canceled() => {}
        Decision::Refresh(_) => match NetClient::new(services) {
            Ok(net) => {
                match download_guide(&net, &guides, matching, now, &mut NoProgress, cancel, None)
                    .await
                {
                    Ok(import) => {
                        let guide =
                            keep_guide(services, id, import, &guides, matching, now, stops).await;
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
                    Err(ImportError::Canceled) => {}
                    Err(_) if stops.iter().any(|stop| stop.is_canceled()) => {}
                    Err(e) => stored::record_failure(services, id, &e, &guides, stops),
                }
            }
            Err(e) => {
                log::warn!("iptv data: network client setup failed: {e}");
                stored::record_client_failure(services, id, &guides, stops);
            }
        },
        Decision::Discard => {
            stored::discard(services, id, stops);
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
    send_next_check(services, tx, id, &guides, matching);
}

struct GuidePlan {
    sources: Vec<GuideSource>,
    overrides: iptv_epg::Overrides,
    typed: Option<Downloaded>,
}

struct SavedImport {
    id: SourceId,
    counts: ImportCounts,
    sources: Vec<EpgSourceConfig>,
    overrides: iptv_epg::Overrides,
    authority: std::sync::Arc<iptv_guide::playback::SourceAuthority>,
    typed: Option<Downloaded>,
}

async fn run_fresh_catalog(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    revoked: &CancelToken,
    publication: &Publication,
    now: i64,
    stops: &[&CancelToken; 2],
) {
    let stored_guide = job.stored.and_then(|id| stored::guide_state(services, id));
    let (net, stage, catalog) = match import_staged_catalog(
        services,
        job,
        tx,
        cancel,
        publication,
        stored_guide.is_some(),
        now,
    )
    .await
    {
        Ok(imported) => imported,
        Err(failure) => return send_failure(tx, failure),
    };
    let plan = match prepare_guide_plan(services, job, tx, cancel, &net, &catalog, now).await {
        Ok(plan) => plan,
        Err(failure) => return send_failure(tx, failure),
    };
    let saved = match save_catalog(
        services,
        job,
        tx,
        cancel,
        revoked,
        publication,
        &catalog,
        stage,
        plan,
        now,
    )
    .await
    {
        Ok(saved) => saved,
        Err(failure) => return send_failure(tx, failure),
    };
    finish_catalog(
        services,
        job,
        tx,
        cancel,
        revoked,
        &net,
        &catalog,
        saved,
        stored_guide,
        now,
        stops,
    )
    .await;
}

fn send_failure(tx: &Sender<Msg>, failure: ImportFailure) {
    let _ = tx.send(Msg::Playlist(Err(failure)));
}

async fn prepare_guide_plan(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    net: &NetClient,
    catalog: &Catalog,
    now: i64,
) -> Result<GuidePlan, ImportFailure> {
    let guide_urls = catalog.guide_urls();
    let sources = match job.stored {
        Some(id) => {
            services.blocking_read(move |store| store.plan_provider_guides(id, &guide_urls))
        }
        None => guides::sync_provider(&planned_guides(services, job), &guide_urls)
            .map_err(|error| StoreError::InvalidData(error.to_string())),
    }
    .map_err(|error| {
        log::warn!("iptv data: guide identity planning failed: {error}");
        ImportFailure::new(FailureKind::Storage)
    })?;
    let overrides = job
        .stored
        .map(|id| stored::overrides(services, id))
        .transpose()
        .map_err(|error| {
            log::warn!("iptv data: guide choices: {error}");
            ImportFailure::new(FailureKind::Storage)
        })?
        .unwrap_or_default();
    let matching = stored::Matching {
        channels: &catalog.channels,
        overrides: &overrides,
    };
    let typed = if job.epg_url.is_some() {
        let mut progress = Forward(tx);
        match download_guide(
            net,
            &stored::configs_for(&sources, &job.source.config),
            matching,
            now,
            &mut progress,
            cancel,
            Some(guides::FIRST_USER_FEED),
        )
        .await
        {
            Ok(import) => Some(import),
            Err(error) => {
                log::warn!("iptv data: the typed guide failed: {error}");
                let mut failure = wizard_map::failure_of(&error);
                if failure.kind != FailureKind::Canceled {
                    failure.input = Some(ImportInput::GuideAddress);
                }
                return Err(failure);
            }
        }
    } else {
        None
    };
    Ok(GuidePlan {
        sources,
        overrides,
        typed,
    })
}

async fn save_catalog(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    revoked: &CancelToken,
    publication: &Publication,
    catalog: &Catalog,
    stage: CatalogStage,
    mut plan: GuidePlan,
    now: i64,
) -> Result<SavedImport, ImportFailure> {
    if cancel.is_canceled() {
        return Err(ImportFailure::new(FailureKind::Canceled));
    }
    let _ = tx.send(Msg::Progress(ImportProgress {
        stage: ImportStage::Saving,
        done: 0,
        total: None,
    }));
    #[cfg(feature = "test-support")]
    services.before_publication();
    let matching = stored::Matching {
        channels: &catalog.channels,
        overrides: &plan.overrides,
    };
    let required_guide = if job.stored.is_none() && services.ports().store_host.is_some() {
        plan.typed.take().map(|download| RequiredGuide {
            download,
            fingerprint: matching.fingerprint(),
            now,
            revoked: revoked.clone(),
        })
    } else {
        None
    };
    let saved = save_published(
        services,
        job,
        catalog,
        &plan.sources,
        stage,
        publication,
        required_guide,
    )
    .await
    .map_err(|error| {
        log::warn!("iptv data: store: {error}");
        if cancel.is_canceled() || !services.is_current() {
            ImportFailure::new(FailureKind::Canceled)
        } else {
            error.failure()
        }
    })?;
    let (id, counts, sources, authority) = saved;
    Ok(SavedImport {
        id,
        counts: ImportCounts {
            channels: counts.channels,
            groups: counts.groups,
            movies: counts.vod,
            series: counts.series,
        },
        sources: stored::configs_for(&sources, &job.source.config),
        overrides: plan.overrides,
        authority: std::sync::Arc::new(authority),
        typed: plan.typed,
    })
}

async fn finish_catalog(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    revoked: &CancelToken,
    net: &NetClient,
    catalog: &Catalog,
    mut saved: SavedImport,
    stored_guide: Option<StoredGuide>,
    now: i64,
    stops: &[&CancelToken; 2],
) {
    let matching = stored::Matching {
        channels: &catalog.channels,
        overrides: &saved.overrides,
    };
    let guide_sources = &saved.sources;
    if job.epg_url.is_some() {
        let guide = match saved.typed.take() {
            Some(import) => {
                keep_guide(
                    services,
                    saved.id,
                    import,
                    &guide_sources,
                    matching,
                    now,
                    stops,
                )
                .await
            }
            None => stored::load_guide(services, saved.id, now).unwrap_or_default(),
        };
        report_playlist(
            services,
            tx,
            job,
            catalog,
            saved.id,
            saved.counts,
            &saved.authority,
            guide,
            now,
        );
        return send_next_check(services, tx, saved.id, &guide_sources, matching);
    }
    finish_automatic_guide(
        services,
        job,
        tx,
        cancel,
        revoked,
        net,
        catalog,
        &saved,
        &guide_sources,
        matching,
        stored_guide.as_ref(),
        now,
        stops,
    )
    .await;
}

async fn finish_automatic_guide(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    revoked: &CancelToken,
    net: &NetClient,
    catalog: &Catalog,
    saved: &SavedImport,
    guides: &[EpgSourceConfig],
    matching: stored::Matching<'_>,
    stored_guide: Option<&StoredGuide>,
    now: i64,
    stops: &[&CancelToken; 2],
) {
    match stored::plan_with(
        &services.settings(),
        job.purpose.trigger(),
        guides,
        matching,
        stored_guide,
        now,
    ) {
        Decision::Refresh(reason) => {
            log::info!("iptv data: downloading the guide ({reason:?})");
            report_playlist(
                services,
                tx,
                job,
                catalog,
                saved.id,
                saved.counts,
                &saved.authority,
                stored::load_guide(services, saved.id, now).unwrap_or_default(),
                now,
            );
            let downloaded = if revoked.is_canceled() {
                Err(ImportError::Canceled)
            } else {
                download_guide(net, guides, matching, now, &mut NoProgress, cancel, None).await
            };
            match downloaded {
                Ok(import) => {
                    let guide =
                        keep_guide(services, saved.id, import, guides, matching, now, stops).await;
                    let _ = tx.send(Msg::Guide(Box::new(Imported {
                        groups: catalog.groups.clone(),
                        channels: catalog.channels.clone(),
                        playback: Streams::Current,
                        guide,
                        now,
                        source: saved.id,
                        resume: false,
                    })));
                }
                Err(ImportError::Canceled) => {}
                Err(_) if stops.iter().any(|stop| stop.is_canceled()) => {}
                Err(error) => {
                    log::warn!("iptv data: guide: {error}");
                    stored::record_failure(services, saved.id, &error, guides, stops);
                }
            }
        }
        Decision::Discard => {
            stored::discard(services, saved.id, stops);
            report_playlist(
                services,
                tx,
                job,
                catalog,
                saved.id,
                saved.counts,
                &saved.authority,
                GuideStore::default(),
                now,
            );
        }
        decision => {
            log::info!("iptv data: using the stored guide ({decision:?})");
            report_playlist(
                services,
                tx,
                job,
                catalog,
                saved.id,
                saved.counts,
                &saved.authority,
                stored::load_guide(services, saved.id, now).unwrap_or_default(),
                now,
            );
        }
    }
    send_next_check(services, tx, saved.id, guides, matching);
}

fn report_playlist(
    services: &Services,
    tx: &Sender<Msg>,
    job: &Job,
    catalog: &Catalog,
    id: SourceId,
    counts: ImportCounts,
    authority: &std::sync::Arc<iptv_guide::playback::SourceAuthority>,
    guide: GuideStore,
    now: i64,
) {
    #[cfg(feature = "test-support")]
    services.before_report();
    let imported = Imported {
        groups: catalog.groups.clone(),
        channels: catalog.channels.clone(),
        playback: Streams::New(
            super::playback::context_for(&job.source.config, &catalog.source),
            super::playback::account_streams(&catalog.source),
            authority.clone(),
        ),
        guide,
        now,
        source: id,
        resume: true,
    };
    let _ = tx.send(Msg::Playlist(Ok((counts, Box::new(imported)))));
}

fn import_options(services: &Services, job: &Job, now: i64) -> ImportOptions {
    let target = job.stored.map_or(iptv_settings::Target::Global, |id| {
        iptv_settings::Target::playlist(id.0.to_string())
    });
    let include_vod = services
        .settings()
        .bool(iptv_settings::keys::PLAYLISTS_INCLUDE_VOD, &target);
    let mut options = ImportOptions::new(now).with_vod(include_vod, include_vod);
    // Bound the whole live/VOD catalog download, including slow Xtream boxes.
    options.limits.elapsed = CATALOG_IMPORT_TIME;
    #[cfg(debug_assertions)]
    {
        options.allow_private_fixture =
            cfg!(feature = "test-support") || crate::build_config::DEV_PLAYLIST.is_some();
    }
    options
}

fn create_catalog_stage(services: &Services) -> Result<CatalogStage, ImportFailure> {
    let _ = std::fs::create_dir_all(services.directory());
    CatalogStage::create_in_with(services.directory(), |path| {
        let path = path.to_owned();
        services.blocking(move |store| store.open_staging_connection(&path))
    })
    .inspect_err(|error| log::warn!("iptv data: catalog staging failed: {error}"))
    .map_err(|_| ImportFailure::new(FailureKind::Storage))
}

async fn import_staged_catalog(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    publication: &Publication,
    has_stored_guide: bool,
    now: i64,
) -> Result<(NetClient, CatalogStage, Catalog), ImportFailure> {
    let net = NetClient::new(services).map_err(|error| {
        log::warn!("iptv data: network client setup failed: {error}");
        ImportFailure::new(FailureKind::Network)
    })?;
    let options = import_options(services, job, now);
    let stage = create_catalog_stage(services)?;
    let (catalog, stage) = download_catalog(
        services,
        job,
        tx,
        cancel,
        publication,
        has_stored_guide,
        now,
        &net,
        &options,
        stage,
    )
    .await?;
    Ok((net, stage, catalog))
}

async fn download_catalog(
    services: &Services,
    job: &Job,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
    publication: &Publication,
    has_stored_guide: bool,
    now: i64,
    net: &NetClient,
    options: &ImportOptions,
    mut stage: CatalogStage,
) -> Result<(Catalog, CatalogStage), ImportFailure> {
    let mut progress = Forward(tx);
    let mut catalog = match import_source_into(
        net,
        &job.source.config,
        options,
        &mut progress,
        cancel,
        &mut stage,
    )
    .await
    {
        Ok(catalog) => catalog,
        Err(error) => {
            log::warn!("iptv data: {:?} import failed: {error}", job.source.kind());
            if !matches!(error, ImportError::Canceled) && !job.edit {
                record_failure(
                    services,
                    job.stored,
                    stored::error_kind(&error),
                    publication,
                );
            }
            if let Some(id) = job.stored.filter(|_| has_stored_guide) {
                match stored::guide_inputs(services, id) {
                    Ok(inputs) => {
                        send_next_check(services, tx, id, &inputs.guides, inputs.matching())
                    }
                    Err(error) => stored::unreadable(services, tx, id, now, &error),
                }
            }
            return Err(wizard_map::failure_of(&error));
        }
    };
    if catalog.is_empty() {
        if !job.edit {
            record_failure(services, job.stored, ImportErrorKind::Other, publication);
        }
        return Err(ImportFailure::new(FailureKind::EmptyPlaylist));
    }
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
            return Err(ImportFailure::new(FailureKind::Storage));
        }
    }
    Ok((catalog, stage))
}

/// The guide sources an import starts from: the stored source's, or the
/// guide typed for a new source.
pub(crate) fn planned_guides(services: &Services, job: &Job) -> Vec<GuideSource> {
    match (job.stored, &job.epg_url) {
        (Some(id), _) => stored::guide_sources(services, id),
        (None, Some(url)) => vec![guides::user_source(
            guides::FIRST_USER_FEED,
            url.clone(),
            None,
        )],
        (None, None) => Vec::new(),
    }
}

/// A downloaded guide not stored yet: how it matched, and its programs
/// staged in a file next to the store (deleted when this is dropped, so a
/// guide that is never kept leaves the stored one as it was).
pub(crate) struct Downloaded {
    pub import: EpgImport,
    pub stage: EpgStage,
}

/// Stores a downloaded guide and reads it back without descriptions (the
/// stored one, if any, when the store fails or one of `stops` superseded
/// the download: `stored::replace`).
pub(crate) async fn keep_guide(
    services: &Services,
    id: SourceId,
    guide: Downloaded,
    guides: &[EpgSourceConfig],
    matching: stored::Matching<'_>,
    now: i64,
    stops: &[&CancelToken],
) -> GuideStore {
    if let Err(e) =
        stored::replace_published(services, id, guide, guides, matching, now, stops).await
    {
        log::warn!("iptv data: could not store the guide: {e}");
    }
    stored::load_guide(services, id, now).unwrap_or_default()
}

/// Downloads `guides` for the catalog's channels with the user's guide
/// choices, streaming the programs to a stage file (memory stays small
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
    // The stage file belongs to the owner the client reads for.
    let services = net.services();
    let directory = services.directory().to_owned();
    let mut stage = services
        .call(move |store| EpgStage::create_in_store(store, directory))
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
type SavedCatalog = (
    SourceId,
    CatalogCounts,
    Vec<GuideSource>,
    iptv_guide::playback::SourceAuthority,
);
pub(crate) fn save(
    services: &Services,
    job: &Job,
    catalog: &Catalog,
    planned: &[GuideSource],
    stage: CatalogStage,
    publication: &Publication,
) -> Result<SavedCatalog, PublicationError> {
    let job = job.clone();
    let guide_urls = catalog.guide_urls();
    let planned = planned.to_vec();
    let publication = publication.clone();
    // Lists of movies or series that failed while the rest imported: kept
    // with the import so an empty Movies screen offers Retry, not Refresh.
    let failed = |kind| {
        catalog
            .on_demand_failures
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, e)| stored::error_kind(e))
    };
    let on_demand = (failed(ContentKind::Movie), failed(ContentKind::Series));
    let saved = services.blocking(move |store| {
        if !publication.current(store)? {
            return Ok(Err(PublicationError::Superseded));
        }
        if !crate::parental::import_allowed(store, &job, &publication.authorization)? {
            return Ok(Err(PublicationError::Denied));
        }
        let mut stage = stage;
        stage.set_on_demand_errors(on_demand.0, on_demand.1);
        let (id, counts, sources) = save_in(store, &job, guide_urls, planned, stage)?;
        let stored = store.source(id)?.ok_or(StoreError::SourceNotFound(id))?;
        if stored.source.config != job.source.config {
            return Ok(Err(PublicationError::Superseded));
        }
        Ok(Ok((
            id,
            counts,
            sources,
            super::playback::source_authority(&stored),
        )))
    })??;
    Ok(saved)
}

/// The new browser source's required guide, staged before publication.
struct RequiredGuide {
    download: Downloaded,
    fingerprint: u64,
    now: i64,
    revoked: CancelToken,
}

enum GuidePhase {
    Staged(Box<RequiredGuide>),
    Writing(Box<stored::GuidePublish>),
    Ready(Store),
}

/// [`save`], where the host keeps the store: the catalog and any required
/// guide run in slices on one publish copy. Natively it is [`save`] itself.
async fn save_published(
    services: &Services,
    job: &Job,
    catalog: &Catalog,
    planned: &[GuideSource],
    stage: CatalogStage,
    publication: &Publication,
    required_guide: Option<RequiredGuide>,
) -> Result<SavedCatalog, PublicationError> {
    if services.ports().store_host.is_none() {
        return save(services, job, catalog, planned, stage, publication);
    }
    let failed = |kind| {
        catalog
            .on_demand_failures
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, e)| stored::error_kind(e))
    };
    let outcome = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let guide_revoked = required_guide.as_ref().map(|guide| guide.revoked.clone());
    let published = services
        .publish(CatalogPublish {
            job: job.clone(),
            guide_urls: catalog.guide_urls(),
            planned: planned.to_vec(),
            stage: Some(stage),
            on_demand: (failed(ContentKind::Movie), failed(ContentKind::Series)),
            publication: publication.clone(),
            outcome: outcome.clone(),
            source: None,
            provisional_source: None,
            running: crate::publish::Replacing::Idle,
            guide: required_guide.map(|guide| GuidePhase::Staged(Box::new(guide))),
            guide_revoked: guide_revoked.clone(),
        })
        .await;
    match (
        published,
        outcome.load(std::sync::atomic::Ordering::Relaxed),
    ) {
        (Ok(saved), _) => saved,
        (Err(_), SUPERSEDED) => Err(PublicationError::Superseded),
        // Canceled between its slices ([`CatalogPublish::canceled`]).
        (Err(_), _)
            if publication.cancel.is_canceled()
                || guide_revoked.is_some_and(|token| token.is_canceled()) =>
        {
            Err(PublicationError::Superseded)
        }
        (Err(_), DENIED) => Err(PublicationError::Denied),
        (Err(error), _) => Err(PublicationError::Store(error)),
    }
}

/// [`CatalogPublish::outcome`]: superseded before it was written.
const SUPERSEDED: u8 = 1;
/// [`CatalogPublish::outcome`]: parental controls refused it.
const DENIED: u8 = 2;

/// A catalog publish as a sliced job ([`save_published`]).
struct CatalogPublish {
    job: Job,
    guide_urls: Vec<SecretUrl>,
    planned: Vec<GuideSource>,
    stage: Option<CatalogStage>,
    on_demand: (Option<ImportErrorKind>, Option<ImportErrorKind>),
    publication: Publication,
    /// Why `prepare` refused it, when it did ([`SUPERSEDED`], [`DENIED`]).
    outcome: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// The source written, including one that exists only on the copy.
    source: Option<SourceId>,
    /// An ordinary add's live row, removed if its replacement fails.
    provisional_source: Option<SourceId>,
    running: crate::publish::Replacing<iptv_store_sqlite::CatalogReplacement<'static, Store>>,
    guide: Option<GuidePhase>,
    guide_revoked: Option<CancelToken>,
}

impl crate::publish::Publish for CatalogPublish {
    type Output = Result<SavedCatalog, PublicationError>;

    fn prepare(&mut self, live: &mut Store) -> Result<(), StoreError> {
        if !self.publication.current(live)? {
            self.outcome
                .store(SUPERSEDED, std::sync::atomic::Ordering::Relaxed);
            return Err(StoreError::Unavailable("import superseded"));
        }
        if !crate::parental::import_allowed(live, &self.job, &self.publication.authorization)? {
            self.outcome
                .store(DENIED, std::sync::atomic::Ordering::Relaxed);
            return Err(StoreError::Unavailable(
                "import blocked by parental controls",
            ));
        }
        self.source = self.job.stored;
        if self.source.is_none() && self.guide.is_none() {
            let id = live.add_source(&self.job.source)?;
            self.source = Some(id);
            self.provisional_source = Some(id);
        }
        Ok(())
    }

    fn start(&mut self, mut copy: Store) -> Result<(), StoreError> {
        let id = if let Some(id) = self.source {
            id
        } else {
            // A new source with a required guide first becomes visible
            // to live readers when both replacements have swapped in.
            let id = copy.add_source(&self.job.source)?;
            self.source = Some(id);
            id
        };
        let mut stage = self
            .stage
            .take()
            .ok_or(StoreError::Unavailable("the catalog publish started twice"))?;
        stage.set_on_demand_errors(self.on_demand.0, self.on_demand.1);
        let mut publication = iptv_store_sqlite::StagePublication::new().provider_guides(
            std::mem::take(&mut self.guide_urls),
            self.job
                .stored
                .is_none()
                .then(|| std::mem::take(&mut self.planned)),
        );
        if self.job.edit {
            publication = publication.details(self.job.source.clone());
        }
        self.running = crate::publish::Replacing::Stepping(
            iptv_store_sqlite::CatalogReplacement::from_stage(copy, id, stage, publication)?,
        );
        Ok(())
    }

    fn step(&mut self, rows: u32) -> Result<bool, StoreError> {
        if let Some(GuidePhase::Writing(guide)) = self.guide.as_mut() {
            if !guide.step(rows)? {
                return Ok(false);
            }
            let Some(GuidePhase::Writing(mut guide)) = self.guide.take() else {
                unreachable!("the guide completed its last slice");
            };
            let mut copy = guide.take_copy().ok_or(crate::publish::MISSING_COPY)?;
            // Feed results and pruning also belong to the provisional add.
            guide.finish(&mut copy)?;
            self.guide = Some(GuidePhase::Ready(copy));
            return Ok(true);
        }
        if matches!(self.guide, Some(GuidePhase::Ready(_))) {
            return Ok(true);
        }
        if !self.running.step(rows)? {
            return Ok(false);
        }
        let Some(GuidePhase::Staged(required)) = self.guide.take() else {
            return Ok(true);
        };
        let required = *required;
        let id = self.source.ok_or(StoreError::Unavailable(
            "the catalog publish was not prepared",
        ))?;
        let sources = self
            .running
            .report()
            .and_then(|report| report.guides.as_deref())
            .unwrap_or_default();
        let configs = stored::configs_for(sources, &self.job.source.config);
        let copy = self
            .running
            .take_copy()
            .ok_or(crate::publish::MISSING_COPY)?;
        let mut guide = Box::new(stored::GuidePublish::new(
            id,
            required.download,
            &configs,
            required.fingerprint,
            required.now,
            stored::Stops::of(&[&required.revoked]),
        ));
        guide.start(copy)?;
        self.guide = Some(GuidePhase::Writing(guide));
        Ok(false)
    }

    fn take_copy(&mut self) -> Option<Store> {
        match self.guide.take() {
            Some(GuidePhase::Ready(copy)) => Some(copy),
            Some(GuidePhase::Writing(mut guide)) => guide.take_copy(),
            Some(GuidePhase::Staged(_)) | None => self.running.take_copy(),
        }
    }

    fn finish(self: Box<Self>, live: &mut Store) -> Result<Self::Output, StoreError> {
        let id = self.source.ok_or(StoreError::Unavailable(
            "the catalog publish was not prepared",
        ))?;
        let published = self.running.done().ok_or(StoreError::Unavailable(
            "the catalog publish did not finish",
        ))?;
        log::info!(
            "iptv data: saved {} channels for source {}",
            published.counts.channels,
            id.0
        );
        let stored = live.source(id)?.ok_or(StoreError::SourceNotFound(id))?;
        if stored.source.config != self.job.source.config {
            return Ok(Err(PublicationError::Superseded));
        }
        Ok(Ok((
            id,
            published.counts,
            published.guides.unwrap_or_default(),
            super::playback::source_authority(&stored),
        )))
    }

    fn failed(&mut self, live: &mut Store) {
        if let Some(id) = self.provisional_source {
            let _ = live.delete_source(id);
        }
    }

    /// The user's Cancel during Saving: `prepare` looked at it once, and
    /// the slices that follow run in later turns.
    fn canceled(&self) -> bool {
        self.publication.cancel.is_canceled()
            || self
                .guide_revoked
                .as_ref()
                .is_some_and(CancelToken::is_canceled)
    }
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
    let details = job.edit.then_some(&job.source);
    let written = store.publish_catalog_stage_with_provider_guides(
        id,
        stage,
        details,
        &guide_urls,
        added.then_some(planned.as_slice()),
    );
    if written.is_err() && added {
        let _ = store.delete_source(id);
    }
    let (counts, sources) = written?;
    log::info!(
        "iptv data: saved {} channels for source {}",
        counts.channels,
        id.0
    );
    Ok((id, counts, sources))
}

/// Notes a failed refresh on the stored source (kind only).
pub(crate) fn record_failure(
    services: &Services,
    stored: Option<SourceId>,
    kind: ImportErrorKind,
    publication: &Publication,
) {
    let Some(id) = stored else {
        return;
    };
    let publication = publication.clone();
    if let Err(e) = services.blocking(move |store| {
        if !publication.current(store)? {
            return Ok(());
        }
        store.record_import_failure(id, kind)
    }) {
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

pub(crate) async fn keep_picked_file(
    services: &Services,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<String> {
    let name = kept_file_name(name);
    let mut random = [0u8; 8];
    (services.ports().entropy)(&mut random)
        .map_err(|_| std::io::Error::other("cannot create document identity"))?;
    let dir = services
        .directory()
        .join("playlists")
        .join(format!("{:016x}", u64::from_le_bytes(random)));
    let path = dir.join(&name);
    let bytes = bytes.to_vec();
    services
        .call(move |store| {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&dir).map_err(|_| {
                iptv_store_sqlite::StoreError::Unavailable(
                    "cannot create private document directory",
                )
            })?;
            super::private_files::write_atomic(store, &path, &mut bytes.as_slice())?;
            Ok(path.to_string_lossy().into_owned())
        })
        .await
        .map_err(|_| std::io::Error::other("private document cannot be saved"))
}

/// Cached startup/switch can avoid a playlist download independently of EPG policy.
pub(crate) fn network_due(services: &Services, id: SourceId, purpose: Purpose) -> bool {
    if !matches!(
        purpose,
        Purpose::Startup | Purpose::Switch | Purpose::Combined
    ) {
        return true;
    }
    services
        .blocking(move |store| Ok((store.source_policy(id)?, store.import_status(id)?)))
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
