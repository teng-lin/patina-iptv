//! The shell's single application-event entry point.
use crate::{Ui, runtime::RuntimeEvent};

pub(crate) fn apply(ui: Ui, event: RuntimeEvent) {
    match event {
        RuntimeEvent::ResumeChannel(channel) => {
            if !crate::desktop::recorder_only_startup() && ui.playing.get_untracked().is_none() {
                crate::parental_view::admitted(ui, channel, move || {
                    ui.play(channel, crate::Playback::Live);
                    if ui.playing.get_untracked() == Some(channel) {
                        crate::lists::resumed(ui, channel);
                    }
                });
            }
        }
        RuntimeEvent::SchedulePresented(steps) => crate::startup::shown(&steps),
        RuntimeEvent::CatalogChanged => {
            ui.app.data_version.update(|version| *version += 1);
            crate::data::spawn_in_app(async move {
                let _ = crate::runtime().parental_refresh().await;
                crate::parental_view::enforce(ui);
            });
        }
        RuntimeEvent::EarlyResume { source, done } => crate::data::spawn_in_app(async move {
            crate::player::resume::early(ui, &source).await;
            let _ = done.send(());
        }),
        RuntimeEvent::Message(message) => crate::settings_view::notify(&message),
        RuntimeEvent::SettingsSaveFailed(failure) => crate::settings_view::save_failed(failure),
        RuntimeEvent::BackupProgress(phase) => {
            log::debug!("iptv backup: {phase:?}");
            ui.app.settings.update(|version| *version += 1);
        }
        RuntimeEvent::FavoritesChanged {
            channels,
            has_vod,
            any_vod,
        } => {
            ui.app.favorites.set(channels);
            ui.app.has_vod.set(has_vod);
            ui.app.any_vod.set(any_vod);
            ui.app.list_version.update(|version| *version += 1);
        }
        RuntimeEvent::FavoriteOrderChanged => {
            crate::lists::favorites_changed(ui);
            ui.app.list_version.update(|version| *version += 1);
        }
        RuntimeEvent::RefreshProgress(source, activity) => {
            ui.app.library.activity.update(|activities| {
                if let Some(activity) = activity {
                    activities.insert(source, activity);
                } else {
                    activities.remove(&source);
                }
            })
        }
        RuntimeEvent::BrowsingAll(all) => ui.app.library.all.set(all),
        RuntimeEvent::ShownSource(source) => ui.app.library.shown.set(source),
        RuntimeEvent::ScheduleReady(schedule) => {
            crate::set_schedule(&ui, schedule);
            crate::player::enforce_guide_blocks(ui);
            crate::external_player::enforce(ui);
        }
        RuntimeEvent::OrganizationReady {
            source,
            ticket,
            schedule,
            replacement,
        } => {
            let current = crate::schedule();
            let same_catalog = schedule
                .as_any()
                .and_then(|s| s.downcast_ref::<iptv_guide::EpgSchedule>())
                .zip(
                    current
                        .as_any()
                        .and_then(|s| s.downcast_ref::<iptv_guide::EpgSchedule>()),
                )
                .is_some_and(|(before, now)| before.same_catalog(now));
            if same_catalog && crate::runtime().organization_edit_current(source, ticket) {
                let kept = crate::lists::keep(ui);
                crate::player::stop_archive_after_clock_edit(ui, &*current, &*replacement);
                crate::menu_view::invalidate_shifted_context(ui, &*current, &*replacement);
                crate::data::combined::remember_single(
                    &crate::runtime(),
                    source,
                    replacement.clone(),
                );
                crate::runtime().set_schedule(replacement);
                crate::lists::restore(ui, kept);
                crate::player::enforce_guide_blocks(ui);
                ui.app.data_version.update(|version| *version += 1);
            }
        }
        RuntimeEvent::SourceReady(resume) => crate::player::source_ready(ui, resume),
        RuntimeEvent::SourceChanged => crate::player::source_changed(ui),
        RuntimeEvent::AccountStreams(limit) => {
            crate::multiview::set_account_streams(limit);
            if ui.app.account_streams.get_untracked() != limit {
                ui.app.account_streams.set(limit);
            }
        }
        RuntimeEvent::ForgetSource(source) => crate::player::forget_source(source),
        RuntimeEvent::OpenPlaylists => crate::playlists_view::open(ui),
        RuntimeEvent::ClosePlaylists => crate::playlists_view::close(ui),
        RuntimeEvent::FirstRun => crate::wizard_view::open_first_run(ui),
        RuntimeEvent::StoreStartupRecovered => crate::data::spawn_in_app(async move {
            // A failed initial read left recent searches unloaded. Reload
            // before unblocking input so a new search cannot overwrite them.
            crate::search_view::refresh_saved_history(ui).await;
            ui.store_startup_failure.set(None);
        }),
        RuntimeEvent::StoreStartupFailed { path, error } => {
            crate::startup::store_failed(ui, path, error)
        }
        RuntimeEvent::Notify(message) => crate::settings_view::notify(crate::i18n::tr(message)),
        RuntimeEvent::ResumeFullscreen => {
            if !crate::desktop::recorder_only_startup() && ui.playing.get_untracked().is_some() {
                ui.mode.set(crate::ViewMode::Fullscreen);
                ui.show_playing();
            }
        }
        RuntimeEvent::GuideChanged => ui.app.guide_pages.update(|version| *version += 1),
        RuntimeEvent::LibraryChanged => {
            ui.app.library.version.update(|version| *version += 1);
            crate::external_player::enforce(ui);
        }
        RuntimeEvent::SettingsChanged => ui.app.settings.update(|version| *version += 1),
    }
}

/// UI context supplied to the runtime's foreground scheduling hint.
pub(crate) fn focused_channel(ui: Ui) -> Option<usize> {
    crate::lists::focused_channel(ui)
}
