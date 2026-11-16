//! Primary row actions, inline management and a separate destructive confirmation.
use super::*;
use crate::play_bar::icons::{self, Icon};

fn control(
    ui: Ui,
    size: Scale,
    job: impl Fn() -> Option<Job> + Copy + 'static,
    icon: Icon,
    action: &'static str,
) -> View {
    view()
        .size(move || size.px(ui_kit::TOUCH))
        .flex_shrink_0()
        .role(AccessibilityRole::Button)
        .label(move || {
            let label = match action {
                "Edit recording" => tr("Edit recording"),
                "Cancel recording" => tr("Cancel recording"),
                "Stop recording" => tr("Stop recording"),
                _ => tr("Delete recording"),
            };
            job()
                .map(|j| format!("{} · {}", label, j.request.title))
                .unwrap_or_default()
        })
        .accessibility_state(move || AccessibilityState {
            disabled: ui.recordings.with(|s| s.busy),
            ..Default::default()
        })
        .items_center()
        .justify_center()
        .on_press(move || {
            ui.touched();
            if let Some(job) = job() {
                if action == "Delete recording" {
                    request_delete(ui, job.id);
                } else {
                    row_action(ui, job.id, action);
                }
            }
        })
        .child(
            svg(icons::src(icon))
                .size(move || size.px(24.0))
                .text_color(rgb(if icon == Icon::Stop {
                    crate::theme::RECORDING
                } else {
                    crate::theme::LIST_TEXT
                })),
        )
}

pub(super) fn job_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let job = move || ui.recordings.with(|s| s.visible().nth(at.index()).cloned());
    let focused = move || job().is_some_and(|j| ui.recordings.with(|s| s.focus == Some(j.id)));
    let day = move || {
        ui.recordings
            .with(|s| {
                let current = s.visible().nth(at.index())?;
                let previous = at.index().checked_sub(1).and_then(|n| s.visible().nth(n));
                previous
                    .is_none_or(|p| {
                        crate::time::date_only(p.request.start)
                            != crate::time::date_only(current.request.start)
                    })
                    .then(|| crate::time::date_only(current.request.start))
            })
            .unwrap_or_default()
    };
    view()
        .gap(move || size.px(6.0))
        .child(when(
            move || !day().is_empty(),
            move || {
                ui_kit::caption(size, day)
                    .px(move || size.px(24.0))
                    .pt(move || size.px(12.0))
            },
        ))
        .child(
            style::row(size, ui_kit::ROW_THREE, focused)
                .flex_row()
                .items_center()
                .role(AccessibilityRole::Row)
                .child(
                    view()
                        .flex_1()
                        .min_h(move || size.px(ui_kit::ROW_THREE))
                        .justify_center()
                        .role(AccessibilityRole::Button)
                        .test_id("recording-job")
                        .label(move || {
                            job()
                                .map(|j| {
                                    format!(
                                        "{} · {} · {} · {}",
                                        status(&j),
                                        j.request.title,
                                        j.request.channel_name,
                                        date(j.request.start)
                                    )
                                })
                                .unwrap_or_default()
                        })
                        .on_press(move || {
                            ui.touched();
                            if let Some(j) = job() {
                                activate_job(ui, j.id);
                            }
                        })
                        .on_long_press(move || {
                            ui.touched();
                            if let Some(j) = job() {
                                select_job(ui, j.id);
                            }
                        })
                        .on_context_menu(move |_: &nsh_core::ContextMenuEvent| {
                            ui.touched();
                            if let Some(j) = job() {
                                select_job(ui, j.id);
                            }
                        })
                        .child(
                            ui_kit::label(size, move || {
                                job().map(|j| j.request.title).unwrap_or_default()
                            })
                            .number_of_lines(1),
                        )
                        .child(
                            ui_kit::caption(size, move || {
                                job()
                                    .map(|j| {
                                        format!(
                                            "{} · {} – {} · {}",
                                            status(&j),
                                            crate::time::time_only(j.request.start),
                                            crate::time::time_only(j.request.end),
                                            j.request.channel_name
                                        )
                                    })
                                    .unwrap_or_default()
                            })
                            .number_of_lines(1),
                        ),
                )
                .child(when(
                    move || job().is_some_and(|j| j.status == Status::Scheduled),
                    move || {
                        view()
                            .flex_row()
                            .child(control(ui, size, job, Icon::Edit, "Edit recording"))
                            .child(control(ui, size, job, Icon::Close, "Cancel recording"))
                    },
                ))
                .child(when(
                    move || job().is_some_and(|j| j.status == Status::Recording),
                    move || control(ui, size, job, Icon::Stop, "Stop recording"),
                ))
                .child(when(
                    move || job().is_some_and(|j| !j.status.active()),
                    move || control(ui, size, job, Icon::Trash, "Delete recording"),
                )),
        )
}

pub(super) fn now_playing(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || ui.player.recorded.with(Option::is_some),
        move || {
            style::action(
                ui,
                size,
                tr("Return to playback"),
                || false,
                move || return_to_player(ui),
            )
            .test_id("recording-return-to-player")
            .label(tr("Return to playback"))
            .flex_row()
            .items_center()
            .gap(move || size.px(16.0))
            .child(
                svg(icons::src(Icon::Play))
                    .size(move || size.px(24.0))
                    .text_color(rgb(crate::theme::LIST_TEXT)),
            )
            .child(
                ui_kit::body(size, move || {
                    ui.player.recorded.with(|r| {
                        r.as_ref()
                            .map(|(job, _)| job.request.title.clone())
                            .unwrap_or_default()
                    })
                })
                .number_of_lines(1),
            )
        },
    )
}

pub(super) fn delete_confirmation(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || ui.recordings.with(|s| s.confirm_delete),
        move || {
            let (outer, card) = crate::wizard_view::entry::frame(ui, size);
            outer.test_id("recording-delete-confirmation").child(
                card.child(ui_kit::title(
                    size,
                    tr("Delete this recording permanently?"),
                ))
                .child(ui_kit::body(size, move || {
                    ui.recordings.with(|s| {
                        s.focused()
                            .map(|j| j.request.title.clone())
                            .unwrap_or_default()
                    })
                }))
                .child(when(
                    move || {
                        ui.player.recorded.with(|r| {
                            r.as_ref()
                                .is_some_and(|(j, _)| ui.recordings.with(|s| s.focus == Some(j.id)))
                        })
                    },
                    move || {
                        ui_kit::caption(size, tr("Deleting this recording will stop its playback."))
                    },
                ))
                .child(
                    view()
                        .flex_row()
                        .gap(move || size.px(12.0))
                        .justify_end()
                        .child(ui_kit::button(
                            ui,
                            size,
                            tr("Cancel"),
                            move || ui.recordings.with(|s| !s.delete_selected),
                            move || {
                                back(ui);
                            },
                        ))
                        .child(ui_kit::button(
                            ui,
                            size,
                            tr("Delete recording"),
                            move || ui.recordings.with(|s| s.delete_selected),
                            move || confirm_delete(ui),
                        )),
                ),
            )
        },
    )
}
