//! Native catalog and guide storage policy and orchestration.
use super::guides;
use super::imports::{Downloaded, Imported, Msg, Streams};
use super::private_files::Client as NetClient;
use super::store_service::Services;
use iptv_epg::{ChannelKey, DAY, GuideStore, Overrides, UnixTime};
use iptv_import::{
    CancelToken, Decision, EpgSourceConfig, ImportError, NoProgress, RefreshPolicy, Trigger,
    decide, matching_fingerprint, store_options,
};
use iptv_providers::SecretUrl;
use iptv_providers::model::{Channel, ContentKind, Group};
use iptv_settings::{Settings, Target};
use iptv_store_sqlite::{
    ChannelOrder, ChannelQuery, EpgCounts, EpgFeedMeta, EpgWindow, GuideSource, ImportErrorKind,
    SourceId, Store, StoreError, StoredGuide,
};
use std::cell::Cell;
use tokio::sync::mpsc::UnboundedSender as Sender;

/// (`epg.keep_past`, whole days rounded up).
pub fn keep_past_days(settings: &Settings) -> u32 {
    store_options(settings, &Target::Global)
        .past_days_to_keep
        .unwrap_or(u32::MAX)
}

/// The guides' storage options from the settings (`epg.keep_past`,
/// `epg.store_descriptions`).
pub fn with_storage(
    mut configs: Vec<EpgSourceConfig>,
    settings: &Settings,
) -> Vec<EpgSourceConfig> {
    let store = store_options(settings, &Target::Global);
    for c in &mut configs {
        c.store = store;
    }
    configs
}

/// A stored source's live groups and channels, in provider order.
pub(crate) fn catalog(
    services: &Services,
    source: SourceId,
) -> Result<(Vec<Group>, Vec<Channel>), StoreError> {
    services.blocking_read(move |store| catalog_in(store, source))
}

/// [`catalog`] from an open store.
pub(crate) fn catalog_in(
    store: &Store,
    source: SourceId,
) -> Result<(Vec<Group>, Vec<Channel>), StoreError> {
    let groups = store.groups(source, Some(ContentKind::Live))?;
    let query = ChannelQuery {
        include_hidden: true,
        order: ChannelOrder::Provider,
        ..ChannelQuery::default()
    };
    let channels = store
        .load_channels(source, &query)?
        .into_iter()
        .map(|row| row.channel)
        .collect();
    Ok((groups, channels))
}

/// What is stored about `source`'s guide.
pub(crate) fn guide_state(services: &Services, source: SourceId) -> Option<StoredGuide> {
    match services.blocking_read(move |store| store.epg_guide(source)) {
        Ok(guide) => guide,
        Err(e) => {
            log::warn!("iptv data: stored guide state: {e}");
            None
        }
    }
}

/// The part of `source`'s stored guide a schedule shown at `now` holds
/// (`pages::window`: the last day on; older days are paged in), without
/// descriptions; `None` when nothing is stored.
pub(crate) fn load_guide(
    services: &Services,
    source: SourceId,
    now: UnixTime,
) -> Option<GuideStore> {
    let window = super::paging::window(now, keep_past_days(&services.settings()));
    read_guide(services, source, &super::paging::store_window(window))
}

/// `window` of `source`'s stored guide; `None` when nothing is stored.
pub(crate) fn read_guide(
    services: &Services,
    source: SourceId,
    window: &EpgWindow<'_>,
) -> Option<GuideStore> {
    let (from, to, descriptions) = (window.from, window.to, window.descriptions);
    let keys = window.keys.map(<[ChannelKey]>::to_vec);
    match services.blocking_read(move |store| {
        store.load_epg(
            source,
            &EpgWindow {
                from,
                to,
                keys: keys.as_deref(),
                descriptions,
            },
        )
    }) {
        Ok(guide) if guide.bound_count() > 0 => Some(guide),
        Ok(_) => None,
        Err(e) => {
            log::warn!("iptv data: stored guide: {e}");
            None
        }
    }
}

/// `source`'s guide sources in the user's order (empty when unreadable).
pub(crate) fn guide_sources(services: &Services, source: SourceId) -> Vec<GuideSource> {
    services
        .blocking_read(move |store| store.guide_sources(source))
        .unwrap_or_else(|e| {
            log::warn!("iptv data: guide sources: {e}");
            Vec::new()
        })
}

/// The download configs of `sources` (those switched on), with the storage
/// settings of now (a stored config has those it was fetched with).
pub fn configs(settings: &Settings, sources: &[GuideSource]) -> Vec<EpgSourceConfig> {
    let template = EpgSourceConfig::new(iptv_epg::FeedId(0), SecretUrl::new(""));
    let template = with_storage(vec![template], settings).remove(0);
    guides::configs_of(sources, &template)
}

/// An Xtream XMLTV endpoint is derived from the user-entered account, rather
/// than read from response content. Its explicit origin remains trusted.
pub fn configs_for(
    settings: &Settings,
    sources: &[GuideSource],
    source: &iptv_providers::model::SourceConfig,
) -> Vec<EpgSourceConfig> {
    let mut configs = configs(settings, sources);
    if let iptv_providers::model::SourceConfig::Xtream(credentials) = source
        && let Ok(client) = iptv_providers::xtream::XtreamClient::from_credentials(credentials)
    {
        let explicit = client.xmltv_url();
        for cfg in &mut configs {
            if cfg.url.expose() == explicit {
                cfg.content_supplied = false;
            }
        }
    }
    configs
}

/// A stored source's guides without its playlist: its guide sources; for a
/// source whose guides were never arranged, the feeds stored with its
/// guide.
pub(crate) fn current_configs(
    services: &Services,
    source: SourceId,
    state: Option<&StoredGuide>,
) -> Vec<EpgSourceConfig> {
    let sources = guide_sources(services, source);
    let source = services
        .blocking_read(move |store| store.source(source))
        .ok()
        .flatten();
    configs_from_sources(
        &services.settings(),
        &sources,
        state,
        source.as_ref().map(|s| &s.source.config),
    )
}

pub fn configs_from_sources(
    settings: &Settings,
    sources: &[GuideSource],
    state: Option<&StoredGuide>,
    source: Option<&iptv_providers::model::SourceConfig>,
) -> Vec<EpgSourceConfig> {
    if sources.is_empty() {
        let stored = state.map(StoredGuide::configs).unwrap_or_default();
        return with_storage(stored, settings);
    }
    match source {
        Some(source) => configs_for(settings, sources, source),
        None => configs(settings, sources),
    }
}

/// The channels of `source` whose guide the user chose by hand.
pub(crate) fn overrides(services: &Services, source: SourceId) -> Overrides {
    match services.blocking_read(move |store| store.epg_overrides(source)) {
        Ok(rows) => rows
            .into_iter()
            .map(|(id, epg)| (ChannelKey::new(id.as_str()), epg.to_override()))
            .collect(),
        Err(e) => {
            log::warn!("iptv data: guide choices: {e}");
            Overrides::default()
        }
    }
}

/// Whether to download the guide again (the `epg.update_*` settings).
/// What EPG matching reads: the playlist's channels and the user's guide
/// choices.
#[derive(Clone, Copy)]
pub struct Matching<'a> {
    pub channels: &'a [Channel],
    pub overrides: &'a Overrides,
}

impl Matching<'_> {
    pub fn fingerprint(&self) -> u64 {
        matching_fingerprint(self.channels, self.overrides)
    }
}

pub fn plan_with(
    settings: &Settings,
    trigger: Trigger,
    configs: &[EpgSourceConfig],
    matching: Matching<'_>,
    stored: Option<&StoredGuide>,
    now: UnixTime,
) -> Decision {
    let state = stored.map(StoredGuide::refresh_state);
    let fingerprint = matching.fingerprint();
    decide(
        &RefreshPolicy::from_settings(settings, &Target::Global),
        trigger,
        configs,
        fingerprint,
        state.as_ref(),
        now,
    )
}

/// When to look at `source`'s guide again: when the policy will want it
/// (stale, running out, after a back-off), `None` when only a request or a
/// restart would.
pub(crate) fn next_check(
    services: &Services,
    source: SourceId,
    configs: &[EpgSourceConfig],
    matching: Matching<'_>,
    now: UnixTime,
) -> Option<UnixTime> {
    let stored = guide_state(services, source);
    check_at(
        plan_with(
            &services.settings(),
            Trigger::Timer,
            configs,
            matching,
            stored.as_ref(),
            now,
        ),
        now,
    )
}

/// Retain automatic checks after a transient storage/worker failure.
pub(crate) fn retry_check(
    services: &Services,
    tx: &Sender<Msg>,
    source: SourceId,
    configs: &[EpgSourceConfig],
    now: UnixTime,
) {
    if !configs.is_empty()
        && RefreshPolicy::from_settings(&services.settings(), &Target::Global)
            .interval
            .is_some()
    {
        let _ = tx.send(Msg::NextCheck {
            source,
            at: now + RETRY_DUE,
        });
    }
}

/// The time a guide check is due after `decision`.
pub fn check_at(decision: Decision, now: UnixTime) -> Option<UnixTime> {
    match decision {
        Decision::Keep { next_check } => next_check,
        Decision::Backoff { retry_at } => Some(retry_at),
        // Still due (the download was cancelled or not recorded): later.
        Decision::Refresh(_) => Some(now + RETRY_DUE),
        Decision::Discard => None,
    }
}

/// How long a check that is still due waits.
const RETRY_DUE: i64 = 3600;

/// Replaces `source`'s stored guide with `guide` (the feeds that loaded)
/// and drops programmes older than [`keep_past_days`]. A feed that failed
/// is recorded, so automatic refreshes back off; each feed's result is
/// kept for the guide sources screen.
pub(crate) fn replace(
    services: &Services,
    source: SourceId,
    mut guide: Downloaded,
    configs: &[EpgSourceConfig],
    matching: Matching<'_>,
    now: UnixTime,
) -> Result<EpgCounts, StoreError> {
    let feeds = EpgFeedMeta::from_import(&guide.import, configs, now);
    let fingerprint = matching.fingerprint();
    let keep = keep_past_days(&services.settings());
    services.blocking(move |store| {
        let import = &guide.import;

        let counts = store.replace_epg(source, &mut guide.stage, import, &feeds, fingerprint)?;
        let results: Vec<_> = import
            .sources
            .iter()
            .map(|o| (o.id, o.result.as_ref().err().map(error_kind)))
            .collect();
        store.record_epg_feed_results(source, &results)?;

        let pruned = store.prune_epg(now.saturating_sub(i64::from(keep).saturating_mul(DAY)))?;
        if let Some(Err(e)) = import
            .sources
            .iter()
            .map(|o| &o.result)
            .find(|r| r.is_err())
        {
            store.record_epg_failure(source, error_kind(e))?;
        }
        log::info!(
            "iptv data: stored the guide of source {}: {} channels, {} programmes ({pruned} old ones pruned)",
            source.0,
            counts.bindings,
            counts.programmes
        );
        Ok(counts)
    })
}

/// Notes a failed guide download (every source in `configs` failed); the
/// stored guide stays.
pub(crate) fn record_failure(
    services: &Services,
    source: SourceId,
    error: &ImportError,
    configs: &[EpgSourceConfig],
) {
    record_failure_kind(services, source, error_kind(error), configs);
}

/// Notes that the guide could not be downloaded because the network
/// client could not be set up: no source in `configs` was reached, which
/// the refresh policy treats as a network failure of each (it backs off
/// and tries again, as after a failed download).
pub(crate) fn record_client_failure(
    services: &Services,
    source: SourceId,
    configs: &[EpgSourceConfig],
) {
    record_failure_kind(services, source, ImportErrorKind::Network, configs);
}

fn record_failure_kind(
    services: &Services,
    source: SourceId,
    kind: ImportErrorKind,
    configs: &[EpgSourceConfig],
) {
    let results: Vec<_> = configs.iter().map(|c| (c.id, Some(kind))).collect();
    let recorded = services.blocking(move |s| {
        s.record_epg_failure(source, kind)?;
        s.record_epg_feed_results(source, &results)
    });
    if let Err(e) = recorded {
        log::warn!("iptv data: could not record the guide failure: {e}");
    }
}

/// Deletes `source`'s stored guide (no guide is configured any more).
pub(crate) fn discard(services: &Services, source: SourceId) {
    match services.blocking(move |store| store.delete_epg(source)) {
        Ok(true) => log::info!("iptv data: no guide configured; deleted the stored one"),
        Ok(false) => {}
        Err(e) => log::warn!("iptv data: could not delete the stored guide: {e}"),
    }
}
/// The download of `data::stored::update_source`, on a refresh worker.
pub(crate) async fn download_now(
    services: &Services,
    source: SourceId,
    guides: Vec<EpgSourceConfig>,
    tx: &Sender<Msg>,
    cancel: &CancelToken,
) {
    let now = super::imports::now_unix();
    let (groups, channels) = match catalog(services, source) {
        Ok(catalog) => catalog,
        Err(e) => {
            log::warn!("iptv data: stored catalog: {e}");
            retry_check(services, tx, source, &guides, now);
            return;
        }
    };
    let overrides = overrides(services, source);
    let matching = Matching {
        channels: &channels,
        overrides: &overrides,
    };
    log::info!("iptv data: downloading the guide (asked for)");
    match NetClient::new(services) {
        Ok(net) => {
            let result = super::imports::download_guide(
                &net,
                &guides,
                matching,
                now,
                &mut NoProgress,
                cancel,
                None,
            )
            .await;
            match result {
                Ok(import) => {
                    let guide = super::imports::keep_guide(
                        services, source, import, &guides, matching, now,
                    );
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
                Err(ImportError::Cancelled) => {}
                Err(e) => {
                    log::warn!("iptv data: guide: {e}");
                    record_failure(services, source, &e, &guides);
                }
            }
        }
        Err(e) => log::warn!("iptv data: network client setup failed: {e}"),
    }
    super::imports::send_next_check(services, tx, source, &guides, matching);
}

/// The stored kind of a failed import (kind only: no URL, no body).
pub fn error_kind(error: &ImportError) -> ImportErrorKind {
    match error {
        ImportError::InvalidConfig(_) => ImportErrorKind::InvalidConfig,
        ImportError::Cancelled => ImportErrorKind::Cancelled,
        ImportError::Network { .. } => ImportErrorKind::Network,
        ImportError::HttpStatus { .. } => ImportErrorKind::HttpStatus,
        ImportError::Auth { .. } => ImportErrorKind::AccountRejected,
        ImportError::Parse { .. } | ImportError::Limit { .. } => ImportErrorKind::InvalidBody,
        ImportError::Portal { .. } | ImportError::Empty { .. } | ImportError::Storage(_) => {
            ImportErrorKind::Other
        }
    }
}

/// `data.rs` put `source`'s schedule on screen.
pub fn note_shown(owner: &super::Runtime, source: Option<SourceId>) {
    owner.shown.with(|s| s.set(source));
}

/// How the guide on screen was last updated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideStatus {
    /// No playlist on screen.
    NoSource,
    Updating,
    /// Nothing stored (not downloaded yet, or cleared).
    Empty,
    Updated {
        at: UnixTime,
        channels: u64,
    },
    Failed {
        at: UnixTime,
        kind: ImportErrorKind,
    },
}

/// The status of the guide on screen (one indexed read of the store).
pub fn guide_status(owner: &super::Runtime) -> GuideStatus {
    let sources = shown_sources(owner);
    if sources.is_empty() {
        return if owner.status.with(|status| status.borrow().combined) {
            GuideStatus::Empty
        } else {
            GuideStatus::NoSource
        };
    }
    if sources.iter().any(|&s| updating(owner, s)) {
        return GuideStatus::Updating;
    }
    let mut updated = None;
    let mut channels = 0;
    for source in sources {
        match status_of(cached_guide_state(owner, source).as_ref()) {
            failed @ GuideStatus::Failed { .. } => return failed,
            GuideStatus::Updated { at, channels: n } => {
                updated = Some(updated.map_or(at, |old: i64| old.min(at)));
                channels += n;
            }
            _ => {}
        }
    }
    updated.map_or(GuideStatus::Empty, |at| GuideStatus::Updated {
        at,
        channels,
    })
}

/// Individual mode has one guide owner; aggregate commands cover every
/// enabled source, with failures and feeds still managed per playlist.
pub fn shown_sources(owner: &super::Runtime) -> Vec<SourceId> {
    if let Some(source) = owner.shown.with(Cell::get) {
        return vec![source];
    }
    owner.status.with(|status| {
        let status = status.borrow();
        if status.combined {
            status.enabled.clone()
        } else {
            Vec::new()
        }
    })
}

/// The source whose guide is on screen.
pub fn shown(owner: &super::Runtime) -> Option<SourceId> {
    owner.shown.with(Cell::get)
}

/// An update of `source`'s guide asked for by hand is running.
pub fn updating(owner: &super::Runtime, source: SourceId) -> bool {
    owner.updating.with(|u| u.borrow().contains(&source))
}

pub use iptv_library::SourceLine;

/// `source`'s guide sources in order, each with its last result (one
/// short read of the store).
pub fn source_lines(owner: &super::Runtime, source: SourceId) -> Vec<SourceLine> {
    owner.status.with(|status| {
        status
            .borrow()
            .lines
            .get(&source)
            .cloned()
            .unwrap_or_default()
    })
}

pub fn status_of(stored: Option<&iptv_library::GuideSnapshot>) -> GuideStatus {
    let Some(g) = stored else {
        return GuideStatus::Empty;
    };
    match (g.last_error, g.attempted_at, g.replaced_at) {
        (Some(kind), Some(at), _) => GuideStatus::Failed { at, kind },
        (_, _, Some(at)) => GuideStatus::Updated {
            at,
            channels: g.bound_channels,
        },
        _ => GuideStatus::Empty,
    }
}

pub fn cached_guide_state(
    owner: &super::Runtime,
    source: SourceId,
) -> Option<iptv_library::GuideSnapshot> {
    owner
        .status
        .with(|status| status.borrow().guides.get(&source).map(guide_snapshot))
}

/// What the views see of a stored guide: how its last update went, without
/// its feeds or their configuration.
fn guide_snapshot(guide: &StoredGuide) -> iptv_library::GuideSnapshot {
    iptv_library::GuideSnapshot {
        replaced_at: guide.replaced_at,
        attempted_at: guide.attempted_at,
        last_error: guide.last_error,
        bound_channels: guide.bound_channels,
    }
}

pub fn next_guide_edit(owner: &super::Runtime, source: SourceId) -> u64 {
    owner.edits.with(|edits| {
        let mut edits = edits.borrow_mut();
        let generation = edits.entry(source).or_default();
        *generation += 1;
        *generation
    })
}

pub fn guide_edit_is_current(owner: &super::Runtime, source: SourceId, generation: u64) -> bool {
    owner
        .edits
        .with(|edits| edits.borrow().get(&source) == Some(&generation))
}

#[cfg(test)]
mod tests {
    use iptv_epg::StoreOptions;
    use iptv_import::RefreshReason;
    use iptv_settings::Value;
    use iptv_store_sqlite::StoredEpgFeed;

    use super::*;

    const NOW: UnixTime = 1_800_000_000;

    /// The download configs under the default settings.
    fn configs(sources: &[GuideSource]) -> Vec<EpgSourceConfig> {
        super::configs(&Settings::new(), sources)
    }

    /// The refresh decision under the default settings.
    fn plan(
        trigger: Trigger,
        configs: &[EpgSourceConfig],
        matching: Matching<'_>,
        stored: Option<&StoredGuide>,
        now: UnixTime,
    ) -> Decision {
        plan_with(&Settings::new(), trigger, configs, matching, stored, now)
    }

    #[test]
    fn guide_edits_debounce_within_a_source_without_cancelling_other_sources() {
        let owner = super::super::Runtime::default();
        let a = SourceId(9001);
        let b = SourceId(9002);
        let first_a = next_guide_edit(&owner, a);
        let first_b = next_guide_edit(&owner, b);
        assert!(guide_edit_is_current(&owner, a, first_a));
        assert!(guide_edit_is_current(&owner, b, first_b));
        let second_a = next_guide_edit(&owner, a);
        assert!(!guide_edit_is_current(&owner, a, first_a));
        assert!(guide_edit_is_current(&owner, a, second_a));
        assert!(guide_edit_is_current(&owner, b, first_b));
    }

    /// A playlist's guide sources: the provider's own `xmltv.php`, after a
    /// guide the user typed when there is one.
    fn sources(typed: bool) -> Vec<GuideSource> {
        let user = typed.then(|| {
            guides::user_source(
                guides::FIRST_USER_FEED,
                SecretUrl::new("http://typed.example.test/guide.xml".to_owned()),
                None,
            )
        });
        let provider = [SecretUrl::new(
            "http://guide.example.test/xmltv.php".to_owned(),
        )];
        guides::sync_provider(&user.into_iter().collect::<Vec<_>>(), &provider)
    }

    fn none() -> Overrides {
        Overrides::default()
    }

    fn nothing(overrides: &Overrides) -> Matching<'_> {
        Matching {
            channels: &[],
            overrides,
        }
    }

    /// What the store holds after a refresh of `feeds` an hour ago.
    fn stored(feeds: &[EpgSourceConfig]) -> StoredGuide {
        StoredGuide {
            replaced_at: Some(NOW - 3600),
            attempted_at: Some(NOW - 3600),
            last_error: None,
            match_fingerprint: Some(matching_fingerprint(&[], &Overrides::default())),
            bound_channels: 10,
            ends_at: Some(NOW + 3 * DAY),
            feeds: feeds
                .iter()
                .map(|config| StoredEpgFeed {
                    meta: EpgFeedMeta {
                        config: config.clone(),
                        fetched_at: NOW - 3600,
                        etag: None,
                        last_modified: None,
                        bytes: 1,
                    },
                    channel_count: 10,
                    programme_count: 100,
                })
                .collect(),
        }
    }

    #[test]
    fn a_fresh_stored_guide_is_kept_at_startup() {
        for typed in [false, true] {
            let guides = configs(&sources(typed));
            let stored = stored(&guides);
            let o = none();
            assert!(matches!(
                plan(
                    Trigger::Startup,
                    &configs(&sources(typed)),
                    nothing(&o),
                    Some(&stored),
                    NOW
                ),
                Decision::Keep { .. }
            ));
        }
    }

    #[test]
    fn a_v5_arrangement_keeps_its_stored_feeds() {
        // As a v5 build stored a typed guide (feed 1, priority 1000) and an
        // Xtream source's own (feed 2, priority 0).
        let c = configs(&sources(true));
        assert_eq!(
            c.iter().map(|c| (c.id.0, c.priority)).collect::<Vec<_>>(),
            [(1, 1000), (2, 0)]
        );
    }

    #[test]
    fn changed_guide_sources_download_the_guide_again() {
        let stored = stored(&configs(&sources(true)));
        let o = none();
        let edits = [
            guides::Edit::SetEnabled(guides::FIRST_USER_FEED, false),
            guides::Edit::Remove(guides::FIRST_USER_FEED),
            guides::Edit::Move(iptv_epg::FeedId(2), -1),
            guides::Edit::Shift(iptv_epg::FeedId(2), 60),
            guides::Edit::Add {
                url: SecretUrl::new("http://more.example.test/g.xml".to_owned()),
                name: None,
            },
        ];
        for edit in edits {
            let edited = guides::apply(&sources(true), &edit).unwrap();
            assert_eq!(
                plan(
                    Trigger::Timer,
                    &configs(&edited),
                    nothing(&o),
                    Some(&stored),
                    NOW
                ),
                Decision::Refresh(RefreshReason::FeedsChanged),
                "{edit:?}"
            );
        }
        // A new name changes no download.
        let renamed = guides::apply(
            &sources(true),
            &guides::Edit::Rename(guides::FIRST_USER_FEED, Some("Mine".into())),
        )
        .unwrap();
        assert!(matches!(
            plan(
                Trigger::Timer,
                &configs(&renamed),
                nothing(&o),
                Some(&stored),
                NOW
            ),
            Decision::Keep { .. }
        ));
    }

    #[test]
    fn a_guide_chosen_by_hand_is_matched_again() {
        let guides = configs(&sources(false));
        let stored = stored(&guides);
        let mut o = none();
        o.insert(ChannelKey::new("c1"), iptv_epg::Override::NoEpg);
        assert_eq!(
            plan(Trigger::Timer, &guides, nothing(&o), Some(&stored), NOW),
            Decision::Refresh(RefreshReason::ChannelsChanged)
        );
    }

    #[test]
    fn a_new_source_downloads_its_guide() {
        let o = none();
        assert_eq!(
            plan(
                Trigger::PlaylistChanged,
                &configs(&sources(false)),
                nothing(&o),
                None,
                NOW
            ),
            Decision::Refresh(RefreshReason::NeverFetched)
        );
    }

    #[test]
    fn the_next_check_is_when_the_guide_gets_stale_or_runs_short() {
        let guides = configs(&sources(false));
        let mut stored = stored(&guides);
        let o = none();
        let next = |s: &StoredGuide| {
            check_at(
                plan(Trigger::Timer, &guides, nothing(&o), Some(s), NOW),
                NOW,
            )
        };
        // Fetched an hour ago, programmes for 3 days: stale in 23 h.
        assert_eq!(next(&stored), Some(NOW - 3600 + DAY));
        // Programmes end in 20 h: 12 h before they run out.
        stored.ends_at = Some(NOW + 20 * 3600);
        assert_eq!(next(&stored), Some(NOW + 8 * 3600));
        // A failed attempt 10 min ago: retry an hour after it.
        stored.last_error = Some(ImportErrorKind::Network);
        stored.attempted_at = Some(NOW - 600);
        assert_eq!(next(&stored), Some(NOW + 3000));
        // No guide configured any more: no checks.
        assert_eq!(
            check_at(
                plan(Trigger::Timer, &[], nothing(&o), Some(&stored), NOW),
                NOW
            ),
            None
        );
    }

    #[test]
    fn the_refresh_follows_the_update_settings() {
        let guides = configs(&sources(false));
        let mut old = stored(&guides);
        old.replaced_at = Some(NOW - DAY);
        let fresh = stored(&guides);
        let o = none();
        let decide = |s: &Settings, trigger, stored: &StoredGuide| {
            plan_with(s, trigger, &guides, nothing(&o), Some(stored), NOW)
        };
        let mut s = Settings::new();
        assert!(matches!(
            decide(&s, Trigger::Startup, &old),
            Decision::Refresh(RefreshReason::Stale { .. })
        ));
        assert_eq!(
            decide(&s, Trigger::PlaylistChanged, &fresh),
            Decision::Refresh(RefreshReason::PlaylistChanged)
        );
        // Not at start: the next timer does it.
        s.set("epg.update_on_start", Value::Bool(false)).unwrap();
        assert!(matches!(
            decide(&s, Trigger::Startup, &old),
            Decision::Keep {
                next_check: Some(_)
            }
        ));
        assert!(matches!(
            decide(&s, Trigger::Timer, &old),
            Decision::Refresh(_)
        ));
        // Interval 0: only on request.
        s.set("epg.update_interval", Value::Duration(0)).unwrap();
        assert_eq!(
            decide(&s, Trigger::Timer, &old),
            Decision::Keep { next_check: None }
        );
        assert_eq!(
            decide(&s, Trigger::Manual, &old),
            Decision::Refresh(RefreshReason::Requested)
        );
        // A changed playlist no longer refreshes a fresh guide.
        s.set("epg.update_on_playlist_change", Value::Bool(false))
            .unwrap();
        assert_eq!(
            decide(&s, Trigger::PlaylistChanged, &fresh),
            Decision::Keep { next_check: None }
        );
    }

    #[test]
    fn storage_settings_travel_with_the_guides() {
        let mut s = Settings::new();
        assert_eq!(keep_past_days(&s), 7, "a week by default");
        s.set("epg.keep_past", Value::Duration(3 * DAY * 1000))
            .unwrap();
        s.set("epg.store_descriptions", Value::Bool(false)).unwrap();
        assert_eq!(keep_past_days(&s), 3);
        let guides = with_storage(configs(&sources(false)), &s);
        assert_eq!(
            guides[0].store,
            StoreOptions {
                store_descriptions: false,
                past_days_to_keep: Some(3)
            }
        );
        // A guide fetched with other storage settings is downloaded again.
        let fetched = stored(&configs(&sources(false)));
        let o = none();
        assert_eq!(
            plan_with(
                &s,
                Trigger::Startup,
                &guides,
                nothing(&o),
                Some(&fetched),
                NOW
            ),
            Decision::Refresh(RefreshReason::FeedsChanged)
        );
    }

    #[test]
    fn the_status_says_how_the_last_update_went() {
        let guides = configs(&sources(false));
        assert_eq!(status_of(None), GuideStatus::Empty);
        let mut g = stored(&guides);
        assert_eq!(
            status_of(Some(&guide_snapshot(&g))),
            GuideStatus::Updated {
                at: NOW - 3600,
                channels: 10
            }
        );
        g.last_error = Some(ImportErrorKind::Network);
        g.attempted_at = Some(NOW - 60);
        assert_eq!(
            status_of(Some(&guide_snapshot(&g))),
            GuideStatus::Failed {
                at: NOW - 60,
                kind: ImportErrorKind::Network
            }
        );
    }

    #[test]
    fn a_failed_imports_kind_is_stored_without_its_details() {
        use iptv_import::{AuthProblem, FetchError, FetchErrorKind, ParseProblem, Step};
        use iptv_providers::{StalkerError, StalkerErrorKind};
        let cases = [
            (
                ImportError::InvalidConfig("no link".into()),
                ImportErrorKind::InvalidConfig,
            ),
            (ImportError::Cancelled, ImportErrorKind::Cancelled),
            (
                ImportError::Network {
                    step: Step::Playlist,
                    error: FetchError::new(FetchErrorKind::Connect, "refused"),
                },
                ImportErrorKind::Network,
            ),
            (
                ImportError::HttpStatus {
                    step: Step::Playlist,
                    status: 500,
                },
                ImportErrorKind::HttpStatus,
            ),
            (
                ImportError::Auth {
                    step: Step::AccountInfo,
                    problem: AuthProblem::HttpStatus(401),
                },
                ImportErrorKind::AccountRejected,
            ),
            (
                ImportError::Parse {
                    step: Step::Playlist,
                    problem: ParseProblem::NotAPlaylist,
                },
                ImportErrorKind::InvalidBody,
            ),
            (
                ImportError::Portal {
                    step: Step::Authentication,
                    error: StalkerError::with_text(StalkerErrorKind::Portal, None),
                },
                ImportErrorKind::Other,
            ),
            (
                ImportError::Empty {
                    step: Step::Playlist,
                },
                ImportErrorKind::Other,
            ),
            (ImportError::Storage("disk".into()), ImportErrorKind::Other),
        ];
        for (error, kind) in cases {
            assert_eq!(error_kind(&error), kind, "{error:?}");
        }
    }

    #[test]
    fn a_day_old_guide_is_downloaded_again() {
        let guides = configs(&sources(false));
        let mut stored = stored(&guides);
        stored.replaced_at = Some(NOW - DAY);
        let o = none();
        assert!(matches!(
            plan(Trigger::Startup, &guides, nothing(&o), Some(&stored), NOW),
            Decision::Refresh(RefreshReason::Stale { .. })
        ));
    }
}
