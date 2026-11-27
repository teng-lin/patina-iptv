//! The view observes the runtime's cached result; closing it never cancels a check.
use std::time::Duration;

use iptv_nav::Action;
use iptv_runtime::app_updates::{
    github::{CheckError, ReleaseStatus},
    state::{Frequency, Snapshot, Trigger},
};
use nsh_core::prelude::*;

use crate::settings_view::updates as facade;
use crate::{
    Ui, ViewMode,
    i18n::{tr, trf},
    scale::Scale,
    ui_kit,
};

fn now() -> i64 {
    crate::host::since_epoch().map_or(0, |d| d.as_secs().min(i64::MAX as u64) as i64)
}

fn changed(ui: Ui) {
    ui.screens
        .update_revision
        .update(|revision| *revision = revision.wrapping_add(1));
}

fn snapshot(ui: Ui) -> Snapshot {
    ui.screens.update_revision.get();
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

pub(crate) fn bind(ui: Ui) {
    if !supported() {
        return;
    }
    let started = std::cell::Cell::new(false);
    let was_foreground = std::cell::Cell::new(false);
    effect(move || {
        let foreground = lifecycle() != nsh_core::platform::Lifecycle::Background;
        if !ui.screens.update_ready.get() || !super::metadata_ready() {
            return;
        }
        let resumed = !was_foreground.replace(foreground) && foreground;
        if resumed {
            let first = !started.replace(true);
            let trigger = if crate::runtime().app_update_manual_check_pending() {
                Trigger::Manual
            } else if !first {
                Trigger::Automatic
            } else {
                Trigger::Startup
            };
            if trigger == Trigger::Manual || scheduled_enabled() {
                check(ui, trigger);
            }
        } else if foreground && crate::runtime().app_update_manual_check_pending() {
            check(ui, Trigger::Manual);
        }
    });
    effect(move || {
        if ui.screens.update_dialog_open.get()
            && lifecycle() != nsh_core::platform::Lifecycle::Background
            && ui.top_overlay() == Some(crate::overlays::Overlay::AppUpdate)
            && snapshot(ui).notice.is_some()
        {
            acknowledge(ui);
        }
    });
    crate::tasks::spawn_in_app(async move {
        if let Err(error) = crate::runtime().restore_app_update_state().await {
            log::warn!("iptv update schedule restore failed: {error}");
        }
        ui.screens.update_ready.set(true);
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

pub(crate) fn open(ui: Ui) {
    if !supported() {
        return;
    }
    ui.screens.update_focus.set(0);
    ui.screens.update_notes_scroll.reset();
    ui.screens.update_dialog_open.set(true);
    crate::runtime().queue_manual_app_update_check();
    if ui.screens.update_ready.get_untracked() {
        check(ui, Trigger::Manual);
    }
}

pub(crate) fn close(ui: Ui) {
    // A retained package or failed re-check can leave a notice waiting behind
    // this panel. Its first visible action must own remote focus on return.
    batch(move || {
        ui.screens.update_focus.set(0);
        ui.screens.update_dialog_open.set(false);
    });
}

fn check(ui: Ui, trigger: Trigger) {
    // A missing Android report is not evidence of a store installation. Do not
    // consume the due check or persist a result for the wrong package channel.
    if ui.screens.update_install_busy.get_untracked()
        || !ui.screens.update_ready.get_untracked()
        || !super::metadata_ready()
    {
        return;
    }
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
        if !ui.screens.update_dialog_open.get_untracked() {
            ui.screens.update_focus.set(0);
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
    if !cancel_download(ui) {
        return;
    }
    if let Some(check) = snapshot(ui).cached {
        crate::runtime().skip_app_update_version(&check.release.version);
        persist(ui);
    }
    facade::close(ui);
}

fn acknowledge(ui: Ui) {
    crate::runtime().acknowledge_app_update_notice();
    persist(ui);
}

fn review(ui: Ui) {
    crate::settings_view::settings_guarded(ui, move || {
        acknowledge(ui);
        ui.screens.update_focus.set(0);
        ui.screens.update_notes_scroll.reset();
        ui.screens.update_dialog_open.set(true);
    });
}

fn releases_fallback(ui: Ui) -> bool {
    let state = snapshot(ui);
    state.cached.is_none() && state.error.is_some()
}

fn newer(ui: Ui) -> bool {
    snapshot(ui).cached.is_some_and(|check| {
        matches!(
            check.status,
            ReleaseStatus::UpdateAvailable | ReleaseStatus::NoCompatiblePackage
        )
    })
}

fn manual_download_visible(ui: Ui) -> bool {
    newer(ui)
        && crate::host::updates::automatic_install_supported()
        && snapshot(ui)
            .cached
            .is_some_and(|check| check.release.download.is_some())
}

fn manual_download(ui: Ui) {
    if download_blocked(ui) {
        return;
    }
    if let Some(download) = snapshot(ui).cached.and_then(|check| check.release.download) {
        crate::tasks::spawn_in_app(async move { super::open_download_url(&download.url).await });
    }
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
    status_for(
        &state,
        ui.screens.update_ready.get() && super::metadata_ready(),
        ui.screens.update_install_busy.get(),
        crate::runtime().app_update_download_progress(),
        pending_ready(ui),
    )
}

fn status_for(
    state: &Snapshot,
    initialized: bool,
    install_busy: bool,
    progress: Option<(u64, u64)>,
    pending: bool,
) -> String {
    if install_busy {
        if let Some((received, total)) = progress {
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
    if state.checking || !initialized {
        return tr("Checking…").to_owned();
    }
    if let Some(error) = &state.error {
        return error_message(error).to_owned();
    }
    if pending {
        return tr("Update ready to install.").to_owned();
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
            trf(
                "Last successful check: {time}",
                &[("time", time.as_str().into())],
            )
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
    // Pair related metadata so a wrapped status and large text still leave a
    // readable notes viewport on a short landscape screen.
    let dates = [date, &checked]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    format!("{installed} · {latest}\n{dates}")
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

struct InstallOperation {
    owner: Owner,
    busy: Signal<bool>,
    panel_open: Signal<bool>,
    focus: Signal<usize>,
    pending_dismissed: Signal<bool>,
    revision: Signal<u64>,
}

impl InstallOperation {
    fn new(ui: Ui) -> Self {
        Self {
            owner: ui.owner,
            busy: ui.screens.update_install_busy,
            panel_open: ui.screens.update_dialog_open,
            focus: ui.screens.update_focus,
            pending_dismissed: ui.screens.update_pending_dismissed,
            revision: ui.screens.update_revision,
        }
    }
}

impl Drop for InstallOperation {
    fn drop(&mut self) {
        self.busy.try_set(false);
        if self.owner.is_live() {
            if !self.panel_open.get_untracked() {
                self.focus.set(0);
            }
            self.pending_dismissed.set(false);
            self.revision
                .update(|revision| *revision = revision.wrapping_add(1));
        }
    }
}

fn progress_timer(ui: Ui) {
    // A pointer press runs under the panel's child owner. Progress belongs to
    // the accepted root operation, including while Later closes that panel.
    ui.owner.run(move || {
        let _timer = set_timeout(Duration::from_millis(200), move || {
            if ui.screens.update_install_busy.get_untracked() {
                changed(ui);
                progress_timer(ui);
            }
        });
    });
}

fn cancellation_allowed(ui: Ui) -> bool {
    ui.screens.update_revision.get();
    !ui.screens.update_install_busy.get() || super::can_cancel_install()
}

fn cancel_download(ui: Ui) -> bool {
    // Native extraction can win between rendering an enabled button and the
    // press. Do not claim cancellation or persist Skip after that boundary.
    if !super::try_cancel_current() {
        changed(ui);
        return false;
    }
    ui.screens.update_cancel_requested.set(true);
    crate::runtime().cancel_app_update_download();
    crate::runtime().cancel_pending_app_update();
    changed(ui);
    true
}

async fn until_cancelled<T>(
    operation: impl std::future::Future<Output = T>,
    cancellation: impl std::future::Future<Output = ()>,
    cancelled: impl Fn() -> bool,
) -> Option<T> {
    let mut operation = std::pin::pin!(operation);
    let mut cancellation = std::pin::pin!(cancellation);
    std::future::poll_fn(move |context| {
        // Read the flag on every poll: the wakeup future can still be waiting
        // for its timer when a late authorization result wakes the shell task.
        if cancelled() || cancellation.as_mut().poll(context).is_ready() {
            return std::task::Poll::Ready(None);
        }
        operation.as_mut().poll(context).map(Some)
    })
    .await
}

async fn authorization_cancelled(ui: Ui) {
    loop {
        if !ui.owner.is_live() || ui.screens.update_cancel_requested.get_untracked() {
            return;
        }
        let (send, reply) = futures_channel::oneshot::channel();
        let _timer = ui.owner.run(move || {
            set_timeout(Duration::from_millis(100), move || {
                let _ = send.send(());
            })
        });
        if reply.await.is_err() {
            return;
        }
    }
}

pub(super) async fn until_update_cancelled<T>(
    ui: Ui,
    operation: impl std::future::Future<Output = T>,
) -> Option<T> {
    until_cancelled(operation, authorization_cancelled(ui), move || {
        !ui.owner.is_live() || ui.screens.update_cancel_requested.get_untracked()
    })
    .await
}

fn download_blocked(ui: Ui) -> bool {
    ui.screens.update_install_busy.get() || snapshot(ui).checking
}

fn download(ui: Ui) {
    if download_blocked(ui) {
        return;
    }
    let Some(check) = snapshot(ui).cached else {
        if releases_fallback(ui) {
            crate::tasks::spawn_in_app(async { super::open_download_url(super::RELEASES).await });
        }
        return;
    };
    if !newer(ui) {
        return;
    }
    ui.screens.update_install_busy.set(true);
    ui.screens.update_cancel_requested.set(false);
    let operation = InstallOperation::new(ui);
    progress_timer(ui);
    crate::tasks::spawn_in_app(async move {
        let _operation = operation;
        let Some(result) =
            until_update_cancelled(ui, crate::host::updates::action_for(&check)).await
        else {
            return;
        };
        match result {
            Ok(Some(action)) => {
                if ui.screens.update_cancel_requested.get_untracked() {
                    return;
                }
                crate::settings_view::notify(action.label());
                action.run(ui).await;
            }
            Ok(None) => super::open_download_url(&check.release.url).await,
            Err(message) => crate::settings_view::notify(message),
        }
    });
}

fn activate(ui: Ui, index: usize) {
    if !current_controls(ui).contains(&index) {
        return;
    }
    match index {
        0 => check(ui, Trigger::Manual),
        1 => download(ui),
        2 if newer(ui) => skip(ui),
        3 => facade::close(ui),
        4 if ui.screens.update_install_busy.get_untracked() => {
            cancel_download(ui);
        }
        5 if manual_download_visible(ui) => manual_download(ui),
        _ => {}
    }
}

pub(crate) fn key(ui: Ui, action: Action) {
    let at = ui.screens.update_focus.get_untracked();
    let controls = current_controls(ui);
    let position = controls.iter().position(|index| *index == at).unwrap_or(0);
    match action {
        Action::MoveLeft => ui
            .screens
            .update_focus
            .set(controls[position.saturating_sub(1)]),
        Action::MoveRight => ui
            .screens
            .update_focus
            .set(controls[(position + 1).min(controls.len() - 1)]),
        Action::MoveUp => ui.screens.update_notes_scroll.scroll_by(-160.0),
        Action::MoveDown => ui.screens.update_notes_scroll.scroll_by(160.0),
        Action::Select => activate(ui, at),
        _ => {}
    }
}

fn current_controls(ui: Ui) -> Vec<usize> {
    panel_controls(
        newer(ui),
        manual_download_visible(ui),
        ui.screens.update_install_busy.get_untracked(),
        snapshot(ui).checking,
        cancellation_allowed(ui),
        releases_fallback(ui),
    )
}

fn panel_controls(
    newer: bool,
    manual_download: bool,
    install_busy: bool,
    checking: bool,
    cancelable: bool,
    releases_fallback: bool,
) -> Vec<usize> {
    let blocked = install_busy || checking;
    let mut controls = Vec::new();
    if !blocked {
        controls.push(0);
    }
    if !blocked {
        if newer || releases_fallback {
            controls.push(1);
        }
        if manual_download {
            controls.push(5);
        }
    }
    if newer && (!install_busy || cancelable) {
        controls.push(2);
    }
    controls.push(3);
    if install_busy && cancelable {
        controls.push(4);
    }
    controls
}

pub(crate) fn layer(ui: Ui) -> View {
    view()
        .pointer_events(PointerEvents::BoxNone)
        .absolute()
        .inset_0()
        .child(when(move || facade::is_open(ui), move || panel(ui)))
        .child(when(move || notice_open(ui), move || notice(ui)))
}

fn panel(ui: Ui) -> View {
    let size = Scale::new();
    effect(move || {
        let focus = ui.screens.update_focus.get();
        let busy = ui.screens.update_install_busy.get();
        let checking = snapshot(ui).checking;
        let cancelable = cancellation_allowed(ui);
        let controls = panel_controls(
            newer(ui),
            manual_download_visible(ui),
            busy,
            checking,
            cancelable,
            releases_fallback(ui),
        );
        if !controls.contains(&focus) {
            ui.screens.update_focus.set(if busy && cancelable {
                4
            } else if busy || checking {
                3
            } else {
                0
            });
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
            facade::close,
        ))
        .child(ui_kit::heading(size, move || status(ui)).test_id("app-update-status"))
        .child(ui_kit::body(size, move || details(ui)))
        .child(ui_kit::heading(size, move || tr("Release notes")))
        .child(
            ui.screens
                .update_notes_scroll
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
                .child(
                    ui_kit::button(
                        ui,
                        size,
                        move || tr("Check for updates"),
                        move || ui.screens.update_focus.get() == 0,
                        move || activate(ui, 0),
                    )
                    .disabled(move || download_blocked(ui))
                    .opacity(move || {
                        if download_blocked(ui) {
                            0.4_f32
                        } else {
                            1.0_f32
                        }
                    }),
                )
                .child(when(
                    move || newer(ui) || releases_fallback(ui),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || {
                                if pending_ready(ui) {
                                    tr("Install update")
                                } else if snapshot(ui)
                                    .cached
                                    .is_none_or(|check| check.release.download.is_none())
                                {
                                    tr("Release notes and downloads")
                                } else if crate::host::updates::automatic_install_supported() {
                                    tr("Download and update")
                                } else {
                                    tr("Download package")
                                }
                            },
                            move || ui.screens.update_focus.get() == 1,
                            move || activate(ui, 1),
                        )
                        .disabled(move || download_blocked(ui))
                        .opacity(move || {
                            if download_blocked(ui) {
                                0.4_f32
                            } else {
                                1.0_f32
                            }
                        })
                    },
                ))
                .child(when(
                    move || manual_download_visible(ui),
                    move || {
                        // Explicit browser download remains available when the
                        // signed installer feed or appcast cannot be verified.
                        // It never authorizes installation inside Patina.
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Download package"),
                            move || ui.screens.update_focus.get() == 5,
                            move || activate(ui, 5),
                        )
                        .disabled(move || download_blocked(ui))
                        .opacity(move || {
                            if download_blocked(ui) {
                                0.4_f32
                            } else {
                                1.0_f32
                            }
                        })
                    },
                ))
                .child(when(
                    move || newer(ui),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Skip this version"),
                            move || ui.screens.update_focus.get() == 2,
                            move || activate(ui, 2),
                        )
                        .disabled(move || !cancellation_allowed(ui))
                        .opacity(move || {
                            if cancellation_allowed(ui) {
                                1.0_f32
                            } else {
                                0.4_f32
                            }
                        })
                    },
                ))
                .child(ui_kit::button(
                    ui,
                    size,
                    move || tr("Later"),
                    move || ui.screens.update_focus.get() == 3,
                    move || activate(ui, 3),
                ))
                .child(when(
                    move || ui.screens.update_install_busy.get() && cancellation_allowed(ui),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Cancel download"),
                            move || ui.screens.update_focus.get() == 4,
                            move || activate(ui, 4),
                        )
                    },
                )),
        )
}

pub(crate) fn notice_open(ui: Ui) -> bool {
    if facade::is_open(ui)
        || ui.chrome.mode.get() == ViewMode::Fullscreen
        || (snapshot(ui).notice.is_none()
            && (!pending_ready(ui) || ui.screens.update_pending_dismissed.get()))
    {
        return false;
    }
    crate::overlays::Overlay::ALL
        .into_iter()
        .filter(|overlay| *overlay != crate::overlays::Overlay::AppUpdateNotice)
        .all(|overlay| !overlay.is_open(ui))
}

pub(crate) fn dismiss_notice(ui: Ui) {
    ui.screens.update_pending_dismissed.set(true);
    acknowledge(ui);
}

#[derive(Debug, PartialEq, Eq)]
enum NoticeAction {
    Review,
    Dismiss,
}

fn notice_navigation(focus: Signal<usize>, action: Action) -> Option<NoticeAction> {
    match action {
        Action::MoveLeft | Action::MoveUp => focus.set(0),
        Action::MoveRight | Action::MoveDown => focus.set(1),
        Action::Select if focus.get_untracked() == 0 => return Some(NoticeAction::Review),
        Action::Select | Action::Back | Action::ShowMenu => return Some(NoticeAction::Dismiss),
        _ => {}
    }
    None
}

pub(crate) fn notice_key(ui: Ui, action: Action) {
    match notice_navigation(ui.screens.update_focus, action) {
        Some(NoticeAction::Review) => review(ui),
        Some(NoticeAction::Dismiss) => dismiss_notice(ui),
        None => {}
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
                    move || ui.screens.update_focus.get() == 0,
                    move || review(ui),
                ))
                .child(ui_kit::button(
                    ui,
                    size,
                    move || tr("Later"),
                    move || ui.screens.update_focus.get() == 1,
                    move || dismiss_notice(ui),
                )),
        )
}

#[cfg(test)]
mod tests;
