//! Batch guide repair inside the existing guide inspection workflow.
use crate::runtime::model::InspectionControl;
use crate::{
    Ui,
    chrome::i18n::tr,
    chrome::scale::Scale,
    chrome::ui_kit,
    guide::list_rows::{self, Mark, ROW_TWO},
};
use iptv_library::{Choice, SourceId, guide_repair as model, inspection::*};
use iptv_nav::Action;
use iptv_presentation::library::guide_repair_screen::{Reply, Row, Screen};
use nsh_core::Checked;
use nsh_core::prelude::*;
use std::time::Duration;

fn state<R>(ui: Ui, f: impl FnOnce(&Screen) -> R) -> Option<R> {
    ui.screens.guide_repair.with(|s| s.as_ref().map(f))
}
fn update(ui: Ui, f: impl FnOnce(&mut Screen)) {
    ui.screens.guide_repair.update(|s| {
        if let Some(s) = s {
            f(s);
        }
    });
}
fn count(ui: Ui) {
    let pending = state(ui, |s| {
        if s.outcome.is_none() {
            s.request.mappings.len()
        } else {
            0
        }
    })
    .unwrap_or(0);
    ui.screens.guide_inspect.update(|s| {
        if let Some(s) = s {
            s.set_pending_repairs(pending);
        }
    });
}
fn cancel(ui: Ui) {
    ui.screens.guide_repair_cancel.with(|token| {
        if let Some(token) = token.take() {
            token.cancel();
        }
    });
}
fn begin(ui: Ui) -> Option<(Screen, u64, InspectionControl)> {
    cancel(ui);
    let control = InspectionControl {
        timeout: Duration::from_secs(15),
        ..Default::default()
    };
    let mut accepted = None;
    update(ui, |s| {
        let generation = s.begin();
        accepted = Some((s.clone(), generation, control.clone()));
    });
    ui.screens
        .guide_repair_cancel
        .with(|token| *token = Some(control.cancel.clone()));
    accepted
}
fn accepts(screen: &Screen, generation: u64, cancel: &iptv_import::CancelToken) -> bool {
    !cancel.is_canceled() && screen.accepts(generation)
}
fn finish(
    ui: Ui,
    generation: u64,
    cancel: &iptv_import::CancelToken,
    result: model::Result<Reply>,
) {
    update(ui, |s| finish_screen(s, generation, cancel, result));
}
fn finish_screen(
    screen: &mut Screen,
    generation: u64,
    cancel: &iptv_import::CancelToken,
    result: model::Result<Reply>,
) {
    if !cancel.is_canceled() {
        screen.finish(generation, result);
    }
}
fn release(preview: Option<model::Preview>) {
    if let Some(p) = preview {
        let _ = crate::wiring::runtime_owner::runtime().release_guide_repair_plan(&p.id, p.digest);
    }
}
fn invalidate(ui: Ui) {
    let mut old = None;
    update(ui, |s| old = s.invalidate());
    release(old);
    count(ui);
}
pub fn is_open(ui: Ui) -> bool {
    state(ui, |s| s.open).unwrap_or(false)
}
pub fn close(ui: Ui) {
    cancel(ui);
    let mut old = None;
    update(ui, |s| old = s.close());
    release(old);
    count(ui);
}
pub fn back(ui: Ui) -> bool {
    close(ui);
    true
}
fn dismiss_inspection_input(ui: Ui) {
    ui.screens.guide_inspect.update(|s| {
        if let Some(s) = s {
            s.typing = false;
        }
    });
    crate::chrome::input::dismiss_keyboard();
}
pub fn reopen(ui: Ui) {
    dismiss_inspection_input(ui);
    update(ui, Screen::reopen);
}

pub fn add(ui: Ui, channel: ChannelRef, target: model::Target, window: TimeWindow) {
    dismiss_inspection_input(ui);
    if state(ui, |s| s.busy).unwrap_or(false) {
        cancel(ui);
    }
    // Concurrent selections share the current screen lifetime. Closing,
    // clearing, or starting a new operation cancels every old selection.
    let cancel = ui.screens.guide_repair_cancel.with(|token| {
        token
            .get_or_insert_with(iptv_import::CancelToken::new)
            .clone()
    });
    let inspection = ui.screens.guide_inspect_cancel.with(|token| token.clone());
    let control = InspectionControl {
        cancel: cancel.clone(),
        ..Default::default()
    };
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let result = owner.inspect_channel(channel.clone(), control).await;
        if cancel.is_canceled()
            || inspection
                .as_ref()
                .is_some_and(iptv_import::CancelToken::is_canceled)
        {
            return;
        }
        let info = match result {
            Ok(info) => info,
            Err(_) => {
                crate::chrome::toast::notify(tr("Guide changes could not be saved"));
                return;
            }
        };
        if state(ui, |s| {
            s.request.source != channel.source
                && s.outcome.is_none()
                && !s.request.mappings.is_empty()
        })
        .unwrap_or(false)
        {
            crate::chrome::toast::notify(tr("Finish the current guide changes first"));
            return;
        }
        if state(ui, |s| {
            s.request.source != channel.source || s.outcome.is_some()
        })
        .unwrap_or(true)
        {
            ui.screens
                .guide_repair
                .set(Some(Screen::new(channel.source, window)));
        }
        invalidate(ui);
        update(ui, |s| {
            s.add(info, target);
        });
        count(ui);
    });
}
pub fn history(ui: Ui, source: SourceId, window: TimeWindow) {
    dismiss_inspection_input(ui);
    if state(ui, |s| {
        s.outcome.is_none() && !s.request.mappings.is_empty()
    })
    .unwrap_or(false)
    {
        reopen(ui);
        return;
    }
    ui.screens
        .guide_repair
        .set(Some(Screen::new(source, window)));
    let Some((_, generation, control)) = begin(ui) else {
        return;
    };
    let cancel = control.cancel.clone();
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let result = owner.recent_guide_repairs(source, 20, control).await;
        finish(ui, generation, &cancel, result.map(Reply::History));
    });
}
fn preview(ui: Ui) {
    let Some((screen, generation, control)) = begin(ui) else {
        return;
    };
    let cancel = control.cancel.clone();
    let owner = crate::wiring::runtime_owner::runtime();
    if let Some(preview) = screen.preview {
        let _ = owner.release_guide_repair_plan(&preview.id, preview.digest);
    }
    crate::wiring::tasks::spawn_in_app(async move {
        match owner.preview_guide_repair(screen.request, control).await {
            Ok(preview) => {
                if state(ui, |s| accepts(s, generation, &cancel)).unwrap_or(false) {
                    finish(ui, generation, &cancel, Ok(Reply::Preview(preview)));
                } else {
                    let _ = owner.release_guide_repair_plan(&preview.id, preview.digest);
                }
            }
            Err(error) => finish(ui, generation, &cancel, Err(error)),
        }
    });
}
// Read-only previews and history entries carry no settings authorization.
// Check again at mutation admission, including after the PIN prompt resumes.
fn request_settings_pin(ui: Ui, retry: fn(Ui)) -> bool {
    let required = crate::wiring::settings::choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR)
        == "blocked_content_and_settings"
        && !crate::wiring::runtime_owner::runtime().parental_settings_allowed();
    if required {
        crate::parental_view::protected_settings(ui, move || retry(ui));
    }
    required
}

fn apply(ui: Ui) {
    if request_settings_pin(ui, apply) {
        return;
    }
    let Some((screen, generation, control)) = begin(ui) else {
        return;
    };
    let cancel = control.cancel.clone();
    let Some(preview) = screen.preview else {
        finish(ui, generation, &cancel, Err(model::Error::PlanMissing));
        return;
    };
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let result = owner
            .apply_guide_repair(preview.id.clone(), preview.digest, control)
            .await;
        let result = match result {
            Ok(outcome) => Ok(Reply::Applied(outcome)),
            Err(model::Error::CompletionUnknown { operation }) => {
                Ok(Reply::CompletionUnknown(operation))
            }
            Err(error) => Err(error),
        };
        finish(ui, generation, &cancel, result);
        count(ui);
    });
}
fn status(ui: Ui, id: Option<String>) {
    let Some((screen, generation, control)) = begin(ui) else {
        return;
    };
    let cancel = control.cancel.clone();
    let Some(id) = id.or_else(|| screen.commit_id()) else {
        finish(ui, generation, &cancel, Err(model::Error::CommitMissing));
        return;
    };
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let result = owner.guide_repair_operation(id).await;
        finish(ui, generation, &cancel, result.map(Reply::Status));
    });
}
fn verify(ui: Ui) {
    let Some((screen, generation, control)) = begin(ui) else {
        return;
    };
    let cancel = control.cancel.clone();
    let Some(id) = screen.commit_id() else {
        finish(ui, generation, &cancel, Err(model::Error::CommitMissing));
        return;
    };
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let result = owner
            .verify_guide_repair(id, screen.request.window, control)
            .await;
        finish(ui, generation, &cancel, result.map(Reply::Verified));
    });
}
fn undo(ui: Ui) {
    if request_settings_pin(ui, undo) {
        return;
    }
    let Some((screen, generation, control)) = begin(ui) else {
        return;
    };
    let cancel = control.cancel.clone();
    let Some(id) = screen.commit_id() else {
        finish(ui, generation, &cancel, Err(model::Error::CommitMissing));
        return;
    };
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let result = owner.undo_guide_repair(id, control).await;
        finish(ui, generation, &cancel, result.map(Reply::Undone));
    });
}
fn page(ui: Ui, indirect: bool) {
    let Some(screen) = state(ui, Clone::clone) else {
        return;
    };
    let Some(preview) = screen.preview else {
        return;
    };
    let mapping = if indirect {
        screen.mapping_offset
    } else {
        preview.next_mapping.unwrap_or(0)
    };
    let effects = if indirect {
        preview.next_indirect.unwrap_or(0)
    } else {
        screen.indirect_offset
    };
    match crate::wiring::runtime_owner::runtime().guide_repair_plan(
        &preview.id,
        preview.digest,
        mapping,
        effects,
        model::MAX_PREVIEW_PAGE,
    ) {
        Ok(preview) => update(ui, |s| s.set_page(preview, mapping, effects)),
        Err(error) => update(ui, |s| s.report_error(Some(error))),
    }
}
fn activate(ui: Ui, row: Row) {
    if state(ui, |s| s.busy).unwrap_or(true) && row != Row::Back {
        return;
    }
    update(ui, |s| s.focus_row(row.clone()));
    match row {
        Row::Mapping(key) => {
            invalidate(ui);
            update(ui, |s| s.toggle_replacement(&key));
        }
        Row::Remove(key) => {
            invalidate(ui);
            update(ui, |s| {
                s.remove(&key);
            });
            count(ui);
        }
        Row::Preview => preview(ui),
        Row::Apply => apply(ui),
        Row::Verify => verify(ui),
        Row::Undo => undo(ui),
        Row::Status => status(ui, None),
        Row::History(id) => status(ui, Some(id)),
        Row::MoreMappings => page(ui, false),
        Row::MoreIndirect => page(ui, true),
        Row::SelectMore | Row::Back => close(ui),
        Row::Clear => {
            cancel(ui);
            invalidate(ui);
            update(ui, Screen::clear);
            count(ui);
        }
        Row::Cancel => {
            if let Some(id) = state(ui, Screen::commit_id).flatten() {
                let result =
                    crate::wiring::runtime_owner::runtime().cancel_guide_repair_operation(&id);
                update(ui, |s| s.report_error(result.err()));
                status(ui, Some(id));
            }
        }
    }
}
pub fn key(ui: Ui, action: Action) {
    match action {
        Action::MoveUp => update(ui, |s| s.move_focus(-1)),
        Action::MoveDown => update(ui, |s| s.move_focus(1)),
        Action::PageUp => update(ui, |s| s.move_focus(-8)),
        Action::PageDown => update(ui, |s| s.move_focus(8)),
        Action::Select => {
            if let Some(row) = state(ui, |s| s.focus.clone()) {
                activate(ui, row);
            }
        }
        Action::Back | Action::ShowMenu => close(ui),
        _ => {}
    }
}
fn target(value: &model::Target) -> String {
    match value {
        model::Target::Automatic => tr("Automatic").into(),
        model::Target::NoGuide => tr("No guide data").into(),
        model::Target::Configured(key) => key.epg_channel.clone(),
        model::Target::Staged { epg_channel, .. } => epg_channel.clone(),
    }
}
fn choice(value: &Choice) -> String {
    match value {
        Choice::Automatic => tr("Automatic").into(),
        Choice::NoGuide => tr("No guide data").into(),
        Choice::Channel { epg_channel, .. } => epg_channel.clone(),
    }
}
fn error(error: &model::Error) -> &'static str {
    match error {
        model::Error::Conflict(_) => tr("Guide changes conflict; inspect again"),
        model::Error::UndoExpired => tr("Undo is no longer available"),
        model::Error::CompletionUnknown { .. } => tr("Save outcome is unknown; check status"),
        model::Error::PlanExpired | model::Error::PlanMissing | model::Error::WrongDigest => {
            tr("Guide evidence changed; inspect again")
        }
        model::Error::Inspection(value) => crate::guide_inspect_view::error_text(value.clone()),
        model::Error::Canceled => tr("Canceled"),
        _ => tr("Guide changes could not be saved"),
    }
}
fn title(ui: Ui, row: &Row) -> String {
    match row {
        Row::Mapping(key) => state(ui, |s| {
            s.names
                .iter()
                .find(|(id, _, _)| id == key)
                .map(|(_, name, _)| name.clone())
        })
        .flatten()
        .unwrap_or_default(),
        Row::Remove(key) => format!(
            "{} · {}",
            tr("Remove"),
            state(ui, |s| s
                .names
                .iter()
                .find(|(id, _, _)| id == key)
                .map(|(_, n, _)| n.clone()))
            .flatten()
            .unwrap_or_default()
        ),
        Row::Preview => tr("Preview guide changes").into(),
        Row::Apply => tr("Apply guide changes").into(),
        Row::MoreMappings | Row::MoreIndirect => tr("Next").into(),
        Row::Verify => tr("Verify guide changes").into(),
        Row::Status => tr("Check repair status").into(),
        Row::Undo => tr("Undo guide changes").into(),
        Row::Cancel => tr("Cancel").into(),
        Row::SelectMore => tr("Keep selecting channels").into(),
        Row::Clear => tr("Clear").into(),
        Row::Back => tr("Back").into(),
        Row::History(id) => state(ui, |s| {
            s.history.iter().find(|h| h.id == *id).map(|h| {
                format!(
                    "{} · {}",
                    tr("History"),
                    crate::guide::clock::label(h.created_at, crate::guide::clock::wall_unix())
                )
            })
        })
        .flatten()
        .unwrap_or_default(),
    }
}
fn detail(ui: Ui, row: &Row) -> String {
    match row {
        Row::Mapping(key) => state(ui, |s| {
            let request = s.request.mappings.iter().find(|r| r.channel == *key)?;
            let before = s
                .preview
                .as_ref()
                .and_then(|p| p.mappings.iter().find(|p| p.channel == *key))
                .map(|p| &p.before)
                .or_else(|| {
                    s.names
                        .iter()
                        .find(|(id, _, _)| id == key)
                        .map(|(_, _, c)| c)
                });
            Some(format!(
                "{} → {} · {}",
                before.map(choice).unwrap_or_default(),
                target(&request.target),
                tr("Allow replacing this saved choice")
            ))
        })
        .flatten()
        .unwrap_or_default(),
        Row::Cancel => tr("Saved choices remain").into(),
        _ => String::new(),
    }
}
fn outcome(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || state(ui, |s| s.outcome.is_some()).unwrap_or(false),
        move || {
            ui_kit::caption(size, move || {
                state(ui, |s| {
                    if s.outcome.as_ref().is_some_and(|o| o.commit.undone) {
                        tr("Guide changes undone")
                    } else if s.verification.as_ref().is_some_and(|v| v.verified_coverage) {
                        tr("Guide coverage verified")
                    } else if s
                        .outcome
                        .as_ref()
                        .is_some_and(|o| o.effective == model::Effective::Failed)
                    {
                        tr("Guide choices saved; guide refresh failed")
                    } else {
                        tr("Guide choices saved; verification pending")
                    }
                })
                .unwrap_or_default()
            })
            .test_id("guide-repair-outcome")
        },
    )
}
fn summary(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || state(ui, |s| s.preview.is_some()).unwrap_or(false),
        move || {
            ui_kit::caption(size, move || {
                state(ui, |s| {
                    s.preview.as_ref().map(|p| {
                        format!(
                            "{}: {} · {}: {}",
                            tr("Guide data"),
                            p.mapping_count,
                            tr("Other channels affected"),
                            p.indirect_count
                        )
                    })
                })
                .flatten()
                .unwrap_or_default()
            })
            .test_id("guide-repair-preview")
        },
    )
}
fn feeds(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || {
            state(ui, |s| {
                s.preview.as_ref().is_some_and(|p| !p.feeds.is_empty())
            })
            .unwrap_or(false)
        },
        move || {
            virtual_list(
                move || state(ui, |s| s.preview.as_ref().map_or(0, |p| p.feeds.len())).unwrap_or(0),
                move |at| {
                    let feed = move || {
                        state(ui, |s| {
                            s.preview
                                .as_ref()
                                .and_then(|p| p.feeds.get(at.index()).cloned())
                        })
                        .flatten()
                    };
                    list_rows::row(size, ROW_TWO, || Mark::of(false, false))
                        .test_id("guide-repair-feed")
                        .child(ui_kit::label(size, move || {
                            feed()
                                .map(|f| {
                                    if f.name.is_empty() {
                                        tr("Guide sources").into()
                                    } else {
                                        f.name
                                    }
                                })
                                .unwrap_or_default()
                        }))
                        .child(ui_kit::detail(size, move || {
                            feed()
                                .map(|f| {
                                    let switch =
                                        |enabled| if enabled { tr("On") } else { tr("Off") };
                                    format!(
                                        "{} → {} · {} → {} · #{} → #{}",
                                        f.before_enabled.map(switch).unwrap_or(tr("Add")),
                                        f.after_enabled.map(switch).unwrap_or(tr("Remove")),
                                        f.before_shift_minutes
                                            .map(|n| n.to_string())
                                            .unwrap_or_default(),
                                        f.after_shift_minutes
                                            .map(|n| n.to_string())
                                            .unwrap_or_default(),
                                        f.before_position
                                            .map(|n| (n + 1).to_string())
                                            .unwrap_or_default(),
                                        f.after_position
                                            .map(|n| (n + 1).to_string())
                                            .unwrap_or_default()
                                    )
                                })
                                .unwrap_or_default()
                        }))
                },
            )
            .estimated_item_size(crate::guide::list_rows::pitch(size, ROW_TWO))
            .h(move || px(crate::guide::list_rows::pitch(size, ROW_TWO)().0 * 2.0))
        },
    )
}
fn indirect(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || {
            state(ui, |s| {
                s.preview.as_ref().is_some_and(|p| !p.indirect.is_empty())
            })
            .unwrap_or(false)
        },
        move || {
            virtual_list(
                move || {
                    state(ui, |s| s.preview.as_ref().map_or(0, |p| p.indirect.len())).unwrap_or(0)
                },
                move |at| {
                    let item = move || {
                        state(ui, |s| {
                            s.preview
                                .as_ref()
                                .and_then(|p| p.indirect.get(at.index()).cloned())
                        })
                        .flatten()
                    };
                    list_rows::row(size, ROW_TWO, || Mark::of(false, false))
                        .test_id("guide-repair-indirect-row")
                        .child(ui_kit::label(size, move || {
                            item().map(|v| v.name).unwrap_or_default()
                        }))
                        .child(ui_kit::detail(size, move || {
                            item()
                                .map(|v| {
                                    format!(
                                        "{} → {}",
                                        v.before.as_ref().map(target).unwrap_or_default(),
                                        if v.availability_unknown {
                                            tr("Availability is not verified").into()
                                        } else {
                                            v.after.as_ref().map(target).unwrap_or_default()
                                        }
                                    )
                                })
                                .unwrap_or_default()
                        }))
                },
            )
            .estimated_item_size(crate::guide::list_rows::pitch(size, ROW_TWO))
            .h(move || px(crate::guide::list_rows::pitch(size, ROW_TWO)().0 * 2.0))
        },
    )
}
fn rows(ui: Ui, size: Scale) -> impl IntoElement {
    let list = element_ref();
    let focused =
        computed(move || state(ui, |s| s.rows().iter().position(|r| *r == s.focus)).flatten());
    effect(move || {
        if let Some(index) = focused.get()
            && ui.keys_shown()
        {
            list.scroll_to_index(index as u32, percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(
        move || state(ui, |s| s.rows().len()).unwrap_or(0),
        move |at| {
            let row = move || state(ui, |s| s.rows().get(at.index()).cloned()).flatten();
            let mark = move || {
                Mark::of(
                    ui.keys_shown()
                        && state(ui, |s| row().is_some_and(|r| s.focus == r)).unwrap_or(false),
                    false,
                )
            };
            list_rows::row(size, ROW_TWO, mark)
                .test_id("guide-repair-row")
                .role(move || {
                    if matches!(row(), Some(Row::Mapping(_))) {
                        AccessibilityRole::Checkbox
                    } else {
                        AccessibilityRole::Button
                    }
                })
                .accessibility_state(move || AccessibilityState {
                    checked: if let Some(Row::Mapping(key)) = row() {
                        Some(
                            if state(ui, |s| {
                                s.request
                                    .mappings
                                    .iter()
                                    .find(|r| r.channel == key)
                                    .is_some_and(|r| r.replace_explicit)
                            })
                            .unwrap_or(false)
                            {
                                Checked::True
                            } else {
                                Checked::False
                            },
                        )
                    } else {
                        None
                    },
                    ..Default::default()
                })
                .child(
                    ui_kit::label(size, move || {
                        row().map(|r| title(ui, &r)).unwrap_or_default()
                    })
                    .text_color(move || mark().text()),
                )
                .child(
                    ui_kit::detail(size, move || {
                        row().map(|r| detail(ui, &r)).unwrap_or_default()
                    })
                    .text_color(move || mark().secondary()),
                )
                .on_press(move || {
                    ui.touched();
                    if let Some(row) = row() {
                        activate(ui, row);
                    }
                })
        },
    )
    .estimated_item_size(move || size.px(ROW_TWO))
    .element_ref(&list)
    .flex_1()
}
pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            let screen = ui_kit::screen(ui, size)
                .test_id("guide-repair-screen")
                .accessibility_elements_hidden(move || {
                    crate::chrome::overlays::Overlay::GuideRepair.accessibility_covered(ui)
                })
                .child(ui_kit::header(
                    ui,
                    size,
                    move || tr("Repair guide data"),
                    close,
                ))
                .child(ui_kit::caption(size, move || {
                    state(ui, |s| {
                        format!(
                            "{} — {}",
                            crate::guide::clock::label(
                                s.request.window.from,
                                crate::guide::clock::wall_unix()
                            ),
                            crate::guide::clock::label(
                                s.request.window.to,
                                crate::guide::clock::wall_unix()
                            )
                        )
                    })
                    .unwrap_or_default()
                }))
                .child(ui_kit::body(size, move || {
                    tr("Program coverage does not verify channel identity")
                }));
            let screen = screen.child(when(
                move || state(ui, |s| s.busy).unwrap_or(false),
                move || ui_kit::body(size, move || tr("Loading…")),
            ));
            let screen = screen.child(when(
                move || state(ui, |s| s.error.is_some()).unwrap_or(false),
                move || {
                    ui_kit::body(size, move || {
                        state(ui, |s| s.error.as_ref().map(error))
                            .flatten()
                            .unwrap_or_default()
                    })
                    .text_color(rgb(ui_kit::ERROR))
                },
            ));
            let screen = screen.child(outcome(ui, size)).child(summary(ui, size));
            let screen = screen.child(when(
                move || {
                    state(ui, |s| {
                        s.preview.as_ref().is_some_and(|p| !p.indirect_complete)
                    })
                    .unwrap_or(false)
                },
                move || {
                    ui_kit::body(size, move || {
                        tr("Some guide inputs are unavailable; preview effects may be incomplete")
                    })
                },
            ));
            let screen = screen.child(feeds(ui, size)).child(indirect(ui, size));
            screen
                .child(when(
                    move || state(ui, |s| s.verification.is_some()).unwrap_or(false),
                    move || {
                        ui_kit::caption(size, move || {
                            state(ui, |s| {
                                s.verification.as_ref().map(|v| {
                                    format!(
                                        "{}: {} / {}",
                                        tr("Program coverage available"),
                                        v.programs_available,
                                        v.channels
                                    )
                                })
                            })
                            .flatten()
                            .unwrap_or_default()
                        })
                    },
                ))
                .child(rows(ui, size))
        },
    )
}

#[cfg(test)]
mod callback_regressions {
    use super::*;
    #[test]
    fn a_canceled_callback_cannot_finish_a_reopened_screen_at_the_same_generation() {
        let window = TimeWindow {
            from: 1_800_000_000,
            to: 1_800_003_600,
        };
        let mut old = Screen::new(SourceId(1), window);
        let generation = old.begin();
        let old_control = iptv_import::CancelToken::new();
        assert!(accepts(&old, generation, &old_control));
        old_control.cancel();
        let mut replacement = Screen::new(SourceId(2), window);
        assert_eq!(
            replacement.begin(),
            generation,
            "generation alone has an ABA collision"
        );
        assert!(!accepts(&replacement, generation, &old_control));
        finish_screen(
            &mut replacement,
            generation,
            &old_control,
            Err(model::Error::PlanMissing),
        );
        assert!(
            replacement.busy,
            "old completion must not finish the replacement operation"
        );
        assert!(replacement.error.is_none());
        let new_control = iptv_import::CancelToken::new();
        assert!(accepts(&replacement, generation, &new_control));
        replacement.close();
        assert!(!accepts(&replacement, generation, &new_control));
    }
}
