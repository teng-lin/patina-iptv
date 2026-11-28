//! Startup splash and the recovery screen for a store that failed to open.

use iptv_presentation::library_recovery::{
    Action as RecoveryAction, Cause as RecoveryCause, ConfirmChoice, Effect, Recovery, Retry,
};

pub(crate) use crate::kit::ui_kit::APP_NAME;

/// Patina's startup splash over its artwork (`host::splash_artwork`).
pub(crate) fn splash() -> impl nsh_core::prelude::IntoElement {
    use nsh_core::prelude::*;
    let artwork = crate::host::splash_artwork();
    view()
        .test_id("startup-splash")
        .absolute()
        .inset_0()
        .bg(crate::tint(
            crate::theme::Theme::default().background.window,
            0xff,
        ))
        .child(
            image(artwork)
                .accessible(true)
                .label(APP_NAME)
                .w_full()
                .h_full()
                .object_fit(ResizeMode::Contain),
        )
}

#[derive(Clone, PartialEq, Eq)]
enum Cause {
    Corrupt,
    KeyUnavailable(&'static str),
    Newer { found: u32, supported: u32 },
    RestartRequired(&'static str),
    Other(&'static str),
}

impl Cause {
    fn recovery_cause(&self) -> RecoveryCause {
        match self {
            Self::Corrupt => RecoveryCause::Corrupt,
            Self::KeyUnavailable(_) => RecoveryCause::KeyUnavailable,
            Self::Newer { .. } => RecoveryCause::NewerSchema,
            Self::RestartRequired(_) => RecoveryCause::RestartRequired,
            Self::Other(_) => RecoveryCause::Other,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Notice {
    Message(&'static str),
    Rejection(iptv_runtime::backup::error::Rejection),
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoreFailure {
    path: Option<std::path::PathBuf>,
    cause: Cause,
    controls: Recovery,
    notice: Option<Notice>,
}

fn can_restore_backup() -> bool {
    crate::host::HAS_WORKERS && crate::library_ui::backup::pickers().open
}

pub(crate) fn set_restore_available(chrome: crate::ChromeUi, available: bool) {
    chrome.store_startup_failure.update(|failure| {
        if let Some(failure) = failure {
            failure.controls.set_restore_available(available);
        }
    });
}

pub(crate) fn store_failed(
    ui: crate::Ui,
    path: Option<std::path::PathBuf>,
    error: iptv_runtime::Error,
) {
    store_failed_in(ui.chrome, path, error);
}

fn store_failed_in(
    chrome: crate::ChromeUi,
    path: Option<std::path::PathBuf>,
    error: iptv_runtime::Error,
) {
    let diagnostic = || {
        error
            .unavailable_reason()
            .filter(|reason| iptv_presentation::i18n::is_cataloged(reason))
            .unwrap_or_else(|| crate::kit::i18n::key("The saved library could not be opened"))
    };
    let (cause, retryable) = if let Some((found, supported)) = error.newer_schema() {
        (Cause::Newer { found, supported }, false)
    } else if crate::host::HAS_WORKERS && path.is_some() && error.is_corrupt_database() {
        log::warn!("iptv storage initialization failed: {error:?}");
        (Cause::Corrupt, false)
    } else if error.reopen_reason().is_some() {
        log::warn!("iptv storage initialization failed: {error:?}");
        (Cause::RestartRequired(diagnostic()), false)
    } else if error.is_credential_unavailable() {
        log::warn!("iptv storage initialization failed: {error:?}");
        let reason = error
            .unavailable_reason()
            .filter(|reason| iptv_presentation::i18n::is_cataloged(reason))
            .unwrap_or_else(|| {
                crate::kit::i18n::key(
                    "The saved library could not be unlocked with the key from the credential store.",
                )
            });
        (
            Cause::KeyUnavailable(reason),
            error.retryable_open() || error.is_not_database(),
        )
    } else {
        log::warn!("iptv storage initialization failed: {error:?}");
        (Cause::Other(diagnostic()), error.retryable_open())
    };
    let recovery_cause = cause.recovery_cause();
    let restore_available = can_restore_backup();
    chrome.store_startup_failure.update(|failure| {
        let retrying = failure
            .as_ref()
            .map(|previous| previous.controls)
            .filter(|controls| {
                controls.cause() == recovery_cause && controls.retry() == Retry::Checking
            });
        let mut controls = if let Some(mut controls) = retrying {
            controls.failed(if retryable {
                Retry::Failed
            } else {
                Retry::Unavailable
            });
            controls
        } else {
            Recovery::for_cause(
                recovery_cause,
                if retryable {
                    Retry::Ready
                } else {
                    Retry::Unavailable
                },
            )
        };
        controls.set_restore_available(restore_available);
        *failure = Some(StoreFailure {
            path,
            cause,
            controls,
            notice: None,
        });
    });
}

fn change_controls(chrome: crate::ChromeUi, f: impl FnOnce(&mut Recovery) -> bool) -> bool {
    let mut changed = false;
    chrome.store_startup_failure.update(|failure| {
        if let Some(failure) = failure {
            changed = f(&mut failure.controls);
        }
    });
    changed
}

fn change_effect(chrome: crate::ChromeUi, f: impl FnOnce(&mut Recovery) -> Effect) -> Effect {
    let mut effect = Effect::None;
    chrome.store_startup_failure.update(|failure| {
        if let Some(failure) = failure {
            effect = f(&mut failure.controls);
            if effect != Effect::None || failure.controls.confirming_start_fresh().is_some() {
                failure.notice = None;
            }
        }
    });
    effect
}

fn finish_work(chrome: crate::ChromeUi, notice: Option<Notice>) {
    chrome.store_startup_failure.update(|failure| {
        if let Some(failure) = failure {
            failure.controls.finish_work();
            failure.notice = notice;
        }
    });
}

fn controls(chrome: crate::ChromeUi) -> Option<Recovery> {
    chrome
        .store_startup_failure
        .with(|failure| failure.as_ref().map(|failure| failure.controls))
}

fn handle_recovery_store_error(
    chrome: crate::ChromeUi,
    owner: &std::rc::Rc<iptv_runtime::Runtime>,
    error: iptv_runtime::Error,
) {
    owner.start_recordings();
    if error.is_credential_unavailable()
        || error.newer_schema().is_some()
        || error.reopen_reason().is_some()
    {
        let path = chrome
            .store_startup_failure
            .with(|failure| failure.as_ref().and_then(|failure| failure.path.clone()));
        store_failed_in(chrome, path, error);
    } else if error.is_library_not_corrupt() {
        let path = chrome
            .store_startup_failure
            .with(|failure| failure.as_ref().and_then(|failure| failure.path.clone()));
        store_failed_in(chrome, path, error);
        iptv_runtime::data::retry_startup(owner);
    } else {
        log::warn!("iptv recovery: library recovery failed: {error}");
        finish_work(
            chrome,
            Some(Notice::Message(crate::kit::i18n::key(
                "Could not move the damaged library aside",
            ))),
        );
    }
}

fn activate_action(chrome: crate::ChromeUi, app: crate::SharedUi, action: RecoveryAction) {
    let effect = change_effect(chrome, |controls| controls.activate(action));
    run_effect(chrome, app, effect);
}

fn cancel_fresh(chrome: crate::ChromeUi) {
    change_controls(chrome, Recovery::cancel_start_fresh);
}

fn confirm_fresh(chrome: crate::ChromeUi, app: crate::SharedUi) {
    let effect = change_effect(chrome, Recovery::confirm_start_fresh);
    run_effect(chrome, app, effect);
}

fn run_effect(chrome: crate::ChromeUi, app: crate::SharedUi, effect: Effect) {
    match effect {
        Effect::None => {}
        Effect::Retry => {
            iptv_runtime::data::retry_startup(&crate::wiring::runtime_owner::runtime());
        }
        Effect::StartFresh => {
            crate::wiring::tasks::spawn_in_app(async move {
                let owner = crate::wiring::runtime_owner::runtime();
                match owner.recover_corrupt_start_fresh().await {
                    Ok(_) => iptv_runtime::data::retry_startup(&owner),
                    Err(error) => handle_recovery_store_error(chrome, &owner, error),
                }
            });
        }
        Effect::RestoreBackup => {
            crate::wiring::tasks::spawn_in_app(async move {
                let owner = crate::wiring::runtime_owner::runtime();
                let root = owner.data_dir().to_owned();
                let label = crate::kit::i18n::tr("Patina backup");
                let file = match crate::host::backup_documents::select_import(root, label).await {
                    Ok(Some(file)) => file,
                    Ok(None) => {
                        finish_work(chrome, None);
                        return;
                    }
                    Err(error) => {
                        let reason = iptv_runtime::backup::report("choosing a backup", error);
                        finish_work(chrome, Some(Notice::Message(reason)));
                        return;
                    }
                };
                let path = file.path.clone();
                let restored = owner.recover_corrupt_restore_backup(path).await;
                drop(file);
                match restored {
                    Ok(_) => {
                        crate::wiring::settings_wiring::refresh(app);
                        iptv_runtime::data::retry_startup(&owner);
                    }
                    Err(iptv_runtime::backup::RecoveryRestoreError::Store(error)) => {
                        handle_recovery_store_error(chrome, &owner, error);
                    }
                    Err(iptv_runtime::backup::RecoveryRestoreError::Backup(error)) => {
                        owner.start_recordings();
                        let reason =
                            iptv_runtime::backup::report("restoring backup in recovery", error);
                        let notice = match error.rejection() {
                            Some(rejection) => Notice::Rejection(rejection),
                            None => Notice::Message(reason),
                        };
                        finish_work(chrome, Some(notice));
                    }
                }
            });
        }
    }
}

pub(crate) fn store_failure_back(chrome: crate::ChromeUi) -> bool {
    change_controls(chrome, Recovery::back)
}

/// Recovery is a root screen once confirmations and details have been closed.
pub(crate) fn store_failure_back_leaves(chrome: crate::ChromeUi) -> bool {
    controls(chrome).is_some_and(Recovery::is_root)
}

pub(crate) fn store_failure_key(ui: crate::Ui, action: iptv_nav::Action) {
    use iptv_nav::Action;
    let delta = match action {
        Action::MoveUp | Action::MoveLeft => -1,
        Action::MoveDown | Action::MoveRight => 1,
        Action::Select => {
            let effect = change_effect(ui.chrome, Recovery::select_focused);
            run_effect(ui.chrome, ui.app, effect);
            return;
        }
        Action::Back => {
            let _ = store_failure_back(ui.chrome) || crate::shell::back::ask_before_leaving(ui);
            return;
        }
        _ => return,
    };
    if !change_controls(ui.chrome, |controls| controls.step(delta)) {
        let distance = crate::kit::scale::now() * 96.0;
        match action {
            Action::MoveUp => ui.chrome.store_failure_scroll.scroll_by(-distance),
            Action::MoveDown => ui.chrome.store_failure_scroll.scroll_by(distance),
            _ => {}
        }
    }
}

pub(crate) fn store_failure_layer(ui: crate::Ui) -> impl nsh_core::prelude::IntoElement {
    use crate::kit::fonts::MEDIUM;
    use crate::kit::i18n::tr;
    use crate::{kit::scale::Scale, kit::ui_kit, library_ui::setup_view};
    use nsh_core::prelude::*;

    let size = Scale::new();
    let chrome = ui.chrome;
    let app = ui.app;
    let scrolling = chrome.store_failure_scroll;
    let retry = move || controls(chrome).map_or(Retry::Unavailable, Recovery::retry);
    let expanded = move || controls(chrome).is_some_and(Recovery::details);
    let working = move || controls(chrome).is_some_and(Recovery::working);
    let confirming = move || controls(chrome).and_then(Recovery::confirming_start_fresh);
    let cause_kind = move || controls(chrome).map(Recovery::cause);
    let has_action =
        move |action| controls(chrome).is_some_and(|controls| controls.actions().contains(&action));
    let focus_at = move |action| {
        ui.keys_shown()
            && confirming().is_none()
            && controls(chrome).is_some_and(|controls| controls.focus() == action)
    };
    let confirm_focus = move |choice| ui.keys_shown() && confirming() == Some(choice);
    when(
        move || controls(chrome).is_some(),
        move || {
            let intro = setup_view::intro(
                size,
                move || {
                    tr(if cause_kind() == Some(RecoveryCause::NewerSchema) {
                        "Update Patina to open your library"
                    } else {
                        "Patina can’t open your library"
                    })
                },
                move || {
                    tr(match cause_kind() {
                        Some(RecoveryCause::NewerSchema) => {
                            "This library was opened by a newer version of Patina."
                        }
                        Some(RecoveryCause::Corrupt) => {
                            "The saved library file is damaged and can’t be read."
                        }
                        Some(RecoveryCause::KeyUnavailable) => {
                            "Patina couldn’t unlock your library from the device’s credential store."
                        }
                        _ if retry() != Retry::Unavailable => {
                            "The device’s storage was unavailable when Patina started."
                        }
                        _ => "Patina couldn’t read the saved library.",
                    })
                },
            );
            let confirm_fresh_card = move || {
                let cancel_focused = move || confirm_focus(ConfirmChoice::Cancel);
                let fresh_focused = move || confirm_focus(ConfirmChoice::StartFresh);
                view()
                    .test_id("store-recovery-confirm-fresh")
                    .role(AccessibilityRole::Alert)
                    .p(move || size.px(16.0))
                    .gap(move || size.px(12.0))
                    .rounded(px(crate::theme::RADIUS))
                    .bg(move || rgb(app.theme.with(|theme| theme.background.window)))
                    .child(
                        ui_kit::body(size, move || tr("Start with an empty library?"))
                            .font_weight(MEDIUM)
                            .text_color(rgb(crate::theme::BRIGHT_TEXT)),
                    )
                    .child(ui_kit::body(size, move || {
                        tr(
                            "Your current library will be kept in a backup folder; the app will start empty. \
                             Your app settings and device credentials will be kept, and you can add your playlists again.",
                        )
                    }))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap(move || size.px(8.0))
                            .child(
                                scrolling.control(
                                    ui_kit::button(
                                        ui,
                                        size,
                                        move || tr("Cancel"),
                                        cancel_focused,
                                        move || cancel_fresh(chrome),
                                    )
                                    .test_id("store-recovery-cancel-fresh")
                                    .accessibility_state(move || AccessibilityState {
                                        disabled: working(),
                                        ..Default::default()
                                    }),
                                    cancel_focused,
                                ),
                            )
                            .child(
                                scrolling.control(
                                    setup_view::primary(
                                        ui,
                                        size,
                                        move || tr("Start fresh"),
                                        fresh_focused,
                                        move || confirm_fresh(chrome, app),
                                    )
                                    .test_id("store-recovery-confirm-fresh-button")
                                    .accessibility_state(move || AccessibilityState {
                                        disabled: working(),
                                        ..Default::default()
                                    }),
                                    fresh_focused,
                                ),
                            ),
                    )
            };
            let content = view()
                .gap(move || size.px(14.0))
                .child(ui_kit::body(
                    size,
                    move || {
                        tr(match cause_kind() {
                            Some(RecoveryCause::NewerSchema) => {
                                "Install the same version you last used, or a newer one, then reopen Patina."
                            }
                            Some(RecoveryCause::Corrupt)
                                if has_action(RecoveryAction::RestoreBackup) =>
                            {
                                "Restore a Patina backup, or start fresh with an empty library. \
                                 Starting fresh keeps your app settings and device credentials and \
                                 moves the unreadable library file into a recovery folder."
                            }
                            Some(RecoveryCause::Corrupt) => {
                                "Start fresh with an empty library. Starting fresh keeps your \
                                 app settings and device credentials and moves the unreadable \
                                 library file into a recovery folder."
                            }
                            Some(RecoveryCause::RestartRequired) => {
                                "Unlock the device, check file permissions, then reopen Patina."
                            }
                            Some(RecoveryCause::KeyUnavailable) if retry() != Retry::Unavailable => {
                                "Unlock the device and check that Patina can access the system \
                                 credential store, then try again. Your saved library has not \
                                 been changed."
                            }
                            _ if retry() != Retry::Unavailable => {
                                "Unlock the device and check that Patina has access to storage \
                                 and enough free space. Then try again."
                            }
                            _ => {
                                "Open the details to see what prevented Patina from loading your library."
                            }
                        })
                    },
                ))
                .child(
                    when(move || confirming().is_some(), confirm_fresh_card).otherwise(move || {
                        action_buttons(
                            ui, size, scrolling, retry, expanded, working, has_action, focus_at,
                        )
                    }),
                )
                .child(when(
                    move || retry() == Retry::Failed,
                    move || {
                        ui_kit::body(size, tr("Patina still can’t open your library."))
                            .role(AccessibilityRole::Alert)
                            .text_color(rgb(ui_kit::ERROR))
                    },
                ))
                .child(failure_notice(chrome, size))
                .child(when(expanded, move || failure_details(chrome, app, size)));
            setup_view::frame(
                ui,
                size,
                move || tr("Library recovery"),
                scrolling,
                setup_view::columns(size, intro, content),
            )
            .test_id("store-startup-failure")
            .accessibility_elements_hidden(move || {
                crate::kit::overlay::Overlay::StoreStartupFailure.accessibility_covered(ui)
            })
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn action_buttons(
    ui: crate::Ui,
    scale: crate::kit::scale::Scale,
    scrolling: crate::kit::ui_kit::FocusScroll,
    retry: impl Fn() -> Retry + Copy + 'static,
    expanded: impl Fn() -> bool + Copy + 'static,
    working: impl Fn() -> bool + Copy + 'static,
    has_action: impl Fn(RecoveryAction) -> bool + Copy + 'static,
    focus_at: impl Fn(RecoveryAction) -> bool + Copy + 'static,
) -> nsh_core::prelude::View {
    use crate::kit::i18n::tr;
    use crate::{kit::ui_kit, library_ui::setup_view};
    use nsh_core::prelude::*;

    let chrome = ui.chrome;
    let app = ui.app;
    let retry_focused = move || focus_at(RecoveryAction::Retry);
    let restore_focused = move || focus_at(RecoveryAction::RestoreBackup);
    let fresh_focused = move || focus_at(RecoveryAction::StartFresh);
    let details_focused = move || focus_at(RecoveryAction::Details);
    let restoring = move || controls(chrome).is_some_and(Recovery::restoring_backup);
    let starting_fresh = move || controls(chrome).is_some_and(Recovery::starting_fresh);

    view()
        .flex_row()
        .flex_wrap()
        .gap(move || scale.px(8.0))
        .child(when(
            move || has_action(RecoveryAction::RestoreBackup),
            move || {
                scrolling.control(
                    setup_view::primary(
                        ui,
                        scale,
                        move || {
                            tr(if restoring() {
                                "Restoring backup…"
                            } else {
                                "Restore backup"
                            })
                        },
                        restore_focused,
                        move || activate_action(chrome, app, RecoveryAction::RestoreBackup),
                    )
                    .test_id("store-recovery-restore")
                    .accessibility_state(move || AccessibilityState {
                        disabled: working(),
                        busy: restoring(),
                        ..Default::default()
                    }),
                    restore_focused,
                )
            },
        ))
        .child(when(
            move || has_action(RecoveryAction::StartFresh),
            move || {
                scrolling.control(
                    ui_kit::button(
                        ui,
                        scale,
                        move || {
                            tr(if starting_fresh() {
                                "Starting fresh…"
                            } else {
                                "Start fresh"
                            })
                        },
                        fresh_focused,
                        move || activate_action(chrome, app, RecoveryAction::StartFresh),
                    )
                    .test_id("store-recovery-start-fresh")
                    .accessibility_state(move || AccessibilityState {
                        disabled: working(),
                        busy: starting_fresh(),
                        ..Default::default()
                    }),
                    fresh_focused,
                )
            },
        ))
        .child(when(
            move || retry() != Retry::Unavailable,
            move || {
                scrolling.control(
                    setup_view::primary(
                        ui,
                        scale,
                        move || {
                            tr(if retry() == Retry::Checking {
                                "Checking library…"
                            } else {
                                "Try again"
                            })
                        },
                        retry_focused,
                        move || activate_action(chrome, app, RecoveryAction::Retry),
                    )
                    .test_id("store-recovery-retry")
                    .accessibility_state(move || AccessibilityState {
                        disabled: working(),
                        busy: retry() == Retry::Checking,
                        ..Default::default()
                    }),
                    retry_focused,
                )
            },
        ))
        .child(
            scrolling.control(
                ui_kit::button(
                    ui,
                    scale,
                    move || {
                        tr(if expanded() {
                            "Hide details"
                        } else {
                            "Show details"
                        })
                    },
                    details_focused,
                    move || activate_action(chrome, app, RecoveryAction::Details),
                )
                .test_id("store-recovery-details-button")
                .accessibility_state(move || AccessibilityState {
                    disabled: working(),
                    expanded: Some(expanded()),
                    ..Default::default()
                }),
                details_focused,
            ),
        )
}

fn failure_notice(
    chrome: crate::ChromeUi,
    scale: crate::kit::scale::Scale,
) -> impl nsh_core::prelude::IntoElement {
    use crate::kit::ui_kit;
    use nsh_core::prelude::*;

    dynamic(move || {
        chrome.store_startup_failure.with(|failure| {
            let notice = failure.as_ref().and_then(|failure| failure.notice)?;
            let message = match notice {
                Notice::Message(reason) => crate::kit::i18n::tr(reason).to_owned(),
                Notice::Rejection(rejection) => {
                    crate::library_ui::backup::recovery_rejection_description(rejection)
                }
            };
            Some(
                ui_kit::body(scale, message)
                    .test_id("store-recovery-error")
                    .role(AccessibilityRole::Alert)
                    .text_color(rgb(ui_kit::ERROR)),
            )
        })
    })
}

fn failure_details(
    chrome: crate::ChromeUi,
    app: crate::SharedUi,
    scale: crate::kit::scale::Scale,
) -> impl nsh_core::prelude::IntoElement {
    use crate::kit::i18n::{tr, trf};
    use crate::kit::ui_kit;
    use nsh_core::prelude::*;

    dynamic(move || {
        chrome.store_startup_failure.with(|failure| {
            failure.as_ref().map(|failure| {
                let diagnostic = match &failure.cause {
                    Cause::Newer { found, supported } => trf(
                        "Database schema version {found} is newer than this build supports ({supported}).",
                        &[("found", (*found).into()), ("supported", (*supported).into())],
                    ),
                    Cause::Corrupt => {
                        tr("The library database failed an integrity check after unlocking.")
                            .to_owned()
                    }
                    Cause::KeyUnavailable(reason)
                    | Cause::RestartRequired(reason)
                    | Cause::Other(reason) => crate::kit::i18n::tr(reason).to_owned(),
                };
                view()
                    .test_id("store-recovery-details")
                    .p(move || scale.px(14.0))
                    .gap(move || scale.px(8.0))
                    .rounded(px(crate::theme::RADIUS))
                    .bg(move || rgb(app.theme.with(|theme| theme.background.window)))
                    .child(ui_kit::body(scale, diagnostic))
                    .children(
                        (!matches!(failure.cause, Cause::Corrupt))
                            .then_some(failure.path.as_ref())
                            .flatten()
                            .into_iter()
                            .map(|path| ui_kit::body(scale, path.display().to_string())),
                    )
            })
        })
    })
}
