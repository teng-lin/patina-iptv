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
    let Some(check) = state.cached else {
        return installed;
    };
    let latest = trf(
        "Latest version: {version}",
        &[("version", check.release.version.as_str().into())],
    );
    let date = check.release.published_at.as_deref().unwrap_or("");
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

struct InstallOperation(Signal<bool>);
impl Drop for InstallOperation {
    fn drop(&mut self) {
        self.0.try_set(false);
    }
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
    let operation = InstallOperation(ui.update_install_busy);
    crate::tasks::spawn_in_app(async move {
        let _operation = operation;
        match crate::host::updates::action_for(&check).await {
            Ok(Some(action)) => {
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
        _ => {}
    }
}

pub(super) fn key(ui: Ui, action: Action) {
    let at = ui.update_focus.get_untracked();
    match action {
        Action::MoveLeft => ui.update_focus.set(if !newer(ui) && at == 3 {
            0
        } else {
            at.saturating_sub(1)
        }),
        Action::MoveRight => ui
            .update_focus
            .set(if !newer(ui) { 3 } else { (at + 1).min(3) }),
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
    ui_kit::screen(ui, size)
        .test_id("app-update-panel")
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
                                if crate::host::updates::automatic_install_supported() {
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
                )),
        )
}

pub(super) fn notice_open(ui: Ui) -> bool {
    if super::is_open(ui) || ui.mode.get() == ViewMode::Fullscreen || snapshot(ui).notice.is_none()
    {
        return false;
    }
    crate::overlays::Overlay::ALL
        .into_iter()
        .filter(|overlay| *overlay != crate::overlays::Overlay::AppUpdateNotice)
        .all(|overlay| !overlay.is_open(ui))
}

pub(super) fn dismiss_notice(ui: Ui) {
    acknowledge(ui);
}

pub(super) fn notice_key(ui: Ui, action: Action) {
    match action {
        Action::MoveLeft | Action::MoveUp => ui.update_focus.set(0),
        Action::MoveRight | Action::MoveDown => ui.update_focus.set(1),
        Action::Select if ui.update_focus.get_untracked() == 0 => review(ui),
        Action::Select => acknowledge(ui),
        Action::Back | Action::ShowMenu => acknowledge(ui),
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
            tr("A new version of Patina is available.")
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
                    move || acknowledge(ui),
                )),
        )
}
