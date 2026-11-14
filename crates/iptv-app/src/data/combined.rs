//! Aggregated mode owns one cached schedule per enabled playlist. A failed
//! refresh retains that playlist's last good schedule and cannot clear others.
use super::{Imported, Purpose, Streams, library, opening, organization, pages, stored};
use crate::Ui;
use crate::runtime::model::SourceId;
use iptv_guide::{
    EpgSchedule,
    combined::{CombinedSchedule, SourceSchedule, split_key},
};
use std::{cell::Cell, rc::Rc};

pub(super) use crate::runtime::state::SingleSchedule;
pub(super) fn reset_single() {
    crate::runtime().single().with(|s| s.borrow_mut().take());
}
pub(super) fn single() -> Option<SingleSchedule> {
    crate::runtime().single().with(|s| s.borrow().clone())
}
/// Snapshot before Add advances the generation. An unfinished account refresh
/// can still replace a cached source's provisional playback metadata, so that
/// source must go through the normal combined loader instead of being retained.
pub(super) fn ready_single() -> Option<SingleSchedule> {
    single()
        .filter(|s| !library::running(s.source) && s.schedule.playback().is_some())
        .map(|mut s| {
            s.pending = library::pending_import(s.source);
            s.revision = library::import_revision(s.source);
            s
        })
}
pub(crate) fn remember_single(source: SourceId, schedule: Rc<EpgSchedule>) {
    crate::runtime().single().with(|s| {
        *s.borrow_mut() = Some(SingleSchedule {
            source,
            schedule,
            limit: crate::runtime().account_streams().get(),
            pending: None,
            revision: library::import_revision(source),
        })
    });
}

/// Add already owns the current generation. Keep that generation so its guide
/// download and future guide checks continue after the browser joins sources.
pub(super) fn added(ui: Ui, source: SourceId, generation: u64, previous: Option<SingleSchedule>) {
    super::spawn_in_app(async move {
        if !crate::favorites::flush().await {
            crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
                "Recent changes could not be saved; try again",
            ));
            return;
        }
        if active(ui)
            || crate::runtime().generation().with(Cell::get) != generation
            || !has_multiple_enabled()
        {
            return;
        }
        // Guide messages may have arrived while writes flushed: take the latest
        // schedule, rather than the first channel-only Add result.
        let Some(added) = single().filter(|s| s.source == source) else {
            return;
        };
        if let Err(e) = crate::runtime::library::save_all(true).await {
            log::warn!("iptv combined: cannot save browsing mode: {e}");
            return;
        }
        if crate::runtime().generation().with(Cell::get) != generation || active(ui) {
            return;
        }
        let Ok(playlists) = library::playlists() else {
            return;
        };
        let enabled = |id| playlists.iter().any(|p| p.id == id && p.policy.enabled);
        super::source_changed(ui, None);
        stored::note_shown(None);
        crate::runtime::library::set_all(true);
        crate::runtime().parts().with(|p| p.borrow_mut().clear());
        crate::runtime().limits().with(|p| p.borrow_mut().clear());
        let previous = previous.filter(|s| {
            enabled(s.source) && library::import_revision_is_current(s.source, &s.revision)
        });
        let previous_guide = previous.as_ref().map(|s| (s.source, s.pending.clone()));
        for kept in previous.into_iter().chain(std::iter::once(added)) {
            crate::runtime()
                .parts()
                .with(|p| p.borrow_mut().insert(kept.source, kept.schedule));
            crate::runtime()
                .limits()
                .with(|p| p.borrow_mut().insert(kept.source, kept.limit));
        }
        reset_single();
        crate::runtime().emit(crate::runtime::RuntimeEvent::AccountStreams(None));
        let now = super::now_unix();
        crate::runtime()
            .epoch()
            .with(|e| e.set(now - now.rem_euclid(3600)));
        rebuild(ui);
        // The previous single source's timer belonged to its older generation.
        // Reattach guide work without fetching its playlist/account again.
        if let Some((previous, pending)) = previous_guide.filter(|(id, _)| *id != source) {
            reattach_guide(ui, previous, generation, pending);
        }
        for playlist in playlists.into_iter().filter(|p| p.policy.enabled) {
            if !crate::runtime()
                .parts()
                .with(|p| p.borrow().contains_key(&playlist.id))
                && let Some(stored) = library::load(playlist.id)
            {
                library::run(ui, stored, Purpose::Combined, false);
            }
        }
    });
}
/// A previous source can finish its original EPG download after Add has moved
/// the browser to a new generation. Keep the worker identity until its final
/// cache is restored; newer imports and explicit guide changes revoke it.
fn reattach_guide(
    ui: Ui,
    source: SourceId,
    generation: u64,
    pending: Option<library::ImportCompletion>,
) {
    let Some(pending) = pending else {
        super::restore_guide(ui, source, generation);
        return;
    };
    super::spawn_in_app(async move {
        let current = || {
            crate::runtime().generation().with(Cell::get) == generation
                && includes(ui, source)
                && library::import_is_current(source, &pending)
        };
        if !current() {
            return;
        }
        // Complete this initial restore before the final one so an older cache
        // read cannot arrive after the original download's successful result.
        super::read_guide(
            ui,
            source,
            generation,
            super::GuideRead::CacheOnly,
            Some(pending.clone()),
        )
        .await;
        if current() {
            pending.until_finished().await;
        }
        if current() {
            super::read_guide(
                ui,
                source,
                generation,
                super::GuideRead::RestoreAndCheck,
                Some(pending),
            )
            .await;
        }
    });
}

pub fn active(_ui: Ui) -> bool {
    crate::runtime().all().get()
}
pub fn includes(ui: Ui, id: SourceId) -> bool {
    active(ui) && library::policy(id).is_ok_and(|p| p.enabled)
}
pub fn owner(ui: Ui, key: &str) -> Option<(SourceId, String)> {
    if active(ui) {
        let (s, k) = split_key(key)?;
        Some((SourceId(s), k.to_owned()))
    } else {
        Some((ui.library.shown.get_untracked()?, key.to_owned()))
    }
}
/// The normal browser includes every enabled source once there is a choice.
pub fn has_multiple_enabled() -> bool {
    library::playlists()
        .is_ok_and(|sources| sources.iter().filter(|s| s.policy.enabled).take(2).count() == 2)
}

pub fn start(ui: Ui) {
    crate::runtime().resume().with(|r| r.set(true));
    // Startup precedes user edits; install the empty real guide at once.
    super::spawn_in_app(async move {
        switch_all_ready(ui).await;
    });
}
pub fn switch_all(ui: Ui) {
    super::spawn_in_app(async move {
        if crate::favorites::flush().await {
            switch_all_ready(ui).await;
        } else {
            crate::runtime().emit(crate::runtime::RuntimeEvent::Notify(
                "Recent changes could not be saved; try again",
            ));
        }
    });
}
async fn switch_all_ready(ui: Ui) {
    let generation = crate::runtime().generation().with(Cell::get);
    if active(ui) {
        return;
    }
    if let Err(e) = crate::runtime::library::save_all(true).await {
        log::warn!("iptv combined: cannot save browsing mode: {e}");
        return;
    }
    if crate::runtime().generation().with(Cell::get) != generation || active(ui) {
        return;
    }
    super::next_generation();
    reset_single();
    super::source_changed(ui, None);
    stored::note_shown(None);
    crate::runtime::library::set_all(true);
    crate::runtime().parts().with(|p| p.borrow_mut().clear());
    crate::runtime().limits().with(|p| p.borrow_mut().clear());
    crate::runtime().emit(crate::runtime::RuntimeEvent::AccountStreams(None));
    let now = super::now_unix();
    crate::runtime()
        .epoch()
        .with(|e| e.set(now - now.rem_euclid(3600)));
    rebuild(ui);
    if let Ok(playlists) = library::playlists() {
        for playlist in playlists
            .into_iter()
            .filter(|playlist| playlist.policy.enabled)
        {
            if let Some(source) = library::load(playlist.id) {
                library::run(ui, source, Purpose::Combined, false);
            }
        }
    }
}
pub fn leave(_ui: Ui) {
    reset_single();
    crate::runtime().resume().with(|r| r.set(false));
    crate::runtime::library::set_all(false);
    crate::runtime().parts().with(|p| p.borrow_mut().clear());
    crate::runtime().limits().with(|p| p.borrow_mut().clear());
    let saved = crate::runtime::library::save_all(false);
    super::spawn_in_app(async move {
        if let Err(error) = saved.await {
            log::warn!("iptv combined: cannot save browsing mode: {error}");
        }
    });
}
pub(super) async fn show(ui: Ui, imported: Box<Imported>, held: Option<opening::Held>) {
    if !includes(ui, imported.source) {
        return;
    }
    let source = imported.source;
    let generation = crate::runtime().generation().with(Cell::get);
    let revision = library::import_revision(source);
    let current = || {
        includes(ui, source)
            && crate::runtime().generation().with(Cell::get) == generation
            && library::import_revision_is_current(source, &revision)
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
    let mut schedule = organization::build(source, groups, channels, guide, now).await;
    if !current() {
        return;
    }
    schedule = match playback {
        Streams::New(context, allowed) => {
            crate::runtime().limits().with(|l| {
                let mut l = l.borrow_mut();
                if allowed.is_some() || !l.contains_key(&source) {
                    l.insert(source, allowed);
                }
            });
            schedule.with_playback(context)
        }
        Streams::Current => schedule.with_shared_playback(
            crate::runtime()
                .parts()
                .with(|p| p.borrow().get(&source).and_then(|s| s.shared_playback())),
        ),
    };
    if let Some(lookup) = stored::description_lookup(ui, source) {
        schedule = schedule.with_description_lookup(lookup);
    }
    schedule = match held {
        Some(h) => schedule.held_only(h.from, h.to).held_channels(h.keys),
        None => pages::paged(ui, schedule, source, now).await,
    };
    if !current() {
        return;
    }
    crate::runtime()
        .parts()
        .with(|p| p.borrow_mut().insert(source, Rc::new(schedule)));
    rebuild_ready(ui).await;
    if can_resume && crate::runtime().resume().with(Cell::get) {
        resume(ui);
    }
}
pub fn rebuild(ui: Ui) {
    super::spawn_in_app(rebuild_ready(ui));
}
async fn rebuild_ready(ui: Ui) {
    let generation = crate::runtime().generation().with(Cell::get);
    let revision = crate::runtime().rebuild().with(|r| {
        let next = r.get().wrapping_add(1);
        r.set(next);
        next
    });
    if !active(ui) {
        return;
    }
    let Ok(playlists) = library::playlists() else {
        return;
    };
    let enabled: Vec<_> = playlists
        .iter()
        .filter(|p| p.policy.enabled)
        .map(|p| p.id)
        .collect();
    crate::runtime()
        .parts()
        .with(|p| p.borrow_mut().retain(|id, _| enabled.contains(id)));
    let sources = crate::runtime().parts().with(|p| {
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
    let Some(install_favorites) = crate::favorites::prepare_combined().await else {
        return;
    };
    if !active(ui) {
        return;
    }
    let collapsed = crate::runtime::library::collapsed()
        .await
        .unwrap_or_default();
    if !active(ui)
        || crate::runtime().generation().with(Cell::get) != generation
        || crate::runtime().rebuild().with(Cell::get) != revision
    {
        return;
    }
    install_favorites();
    crate::runtime().emit(crate::runtime::RuntimeEvent::ScheduleReady(Rc::new(
        CombinedSchedule::new(sources, crate::runtime().epoch().with(Cell::get))
            .with_collapsed_sources(collapsed),
    )));
    pages::replaced();
}

pub fn connection_limit(source: SourceId) -> Option<u32> {
    crate::runtime()
        .limits()
        .with(|l| l.borrow().get(&source).copied().flatten())
}
pub fn organization_changed(ui: Ui, source: SourceId, ticket: u64) {
    let schedule = crate::runtime()
        .parts()
        .with(|parts| parts.borrow().get(&source).cloned());
    super::spawn_in_app(async move {
        if let Some(schedule) = schedule {
            let Some(replacement) = organization::apply(&schedule, source).await else {
                return;
            };
            let current = crate::runtime().parts().with(|parts| {
                parts
                    .borrow()
                    .get(&source)
                    .is_some_and(|s| s.same_catalogue(&schedule))
            });
            if current && crate::runtime().organization_edit_current(source, ticket) {
                crate::runtime().parts().with(|parts| {
                    parts.borrow_mut().insert(source, Rc::new(replacement));
                });
                rebuild(ui);
            }
        }
    });
}
pub fn resume(ui: Ui) {
    if !crate::settings::flag(iptv_settings::keys::RESUME_LAST_CHANNEL)
        || ui.playing.get_untracked().is_some()
    {
        crate::runtime().resume().with(|r| r.set(false));
        return;
    }
    let loaded = crate::runtime::commands::combined_resume_key();
    super::spawn_in_app(async move {
        let key = loaded.await.ok().flatten();
        if !crate::runtime().resume().with(Cell::get)
            || ui.playing.get_untracked().is_some()
            || !active(ui)
        {
            return;
        }
        let schedule = crate::schedule();
        if let Some(c) = key.and_then(|k| schedule.find_channel(&k)) {
            // Another playlist's import may call resume while the target portal is
            // still only cached rows. Wait for that owning source's authentication.
            if iptv_guide::combined::epg_channel(&*schedule, c)
                .is_none_or(|(child, _)| child.playback().is_none())
            {
                return;
            }
            crate::runtime().resume().with(|r| r.set(false));
            ui.play(c, crate::Playback::Live);
            crate::runtime().emit(crate::runtime::RuntimeEvent::ResumeFullscreen);
        }
    });
}

/// Toggle a parent row without changing the channel catalogue or player surface.
pub fn toggle_playlist(ui: Ui, group: usize) -> bool {
    let schedule = crate::schedule();
    let Some(all) = schedule
        .as_any()
        .and_then(|a| a.downcast_ref::<CombinedSchedule>())
    else {
        return false;
    };
    let Some(collapsed) = all.toggle_playlist(group) else {
        return false;
    };
    let saved = crate::runtime::library::save_collapsed(collapsed);
    crate::runtime().emit(crate::runtime::RuntimeEvent::CatalogChanged);
    super::spawn_in_app(async move {
        match saved.await {
            Ok(()) => rebuild(ui),
            Err(error) => log::warn!("iptv browser: could not save expansion: {error}"),
        }
    });
    true
}
