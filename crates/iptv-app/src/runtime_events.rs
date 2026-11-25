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
            if ui.playing.get_untracked().is_none() {
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
                let _ = crate::runtime_owner::runtime().parental_refresh().await;
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
        RuntimeEvent::ScheduleChanged {
            previous,
            current,
            cause,
        } => schedule_changed(ui, previous, current, cause),
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
            crate::startup_view::store_failed(ui, path, error)
        }
        RuntimeEvent::Notify(message) => crate::settings_view::notify(crate::i18n::tr(message)),
        RuntimeEvent::ResumeFullscreen => {
            if ui.playing.get_untracked().is_some() {
                ui.mode.apply(crate::mode::Action::EnterFullscreen);
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
        let kept = crate::lists::keep_in(ui, previous.clone());
        crate::player::stop_archive_after_clock_edit(ui, &*previous, &*current);
        crate::menu_view::invalidate_shifted_context(ui, &*previous, &*current);
        if let ScheduleCause::Catalog { has_vod, any_vod } = cause {
            crate::clock::mark_shown();
            if ui
                .playing
                .get_untracked()
                .is_some_and(|ch| ch >= current.channel_count())
            {
                crate::player::row_dropped(ui, &*current);
            }
            ui.app.has_vod.set(has_vod);
            ui.app.any_vod.set(any_vod);
        }
        crate::lists::restore(ui, kept, current.clone());
        if catalog_changed {
            let n = ui.list.with_untracked(|l| l.len());
            let rows = ui.visible_rows.get_untracked();
            let clamped = rows.and_then(|(a, b)| (n > 0).then(|| (a.min(n - 1), b.min(n - 1))));
            if clamped != rows {
                ui.visible_rows.set(clamped);
            }
            ui.menu.set(None);
            ui.description.set(None);
            ui.editor.set(None);
            ui.app.data_version.update(|version| *version += 1);
        }
        crate::player::enforce_guide_blocks(ui, &*current);
        if catalog_changed {
            crate::external_player::enforce(ui);
        } else {
            ui.app.data_version.update(|version| *version += 1);
        }
    });
    debug_assert!(Rc::ptr_eq(&current, &crate::runtime_owner::schedule()));
}

/// UI context supplied to the runtime's foreground scheduling hint.
pub(crate) fn focused_channel(ui: Ui) -> Option<usize> {
    crate::lists::focused_channel(ui)
}

/// The headless scenarios' stand-in for an import reporting the shown
/// account's connection limit (`device_wiring.rs`'s test seam).
#[cfg(feature = "test-support")]
pub(crate) fn account_streams_reported(limit: Option<u32>) {
    crate::runtime_owner::runtime().emit(RuntimeEvent::AccountStreams(limit));
}
