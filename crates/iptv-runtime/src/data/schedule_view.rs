use super::*;
use crate::imports::{GuideRead, Imported, Streams};
use crate::state::SingleSchedule;
use crate::time::now_unix;
use crate::{Runtime, RuntimeEvent, ScheduleCause};
use iptv_core::guide::Schedule;
use iptv_epg::GuideStore;
use iptv_guide::EpgSchedule;
use iptv_guide::combined::{CombinedSchedule, SourceSchedule};
use iptv_library::SourceId;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

pub(super) enum ScheduleUpdate<'a> {
    Source {
        source: SourceId,
        generation: u64,
        guide_revision: u64,
        schedule: Rc<EpgSchedule>,
    },
    Combined {
        generation: u64,
        revision: u64,
        sources: Vec<SourceSchedule>,
        collapsed: Vec<i64>,
    },
    Organization {
        source: SourceId,
        ticket: u64,
        expected: &'a EpgSchedule,
        schedule: Rc<EpgSchedule>,
    },
    /// Opening, an empty guide, or an expansion toggle leaves page waiters pending.
    Catalog(Rc<dyn Schedule>),
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum CommitOutcome {
    Refused,
    Installed,
    Superseded,
}

/// Install and reconcile synchronously. Source imports and combined rebuilds
/// pass prepared favorites. The sink may read the new schedule or install
/// another one during delivery, so hold no state borrows across emits.
pub(super) fn commit_schedule(
    owner: &Rc<Runtime>,
    update: ScheduleUpdate<'_>,
    install_favorites: impl FnOnce(),
) -> CommitOutcome {
    let catalog_changed = !matches!(update, ScheduleUpdate::Organization { .. });
    let wake_waiters = matches!(
        update,
        ScheduleUpdate::Source { .. } | ScheduleUpdate::Combined { .. }
    );
    let current: Rc<dyn Schedule> = match update {
        ScheduleUpdate::Source {
            source,
            generation,
            guide_revision,
            schedule,
        } => {
            if owner.imports.generation.get() != generation
                || combined::active(owner)
                || crate::stored::guide_edit_revision(owner, source) != guide_revision
            {
                return CommitOutcome::Refused;
            }
            install_favorites();
            remember_single(owner, source, schedule.clone());
            schedule
        }
        ScheduleUpdate::Combined {
            generation,
            revision,
            sources,
            collapsed,
        } => {
            if !combined::active(owner)
                || owner.imports.generation.get() != generation
                || owner.imports.rebuild.get() != revision
            {
                return CommitOutcome::Refused;
            }
            install_favorites();
            let schedule = CombinedSchedule::new(sources, owner.imports.epoch.get())
                .with_collapsed_sources(collapsed);
            Rc::new(schedule)
        }
        ScheduleUpdate::Organization {
            source,
            ticket,
            expected,
            schedule,
        } => {
            let actual = owner.schedule();
            let same_catalog = actual
                .as_any()
                .and_then(|schedule| schedule.downcast_ref::<EpgSchedule>())
                .is_some_and(|actual| expected.same_catalog(actual));
            if owner.catalog.shown_source() != Some(source)
                || !owner.organization_edit_current(source, ticket)
                || !same_catalog
            {
                return CommitOutcome::Refused;
            }
            remember_single(owner, source, schedule.clone());
            schedule
        }
        ScheduleUpdate::Catalog(schedule) => schedule,
    };
    let previous = owner.set_schedule(current.clone());
    let revision = owner.catalog.schedule_revision().wrapping_add(1);
    owner.catalog.set_schedule_revision(revision);
    let cause = if catalog_changed {
        crate::favorites::rebuild_index(owner, &*current);
        let (has_vod, any_vod) = crate::favorites::vod_availability(owner);
        ScheduleCause::Catalog { has_vod, any_vod }
    } else {
        ScheduleCause::Organization
    };
    owner.emit(RuntimeEvent::ScheduleChanged {
        previous,
        current,
        cause,
    });
    if catalog_changed && owner.catalog.schedule_revision() == revision {
        crate::favorites::publish(owner);
    }
    // A nested projection may supersede this commit without waking waiters.
    // Finish the replacement's wakeup even in that case.
    if wake_waiters {
        pages::replaced(owner);
    }
    if owner.catalog.schedule_revision() == revision {
        CommitOutcome::Installed
    } else {
        CommitOutcome::Superseded
    }
}
pub(super) fn remember_single(owner: &Rc<Runtime>, source: SourceId, schedule: Rc<EpgSchedule>) {
    combined::remember_single(
        owner,
        source,
        schedule,
        crate::library::import_revision(owner, source),
    );
}

/// Snapshot before Add advances the generation. An unfinished account refresh
/// can still replace a cached source's provisional playback metadata, so that
/// source must go through the normal combined loader instead of being retained.
pub(super) fn ready_single(owner: &Rc<crate::Runtime>) -> Option<SingleSchedule> {
    combined::single(owner)
        .filter(|s| !crate::library::running(owner, s.source) && s.schedule.playback().is_some())
        .map(|mut s| {
            s.pending = crate::library::pending_import(owner, s.source);
            s.revision = crate::library::import_revision(owner, s.source);
            s
        })
}
/// Add already owns the current generation. Keep that generation so its guide
/// download and future guide checks continue after the browser joins sources.
pub(super) fn join_after_add(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    previous: Option<SingleSchedule>,
) {
    owner.spawn({
        let owner = owner.clone();
        async move {
            if !crate::favorites::flush(&owner).await {
                owner.emit(crate::RuntimeEvent::Notify(
                    "Recent changes could not be saved; try again",
                ));
                return;
            }
            if combined::active(&owner)
                || owner.imports.generation.with(Cell::get) != generation
                || !has_multiple_enabled(&owner)
            {
                return;
            }
            // Guide messages may have arrived while writes flushed: take the latest
            // schedule, rather than the first channel-only Add result.
            let Some(added) = combined::single(&owner).filter(|s| s.source == source) else {
                return;
            };
            if let Err(e) = crate::library::save_all(&owner, true).await {
                log::warn!("iptv combined: cannot save browsing mode: {e}");
                return;
            }
            if owner.imports.generation.with(Cell::get) != generation || combined::active(&owner) {
                return;
            }
            let Ok(playlists) = library::playlists(&owner) else {
                return;
            };
            let enabled = |id| playlists.iter().any(|p| p.id == id && p.policy.enabled);
            source_changed(&owner, None);
            crate::stored::note_shown(&owner, None);
            crate::library::set_all(&owner, true);
            combined::clear_parts(&owner);
            let previous = previous.filter(|s| {
                enabled(s.source)
                    && crate::library::import_revision_is_current(&owner, s.source, &s.revision)
            });
            let previous_guide = previous.as_ref().map(|s| (s.source, s.pending.clone()));
            for kept in previous.into_iter().chain(std::iter::once(added)) {
                combined::retain_single(&owner, kept);
            }
            combined::reset_single(&owner);
            owner.emit(crate::RuntimeEvent::AccountStreams(None));
            combined::reset_epoch(&owner, now_unix());
            rebuild_combined(&owner);
            // The previous single source's timer belonged to its older generation.
            // Reattach guide work without fetching its playlist/account again.
            if let Some((previous, pending)) = previous_guide.filter(|(id, _)| *id != source) {
                reattach_guide(&owner, previous, generation, pending);
            }
            for playlist in playlists.into_iter().filter(|p| p.policy.enabled) {
                if combined::part(&owner, playlist.id).is_none()
                    && let Some(stored) = crate::library::load(&owner, playlist.id)
                {
                    run_import(&owner, stored, Purpose::Combined, false);
                }
            }
        }
    });
}

/// A previous source can finish its original EPG download after Add has moved
/// the browser to a new generation. Keep the worker identity until its final
/// cache is restored; newer imports and explicit guide changes revoke it.
pub(super) fn reattach_guide(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    pending: Option<crate::library::ImportCompletion>,
) {
    let Some(pending) = pending else {
        restore_guide(owner, source, generation);
        return;
    };
    owner.spawn({
        let owner = owner.clone();
        async move {
            let current = || {
                owner.imports.generation.with(Cell::get) == generation
                    && combined_includes(&owner, source)
                    && crate::library::import_is_current(&owner, source, &pending)
            };
            if !current() {
                return;
            }
            // Complete this initial restore before the final one so an older cache
            // read cannot arrive after the original download's successful result.
            read_guide(
                &owner,
                source,
                generation,
                GuideRead::CacheOnly,
                Some(pending.clone()),
                false,
            )
            .await;
            if current() {
                pending.until_finished().await;
            }
            if current() {
                read_guide(
                    &owner,
                    source,
                    generation,
                    GuideRead::RestoreAndCheck,
                    Some(pending),
                    false,
                )
                .await;
            }
        }
    });
}

/// Whether the combined guide may include the enabled playlist.
pub fn combined_includes(owner: &Rc<crate::Runtime>, id: SourceId) -> bool {
    combined::active(owner) && library::policy(owner, id).is_ok_and(|p| p.enabled)
}
/// The normal browser includes every enabled source once there is a choice.
pub fn has_multiple_enabled(owner: &Rc<crate::Runtime>) -> bool {
    library::playlists(owner)
        .is_ok_and(|sources| sources.iter().filter(|s| s.policy.enabled).take(2).count() == 2)
}

pub(super) fn start_combined(owner: &Rc<crate::Runtime>) {
    owner.imports.resume.with(|r| r.set(true));
    // Startup precedes user edits; install the empty real guide at once.
    owner.spawn({
        let owner = owner.clone();
        async move {
            switch_all_ready(&owner).await;
        }
    });
}

pub fn switch_all(owner: &Rc<crate::Runtime>) {
    owner.spawn({
        let owner = owner.clone();
        async move {
            if crate::favorites::flush(&owner).await {
                switch_all_ready(&owner).await;
            } else {
                owner.emit(crate::RuntimeEvent::Notify(
                    "Recent changes could not be saved; try again",
                ));
            }
        }
    });
}
pub(super) async fn switch_all_ready(owner: &Rc<crate::Runtime>) {
    let generation = owner.imports.generation.with(Cell::get);
    if combined::active(owner) {
        return;
    }
    if let Err(e) = crate::library::save_all(owner, true).await {
        log::warn!("iptv combined: cannot save browsing mode: {e}");
        return;
    }
    if owner.imports.generation.with(Cell::get) != generation || combined::active(owner) {
        return;
    }
    next_generation(owner, None);
    combined::reset_single(owner);
    source_changed(owner, None);
    crate::stored::note_shown(owner, None);
    crate::library::set_all(owner, true);
    combined::clear_parts(owner);
    owner.emit(crate::RuntimeEvent::AccountStreams(None));
    combined::reset_epoch(owner, now_unix());
    rebuild_combined(owner);
    if let Ok(playlists) = library::playlists(owner) {
        for playlist in playlists
            .into_iter()
            .filter(|playlist| playlist.policy.enabled)
        {
            if let Some(source) = crate::library::load(owner, playlist.id) {
                run_import(owner, source, Purpose::Combined, false);
            }
        }
    }
}

pub(super) fn leave_combined(owner: &Rc<crate::Runtime>) {
    combined::reset_single(owner);
    owner.imports.resume.with(|r| r.set(false));
    crate::library::set_all(owner, false);
    combined::clear_parts(owner);
    let saved = crate::library::save_all(owner, false);
    owner.spawn(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv combined: cannot save browsing mode: {error}");
        }
    });
}

pub(super) async fn show_combined(
    owner: &Rc<crate::Runtime>,
    imported: Box<Imported>,
    held: Option<opening::Held>,
) {
    if !combined_includes(owner, imported.source) {
        return;
    }
    let source = imported.source;
    let generation = owner.imports.generation.with(Cell::get);
    let revision = crate::library::import_revision(owner, source);
    let guide_revision = crate::stored::guide_edit_revision(owner, source);
    let current = || {
        combined_includes(owner, source)
            && owner.imports.generation.with(Cell::get) == generation
            && crate::library::import_revision_is_current(owner, source, &revision)
            && crate::stored::guide_edit_revision(owner, source) == guide_revision
    };
    let Imported {
        groups,
        channels,
        playback,
        guide,
        now,
        resume: can_resume,
        ..
    } = *imported;
    let mut organization_revision = organization::revision(owner);
    let mut schedule = organization::build(owner, source, groups, channels, guide, now).await;
    if !current() {
        return;
    }
    schedule = match playback {
        Streams::New(context, allowed, authority) => {
            combined::note_account_limit(owner, source, allowed);
            schedule
                .with_playback(context)
                .with_source_authority(Some(authority))
        }
        Streams::Current => {
            let (context, authority) = combined::part(owner, source)
                .map(|s| (s.shared_playback(), s.source_authority()))
                .unwrap_or_default();
            schedule
                .with_shared_playback(context)
                .with_source_authority(authority)
        }
    };
    if let Some(lookup) = stored::description_lookup(owner, source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    schedule = match held {
        Some(h) => schedule.held_only(h.from, h.to).held_channels(h.keys),
        None => pages::paged(owner, schedule, source, now).await,
    };
    while current() && organization::revision(owner) != organization_revision {
        organization_revision = organization::revision(owner);
        if let Some(replacement) = organization::apply(owner, &schedule, source).await {
            schedule = replacement;
        }
    }
    if !current() {
        return;
    }
    combined::store_part(owner, source, Rc::new(schedule));
    rebuild_combined_ready(owner).await;
    if can_resume && owner.imports.resume.with(Cell::get) {
        resume_combined(owner);
    }
}

pub(super) fn rebuild_combined(owner: &Rc<crate::Runtime>) {
    let task = owner.clone();
    owner.spawn(async move {
        rebuild_combined_ready(&task).await;
    });
}

pub(super) async fn rebuild_combined_ready(owner: &Rc<crate::Runtime>) -> bool {
    let generation = owner.imports.generation.with(Cell::get);
    let revision = combined::invalidate_rebuild(owner);
    if !combined::active(owner) {
        return false;
    }
    let Ok(playlists) = library::playlists(owner) else {
        return false;
    };
    let enabled = playlists
        .into_iter()
        .filter(|playlist| playlist.policy.enabled)
        .map(|playlist| (playlist.id, playlist.name))
        .collect();
    let sources = combined::source_schedules(owner, enabled);
    let Some(install_favorites) = crate::favorites::prepare_combined(owner).await else {
        return false;
    };
    if !combined::active(owner) {
        return false;
    }
    let collapsed = crate::library::collapsed(owner).await.unwrap_or_default();
    commit_schedule(
        owner,
        ScheduleUpdate::Combined {
            generation,
            revision,
            sources,
            collapsed,
        },
        install_favorites,
    ) == CommitOutcome::Installed
}

pub(super) async fn replace_combined_organization(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    ticket: u64,
) -> bool {
    let Some(schedule) = combined::part(owner, source) else {
        return false;
    };
    let Some(replacement) = organization::apply(owner, &schedule, source).await else {
        return false;
    };
    let current = combined::part(owner, source).is_some_and(|s| s.same_catalog(&schedule));
    if !current || !owner.organization_edit_current(source, ticket) {
        return false;
    }
    combined::store_part(owner, source, Rc::new(replacement));
    rebuild_combined_ready(owner).await
}

pub(super) fn resume_combined(owner: &Rc<crate::Runtime>) {
    if !crate::settings::flag(&owner.settings(), iptv_settings::keys::RESUME_LAST_CHANNEL)
        || owner.browser().playing
    {
        owner.imports.resume.with(|r| r.set(false));
        return;
    }
    let loaded = crate::commands::combined_resume_key(owner);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let key = loaded.await.ok().flatten();
            if !owner.imports.resume.with(Cell::get)
                || owner.browser().playing
                || !combined::active(&owner)
            {
                return;
            }
            let schedule = owner.schedule();
            if let Some(c) = key.and_then(|k| schedule.find_channel(&k)) {
                // Another playlist's import may call resume while the target portal is
                // still only cached rows. Wait for that owning source's authentication.
                if iptv_guide::combined::epg_channel(&*schedule, c)
                    .is_none_or(|(child, _)| child.playback().is_none())
                {
                    return;
                }
                owner.imports.resume.with(|r| r.set(false));
                if schedule.channel_block(c) != iptv_core::organization::Block::None {
                    return;
                }
                owner.emit(crate::RuntimeEvent::ResumeChannel(c));
                owner.emit(crate::RuntimeEvent::ResumeFullscreen);
            }
        }
    });
}

/// Toggle a parent row without changing the channel catalog or player surface.
pub fn toggle_playlist(owner: &Rc<crate::Runtime>, group: usize) -> bool {
    let Some((projection, collapsed)) = combined::toggled_playlist(owner, group) else {
        return false;
    };
    let saved = crate::library::save_collapsed(owner, collapsed);
    // A following key must see the new rows, even while persistence waits.
    // Invalidate assemblies that began before this projection was chosen.
    combined::invalidate_rebuild(owner);
    commit_schedule(owner, ScheduleUpdate::Catalog(projection), || {});
    owner.spawn({
        let owner = owner.clone();
        async move {
            if let Err(error) = saved.await {
                log::warn!("iptv browser: could not save expansion: {error}");
                owner.emit(crate::RuntimeEvent::Notify(
                    "The change could not be saved. Reopen the list and try again.",
                ));
                // Restore persisted expansion on failure. This read follows all
                // newer queued choices, and later choices invalidate its revision.
                rebuild_combined(&owner);
            }
        }
    });
    true
}

pub fn organization_changed(owner: &Rc<crate::Runtime>, source: SourceId) {
    reload_organization(owner, source, false);
}

/// A watch sort remains eligible for the next guide entry until installed.
pub(crate) fn watch_order_changed(owner: &Rc<crate::Runtime>, source: SourceId) {
    reload_organization(owner, source, true);
}

pub(super) fn reload_organization(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    retry_watch_order: bool,
) {
    owner
        .organization
        .revision()
        .set(owner.organization.revision().get().wrapping_add(1));
    let ticket = owner.begin_organization_edit(source);
    owner.emit(crate::RuntimeEvent::LibraryChanged);
    let task = owner.clone();
    owner.spawn(async move {
        let installed = if combined::active(&task) {
            replace_combined_organization(&task, source, ticket).await
        } else {
            replace_single_organization(&task, source, ticket).await
        };
        if retry_watch_order && !installed {
            task.organization
                .watch_sort_dirty()
                .borrow_mut()
                .insert(source);
        }
    });
}

pub(super) async fn replace_single_organization(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    ticket: u64,
) -> bool {
    if owner.catalog.shown_source() != Some(source) {
        return false;
    }
    let schedule = owner.schedule();
    let Some(epg) = schedule
        .as_any()
        .and_then(|s| s.downcast_ref::<EpgSchedule>())
    else {
        return false;
    };
    let Some(replacement) = organization::apply(owner, epg, source).await else {
        return false;
    };
    commit_schedule(
        owner,
        ScheduleUpdate::Organization {
            source,
            ticket,
            expected: epg,
            schedule: Rc::new(replacement),
        },
        || {},
    ) == CommitOutcome::Installed
}

/// Puts `imported` on screen unless a newer import owns the guide.
pub(crate) fn repair_source_shown(owner: &Rc<crate::Runtime>, source: SourceId) -> bool {
    owner.catalog.shown_source() == Some(source) || combined_includes(owner, source)
}

pub(crate) async fn show_repair(
    owner: &Rc<crate::Runtime>,
    imported: Box<Imported>,
    generation: u64,
    edit: u64,
) {
    if !repair_source_shown(owner, imported.source)
        || crate::stored::guide_edit_revision(owner, imported.source) != edit
    {
        return;
    }
    show(owner, imported, generation, false).await;
}

pub(super) async fn show(
    owner: &std::rc::Rc<crate::Runtime>,
    imported: Box<Imported>,
    generation: u64,
    resume: bool,
) {
    show_part(owner, imported, generation, resume, None, None).await;
}

/// [`show`]; with `held`, its guide holds only those programs (a first
/// screen, `opening.rs`).
///
/// Shows of one source take turns in the order they began: each has
/// several waits, and one that began earlier (with older data) would
/// otherwise replace a later one when it happens to finish last. With
/// `job`, the import that read `imported`: when a guide action revoked it
/// meanwhile ("Delete stored guide data", a channel's guide chosen), the
/// guide it read may predate what that action deleted or matched anew.
/// Its catalog and playback are still shown, with the guide as stored
/// now (read after that action's writes) in place of the one it read.
pub(super) async fn show_part(
    owner: &std::rc::Rc<crate::Runtime>,
    mut imported: Box<Imported>,
    generation: u64,
    resume: bool,
    mut held: Option<opening::Held>,
    job: Option<&crate::library::ImportCompletion>,
) {
    if owner.imports.generation.with(Cell::get) != generation {
        return;
    }
    let source = imported.source;
    let guide_revision = crate::stored::guide_edit_revision(owner, source);
    let _turn = owner.show_turn(source).await;
    if crate::stored::guide_edit_revision(owner, source) != guide_revision {
        return;
    }
    if owner.imports.generation.with(Cell::get) != generation {
        return;
    }
    if let Some(job) = job
        && !crate::library::import_is_current(owner, source, job)
    {
        let stored = crate::commands::stored_guide(owner, source, imported.now).await;
        // Unreadable: without programs rather than with ones that may
        // just have been deleted, until the stored guide can be read.
        imported.guide = stored.unwrap_or_else(|error| {
            log::warn!("iptv data: the stored guide cannot be read: {error}");
            restore_guide(owner, source, generation);
            GuideStore::default()
        });
        held = None;
        if owner.imports.generation.with(Cell::get) != generation {
            return;
        }
    }
    if crate::stored::guide_edit_revision(owner, source) != guide_revision {
        return;
    }
    let first_screen = held.is_some();
    if combined::active(owner) {
        show_combined(owner, imported, held).await;
        if resume {
            resume_combined(owner);
        }
        return;
    }
    if Some(source) != owner.catalog.shown_source() {
        source_changed(owner, Some(source));
    }
    crate::stored::note_shown(owner, Some(source));
    let channels = imported.channels.len();
    let started = crate::time::Instant::now();
    let Imported {
        groups,
        channels: catalog,
        playback,
        guide,
        now,
        resume: resumes,
        ..
    } = *imported;
    let mut organization_revision = organization::revision(owner);
    let schedule = organization::build(owner, source, groups, catalog, guide, now).await;
    if owner.imports.generation.with(Cell::get) != generation
        || combined::active(owner)
        || crate::stored::guide_edit_revision(owner, source) != guide_revision
    {
        return;
    }
    let built = started.elapsed();
    let mut schedule = match playback {
        Streams::New(context, allowed, authority) => {
            owner.emit(crate::RuntimeEvent::AccountStreams(allowed));
            schedule
                .with_playback(context)
                .with_source_authority(Some(authority))
        }
        Streams::Current => {
            let current = owner.schedule();
            let shared = current
                .as_any()
                .and_then(|s| s.downcast_ref::<EpgSchedule>())
                .and_then(EpgSchedule::shared_playback);
            let authority = current
                .as_any()
                .and_then(|s| s.downcast_ref::<EpgSchedule>())
                .and_then(EpgSchedule::source_authority);
            schedule
                .with_shared_playback(shared)
                .with_source_authority(authority)
        }
    };
    if let Some(lookup) = stored::description_lookup(owner, source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    // Its guide is the stored one's recent part (`stored::load_guide`), or
    // only its first screen until that arrives.
    schedule = match held {
        Some(held) => schedule
            .held_only(held.from, held.to)
            .held_channels(held.keys),
        None => pages::paged(owner, schedule, source, now).await,
    };
    if owner.imports.generation.with(Cell::get) != generation
        || combined::active(owner)
        || crate::stored::guide_edit_revision(owner, source) != guide_revision
    {
        return;
    }
    let wrapped = started.elapsed();
    log::info!("iptv data: guide switched to {channels} imported channels");
    let Some(install_favorites) = crate::favorites::prepare_source(owner, Some(source)).await
    else {
        return;
    };
    while organization::revision(owner) != organization_revision {
        organization_revision = organization::revision(owner);
        if let Some(replacement) = organization::apply(owner, &schedule, source).await {
            schedule = replacement;
        }
    }
    let favorites = started.elapsed();
    let outcome = commit_schedule(
        owner,
        ScheduleUpdate::Source {
            source,
            generation,
            guide_revision,
            schedule: Rc::new(schedule),
        },
        install_favorites,
    );
    if outcome == CommitOutcome::Refused {
        return;
    }
    owner.emit(crate::RuntimeEvent::SchedulePresented(vec![
        ("schedule", built),
        ("pages", wrapped),
        ("favorites", favorites),
        ("swapped", started.elapsed()),
    ]));
    if first_screen {
        owner
            .services()
            .mark("first screen shown", format_args!("{channels} channels"));
    }
    owner.emit(crate::RuntimeEvent::SourceReady(resume && resumes));
}

/// Another stored source (or none) is about to be shown: what played and
/// the favorites belong to the old one, and the choice is remembered.
pub(super) fn source_changed(owner: &std::rc::Rc<crate::Runtime>, source: Option<SourceId>) {
    log::info!("iptv data: showing source {:?}", source.map(|id| id.0));
    owner.emit(crate::RuntimeEvent::SourceChanged);
    let (has_vod, any_vod) = crate::favorites::vod_availability(owner);
    owner.emit(crate::RuntimeEvent::FavoritesChanged {
        channels: BTreeSet::new(),
        has_vod,
        any_vod,
    });
    crate::library::set_shown(owner, source);
    if let Some(id) = source {
        library::remember_shown(owner, id);
    }
}

/// No stored source is left: the guide shows nothing real and no timers
/// run (the wizard's first-run state goes over it).
pub(super) fn show_nothing(owner: &std::rc::Rc<crate::Runtime>) {
    combined::reset_single(owner);
    next_generation(owner, None);
    source_changed(owner, None);
    commit_schedule(
        owner,
        ScheduleUpdate::Catalog(Rc::new(EpgSchedule::new(
            Vec::new(),
            GuideStore::default(),
            now_unix(),
        ))),
        || {},
    );
}

/// Work that canceled `source`'s guide timer ended without answering for
/// it: keeps a check unless one is registered or newer work for the source
/// will set it. `asked` is the check the work asked for itself before its
/// handoff was revoked (or the time a guide action gives a check it
/// stopped).
///
/// Without one, with automatic updates the guide is looked at again
/// shortly: the policy is in memory, so an unavailable store cannot
/// suppress its own recovery. With updates on request only
/// (`epg.update_interval` = 0) the policy is not asked anew, which would
/// make up a check: only the one it had asked for before this work, and
/// which this work canceled, is kept at its own time. The refresh policy
/// (`iptv_import::refresh`) sets such checks whatever the interval, for a
/// download that failed (rule 3, the back-off) or is still due (rule 4: a
/// guide never fetched or with changed feeds "is not an update").
/// Current browser generation for async state installation guards.
pub fn generation(owner: &crate::Runtime) -> u64 {
    owner.imports.generation.get()
}
