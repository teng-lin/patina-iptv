//! The view observes the runtime's cached result; closing it never cancels a check.
use std::time::Duration;

use iptv_nav::Action;
use nsh_core::prelude::*;

use crate::screens::settings_view::updates as facade;
use crate::{
    Ui, ViewMode,
    chrome::i18n::{tr, trf},
    chrome::scale::Scale,
    chrome::ui_kit,
    host::updates::{
        self as host, CheckError, Frequency, Metadata, ReleaseStatus, Service, Snapshot, Trigger,
        UpdatePrompts,
    },
};

#[derive(Clone, Copy)]
pub(crate) struct UpdateUi {
    revision: Signal<u64>,
    ready: Signal<bool>,
    install_busy: Signal<bool>,
    cancel_requested: Signal<bool>,
    pending_dismissed: Signal<bool>,
    focus: Signal<usize>,
    notes_scroll: ui_kit::FocusScroll,
}

impl UpdateUi {
    pub(crate) fn new() -> Self {
        Self {
            revision: signal(0),
            ready: signal(false),
            install_busy: signal(false),
            cancel_requested: signal(false),
            pending_dismissed: signal(false),
            focus: signal(0),
            notes_scroll: ui_kit::FocusScroll::new(),
        }
    }
}

fn update_ui(screens: crate::ScreensUi) -> UpdateUi {
    screens.updates.expect("update view requires its host")
}

fn metadata() -> Metadata {
    let report = crate::wiring::device::reported();
    Metadata {
        platform: crate::wiring::device::host().platform,
        direct_apk: report.and_then(|report| report.direct_apk),
        android_build: report.and_then(|report| report.direct_android_build),
    }
}

fn service() -> Service {
    Service::new(crate::runtime())
}

struct Prompts(Ui);

impl UpdatePrompts for Prompts {
    fn notify(&self, key: &'static str) {
        crate::chrome::toast::notify(tr(key));
    }

    fn translate(&self, key: &'static str) -> &'static str {
        tr(key)
    }

    fn is_open(&self) -> bool {
        facade::is_open(self.0.screens)
    }

    fn is_live(&self) -> bool {
        self.0.owner.is_live()
    }

    fn cancelled(&self) -> bool {
        update_ui(self.0.screens).cancel_requested.get_untracked()
    }

    fn wait_cancelled(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + '_>> {
        Box::pin(authorization_cancelled(self.0))
    }

    fn open_url<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
        Box::pin(host::open_download_url(url, self))
    }

    fn close_after_shutdown(&self) {
        crate::chrome::desktop::close_after_shutdown(self.0.playback);
    }

    fn flush_bookmarks(&self) {
        crate::playback::player::watch_time::flush(self.0.playback);
    }
}

fn changed(screens: crate::ScreensUi) {
    update_ui(screens)
        .revision
        .update(|revision| *revision = revision.wrapping_add(1));
}

fn snapshot(screens: crate::ScreensUi) -> Snapshot {
    update_ui(screens).revision.get();
    service().snapshot()
}

fn supported() -> bool {
    !crate::host::is_tui_mode() && !crate::host::IPAD
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
        if !update_ui(ui.screens).ready.get() || !metadata().ready() {
            return;
        }
        let resumed = !was_foreground.replace(foreground) && foreground;
        if resumed {
            let first = !started.replace(true);
            let trigger = if service().manual_check_pending() {
                Trigger::Manual
            } else if !first {
                Trigger::Automatic
            } else {
                Trigger::Startup
            };
            if trigger == Trigger::Manual || scheduled_enabled() {
                check(ui, trigger);
            }
        } else if foreground && service().manual_check_pending() {
            check(ui, Trigger::Manual);
        }
    });
    effect(move || {
        if ui.screens.update_dialog_open.get()
            && lifecycle() != nsh_core::platform::Lifecycle::Background
            && ui.top_overlay() == Some(crate::chrome::overlays::Overlay::AppUpdate)
            && snapshot(ui.screens).notice.is_some()
        {
            acknowledge(ui.screens);
        }
    });
    crate::wiring::tasks::spawn_in_app(async move {
        if let Err(error) = service().restore().await {
            log::warn!("iptv update schedule restore failed: {error}");
        }
        update_ui(ui.screens).ready.set(true);
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
    update_ui(ui.screens).focus.set(0);
    update_ui(ui.screens).notes_scroll.reset();
    ui.screens.update_dialog_open.set(true);
    service().queue_manual_check();
    if update_ui(ui.screens).ready.get_untracked() {
        check(ui, Trigger::Manual);
    }
}

pub(crate) fn close(screens: crate::ScreensUi) {
    // A retained package or failed re-check can leave a notice waiting behind
    // this panel. Its first visible action must own remote focus on return.
    batch(move || {
        update_ui(screens).focus.set(0);
        screens.update_dialog_open.set(false);
    });
}

fn check(ui: Ui, trigger: Trigger) {
    // A missing Android report is not evidence of a store installation. Do not
    // consume the due check or persist a result for the wrong package channel.
    if update_ui(ui.screens).install_busy.get_untracked()
        || !update_ui(ui.screens).ready.get_untracked()
        || !metadata().ready()
    {
        return;
    }
    let service = service();
    let frequency = Frequency::from_setting(crate::wiring::settings::choice(
        iptv_settings::keys::APP_UPDATE_FREQUENCY,
    ));
    let Some(check) = service.begin_check(trigger, frequency, metadata()) else {
        return;
    };
    changed(ui.screens);
    crate::wiring::tasks::spawn_in_app(async move {
        check.await;
        if !ui.screens.update_dialog_open.get_untracked() {
            update_ui(ui.screens).focus.set(0);
        }
        changed(ui.screens);
        if let Err(error) = service.persist().await {
            log::warn!("iptv update schedule save failed: {error}");
            if trigger == Trigger::Manual {
                crate::chrome::toast::notify(tr("Could not save update preferences. Try again."));
            }
        }
    });
}

fn persist(screens: crate::ScreensUi) {
    changed(screens);
    crate::wiring::tasks::spawn_in_app(async move {
        if let Err(error) = service().persist().await {
            log::warn!("iptv update preferences save failed: {error}");
            crate::chrome::toast::notify(tr("Could not save update preferences. Try again."));
        }
    });
}

fn skip(screens: crate::ScreensUi) {
    if !cancel_download(screens) {
        return;
    }
    if let Some(check) = snapshot(screens).cached {
        service().skip(&check.release.version);
        persist(screens);
    }
    facade::close(screens);
}

fn acknowledge(screens: crate::ScreensUi) {
    service().acknowledge();
    persist(screens);
}

fn review(screens: crate::ScreensUi) {
    crate::wiring::navigation::settings_guarded(screens, move || {
        acknowledge(screens);
        update_ui(screens).focus.set(0);
        update_ui(screens).notes_scroll.reset();
        screens.update_dialog_open.set(true);
    });
}

fn releases_fallback(screens: crate::ScreensUi) -> bool {
    let state = snapshot(screens);
    state.cached.is_none() && state.error.is_some()
}

fn newer(screens: crate::ScreensUi) -> bool {
    snapshot(screens).cached.is_some_and(|check| {
        matches!(
            check.status,
            ReleaseStatus::UpdateAvailable | ReleaseStatus::NoCompatiblePackage
        )
    })
}

fn manual_download_visible(screens: crate::ScreensUi) -> bool {
    newer(screens)
        && host::automatic_install_supported(metadata())
        && snapshot(screens)
            .cached
            .is_some_and(|check| check.release.download.is_some())
}

fn manual_download(ui: Ui) {
    if download_blocked(ui.screens) {
        return;
    }
    if let Some(download) = snapshot(ui.screens)
        .cached
        .and_then(|check| check.release.download)
    {
        crate::wiring::tasks::spawn_in_app(
            async move { Prompts(ui).open_url(&download.url).await },
        );
    }
}

fn pending_ready(screens: crate::ScreensUi) -> bool {
    let Some(asset) = service().pending_url() else {
        return false;
    };
    snapshot(screens)
        .cached
        .and_then(|check| check.release.download)
        .is_some_and(|download| download.url == asset)
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

fn status(screens: crate::ScreensUi) -> String {
    let state = snapshot(screens);
    status_for(
        &state,
        update_ui(screens).ready.get() && metadata().ready(),
        update_ui(screens).install_busy.get(),
        service().progress(),
        pending_ready(screens),
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

fn details(screens: crate::ScreensUi) -> String {
    let state = snapshot(screens);
    let installed = trf(
        "Installed version: {version}",
        &[("version", env!("CARGO_PKG_VERSION").into())],
    );
    let checked = state
        .last_success
        .map(|time| {
            let time = format!(
                "{} {}",
                crate::guide::clock::date_only(time),
                crate::guide::clock::time_only(time)
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

fn notes(screens: crate::ScreensUi) -> String {
    snapshot(screens).cached.map_or_else(String::new, |check| {
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
            busy: update_ui(ui.screens).install_busy,
            panel_open: ui.screens.update_dialog_open,
            focus: update_ui(ui.screens).focus,
            pending_dismissed: update_ui(ui.screens).pending_dismissed,
            revision: update_ui(ui.screens).revision,
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
            if update_ui(ui.screens).install_busy.get_untracked() {
                changed(ui.screens);
                progress_timer(ui);
            }
        });
    });
}

fn cancellation_allowed(screens: crate::ScreensUi) -> bool {
    update_ui(screens).revision.get();
    !update_ui(screens).install_busy.get() || host::can_cancel_install(&crate::runtime())
}

fn cancel_download(screens: crate::ScreensUi) -> bool {
    // Native extraction can win between rendering an enabled button and the
    // press. Do not claim cancellation or persist Skip after that boundary.
    if !host::try_cancel_current(&crate::runtime()) {
        changed(screens);
        return false;
    }
    update_ui(screens).cancel_requested.set(true);
    service().cancel_download();
    changed(screens);
    true
}

async fn authorization_cancelled(ui: Ui) {
    loop {
        if !ui.owner.is_live() || update_ui(ui.screens).cancel_requested.get_untracked() {
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

fn download_blocked(screens: crate::ScreensUi) -> bool {
    update_ui(screens).install_busy.get() || snapshot(screens).checking
}

fn download(ui: Ui) {
    if download_blocked(ui.screens) {
        return;
    }
    let Some(check) = snapshot(ui.screens).cached else {
        if releases_fallback(ui.screens) {
            crate::wiring::tasks::spawn_in_app(async move {
                Prompts(ui).open_url(host::RELEASES).await
            });
        }
        return;
    };
    if !newer(ui.screens) {
        return;
    }
    update_ui(ui.screens).install_busy.set(true);
    update_ui(ui.screens).cancel_requested.set(false);
    let operation = InstallOperation::new(ui);
    progress_timer(ui);
    crate::wiring::tasks::spawn_in_app(async move {
        let _operation = operation;
        let prompts = Prompts(ui);
        let owner = crate::runtime();
        let Some(result) =
            host::until_update_cancelled(&prompts, host::action_for(&owner, metadata(), &check))
                .await
        else {
            return;
        };
        match result {
            Ok(Some(action)) => {
                if update_ui(ui.screens).cancel_requested.get_untracked() {
                    return;
                }
                prompts.notify(action.label());
                action.run(&owner, &prompts).await;
            }
            Ok(None) => prompts.open_url(&check.release.url).await,
            Err(message) => prompts.notify(message),
        }
    });
}

fn activate(ui: Ui, index: usize) {
    if !current_controls(ui.screens).contains(&index) {
        return;
    }
    match index {
        0 => check(ui, Trigger::Manual),
        1 => download(ui),
        2 if newer(ui.screens) => skip(ui.screens),
        3 => facade::close(ui.screens),
        4 if update_ui(ui.screens).install_busy.get_untracked() => {
            cancel_download(ui.screens);
        }
        5 if manual_download_visible(ui.screens) => manual_download(ui),
        _ => {}
    }
}

pub(crate) fn key(ui: Ui, action: Action) {
    let at = update_ui(ui.screens).focus.get_untracked();
    let controls = current_controls(ui.screens);
    let position = controls.iter().position(|index| *index == at).unwrap_or(0);
    match action {
        Action::MoveLeft => update_ui(ui.screens)
            .focus
            .set(controls[position.saturating_sub(1)]),
        Action::MoveRight => update_ui(ui.screens)
            .focus
            .set(controls[(position + 1).min(controls.len() - 1)]),
        Action::MoveUp => update_ui(ui.screens).notes_scroll.scroll_by(-160.0),
        Action::MoveDown => update_ui(ui.screens).notes_scroll.scroll_by(160.0),
        Action::Select => activate(ui, at),
        _ => {}
    }
}

fn current_controls(screens: crate::ScreensUi) -> Vec<usize> {
    panel_controls(
        newer(screens),
        manual_download_visible(screens),
        update_ui(screens).install_busy.get_untracked(),
        snapshot(screens).checking,
        cancellation_allowed(screens),
        releases_fallback(screens),
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
        .child(when(move || facade::is_open(ui.screens), move || panel(ui)))
        .child(when(move || notice_open(ui), move || notice(ui)))
}

fn panel(ui: Ui) -> View {
    let size = Scale::new();
    effect(move || {
        let focus = update_ui(ui.screens).focus.get();
        let busy = update_ui(ui.screens).install_busy.get();
        let checking = snapshot(ui.screens).checking;
        let cancelable = cancellation_allowed(ui.screens);
        let controls = panel_controls(
            newer(ui.screens),
            manual_download_visible(ui.screens),
            busy,
            checking,
            cancelable,
            releases_fallback(ui.screens),
        );
        if !controls.contains(&focus) {
            update_ui(ui.screens).focus.set(if busy && cancelable {
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
            crate::chrome::overlays::Overlay::AppUpdate.accessibility_covered(ui)
        })
        .child(ui_kit::header(
            ui,
            size,
            move || tr("App updates"),
            |ui| facade::close(ui.screens),
        ))
        .child(ui_kit::heading(size, move || status(ui.screens)).test_id("app-update-status"))
        .child(ui_kit::body(size, move || details(ui.screens)))
        .child(ui_kit::heading(size, move || tr("Release notes")))
        .child(
            update_ui(ui.screens)
                .notes_scroll
                .view()
                .flex_1()
                .test_id("app-update-notes")
                .child(ui_kit::body(size, move || notes(ui.screens))),
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
                        move || update_ui(ui.screens).focus.get() == 0,
                        move || activate(ui, 0),
                    )
                    .disabled(move || download_blocked(ui.screens))
                    .opacity(move || {
                        if download_blocked(ui.screens) {
                            0.4_f32
                        } else {
                            1.0_f32
                        }
                    }),
                )
                .child(when(
                    move || newer(ui.screens) || releases_fallback(ui.screens),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || {
                                if pending_ready(ui.screens) {
                                    tr("Install update")
                                } else if snapshot(ui.screens)
                                    .cached
                                    .is_none_or(|check| check.release.download.is_none())
                                {
                                    tr("Release notes and downloads")
                                } else if host::automatic_install_supported(metadata()) {
                                    tr("Download and update")
                                } else {
                                    tr("Download package")
                                }
                            },
                            move || update_ui(ui.screens).focus.get() == 1,
                            move || activate(ui, 1),
                        )
                        .disabled(move || download_blocked(ui.screens))
                        .opacity(move || {
                            if download_blocked(ui.screens) {
                                0.4_f32
                            } else {
                                1.0_f32
                            }
                        })
                    },
                ))
                .child(when(
                    move || manual_download_visible(ui.screens),
                    move || {
                        // Explicit browser download remains available when the
                        // signed installer feed or appcast cannot be verified.
                        // It never authorizes installation inside Patina.
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Download package"),
                            move || update_ui(ui.screens).focus.get() == 5,
                            move || activate(ui, 5),
                        )
                        .disabled(move || download_blocked(ui.screens))
                        .opacity(move || {
                            if download_blocked(ui.screens) {
                                0.4_f32
                            } else {
                                1.0_f32
                            }
                        })
                    },
                ))
                .child(when(
                    move || newer(ui.screens),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Skip this version"),
                            move || update_ui(ui.screens).focus.get() == 2,
                            move || activate(ui, 2),
                        )
                        .disabled(move || !cancellation_allowed(ui.screens))
                        .opacity(move || {
                            if cancellation_allowed(ui.screens) {
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
                    move || update_ui(ui.screens).focus.get() == 3,
                    move || activate(ui, 3),
                ))
                .child(when(
                    move || {
                        update_ui(ui.screens).install_busy.get() && cancellation_allowed(ui.screens)
                    },
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            move || tr("Cancel download"),
                            move || update_ui(ui.screens).focus.get() == 4,
                            move || activate(ui, 4),
                        )
                    },
                )),
        )
}

pub(crate) fn notice_open(ui: Ui) -> bool {
    if facade::is_open(ui.screens)
        || ui.chrome.mode.get() == ViewMode::Fullscreen
        || (snapshot(ui.screens).notice.is_none()
            && (!pending_ready(ui.screens) || update_ui(ui.screens).pending_dismissed.get()))
    {
        return false;
    }
    crate::chrome::overlays::Overlay::ALL
        .into_iter()
        .filter(|overlay| *overlay != crate::chrome::overlays::Overlay::AppUpdateNotice)
        .all(|overlay| !overlay.is_open(ui))
}

pub(crate) fn dismiss_notice(screens: crate::ScreensUi) {
    update_ui(screens).pending_dismissed.set(true);
    acknowledge(screens);
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

pub(crate) fn notice_key(screens: crate::ScreensUi, action: Action) {
    match notice_navigation(update_ui(screens).focus, action) {
        Some(NoticeAction::Review) => review(screens),
        Some(NoticeAction::Dismiss) => dismiss_notice(screens),
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
            if pending_ready(ui.screens) {
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
                    move || update_ui(ui.screens).focus.get() == 0,
                    move || review(ui.screens),
                ))
                .child(ui_kit::button(
                    ui,
                    size,
                    move || tr("Later"),
                    move || update_ui(ui.screens).focus.get() == 1,
                    move || dismiss_notice(ui.screens),
                )),
        )
}

#[cfg(test)]
mod tests;
