//! Aggregated mode owns one cached schedule per enabled playlist. A failed
//! refresh retains that playlist's last good schedule and cannot clear others.
use super::{Imported, Purpose, Streams, library, opening, organization, pages, stored};
use crate::model::SourceId;
use iptv_guide::{
    EpgSchedule,
    combined::{CombinedSchedule, SourceSchedule, split_key},
};
use std::{cell::Cell, rc::Rc};

pub(super) use crate::state::SingleSchedule;
pub(super) fn reset_single(owner: &Rc<crate::Runtime>) {
    owner.single.with(|s| s.borrow_mut().take());
}
pub(super) fn single(owner: &Rc<crate::Runtime>) -> Option<SingleSchedule> {
    owner.single.with(|s| s.borrow().clone())
}
/// Snapshot before Add advances the generation. An unfinished account refresh
/// can still replace a cached source's provisional playback metadata, so that
/// source must go through the normal combined loader instead of being retained.
pub(super) fn ready_single(owner: &Rc<crate::Runtime>) -> Option<SingleSchedule> {
    single(owner)
        .filter(|s| !library::running(owner, s.source) && s.schedule.playback().is_some())
        .map(|mut s| {
            s.pending = library::pending_import(owner, s.source);
            s.revision = library::import_revision(owner, s.source);
            s
        })
}
pub fn remember_single(owner: &Rc<crate::Runtime>, source: SourceId, schedule: Rc<EpgSchedule>) {
    owner.single.with(|s| {
        *s.borrow_mut() = Some(SingleSchedule {
            source,
            schedule,
            limit: owner.account_streams.get(),
            pending: None,
            revision: library::import_revision(owner, source),
        })
    });
}

/// Add already owns the current generation. Keep that generation so its guide
/// download and future guide checks continue after the browser joins sources.
pub(super) fn added(
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
            if active(&owner)
                || owner.generation.with(Cell::get) != generation
                || !has_multiple_enabled(&owner)
            {
                return;
            }
            // Guide messages may have arrived while writes flushed: take the latest
            // schedule, rather than the first channel-only Add result.
            let Some(added) = single(&owner).filter(|s| s.source == source) else {
                return;
            };
            if let Err(e) = crate::library::save_all(&owner, true).await {
                log::warn!("iptv combined: cannot save browsing mode: {e}");
                return;
            }
            if owner.generation.with(Cell::get) != generation || active(&owner) {
                return;
            }
            let Ok(playlists) = library::playlists(&owner) else {
                return;
            };
            let enabled = |id| playlists.iter().any(|p| p.id == id && p.policy.enabled);
            super::source_changed(&owner, None);
            stored::note_shown(&owner, None);
            crate::library::set_all(&owner, true);
            owner.parts.with(|p| p.borrow_mut().clear());
            owner.limits.with(|p| p.borrow_mut().clear());
            let previous = previous.filter(|s| {
                enabled(s.source)
                    && library::import_revision_is_current(&owner, s.source, &s.revision)
            });
            let previous_guide = previous.as_ref().map(|s| (s.source, s.pending.clone()));
            for kept in previous.into_iter().chain(std::iter::once(added)) {
                owner
                    .parts
                    .with(|p| p.borrow_mut().insert(kept.source, kept.schedule));
                owner
                    .limits
                    .with(|p| p.borrow_mut().insert(kept.source, kept.limit));
            }
            reset_single(&owner);
            owner.emit(crate::RuntimeEvent::AccountStreams(None));
            let now = super::now_unix();
            owner.epoch.with(|e| e.set(now - now.rem_euclid(3600)));
            rebuild(&owner);
            // The previous single source's timer belonged to its older generation.
            // Reattach guide work without fetching its playlist/account again.
            if let Some((previous, pending)) = previous_guide.filter(|(id, _)| *id != source) {
                reattach_guide(&owner, previous, generation, pending);
            }
            for playlist in playlists.into_iter().filter(|p| p.policy.enabled) {
                if !owner.parts.with(|p| p.borrow().contains_key(&playlist.id))
                    && let Some(stored) = library::load(&owner, playlist.id)
                {
                    library::run(&owner, stored, Purpose::Combined, false);
                }
            }
        }
    });
}
/// A previous source can finish its original EPG download after Add has moved
/// the browser to a new generation. Keep the worker identity until its final
/// cache is restored; newer imports and explicit guide changes revoke it.
fn reattach_guide(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    generation: u64,
    pending: Option<library::ImportCompletion>,
) {
    let Some(pending) = pending else {
        super::restore_guide(owner, source, generation);
        return;
    };
    owner.spawn({
        let owner = owner.clone();
        async move {
            let current = || {
                owner.generation.with(Cell::get) == generation
                    && includes(&owner, source)
                    && library::import_is_current(&owner, source, &pending)
            };
            if !current() {
                return;
            }
            // Complete this initial restore before the final one so an older cache
            // read cannot arrive after the original download's successful result.
            super::read_guide(
                &owner,
                source,
                generation,
                super::GuideRead::CacheOnly,
                Some(pending.clone()),
                false,
            )
            .await;
            if current() {
                pending.until_finished().await;
            }
            if current() {
                super::read_guide(
                    &owner,
                    source,
                    generation,
                    super::GuideRead::RestoreAndCheck,
                    Some(pending),
                    false,
                )
                .await;
            }
        }
    });
}

pub fn active(owner: &Rc<crate::Runtime>) -> bool {
    owner.all.get()
}
pub fn includes(owner: &Rc<crate::Runtime>, id: SourceId) -> bool {
    active(owner) && library::policy(owner, id).is_ok_and(|p| p.enabled)
}
pub fn owner(owner: &Rc<crate::Runtime>, key: &str) -> Option<(SourceId, String)> {
    if active(owner) {
        let (s, k) = split_key(key)?;
        Some((SourceId(s), k.to_owned()))
    } else {
        Some((owner.shown.get()?, key.to_owned()))
    }
}
/// The normal browser includes every enabled source once there is a choice.
pub fn has_multiple_enabled(owner: &Rc<crate::Runtime>) -> bool {
    library::playlists(owner)
        .is_ok_and(|sources| sources.iter().filter(|s| s.policy.enabled).take(2).count() == 2)
}

pub fn start(owner: &Rc<crate::Runtime>) {
    owner.resume.with(|r| r.set(true));
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
async fn switch_all_ready(owner: &Rc<crate::Runtime>) {
    let generation = owner.generation.with(Cell::get);
    if active(owner) {
        return;
    }
    if let Err(e) = crate::library::save_all(owner, true).await {
        log::warn!("iptv combined: cannot save browsing mode: {e}");
        return;
    }
    if owner.generation.with(Cell::get) != generation || active(owner) {
        return;
    }
    super::next_generation(owner, None);
    reset_single(owner);
    super::source_changed(owner, None);
    stored::note_shown(owner, None);
    crate::library::set_all(owner, true);
    owner.parts.with(|p| p.borrow_mut().clear());
    owner.limits.with(|p| p.borrow_mut().clear());
    owner.emit(crate::RuntimeEvent::AccountStreams(None));
    let now = super::now_unix();
    owner.epoch.with(|e| e.set(now - now.rem_euclid(3600)));
    rebuild(owner);
    if let Ok(playlists) = library::playlists(owner) {
        for playlist in playlists
            .into_iter()
            .filter(|playlist| playlist.policy.enabled)
        {
            if let Some(source) = library::load(owner, playlist.id) {
                library::run(owner, source, Purpose::Combined, false);
            }
        }
    }
}
pub fn leave(owner: &Rc<crate::Runtime>) {
    reset_single(owner);
    owner.resume.with(|r| r.set(false));
    crate::library::set_all(owner, false);
    owner.parts.with(|p| p.borrow_mut().clear());
    owner.limits.with(|p| p.borrow_mut().clear());
    let saved = crate::library::save_all(owner, false);
    owner.spawn(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv combined: cannot save browsing mode: {error}");
        }
    });
}
pub(super) async fn show(
    owner: &Rc<crate::Runtime>,
    imported: Box<Imported>,
    held: Option<opening::Held>,
) {
    if !includes(owner, imported.source) {
        return;
    }
    let source = imported.source;
    let generation = owner.generation.with(Cell::get);
    let revision = library::import_revision(owner, source);
    let current = || {
        includes(owner, source)
            && owner.generation.with(Cell::get) == generation
            && library::import_revision_is_current(owner, source, &revision)
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
    let mut schedule = organization::build(owner, source, groups, channels, guide, now).await;
    if !current() {
        return;
    }
    schedule = match playback {
        Streams::New(context, allowed) => {
            owner.limits.with(|l| {
                let mut l = l.borrow_mut();
                if allowed.is_some() || !l.contains_key(&source) {
                    l.insert(source, allowed);
                }
            });
            schedule.with_playback(context)
        }
        Streams::Current => schedule.with_shared_playback(
            owner
                .parts
                .with(|p| p.borrow().get(&source).and_then(|s| s.shared_playback())),
        ),
    };
    if let Some(lookup) = stored::description_lookup(owner, source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    schedule = match held {
        Some(h) => schedule.held_only(h.from, h.to).held_channels(h.keys),
        None => pages::paged(owner, schedule, source, now).await,
    };
    if !current() {
        return;
    }
    owner
        .parts
        .with(|p| p.borrow_mut().insert(source, Rc::new(schedule)));
    rebuild_ready(owner).await;
    if can_resume && owner.resume.with(Cell::get) {
        resume(owner);
    }
}
pub fn rebuild(owner: &Rc<crate::Runtime>) {
    let task = owner.clone();
    owner.spawn(async move { rebuild_ready(&task).await });
}
async fn rebuild_ready(owner: &Rc<crate::Runtime>) {
    let generation = owner.generation.with(Cell::get);
    let revision = owner.rebuild.with(|r| {
        let next = r.get().wrapping_add(1);
        r.set(next);
        next
    });
    if !active(owner) {
        return;
    }
    let Ok(playlists) = library::playlists(owner) else {
        return;
    };
    let enabled: Vec<_> = playlists
        .iter()
        .filter(|p| p.policy.enabled)
        .map(|p| p.id)
        .collect();
    owner
        .parts
        .with(|p| p.borrow_mut().retain(|id, _| enabled.contains(id)));
    let sources = owner.parts.with(|p| {
        let parts = p.borrow();
        playlists
            .into_iter()
            .filter(|s| s.policy.enabled)
            .filter_map(|s| {
                Some(SourceSchedule {
                    id: s.id.0,
                    name: s.name,
                    schedule: parts.get(&s.id)?.clone(),
                })
            })
            .collect()
    });
    let Some(install_favorites) = crate::favorites::prepare_combined(owner).await else {
        return;
    };
    if !active(owner) {
        return;
    }
    let collapsed = crate::library::collapsed(owner).await.unwrap_or_default();
    if !active(owner)
        || owner.generation.with(Cell::get) != generation
        || owner.rebuild.with(Cell::get) != revision
    {
        return;
    }
    install_favorites();
    owner.emit(crate::RuntimeEvent::ScheduleReady(Rc::new(
        CombinedSchedule::new(sources, owner.epoch.with(Cell::get))
            .with_collapsed_sources(collapsed),
    )));
    pages::replaced(owner);
}

pub fn connection_limit(owner: &Rc<crate::Runtime>, source: SourceId) -> Option<u32> {
    owner
        .limits
        .with(|l| l.borrow().get(&source).copied().flatten())
}
pub fn organization_changed(owner: &Rc<crate::Runtime>, source: SourceId, ticket: u64) {
    let schedule = owner
        .parts
        .with(|parts| parts.borrow().get(&source).cloned());
    owner.spawn({
        let owner = owner.clone();
        async move {
            if let Some(schedule) = schedule {
                let Some(replacement) = organization::apply(&owner, &schedule, source).await else {
                    return;
                };
                let current = owner.parts.with(|parts| {
                    parts
                        .borrow()
                        .get(&source)
                        .is_some_and(|s| s.same_catalogue(&schedule))
                });
                if current && owner.organization_edit_current(source, ticket) {
                    owner.parts.with(|parts| {
                        parts.borrow_mut().insert(source, Rc::new(replacement));
                    });
                    rebuild(&owner);
                }
            }
        }
    });
}
pub fn resume(owner: &Rc<crate::Runtime>) {
    if !crate::settings::flag(&owner.settings(), iptv_settings::keys::RESUME_LAST_CHANNEL)
        || owner.browser().playing
    {
        owner.resume.with(|r| r.set(false));
        return;
    }
    let loaded = crate::commands::combined_resume_key(owner);
    owner.spawn({
        let owner = owner.clone();
        async move {
            let key = loaded.await.ok().flatten();
            if !owner.resume.with(Cell::get) || owner.browser().playing || !active(&owner) {
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
                owner.resume.with(|r| r.set(false));
                owner.emit(crate::RuntimeEvent::ResumeChannel(c));
                owner.emit(crate::RuntimeEvent::ResumeFullscreen);
            }
        }
    });
}

/// Toggle a parent row without changing the channel catalogue or player surface.
pub fn toggle_playlist(owner: &Rc<crate::Runtime>, group: usize) -> bool {
    let schedule = owner.schedule();
    let Some(all) = schedule
        .as_any()
        .and_then(|a| a.downcast_ref::<CombinedSchedule>())
    else {
        return false;
    };
    let Some(collapsed) = all.toggle_playlist(group) else {
        return false;
    };
    let saved = crate::library::save_collapsed(owner, collapsed);
    owner.emit(crate::RuntimeEvent::CatalogChanged);
    owner.spawn({
        let owner = owner.clone();
        async move {
            match saved.await {
                Ok(()) => rebuild(&owner),
                Err(error) => log::warn!("iptv browser: could not save expansion: {error}"),
            }
        }
    });
    true
}
