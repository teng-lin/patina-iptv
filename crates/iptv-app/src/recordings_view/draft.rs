//! Custom scheduling and exact field entry. Editing state stays in core.
use super::*;
use crate::input::dismiss_keyboard;
use crate::wizard_view::entry::{self, Sheet};
use iptv_core::recording::{Entry, EntryField};
use std::rc::Rc;

fn local(unix: i64) -> i64 {
    unix + i64::from(crate::time::offset_minutes(unix)) * 60
}

fn utc(local: i64, reference: i64) -> Option<i64> {
    let mut candidate =
        local.checked_sub(i64::from(crate::time::offset_minutes(reference)) * 60)?;
    for _ in 0..3 {
        candidate = local.checked_sub(i64::from(crate::time::offset_minutes(candidate)) * 60)?;
    }
    // A skipped local time at a DST transition must not become another hour.
    (self::local(candidate) == local).then_some(candidate)
}

fn hours(minutes: i64) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

fn adjust(d: &mut Draft, row: usize, step: i64) -> Result<(), &'static str> {
    let now = crate::time::wall_unix();
    if row == 0 {
        let reference = d.request.start;
        d.shift_local_day(step, now, local(reference), |time| utc(time, reference))
    } else {
        d.adjust(row, step, now);
        Ok(())
    }
}

fn begin(ui: Ui, row: usize) {
    ui.recordings.update(|s| {
        if s.busy {
            return;
        }
        let Some(d) = &mut s.draft else {
            return;
        };
        d.focus = row;
        if row == Draft::OPTIONS {
            d.options_open = !d.options_open;
            return;
        }
        let (field, text) = match row {
            0 => (EntryField::Day, local(d.request.start).to_string()),
            1 => {
                let minutes = local(d.request.start).rem_euclid(86400) / 60;
                (
                    EntryField::Time,
                    format!("{:02}:{:02}", minutes / 60, minutes % 60),
                )
            }
            2 => (
                EntryField::Duration,
                hours((d.request.end - d.request.start) / 60),
            ),
            3 => (EntryField::Name, d.request.title.clone()),
            5 => (EntryField::LeadMinutes, d.lead_minutes.to_string()),
            6 => (EntryField::TrailMinutes, d.trail_minutes.to_string()),
            _ => {
                let _ = adjust(d, row, 1);
                return;
            }
        };
        d.entry = Some(Entry {
            field,
            text,
            error: None,
        });
    });
}

pub(super) fn cancel_entry(ui: Ui) -> bool {
    let mut canceled = false;
    ui.recordings.update(|s| {
        if let Some(d) = &mut s.draft {
            canceled = d.entry.take().is_some();
        }
    });
    if canceled {
        dismiss_keyboard();
    }
    canceled
}

fn commit(ui: Ui) {
    let mut committed = false;
    ui.recordings.update(|s| {
        let Some(d) = &mut s.draft else {
            return;
        };
        let reference = d.request.start;
        match d.commit_entry(crate::time::wall_unix(), local(reference), |time| {
            utc(time, reference)
        }) {
            Ok(()) => committed = true,
            Err(error) => {
                if let Some(entry) = &mut d.entry {
                    entry.error = Some(error);
                }
            }
        }
    });
    if committed {
        dismiss_keyboard();
    }
}

pub(super) fn key(ui: Ui, action: Action) -> bool {
    let Some(d) = ui.recordings.with_untracked(|s| s.draft.clone()) else {
        return false;
    };
    if let Some(entry) = &d.entry {
        if entry.field == EntryField::Day && matches!(action, Action::MoveUp | Action::MoveDown) {
            ui.recordings.update(|s| {
                if let Some(entry) = s.draft.as_mut().and_then(|d| d.entry.as_mut()) {
                    let step = if action == Action::MoveUp {
                        -86400
                    } else {
                        86400
                    };
                    if let Ok(time) = entry.text.parse::<i64>() {
                        let minimum = local(crate::time::wall_unix()).div_euclid(86400) * 86400;
                        let time = time.saturating_add(step);
                        let first =
                            (local(d.request.start).div_euclid(86400) * 86400 - 86400).max(minimum);
                        if time >= first && time < first + 32 * 86400 {
                            entry.text = time.to_string();
                        }
                    }
                }
            });
        } else if action == Action::Select {
            commit(ui);
        }
        return true;
    }
    if action == Action::Select {
        if d.focus == Draft::SAVE {
            super::save(ui);
        } else {
            begin(ui, d.focus);
        }
    } else {
        ui.recordings.update(|s| {
            if let Some(d) = &mut s.draft {
                match action {
                    Action::MoveUp => d.move_field(-1),
                    Action::MoveDown => d.move_field(1),
                    Action::MoveLeft => s.error = adjust(d, d.focus, -1).err().map(str::to_owned),
                    Action::MoveRight => s.error = adjust(d, d.focus, 1).err().map(str::to_owned),
                    _ => {}
                }
            }
        });
    }
    true
}

fn row_value(d: &Draft, row: usize) -> String {
    match row {
        0 => trf(
            "Start day · {time}",
            &[(
                "time",
                crate::time::date_only(d.request.start).as_str().into(),
            )],
        ),
        1 => trf(
            "Start time · {time}",
            &[(
                "time",
                crate::time::time_only(d.request.start).as_str().into(),
            )],
        ),
        2 => trf(
            "Duration · {duration}",
            &[(
                "duration",
                hours((d.request.end - d.request.start) / 60)
                    .as_str()
                    .into(),
            )],
        ),
        3 => format!("{} · {}", tr("Name"), d.request.title),
        4 => trf(
            "Repeat · {frequency}",
            &[(
                "frequency",
                match d.repeat {
                    iptv_core::recording::Repeat::Once => tr("Once"),
                    iptv_core::recording::Repeat::Daily => tr("Every day"),
                    iptv_core::recording::Repeat::Weekly => tr("Every week"),
                    iptv_core::recording::Repeat::Series => tr("Every matching program"),
                }
                .into(),
            )],
        ),
        5 => trf(
            "Start early · {minutes} min",
            &[("minutes", (d.lead_minutes as u64).into())],
        ),
        _ => trf(
            "Finish late · {minutes} min",
            &[("minutes", (d.trail_minutes as u64).into())],
        ),
    }
}

fn row(ui: Ui, size: Scale, row: usize) -> View {
    let value = move || {
        ui.recordings.with(|s| {
            s.draft
                .as_ref()
                .map(|d| row_value(d, row))
                .unwrap_or_default()
        })
    };
    let focused = move || {
        ui.recordings
            .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == row))
    };
    super::style::row(size, 72.0, focused)
        .label(value)
        .on_press(move || {
            ui.touched();
            begin(ui, row);
        })
        .child(
            ui_kit::body(size, move || {
                value()
                    .split_once(" · ")
                    .map(|(label, _)| label.to_owned())
                    .unwrap_or_default()
            })
            .text_color(rgb(crate::theme::LIST_TEXT)),
        )
        .child(
            ui_kit::caption(size, move || {
                value()
                    .split_once(" · ")
                    .map(|(_, value)| value.to_owned())
                    .unwrap_or_default()
            })
            .number_of_lines(1),
        )
}

pub(super) fn draft_view(ui: Ui, size: Scale, scrolling: ui_kit::FocusScroll) -> View {
    view()
        .child(
            ui_kit::heading(size, move || {
                ui.recordings.with(|s| {
                    s.draft
                        .as_ref()
                        .map(|d| d.request.channel_name.clone())
                        .unwrap_or_default()
                })
            })
            .px(move || size.px(24.0))
            .pb(move || size.px(16.0)),
        )
        .children((0..4).map(move |at| {
            scrolling.control(row(ui, size, at), move || {
                ui.recordings
                    .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == at))
            })
        }))
        .child(
            scrolling.control(
                super::style::action(
                    ui,
                    size,
                    tr("Recording options"),
                    move || {
                        ui.recordings
                            .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == Draft::OPTIONS))
                    },
                    move || begin(ui, Draft::OPTIONS),
                )
                .label(tr("Recording options"))
                .test_id("recording-options"),
                move || {
                    ui.recordings
                        .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == Draft::OPTIONS))
                },
            ),
        )
        .child(when(
            move || {
                ui.recordings
                    .with(|s| s.draft.as_ref().is_some_and(|d| d.options_open))
            },
            move || {
                view().children((4..7).map(move |at| {
                    scrolling.control(row(ui, size, at), move || {
                        ui.recordings
                            .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == at))
                    })
                }))
            },
        ))
        .child(
            scrolling.control(
                super::style::action(
                    ui,
                    size,
                    tr("Save recording"),
                    move || {
                        ui.recordings
                            .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == Draft::SAVE))
                    },
                    move || super::save(ui),
                )
                .disabled(move || ui.recordings.with(|s| s.busy))
                .test_id("recording-save"),
                move || {
                    ui.recordings
                        .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == Draft::SAVE))
                },
            ),
        )
}

pub(super) fn entry_layer(ui: Ui) -> impl IntoElement {
    when(
        move || {
            ui.recordings
                .with(|s| s.draft.as_ref().is_some_and(|d| d.entry.is_some()))
        },
        move || {
            let field = ui.recordings.with_untracked(|s| {
                s.draft
                    .as_ref()
                    .and_then(|d| d.entry.as_ref())
                    .unwrap()
                    .field
            });
            if field == EntryField::Day {
                return day_picker(ui);
            }
            let label = ui.recordings.with_untracked(|s| {
                let d = s.draft.as_ref().unwrap();
                row_value(d, d.focus)
                    .split_once(" · ")
                    .map(|(label, _)| label.to_owned())
                    .unwrap_or_default()
            });
            entry::panel(
                ui,
                Sheet {
                    label,
                    placeholder: if matches!(field, EntryField::Time | EntryField::Duration) {
                        tr("HH:MM")
                    } else {
                        tr("Enter text")
                    },
                    keyboard: KeyboardType::Default,
                    masked: false,
                    current: Rc::new(move || {
                        ui.recordings.with(|s| {
                            s.draft
                                .as_ref()
                                .and_then(|d| d.entry.as_ref())
                                .map(|e| e.text.clone())
                                .unwrap_or_default()
                        })
                    }),
                    note: Rc::new(move || {
                        ui.recordings.with(|s| {
                            s.draft
                                .as_ref()
                                .and_then(|d| d.entry.as_ref())
                                .and_then(|e| e.error)
                                .map(super::translated_reason)
                                .unwrap_or("")
                                .to_owned()
                        })
                    }),
                    set: Rc::new(move |text| {
                        ui.recordings.update(|s| {
                            if let Some(e) = s.draft.as_mut().and_then(|d| d.entry.as_mut()) {
                                e.text = text.into();
                                e.error = None;
                            }
                        })
                    }),
                    done: commit,
                    cancel: |ui| {
                        cancel_entry(ui);
                    },
                },
            )
            .test_id("recording-entry")
        },
    )
}

fn day_picker(ui: Ui) -> View {
    let size = Scale::new();
    let reference = ui
        .recordings
        .with_untracked(|s| s.draft.as_ref().unwrap().request.start);
    let local_start = local(reference);
    let minimum = local(crate::time::wall_unix()).div_euclid(86400) * 86400;
    let first = (local_start.div_euclid(86400) * 86400 - 86400).max(minimum);
    let selected = move || {
        ui.recordings.with(|s| {
            s.draft
                .as_ref()
                .and_then(|d| d.entry.as_ref())
                .and_then(|e| e.text.parse::<i64>().ok())
                .unwrap_or(local_start)
        })
    };
    let rows = element_ref();
    effect(move || {
        rows.scroll_to_index(
            ((selected().div_euclid(86400) * 86400 - first) / 86400).max(0) as u32,
            percentage(0.5),
            px(0.0),
            false,
        )
    });
    let (outer, card) = entry::frame(ui, size);
    outer.child(
        card.max_h(percentage(0.85))
            .child(ui_kit::title(size, tr("Start day")))
            .child(
                virtual_list(
                    || 32usize,
                    move |at| {
                        let time =
                            first + at.index() as i64 * 86400 + local_start.rem_euclid(86400);
                        ui_kit::button(
                            ui,
                            size,
                            utc(time, reference)
                                .map(crate::time::date_only)
                                .unwrap_or_default(),
                            move || selected().div_euclid(86400) == time.div_euclid(86400),
                            move || {
                                ui.recordings.update(|s| {
                                    if let Some(e) = s.draft.as_mut().and_then(|d| d.entry.as_mut())
                                    {
                                        e.text = time.to_string();
                                    }
                                });
                                commit(ui);
                            },
                        )
                        .test_id("recording-day-option")
                    },
                )
                .element_ref(&rows)
                .estimated_item_size(move || size.px(ui_kit::TOUCH))
                .h(move || size.px(360.0)),
            )
            .child(ui_kit::body(size, move || {
                ui.recordings.with(|s| {
                    s.draft
                        .as_ref()
                        .and_then(|d| d.entry.as_ref())
                        .and_then(|e| e.error)
                        .map(super::translated_reason)
                        .unwrap_or("")
                        .to_owned()
                })
            }))
            .child(ui_kit::button(
                ui,
                size,
                tr("Cancel"),
                || false,
                move || {
                    cancel_entry(ui);
                },
            )),
    )
}
