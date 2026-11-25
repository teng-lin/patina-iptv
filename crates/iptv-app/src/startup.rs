//! Cold-start phase marks: one `iptv startup: <phase> +<ms> ms` line per
//! phase in the log, in milliseconds since the process started (where the
//! host knows the process's start, `crate::host::process_age`; elsewhere
//! since the first mark),
//! with what it took or counted. Each phase is logged once per process:
//! later imports, guide checks and zaps stay quiet. `tools/perf-android.sh`
//! reads them for its phase table.
//!
//! The phases, in the order a stored playlist's cold start reaches them:
//! `rust root` (the app's root component runs), `settings loaded`,
//! `fonts registered` (all three registered), `store opened` (the
//! startup source read), `last channel resolved` (its stream known from
//! the stored catalog), `channels read`, `first screen read` and `guide
//! window read` (on the import thread; the window is read once `first
//! screen shown`, the first screen's schedule on screen, which a combined
//! guide of several playlists does not mark), `guide shown`
//! (the schedule swapped in on the UI thread, at a cold start the first
//! screen's), `guide frame` (the next display frame after it), `live input
//! ready` (a shared live stream's first packets from the provider, with how
//! long they took), `player opened` (the stream handed to the player),
//! `video size` (reported dimensions;
//! the shared `size` event does not distinguish metadata from a renderer callback),
//! and `video rendered` (a current-source native renderer submission where supported,
//! never display presentation/scanout).

use iptv_presentation::library_recovery::{Action as RecoveryAction, Recovery, Retry};
use nsh_core::time::Instant;
use std::sync::Mutex;
use std::time::Duration;

pub(crate) const APP_NAME: &str = "Patina";

/// The phases marked so far.
static MARKED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Where [`since_start`] counts from where the process start is unknown.
static FALLBACK: Mutex<Option<Instant>> = Mutex::new(None);

/// Logs `phase` (the first time only).
pub fn mark(phase: &'static str) {
    mark_with(phase, "");
}

/// Logs `phase` with `detail` (the first time only).
pub fn mark_with(phase: &'static str, detail: impl std::fmt::Display) {
    if !first(phase) {
        return;
    }
    let detail = detail.to_string();
    let sep = if detail.is_empty() { "" } else { ", " };
    log::info!(
        "iptv startup: {phase} +{} ms{sep}{detail}",
        since_start().as_millis()
    );
}

/// `data.rs` put a schedule on screen; `steps` say how long after it
/// started each step had ended. Marks `guide shown`, and `guide frame` at
/// the next display frame (the first one showing it).
pub fn shown(steps: &[(&str, Duration)]) {
    if marked("guide shown") {
        return;
    }
    let steps: Vec<String> = steps
        .iter()
        .map(|(step, at)| format!("{step} {} ms", at.as_millis()))
        .collect();
    mark_with("guide shown", steps.join(", "));
    drop(nsh_core::prelude::request_animation_frame(|_| {
        mark("guide frame")
    }));
}

/// Whether `phase` was marked already (without marking it).
pub fn marked(phase: &'static str) -> bool {
    MARKED.lock().is_ok_and(|m| m.contains(&phase))
}

/// Records `phase`; false when it was recorded before.
fn first(phase: &'static str) -> bool {
    let Ok(mut marked) = MARKED.lock() else {
        return false;
    };
    if marked.contains(&phase) {
        return false;
    }
    marked.push(phase);
    true
}

/// Time since the process started.
pub fn since_start() -> Duration {
    if let Some(d) = crate::host::process_age() {
        return d;
    }
    let mut origin = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    origin.get_or_insert_with(Instant::now).elapsed()
}

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
    Newer { found: u32, supported: u32 },
    RestartRequired(&'static str),
    Other(String),
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoreFailure {
    path: Option<std::path::PathBuf>,
    cause: Cause,
    controls: Recovery,
}

pub(crate) fn store_failed(
    ui: crate::Ui,
    path: Option<std::path::PathBuf>,
    error: crate::runtime::Error,
) {
    let retryable = error.retryable_open();
    let cause = if let Some((found, supported)) = error.newer_schema() {
        Cause::Newer { found, supported }
    } else if let Some(reason) = error.reopen_reason() {
        Cause::RestartRequired(reason)
    } else {
        Cause::Other(
            error
                .unavailable_reason()
                .map_or_else(|| error.to_string(), str::to_owned),
        )
    };
    ui.store_startup_failure.update(|failure| {
        let retrying = failure
            .as_ref()
            .map(|previous| previous.controls)
            .filter(|controls| controls.retry() == Retry::Checking);
        let controls = if let Some(mut controls) = retrying {
            controls.failed(if retryable {
                Retry::Failed
            } else {
                Retry::Unavailable
            });
            controls
        } else {
            Recovery::new(if retryable {
                Retry::Ready
            } else {
                Retry::Unavailable
            })
        };
        *failure = Some(StoreFailure {
            path,
            cause,
            controls,
        });
    });
}

fn change_controls(ui: crate::Ui, f: impl FnOnce(&mut Recovery) -> bool) -> bool {
    let mut changed = false;
    ui.store_startup_failure.update(|failure| {
        if let Some(failure) = failure {
            changed = f(&mut failure.controls);
        }
    });
    changed
}

fn controls(ui: crate::Ui) -> Option<Recovery> {
    ui.store_startup_failure
        .with(|failure| failure.as_ref().map(|failure| failure.controls))
}

fn select_recovery(ui: crate::Ui, action: RecoveryAction) {
    if change_controls(ui, |controls| controls.select(action)) {
        crate::runtime::data::retry_startup(&crate::runtime());
    }
}

pub(crate) fn store_failure_back(ui: crate::Ui) -> bool {
    change_controls(ui, Recovery::back)
}

/// Recovery is a root screen once its details have been closed.
pub(crate) fn store_failure_back_leaves(ui: crate::Ui) -> bool {
    controls(ui).is_some_and(|controls| !controls.details())
}

pub(crate) fn store_failure_key(ui: crate::Ui, action: iptv_nav::Action) {
    use iptv_nav::Action;
    let focus = match action {
        Action::MoveUp | Action::MoveLeft => RecoveryAction::Retry,
        Action::MoveDown | Action::MoveRight => RecoveryAction::Details,
        Action::Select => {
            if let Some(controls) = controls(ui) {
                select_recovery(ui, controls.focus());
            }
            return;
        }
        _ => return,
    };
    if !change_controls(ui, |controls| controls.move_to(focus)) {
        let distance = crate::scale::now() * 96.0;
        match action {
            Action::MoveUp => ui.store_failure_scroll.scroll_by(-distance),
            Action::MoveDown => ui.store_failure_scroll.scroll_by(distance),
            _ => {}
        }
    }
}

pub(crate) fn store_failure_layer(ui: crate::Ui) -> impl nsh_core::prelude::IntoElement {
    use crate::i18n::tr;
    use crate::{scale::Scale, setup_view, ui_kit};
    use nsh_core::prelude::*;

    let size = Scale::new();
    let scrolling = ui.store_failure_scroll;
    let retry = move || controls(ui).map_or(Retry::Unavailable, Recovery::retry);
    let expanded = move || controls(ui).is_some_and(Recovery::details);
    let newer = move || {
        ui.store_startup_failure.with(|failure| {
            failure
                .as_ref()
                .is_some_and(|failure| matches!(failure.cause, Cause::Newer { .. }))
        })
    };
    let restart = move || {
        ui.store_startup_failure.with(|failure| {
            failure
                .as_ref()
                .is_some_and(|failure| matches!(failure.cause, Cause::RestartRequired(_)))
        })
    };
    let focus_at = move |action| {
        ui.keys_shown() && controls(ui).is_some_and(|controls| controls.focus() == action)
    };
    let retry_focused = move || focus_at(RecoveryAction::Retry);
    let details_focused = move || focus_at(RecoveryAction::Details);
    when(
        move || controls(ui).is_some(),
        move || {
            let intro = setup_view::intro(
                size,
                move || {
                    tr(if newer() {
                        "Update Patina to open your library"
                    } else {
                        "Patina can’t open your library"
                    })
                },
                move || {
                    tr(if newer() {
                        "This library was opened by a newer version of Patina."
                    } else if retry() != Retry::Unavailable {
                        "The device’s storage was unavailable when Patina started."
                    } else {
                        "Patina couldn’t read the saved library."
                    })
                },
            );
            let content = view()
            .gap(move || size.px(14.0))
            .child(ui_kit::body(size, move || tr(if newer() {
                "Install the same version you last used, or a newer one, then reopen Patina."
            } else if restart() {
                "Unlock the device, check file permissions, then reopen Patina."
            } else if retry() != Retry::Unavailable {
                "Unlock the device and check that Patina has access to storage and enough free space. Then try again."
            } else {
                "Open the details to see what prevented Patina from loading your library."
            })))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap(move || size.px(8.0))
                    .child(when(move || retry() != Retry::Unavailable, move || {
                        scrolling.control(
                            setup_view::primary(
                                ui, size,
                                move || tr(if retry() == Retry::Checking { "Checking library…" } else { "Try again" }),
                                retry_focused,
                                move || select_recovery(ui, RecoveryAction::Retry),
                            )
                            .test_id("store-recovery-retry")
                            .accessibility_state(move || AccessibilityState {
                                disabled: retry() == Retry::Checking,
                                busy: retry() == Retry::Checking,
                                ..Default::default()
                            }),
                            retry_focused,
                        )
                    }))
                    .child(scrolling.control(
                        ui_kit::button(
                            ui, size,
                            move || tr(if expanded() { "Hide details" } else { "Show details" }),
                            details_focused,
                            move || select_recovery(ui, RecoveryAction::Details),
                        )
                        .test_id("store-recovery-details-button")
                        .accessibility_state(move || AccessibilityState {
                            expanded: Some(expanded()),
                            ..Default::default()
                        }),
                        details_focused,
                    )),
            )
            .child(when(move || retry() == Retry::Failed, move || {
                ui_kit::body(size, tr("Patina still can’t open your library."))
                    .text_color(rgb(ui_kit::ERROR))
            }))
            .child(when(expanded, move || failure_details(ui, size)));
            setup_view::frame(
                ui,
                size,
                move || tr("Library recovery"),
                scrolling,
                setup_view::columns(size, intro, content),
            )
            .test_id("store-startup-failure")
        },
    )
}

fn failure_details(
    ui: crate::Ui,
    scale: crate::scale::Scale,
) -> impl nsh_core::prelude::IntoElement {
    use crate::i18n::trf;
    use crate::ui_kit;
    use nsh_core::prelude::*;

    dynamic(move || {
        ui.store_startup_failure.with(|failure| {
            failure.as_ref().map(|failure| {
                let diagnostic = match &failure.cause {
                    Cause::Newer { found, supported } => trf(
                        "Database schema version {found} is newer than this build supports ({supported}).",
                        &[("found", (*found).into()), ("supported", (*supported).into())],
                    ),
                    Cause::RestartRequired(reason) => (*reason).to_owned(),
                    Cause::Other(message) => message.clone(),
                };
                view()
                    .test_id("store-recovery-details")
                    .p(move || scale.px(14.0))
                    .gap(move || scale.px(8.0))
                    .rounded(px(crate::theme::RADIUS))
                    .bg(move || rgb(ui.app.theme.with(|theme| theme.background.window)))
                    .child(ui_kit::body(scale, diagnostic))
                    .children(failure.path.iter().map(|path| ui_kit::body(scale, path.display().to_string())))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_is_marked_once() {
        assert!(!marked("test phase"));
        assert!(first("test phase"));
        assert!(!first("test phase"));
        assert!(marked("test phase"));
    }
}
