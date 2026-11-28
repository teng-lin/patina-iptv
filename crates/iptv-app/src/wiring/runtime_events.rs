//! The shell's single application-event entry point.
use crate::{
    Ui,
    runtime::{RuntimeEvent, ScheduleCause},
};
use iptv_core::guide::Schedule;
use nsh_core::prelude::batch;
use std::rc::Rc;

pub(crate) fn apply(ui: Ui, event: RuntimeEvent) {
    match event {
        RuntimeEvent::ResumeChannel(channel) => {
            if ui.playback.playing.get_untracked().is_none() {
                crate::screens::parental_view::admitted(ui, channel, move || {
                    ui.play(channel, crate::Playback::Live);
                    if ui.playback.playing.get_untracked() == Some(channel) {
                        crate::guide::lists::resumed(ui, channel);
                    }
                });
            }
        }
        RuntimeEvent::SchedulePresented(steps) => crate::chrome::startup::shown(&steps),
        RuntimeEvent::CatalogChanged => {
            ui.app.data_version.update(|version| *version += 1);
            crate::wiring::tasks::spawn_in_app(async move {
                let _ = crate::wiring::runtime_owner::runtime()
                    .parental_refresh()
                    .await;
                crate::screens::parental_view::enforce(ui);
            });
        }
        RuntimeEvent::EarlyResume { source, done } => {
            crate::wiring::tasks::spawn_in_app(async move {
                crate::playback::player::resume::early(ui, &source).await;
                let _ = done.send(());
            })
        }
        RuntimeEvent::Message(message) => crate::chrome::toast::notify(&message),
        RuntimeEvent::SettingsSaveFailed(failure) => {
            crate::screens::settings_view::save_failed(failure)
        }
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
            crate::guide::lists::favorites_changed(ui);
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
        RuntimeEvent::ScheduleChanged {
            previous,
            current,
            cause,
        } => schedule_changed(ui, previous, current, cause),
        RuntimeEvent::SourceReady(resume) => crate::playback::player::source_ready(ui, resume),
        RuntimeEvent::SourceChanged => crate::playback::player::source_changed(ui),
        RuntimeEvent::AccountStreams(limit) => {
            crate::playback::multiview::set_account_streams(limit);
            if ui.app.account_streams.get_untracked() != limit {
                ui.app.account_streams.set(limit);
            }
        }
        RuntimeEvent::ForgetSource(source) => crate::playback::player::forget_source(source),
        RuntimeEvent::OpenPlaylists => crate::library_ui::playlists_view::open(ui),
        RuntimeEvent::ClosePlaylists => crate::library_ui::playlists_view::close(ui.screens),
        RuntimeEvent::FirstRun => crate::library_ui::wizard_view::open_first_run(ui),
        RuntimeEvent::StoreStartupRecovered => crate::wiring::tasks::spawn_in_app(async move {
            // A failed initial read left recent searches unloaded. Reload
            // before unblocking input so a new search cannot overwrite them.
            crate::screens::search_view::refresh_saved_history(ui.screens).await;
            ui.chrome.store_startup_failure.set(None);
        }),
        RuntimeEvent::StoreStartupFailed { path, error } => {
            crate::chrome::startup_view::store_failed(ui, path, error)
        }
        RuntimeEvent::Notify(message) => {
            crate::chrome::toast::notify(crate::chrome::i18n::tr(message))
        }
        RuntimeEvent::ResumeFullscreen => {
            if ui.playback.playing.get_untracked().is_some() {
                ui.chrome
                    .mode
                    .apply(crate::chrome::mode::Action::EnterFullscreen);
                ui.show_playing();
            }
        }
        RuntimeEvent::GuideChanged => ui.app.guide_pages.update(|version| *version += 1),
        RuntimeEvent::LibraryChanged => {
            ui.app.library.version.update(|version| *version += 1);
            crate::playback::external_player::enforce(ui.cast);
        }
        RuntimeEvent::SettingsChanged => ui.app.settings.update(|version| *version += 1),
    }
}

fn schedule_changed(
    ui: Ui,
    previous: Rc<dyn Schedule>,
    current: Rc<dyn Schedule>,
    cause: ScheduleCause,
) {
    let catalog_changed = matches!(cause, ScheduleCause::Catalog { .. });
    // Effects such as preview autoplay must see the new schedule together
    // with the list and focus built from it.
    batch(|| {
        let kept = crate::guide::lists::keep_in(ui, previous.clone());
        crate::playback::player::stop_archive_after_clock_edit(ui, &*previous, &*current);
        crate::chrome::menu_view::invalidate_shifted_context(ui, &*previous, &*current);
        if let ScheduleCause::Catalog { has_vod, any_vod } = cause {
            crate::guide::clock::mark_shown();
            if ui
                .playback
                .playing
                .get_untracked()
                .is_some_and(|ch| ch >= current.channel_count())
            {
                crate::playback::player::row_dropped(ui, &*current);
            }
            ui.app.has_vod.set(has_vod);
            ui.app.any_vod.set(any_vod);
        }
        crate::guide::lists::restore(ui, kept, current.clone());
        if catalog_changed {
            let n = ui.guide.list.with_untracked(|l| l.len());
            let rows = ui.guide.visible_rows.get_untracked();
            let clamped = rows.and_then(|(a, b)| (n > 0).then(|| (a.min(n - 1), b.min(n - 1))));
            if clamped != rows {
                ui.guide.visible_rows.set(clamped);
            }
            crate::chrome::menu_view::catalog_changed(ui.screens);
            ui.screens.description.set(None);
            ui.screens.editor.set(None);
            ui.app.data_version.update(|version| *version += 1);
        }
        crate::playback::player::enforce_guide_blocks(ui, &*current);
        if catalog_changed {
            crate::playback::external_player::enforce(ui.cast);
        } else {
            ui.app.data_version.update(|version| *version += 1);
        }
    });
    debug_assert!(Rc::ptr_eq(
        &current,
        &crate::wiring::runtime_owner::schedule()
    ));
}

/// UI context supplied to the runtime's foreground scheduling hint.
pub(crate) fn focused_channel(ui: crate::GuideUi) -> Option<usize> {
    crate::guide::lists::focused_channel(ui)
}

/// The headless scenarios' stand-in for an import reporting the shown
/// account's connection limit (`device_wiring.rs`'s test seam).
#[cfg(feature = "test-support")]
pub(crate) fn account_streams_reported(limit: Option<u32>) {
    crate::wiring::runtime_owner::runtime().emit(RuntimeEvent::AccountStreams(limit));
}
