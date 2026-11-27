//! The view observes the runtime's cached result; closing it never cancels a check.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iptv_nav::Action;
use iptv_runtime::app_updates::{
    github::{CheckError, ReleaseStatus},
    state::{Frequency, Snapshot, Trigger},
};
use nsh_core::prelude::*;

use crate::{
    Ui, ViewMode,
    i18n::{tr, trf},
    scale::Scale,
    ui_kit,
};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().min(i64::MAX as u64) as i64)
}

fn changed(ui: Ui) {
    ui.update_revision
        .update(|revision| *revision = revision.wrapping_add(1));
}

fn snapshot(ui: Ui) -> Snapshot {
    ui.update_revision.get();
    crate::runtime().app_update_snapshot()
}

fn supported() -> bool {
    !crate::is_tui_mode() && !crate::host::IPAD
}

fn scheduled_enabled() -> bool {
    // Ordinary synthetic app scenarios must never contact public update services.
    #[cfg(feature = "test-support")]
    return std::env::var_os("IPTV_TEST_UPDATE_API").is_some();
    #[cfg(not(feature = "test-support"))]
    true
}

pub(super) fn bind(ui: Ui) {
    if !supported() {
        return;
    }
    let started = std::cell::Cell::new(false);
    let was_foreground = std::cell::Cell::new(false);
    effect(move || {
        let foreground = lifecycle() != nsh_core::platform::Lifecycle::Background;
        if !ui.update_ready.get() {
            return;
        }
        let resumed = !was_foreground.replace(foreground) && foreground;
        if resumed && scheduled_enabled() {
            let trigger = if started.replace(true) {
                Trigger::Automatic
            } else {
                Trigger::Startup
            };
            check(ui, trigger);
        }
    });
    crate::tasks::spawn_in_app(async move {
        if let Err(error) = crate::runtime().restore_app_update_state().await {
            log::warn!("iptv update schedule restore failed: {error}");
        }
        ui.update_ready.set(true);
        if ui.update_dialog_open.get_untracked() {
            check(ui, Trigger::Manual);
        }
        timer(ui);
    });
}

fn timer(ui: Ui) {
    let _timer = set_timeout(Duration::from_secs(60), move || {
        if scheduled_enabled() && lifecycle() != nsh_core::platform::Lifecycle::Background {
            check(ui, Trigger::Automatic);
        }
        timer(ui);
    });
}

pub(super) fn open(ui: Ui) {
    if !supported() {
        return;
    }
    ui.update_focus.set(0);
    ui.update_notes_scroll.reset();
    ui.update_dialog_open.set(true);
    if ui.update_ready.get_untracked() {
        check(ui, Trigger::Manual);
    }
}

fn check(ui: Ui, trigger: Trigger) {
    let owner = crate::runtime();
    let frequency = Frequency::from_setting(crate::settings::choice(
        iptv_settings::keys::APP_UPDATE_FREQUENCY,
    ));
    let Some(token) = owner.begin_app_update_check(now(), trigger, frequency) else {
        return;
    };
    changed(ui);
    crate::tasks::spawn_in_app(async move {
        let result = owner
            .check_github_release(crate::host::updates::installed())
            .await;
        owner.finish_app_update_check(token, now(), result);
        if !ui.update_dialog_open.get_untracked() {
            ui.update_focus.set(0);
        }
        changed(ui);
        if let Err(error) = owner.persist_app_update_state().await {
            log::warn!("iptv update schedule save failed: {error}");
            if trigger == Trigger::Manual {
                crate::settings_view::notify(tr("Could not save update preferences. Try again."));
            }
        }
    });
}

fn persist(ui: Ui) {
    changed(ui);
    crate::tasks::spawn_in_app(async move {
        if let Err(error) = crate::runtime().persist_app_update_state().await {
            log::warn!("iptv update preferences save failed: {error}");
            crate::settings_view::notify(tr("Could not save update preferences. Try again."));
        }
    });
}

fn skip(ui: Ui) {
    cancel_download(ui);
    if let Some(check) = snapshot(ui).cached {
        crate::runtime().skip_app_update_version(&check.release.version);
        persist(ui);
    }
    super::close(ui);
}

fn acknowledge(ui: Ui) {
    crate::runtime().acknowledge_app_update_notice();
    persist(ui);
}

fn review(ui: Ui) {
    acknowledge(ui);
    ui.update_focus.set(0);
    ui.update_notes_scroll.reset();
    ui.update_dialog_open.set(true);
}

fn newer(ui: Ui) -> bool {
    snapshot(ui).cached.is_some_and(|check| {
        matches!(
            check.status,
            ReleaseStatus::UpdateAvailable | ReleaseStatus::NoCompatiblePackage
        )
    })
}

fn pending_ready(ui: Ui) -> bool {
    let Some(asset) = crate::runtime().pending_app_update_asset() else {
        return false;
    };
    snapshot(ui)
        .cached
        .and_then(|check| check.release.download)
        .is_some_and(|download| download.url == asset.url())
}

fn error_message(error: &CheckError) -> &'static str {
    match error {
        CheckError::RateLimited => tr("GitHub is busy. Try checking again later."),
        CheckError::Network | CheckError::Timeout => {
            tr("Could not connect to GitHub. Check your connection and try again.")
        }
        _ => tr("Update check failed. Try again."),
    }
}

fn status(ui: Ui) -> String {
    let state = snapshot(ui);
    if ui.update_install_busy.get() {
        if let Some((received, total)) = crate::runtime().app_update_download_progress() {
            let percent = (u128::from(received) * 100 / u128::from(total.max(1)))
                .min(100)
                .to_string();
            return trf(
                "Downloading update: {percent}%",
                &[("percent", percent.as_str().into())],
            );
        }
        return tr("Checking…").to_owned();
    }
    if pending_ready(ui) {
        return tr("Update ready to install.").to_owned();
    }
    if state.checking || !ui.update_ready.get() {
        return tr("Checking…").to_owned();
    }
    if let Some(error) = &state.error {
        return error_message(error).to_owned();
    }
    state.cached.as_ref().map_or_else(|| tr("Check for updates").to_owned(), |check| {
        match check.status {
            ReleaseStatus::Current => tr("Patina is up to date.").to_owned(),
            ReleaseStatus::Ahead => tr("This build is newer than the latest public release.").to_owned(),
            ReleaseStatus::UpdateAvailable => tr("Update available").to_owned(),
            ReleaseStatus::NoCompatiblePackage => tr("A newer version is available, but there is no compatible package for this installation.").to_owned(),
        }
    })
}

fn details(ui: Ui) -> String {
    let state = snapshot(ui);
    let installed = trf(
        "Installed version: {version}",
        &[("version", env!("CARGO_PKG_VERSION").into())],
    );
    let checked = state
        .last_success
        .map(|time| {
            let time = format!(
                "{} {}",
                crate::clock::date_only(time),
                crate::clock::time_only(time)
            );
            trf("Last checked: {time}", &[("time", time.as_str().into())])
        })
        .unwrap_or_default();
    let Some(check) = state.cached else {
        return format!("{installed}\n{checked}");
    };
    let latest = trf(
        "Latest version: {version}",
        &[("version", check.release.version.as_str().into())],
    );
    let date = check.release.published_at.as_deref().unwrap_or("");
    format!("{installed}\n{latest}\n{date}\n{checked}")
}

fn notes(ui: Ui) -> String {
    snapshot(ui).cached.map_or_else(String::new, |check| {
        if check.release.notes.trim().is_empty() {
            tr("No release notes were provided.").to_owned()
        } else {
            check.release.notes
        }
    })
}

struct InstallOperation(Ui);
impl Drop for InstallOperation {
    fn drop(&mut self) {
        self.0.update_install_busy.try_set(false);
        if self.0.owner.is_live() {
            self.0.update_pending_dismissed.set(false);
            changed(self.0);
        }
    }
}

fn progress_timer(ui: Ui) {
    let _timer = set_timeout(Duration::from_millis(200), move || {
        if ui.update_install_busy.get_untracked() {
            changed(ui);
            progress_timer(ui);
        }
    });
}

fn cancel_download(ui: Ui) {
    ui.update_cancel_requested.set(true);
    crate::runtime().cancel_app_update_download();
    crate::runtime().cancel_pending_app_update();
    changed(ui);
}

fn download(ui: Ui) {
    if ui.update_install_busy.get_untracked() {
        return;
    }
    let Some(check) = snapshot(ui).cached else {
        return;
    };
    if !newer(ui) {
        return;
    }
    ui.update_install_busy.set(true);
    ui.update_cancel_requested.set(false);
    let operation = InstallOperation(ui);
    progress_timer(ui);
    crate::tasks::spawn_in_app(async move {
        let _operation = operation;
        match crate::host::updates::action_for(&check).await {
            Ok(Some(action)) => {
                if ui.update_cancel_requested.get_untracked() {
                    return;
                }
                crate::settings_view::notify(action.label());
                action.run(ui).await;
            }
            Ok(None) => super::open(&check.release.url).await,
            Err(message) => crate::settings_view::notify(message),
        }
    });
}

fn activate(ui: Ui, index: usize) {
    match index {
        0 => check(ui, Trigger::Manual),
        1 if newer(ui) => download(ui),
        2 if newer(ui) => skip(ui),
        3 => super::close(ui),
        4 if ui.update_install_busy.get_untracked() => cancel_download(ui),
        _ => {}
    }
}

pub(super) fn key(ui: Ui, action: Action) {
    let at = ui.update_focus.get_untracked();
    let mut controls = vec![0];
    if newer(ui) {
        controls.extend([1, 2]);
    }
    controls.push(3);
    if ui.update_install_busy.get_untracked() {
        controls.push(4);
    }
    let position = controls.iter().position(|index| *index == at).unwrap_or(0);
    match action {
        Action::MoveLeft => ui.update_focus.set(controls[position.saturating_sub(1)]),
        Action::MoveRight => ui
            .update_focus
            .set(controls[(position + 1).min(controls.len() - 1)]),
        Action::MoveUp => ui.update_notes_scroll.scroll_by(-160.0),
        Action::MoveDown => ui.update_notes_scroll.scroll_by(160.0),
        Action::Select => activate(ui, at),
        _ => {}
    }
}

pub(super) fn layer(ui: Ui) -> View {
    view()
        .pointer_events(PointerEvents::BoxNone)
        .absolute()
        .inset_0()
        .child(when(move || super::is_open(ui), move || panel(ui)))
        .child(when(move || notice_open(ui), move || notice(ui)))
}

fn panel(ui: Ui) -> View {
    let size = Scale::new();
    effect(move || {
        let focus = ui.update_focus.get();
        if !newer(ui) && matches!(focus, 1 | 2) {
            ui.update_focus.set(0);
        }
        if !ui.update_install_busy.get() && focus == 4 {
            ui.update_focus.set(3);
        }
    });
    ui_kit::screen(ui, size)
        .test_id("app-update-panel")
        .on_press(|| {})
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::AppUpdate.accessibility_covered(ui)
        })
        .child(ui_kit::header(
            ui,
            size,
            move || tr("App updates"),
            super::close,
        ))
        .child(ui_kit::heading(size, move || status(ui)).test_id("app-update-status"))
        .child(ui_kit::body(size, move || details(ui)))
        .child(ui_kit::heading(size, move || tr("Release notes")))
        .child(
            ui.update_notes_scroll
                .view()
                .flex_1()
                .test_id("app-update-notes")
                .child(ui_kit::body(size, move || notes(ui))),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(move || size.px(12.0))
                .child(ui_kit::button(
                    ui,
                    size,
                    move || tr("Check for updates"),
                    move || ui.update_focus.get() == 0,
                    move || activate(ui, 0),
                ))
                .child(when(
                    move || newer(ui),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || {
                                if pending_ready(ui) {
                                    tr("Install update")
                                } else if snapshot(ui)
                                    .cached
                                    .is_some_and(|check| check.release.download.is_none())
                                {
                                    tr("Release notes and downloads")
                                } else if crate::host::updates::automatic_install_supported() {
                                    tr("Download and update")
                                } else {
                                    tr("Download package")
                                }
                            },
                            move || ui.update_focus.get() == 1,
                            move || activate(ui, 1),
                        )
                    },
                ))
                .child(when(
                    move || newer(ui),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Skip this version"),
                            move || ui.update_focus.get() == 2,
                            move || activate(ui, 2),
                        )
                    },
                ))
                .child(ui_kit::button(
                    ui,
                    size,
                    move || tr("Later"),
                    move || ui.update_focus.get() == 3,
                    move || activate(ui, 3),
                ))
                .child(when(
                    move || ui.update_install_busy.get(),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Cancel download"),
                            move || ui.update_focus.get() == 4,
                            move || activate(ui, 4),
                        )
                    },
                )),
        )
}

pub(super) fn notice_open(ui: Ui) -> bool {
    if super::is_open(ui)
        || ui.mode.get() == ViewMode::Fullscreen
        || (snapshot(ui).notice.is_none()
            && (!pending_ready(ui) || ui.update_pending_dismissed.get()))
    {
        return false;
    }
    crate::overlays::Overlay::ALL
        .into_iter()
        .filter(|overlay| *overlay != crate::overlays::Overlay::AppUpdateNotice)
        .all(|overlay| !overlay.is_open(ui))
}

pub(super) fn dismiss_notice(ui: Ui) {
    ui.update_pending_dismissed.set(true);
    acknowledge(ui);
}

pub(super) fn notice_key(ui: Ui, action: Action) {
    match action {
        Action::MoveLeft | Action::MoveUp => ui.update_focus.set(0),
        Action::MoveRight | Action::MoveDown => ui.update_focus.set(1),
        Action::Select if ui.update_focus.get_untracked() == 0 => review(ui),
        Action::Select => dismiss_notice(ui),
        Action::Back | Action::ShowMenu => dismiss_notice(ui),
        _ => {}
    }
}

fn notice(ui: Ui) -> View {
    let size = Scale::new();
    ui_kit::card(ui, size, 600.0)
        .test_id("app-update-notice")
        .role(AccessibilityRole::Alert)
        .absolute()
        .right(move || size.px(24.0))
        .bottom(move || size.px(24.0))
        .child(ui_kit::heading(size, move || {
            if pending_ready(ui) {
                tr("Update ready to install.")
            } else {
                tr("A new version of Patina is available.")
            }
        }))
        .child(
            view()
                .flex_row()
                .gap(move || size.px(12.0))
                .child(ui_kit::button(
                    ui,
                    size,
                    move || tr("View update"),
                    move || ui.update_focus.get() == 0,
                    move || review(ui),
                ))
                .child(ui_kit::button(
                    ui,
                    size,
                    move || tr("Later"),
                    move || ui.update_focus.get() == 1,
                    move || dismiss_notice(ui),
                )),
        )
}
