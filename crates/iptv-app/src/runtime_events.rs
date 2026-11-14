//! The shell's single application-event entry point.
use crate::{Ui, runtime::RuntimeEvent};

pub(crate) fn apply(ui: Ui, event: RuntimeEvent) {
    match event {
        RuntimeEvent::RefreshDue(source) => crate::data::refresh(ui, source),
        RuntimeEvent::CatalogChanged => ui.data_version.update(|version| *version += 1),
        RuntimeEvent::EarlyResume { source, done } => crate::data::spawn_in_app(async move {
            crate::player::resume::early(ui, &source).await;
            let _ = done.send(());
        }),
        RuntimeEvent::Message(message) => crate::settings_view::notify(&message),
        RuntimeEvent::BackupProgress(phase) => {
            log::debug!("iptv backup: {phase:?}");
            ui.settings.update(|version| *version += 1);
        }
        RuntimeEvent::FavoritesChanged { channels, has_vod } => {
            ui.favorites.set(channels);
            ui.has_vod.set(has_vod);
        }
        RuntimeEvent::FavoriteOrderChanged => crate::lists::favorites_changed(ui),
        RuntimeEvent::RefreshProgress(source, activity) => {
            ui.library.activity.update(|activities| {
                if let Some(activity) = activity {
                    activities.insert(source, activity);
                } else {
                    activities.remove(&source);
                }
            })
        }
        RuntimeEvent::BrowsingAll(all) => ui.library.all.set(all),
        RuntimeEvent::ShownSource(source) => ui.library.shown.set(source),
        RuntimeEvent::ScheduleReady(schedule) => crate::set_schedule(&ui, schedule),
        RuntimeEvent::OrganizationReady {
            source,
            ticket,
            schedule,
            replacement,
        } => {
            let current = crate::schedule();
            let same_catalogue = schedule
                .as_any()
                .and_then(|s| s.downcast_ref::<iptv_guide::EpgSchedule>())
                .zip(
                    current
                        .as_any()
                        .and_then(|s| s.downcast_ref::<iptv_guide::EpgSchedule>()),
                )
                .is_some_and(|(before, now)| before.same_catalogue(now));
            if same_catalogue && crate::runtime().organization_edit_current(source, ticket) {
                let kept = crate::lists::keep(ui);
                crate::data::combined::remember_single(source, replacement.clone());
                crate::runtime().set_schedule(replacement);
                crate::lists::restore(ui, kept);
                ui.data_version.update(|version| *version += 1);
            }
        }
        RuntimeEvent::SourceReady(resume) => crate::player::source_ready(ui, resume),
        RuntimeEvent::SourceChanged => crate::player::source_changed(ui),
        RuntimeEvent::AccountStreams(limit) => crate::multiview::set_account_streams(limit),
        RuntimeEvent::ForgetSource(source) => crate::player::forget_source(source),
        RuntimeEvent::OpenPlaylists => crate::playlists_view::open(ui),
        RuntimeEvent::ClosePlaylists => crate::playlists_view::close(ui),
        RuntimeEvent::FirstRun => crate::wizard_view::open_first_run(ui),
        RuntimeEvent::Notify(message) => crate::settings_view::notify(message),
        RuntimeEvent::ResumeFullscreen => {
            ui.mode.set(crate::ViewMode::Fullscreen);
            ui.show_playing();
        }
        RuntimeEvent::GuideChanged => ui.guide_pages.update(|version| *version += 1),
        RuntimeEvent::LibraryChanged => ui.library.version.update(|version| *version += 1),
        RuntimeEvent::SettingsChanged => ui.settings.update(|version| *version += 1),
    }
}

/// UI context supplied to the runtime's foreground scheduling hint.
pub(crate) fn focused_channel(ui: Ui) -> Option<usize> {
    crate::lists::focused_channel(ui)
}
