//! Existing guide-source workflow's read-only coverage/candidate inspection.
use crate::input::dismiss_keyboard;
use crate::runtime::inspection::{InspectionControl, ProbeRequest, ProbeTarget};
use crate::{
    Ui,
    i18n::tr,
    list_rows::{self, Mark, ROW_TWO},
    scale::Scale,
    ui_kit,
};
use iptv_library::{SourceId, inspection::*};
use iptv_nav::Action;
use iptv_presentation::library::guide_inspect::{Mode, Row, Screen};
use nsh_core::{element::input, prelude::*};
use std::time::Duration;

fn state<R>(ui: Ui, f: impl FnOnce(&Screen) -> R) -> Option<R> {
    ui.guide_inspect.with(|s| s.as_ref().map(f))
}
fn update(ui: Ui, f: impl FnOnce(&mut Screen)) {
    ui.guide_inspect.update(|s| {
        if let Some(s) = s {
            f(s);
        }
    });
}
fn cancel(ui: Ui) {
    ui.guide_inspect_cancel.with(|token| {
        if let Some(token) = token.take() {
            token.cancel();
        }
    });
}
fn begin(ui: Ui) -> Option<(Screen, u64, InspectionControl)> {
    cancel(ui);
    let control = InspectionControl {
        timeout: Duration::from_secs(60),
        ..Default::default()
    };
    let mut request = None;
    update(ui, |s| {
        let id = s.begin();
        request = Some((s.clone(), id, control.clone()));
    });
    ui.guide_inspect_cancel
        .with(|token| *token = Some(control.cancel.clone()));
    request
}
fn finish(ui: Ui, id: u64, result: InspectionResult<()>) {
    update(ui, |s| {
        if s.finish(id) {
            s.error = result.err();
            s.restore_focus();
        }
    });
}
fn current(ui: Ui, id: u64, control: &InspectionControl) -> bool {
    !control.cancel.is_canceled() && state(ui, |s| s.request == id).unwrap_or(false)
}

pub fn open(ui: Ui, source: SourceId) {
    let from = crate::time::wall_unix();
    let screen = Screen::new(
        source,
        TimeWindow {
            from,
            to: from + 24 * 3600,
        },
    );
    // Staged probes need blocking workers.
    let screen = if crate::host::HAS_WORKERS {
        screen
    } else {
        screen.without_probes()
    };
    ui.guide_inspect.set(Some(screen));
    audit(ui, false);
}
pub fn is_open(ui: Ui) -> bool {
    ui.guide_inspect.with(Option::is_some)
}
pub fn close(ui: Ui) {
    cancel(ui);
    ui.guide_inspect.set(None);
    dismiss_keyboard();
}
pub fn back(ui: Ui) -> bool {
    if state(ui, |s| s.typing).unwrap_or(false) {
        update(ui, |s| s.typing = false);
        dismiss_keyboard();
        return true;
    }
    cancel(ui);
    let mut keep = false;
    update(ui, |s| keep = s.back());
    if !keep {
        close(ui);
    }
    true
}
pub fn key(ui: Ui, action: Action) {
    match action {
        Action::MoveUp => update(ui, |s| s.move_focus(-1)),
        Action::MoveDown => update(ui, |s| s.move_focus(1)),
        Action::PageUp => update(ui, |s| s.move_focus(-8)),
        Action::PageDown => update(ui, |s| s.move_focus(8)),
        Action::Back | Action::ShowMenu => {
            back(ui);
        }
        Action::Select => {
            if let Some(row) = state(ui, |s| s.focus.clone()) {
                activate(ui, row);
            }
        }
        _ => {}
    }
}
fn audit(ui: Ui, next: bool) {
    let Some((screen, id, control)) = begin(ui) else {
        return;
    };
    crate::data::spawn_in_app(async move {
        let mut query = Query::source(screen.source);
        query.text = screen.query;
        if next {
            query.cursor = screen.next;
        }
        let result = crate::runtime()
            .epg_audit(query, screen.window, control.clone())
            .await;
        if !current(ui, id, &control) {
            return;
        }
        match result {
            Ok(page) => {
                update(ui, |s| {
                    s.mode = Mode::Audit;
                    s.audit = page.items;
                    s.next = page.next;
                });
                finish(ui, id, Ok(()));
            }
            Err(error) => finish(ui, id, Err(error)),
        }
    });
}
fn candidates(ui: Ui, subject: ChannelRef, next: bool) {
    let Some((screen, id, control)) = begin(ui) else {
        return;
    };
    crate::data::spawn_in_app(async move {
        let mut query = Query::source(screen.source);
        query.text = screen.query;
        if next {
            query.cursor = screen.next;
        }
        let result = crate::runtime()
            .epg_candidates(subject.clone(), query, control.clone())
            .await;
        if !current(ui, id, &control) {
            return;
        }
        match result {
            Ok(page) => {
                let mut query = Query::source(screen.source);
                query.limit = 5;
                let programs = crate::runtime()
                    .query_programs(query, screen.window, Some(subject.clone()), control.clone())
                    .await;
                if !current(ui, id, &control) {
                    return;
                }
                update(ui, |s| {
                    s.mode = Mode::Candidates;
                    s.subject = Some(subject);
                    s.candidates = page.items;
                    s.next = page.next;
                    s.current = programs
                        .as_ref()
                        .map(|p| p.items.clone())
                        .unwrap_or_default();
                });
                finish(ui, id, programs.map(|_| ()));
            }
            Err(error) => finish(ui, id, Err(error)),
        }
    });
}
fn inspect_link(ui: Ui) {
    update(ui, |s| s.typing = false);
    let Some((screen, id, control)) = begin(ui) else {
        return;
    };
    dismiss_keyboard();
    crate::data::spawn_in_app(async move {
        let request = ProbeRequest {
            source: screen.source,
            target: ProbeTarget::Supplied(iptv_providers::SecretUrl::new(screen.link)),
        };
        let result = crate::runtime().probe_feed(request, control.clone()).await;
        if !current(ui, id, &control) {
            return;
        }
        match result {
            Ok(stage) => {
                let page = crate::runtime()
                    .probe_candidates(
                        stage.evidence.clone(),
                        String::new(),
                        None,
                        50,
                        control.clone(),
                    )
                    .await;
                if !current(ui, id, &control) {
                    return;
                }
                match page {
                    Ok(page) => {
                        update(ui, |s| {
                            s.retained_stage = Some(stage.clone());
                            s.stage = Some(stage);
                            s.enter(Mode::ProbeCandidates);
                            s.candidates = page.items;
                            s.next = page.next;
                        });
                        finish(ui, id, Ok(()));
                    }
                    Err(error) => finish(ui, id, Err(error)),
                }
            }
            Err(error) => finish(ui, id, Err(error)),
        }
    });
}
fn staged_candidates(ui: Ui) {
    let Some((screen, id, control)) = begin(ui) else {
        return;
    };
    let Some(stage) = screen.stage else {
        return;
    };
    crate::data::spawn_in_app(async move {
        let after = screen.next;
        let result = crate::runtime()
            .probe_candidates(stage.evidence, screen.query, after, 50, control.clone())
            .await;
        if !current(ui, id, &control) {
            return;
        }
        match result {
            Ok(page) => {
                update(ui, |s| {
                    s.candidates = page.items;
                    s.next = page.next;
                });
                finish(ui, id, Ok(()));
            }
            Err(error) => finish(ui, id, Err(error)),
        }
    });
}
fn samples(ui: Ui, candidate: CandidateRef) {
    let Some((screen, id, control)) = begin(ui) else {
        return;
    };
    crate::data::spawn_in_app(async move {
        let stage = if let Some(stage) = screen.stage.filter(|s| {
            s.evidence.feed.unwrap_or(iptv_epg::FeedId(0)) == candidate.feed
                && s.evidence.incarnation == candidate.incarnation
        }) {
            Ok(stage)
        } else {
            crate::runtime()
                .probe_feed(
                    ProbeRequest {
                        source: screen.source,
                        target: ProbeTarget::Configured(candidate.feed),
                    },
                    control.clone(),
                )
                .await
        };
        if !current(ui, id, &control) {
            return;
        }
        let stage = match stage {
            Ok(stage) => stage,
            Err(error) => {
                finish(ui, id, Err(error));
                return;
            }
        };
        let selected = candidate.clone();
        let result = crate::runtime()
            .probe_schedule(
                stage.evidence.clone(),
                candidate,
                screen.window,
                control.clone(),
            )
            .await;
        if !current(ui, id, &control) {
            return;
        }
        match result {
            Ok(programs) => {
                update(ui, |s| {
                    s.retained_stage = Some(stage.clone());
                    s.stage = Some(stage);
                    s.samples_back = screen.mode;
                    s.enter(Mode::Samples);
                    s.samples = programs;
                    s.selected = Some(selected);
                    s.next = None;
                });
                finish(ui, id, Ok(()));
            }
            Err(error) => finish(ui, id, Err(error)),
        }
    });
}
fn activate(ui: Ui, row: Row) {
    if state(ui, |s| s.busy).unwrap_or(true) && row != Row::Back {
        return;
    }
    update(ui, |s| {
        s.focus = row.clone();
        if row != Row::Search {
            s.typing = false;
        }
    });
    if row != Row::Search {
        dismiss_keyboard();
    }
    match row {
        Row::Channel(subject) => {
            update(ui, |s| {
                s.enter(Mode::Candidates);
                s.query.clear();
                s.candidates.clear();
                s.subject = Some(subject.clone());
                s.stage = None;
            });
            candidates(ui, subject, false);
        }
        Row::Candidate(candidate) => samples(ui, candidate),
        Row::ReuseInspected => {
            update(ui, |s| {
                s.enter(Mode::ProbeCandidates);
                s.stage = s.retained_stage.clone();
                s.candidates.clear();
                s.query.clear();
                s.next = None;
            });
            staged_candidates(ui);
        }
        Row::UseCandidate => {
            if let Some((Some(channel), Some(candidate), stage, window)) = state(ui, |s| {
                (
                    s.subject.clone(),
                    s.selected.clone(),
                    s.stage.clone(),
                    s.window,
                )
            }) {
                let target = stage.map_or_else(
                    || iptv_library::guide_repair::Target::Configured(candidate.clone()),
                    |stage| iptv_library::guide_repair::Target::Staged {
                        evidence: stage.evidence,
                        epg_channel: candidate.epg_channel.clone(),
                        addition: None,
                    },
                );
                crate::guide_repair_view::add(ui, channel, target, window);
            }
        }
        Row::AutomaticRepair | Row::NoGuideRepair => {
            if let Some((Some(channel), window)) = state(ui, |s| (s.subject.clone(), s.window)) {
                let target = if row == Row::AutomaticRepair {
                    iptv_library::guide_repair::Target::Automatic
                } else {
                    iptv_library::guide_repair::Target::NoGuide
                };
                crate::guide_repair_view::add(ui, channel, target, window);
            }
        }
        Row::ReviewRepair => crate::guide_repair_view::reopen(ui),
        Row::RepairHistory => {
            if let Some((source, window)) = state(ui, |s| (s.source, s.window)) {
                crate::guide_repair_view::history(ui, source, window);
            }
        }
        Row::Search => update(ui, |s| s.typing = true),
        Row::InspectLink => inspect_link(ui),
        Row::NewLink => update(ui, |s| {
            s.enter(Mode::Link);
            s.query.clear();
            s.stage = None;
            s.typing = true;
            s.restore_focus();
        }),
        Row::Back => {
            back(ui);
        }
        Row::Next => match state(ui, |s| (s.mode, s.subject.clone())) {
            Some((Mode::Audit, _)) => audit(ui, true),
            Some((Mode::Candidates, Some(subject))) => candidates(ui, subject, true),
            Some((Mode::ProbeCandidates, _)) => staged_candidates(ui),
            _ => {}
        },
    }
}
fn diagnosis(value: Diagnosis) -> &'static str {
    match value {
        Diagnosis::InputFailure => tr("Guide source failed"),
        Diagnosis::Disabled => tr("No guide data"),
        Diagnosis::UnresolvedOverride => tr("Saved guide channel is unavailable"),
        Diagnosis::Unmatched => tr("No matching guide channel"),
        Diagnosis::MissingMetadata => tr("Missing guide identity metadata"),
        Diagnosis::Ambiguous => tr("Several guide channels match"),
        Diagnosis::Empty => tr("No programs in this interval"),
        Diagnosis::Stale => tr("Guide data is out of date"),
        Diagnosis::Partial => tr("Partial program coverage"),
        Diagnosis::SuspiciousTiming => tr("Check program times"),
        Diagnosis::Covered => tr("Program coverage available"),
    }
}
fn evidence(value: Evidence) -> &'static str {
    match value {
        Evidence::Manual => tr("Current"),
        Evidence::ExactId => tr("Exact guide id"),
        Evidence::CaseInsensitiveId => tr("Guide id differs in case"),
        Evidence::NormalizedName => tr("Similar channel name"),
        Evidence::Alternative => tr("Alternative guide channel"),
    }
}
pub(crate) fn error_text(error: InspectionError) -> &'static str {
    match error {
        InspectionError::Canceled => tr("Canceled"),
        InspectionError::Deadline => tr("The request timed out"),
        InspectionError::PermissionDenied => tr("Channel is blocked"),
        InspectionError::StaleCursor | InspectionError::StaleEvidence => {
            tr("Guide evidence changed; inspect again")
        }
        InspectionError::InvalidFeed => tr("The source is not a complete XMLTV guide"),
        InspectionError::LimitExceeded | InspectionError::Busy => {
            tr("Guide inspection limit reached")
        }
        _ => tr("Guide inspection failed"),
    }
}
fn title(ui: Ui, row: &Row) -> String {
    match row {
        Row::Channel(key) => state(ui, |s| {
            s.audit
                .iter()
                .find(|r| r.channel.key == *key)
                .map(|r| r.channel.name.clone())
        })
        .flatten()
        .unwrap_or_default(),
        Row::Candidate(key) => state(ui, |s| {
            s.candidates
                .iter()
                .find(|c| c.key == *key)
                .map(|c| c.name.clone().unwrap_or_else(|| key.epg_channel.clone()))
        })
        .flatten()
        .unwrap_or_default(),
        Row::UseCandidate => tr("Add this guide choice").into(),
        Row::AutomaticRepair => tr("Automatic").into(),
        Row::NoGuideRepair => tr("No guide data").into(),
        Row::ReviewRepair => tr("Review guide changes").into(),
        Row::RepairHistory => tr("Recent guide repairs").into(),
        Row::Search => tr("Search").to_owned(),
        Row::Next => tr("Next").to_owned(),
        Row::NewLink => tr("Inspect a guide link").to_owned(),
        Row::ReuseInspected => tr("Reuse inspected guide").into(),
        Row::InspectLink => tr("Inspect guide coverage").to_owned(),
        Row::Back => tr("Back").to_owned(),
    }
}
fn detail(ui: Ui, row: &Row) -> String {
    match row {
        Row::Channel(key) => state(ui, |s| {
            s.audit.iter().find(|r| r.channel.key == *key).map(|r| {
                format!(
                    "{} · {} / {}",
                    diagnosis(r.diagnosis),
                    r.covered_seconds,
                    s.window.to - s.window.from
                )
            })
        })
        .flatten()
        .unwrap_or_default(),
        Row::Candidate(key) => state(ui, |s| {
            s.candidates.iter().find(|c| c.key == *key).map(|c| {
                let evidence = c
                    .evidence
                    .iter()
                    .map(|e| evidence(*e))
                    .collect::<Vec<_>>()
                    .join(" · ");
                format!(
                    "{} · {} · {}",
                    key.epg_channel,
                    c.names.join(" / "),
                    evidence
                )
            })
        })
        .flatten()
        .unwrap_or_default(),
        _ => String::new(),
    }
}
pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let rows = element_ref();
    let focused =
        computed(move || state(ui, |s| s.rows().iter().position(|row| *row == s.focus)).flatten());
    effect(move || {
        if ui.keys_shown()
            && let Some(index) = focused.get()
        {
            rows.scroll_to_index(index as u32, percentage(0.5), px(0.0), false);
        }
    });
    let field = element_ref();
    crate::input::bind_field_focus(field, move || state(ui, |s| s.typing).unwrap_or(false));
    when(
        move || is_open(ui),
        move || {
            ui_kit::screen(ui, size)
                .test_id("guide-inspection-screen")
                .accessibility_elements_hidden(move || {
                    crate::overlays::Overlay::GuideInspection.accessibility_covered(ui)
                })
                .child(ui_kit::header(
                    ui,
                    size,
                    move || tr("Inspect guide coverage"),
                    close,
                ))
                .child(ui_kit::caption(size, move || {
                    state(ui, |s| {
                        format!(
                            "{} — {}",
                            crate::time::label(s.window.from, crate::time::wall_unix()),
                            crate::time::label(s.window.to, crate::time::wall_unix())
                        )
                    })
                    .unwrap_or_default()
                }))
                .child(ui_kit::body(size, move || {
                    tr("Program coverage does not verify channel identity")
                }))
                .child(when(
                    move || state(ui, |s| s.busy).unwrap_or(false),
                    move || ui_kit::body(size, move || tr("Loading…")),
                ))
                .child(when(
                    move || state(ui, |s| s.error.is_some()).unwrap_or(false),
                    move || {
                        ui_kit::body(size, move || {
                            state(ui, |s| s.error.clone().map(error_text))
                                .flatten()
                                .unwrap_or_default()
                        })
                        .text_color(rgb(ui_kit::ERROR))
                    },
                ))
                .child(when(
                    move || {
                        state(ui, |s| s.mode != Mode::Samples && s.mode != Mode::Link)
                            .unwrap_or(false)
                    },
                    move || {
                        ui_kit::field(size, input())
                            .test_id("guide-inspection-search")
                            .element_ref(&field)
                            .value(move || state(ui, |s| s.query.clone()).unwrap_or_default())
                            .placeholder(tr("Search"))
                            .auto_capitalize(AutoCapitalize::None)
                            .auto_correct(false)
                            .on_change(move |value: &str| {
                                update(ui, |s| s.query = value.to_owned())
                            })
                            .on_focus(move || update(ui, |s| s.typing = true))
                            .on_submit(move |_: &str| {
                                update(ui, |s| {
                                    s.typing = false;
                                    s.next = None;
                                });
                                dismiss_keyboard();
                                match state(ui, |s| (s.mode, s.subject.clone())) {
                                    Some((Mode::Audit, _)) => audit(ui, false),
                                    Some((Mode::Candidates, Some(subject))) => {
                                        candidates(ui, subject, false)
                                    }
                                    Some((Mode::ProbeCandidates, _)) => staged_candidates(ui),
                                    _ => {}
                                }
                            })
                    },
                ))
                .child(when(
                    move || {
                        state(ui, |s| s.mode == Mode::Candidates && !s.current.is_empty())
                            .unwrap_or(false)
                    },
                    move || {
                        ui_kit::caption(size, move || {
                            state(ui, |s| {
                                format!(
                                    "{}: {}",
                                    tr("Current"),
                                    s.current
                                        .iter()
                                        .map(|p| p.title.as_str())
                                        .collect::<Vec<_>>()
                                        .join(" · ")
                                )
                            })
                            .unwrap_or_default()
                        })
                    },
                ))
                .child(when(
                    move || {
                        state(ui, |s| s.mode == Mode::Samples && s.samples.is_empty())
                            .unwrap_or(false)
                    },
                    move || ui_kit::body(size, move || tr("No programs in this interval")),
                ))
                .child(when(
                    move || state(ui, |s| s.mode == Mode::Link).unwrap_or(false),
                    move || {
                        ui_kit::field(size, input())
                            .test_id("guide-inspection-link")
                            .element_ref(&field)
                            .value(move || state(ui, |s| s.link.clone()).unwrap_or_default())
                            .placeholder(tr("An XMLTV link (.xml or .xml.gz)"))
                            .auto_capitalize(AutoCapitalize::None)
                            .auto_correct(false)
                            .on_change(move |value: &str| update(ui, |s| s.link = value.to_owned()))
                            .on_submit(move |_: &str| inspect_link(ui))
                    },
                ))
                .child(when(
                    move || state(ui, |s| s.mode == Mode::Samples).unwrap_or(false),
                    move || {
                        virtual_list(
                            move || state(ui, |s| s.samples.len()).unwrap_or(0),
                            move |at| {
                                ui_kit::body(size, move || {
                                    state(ui, |s| {
                                        s.samples.get(at.index()).map(|p| {
                                            format!(
                                                "{} — {} · {}",
                                                crate::time::label(
                                                    p.start,
                                                    crate::time::wall_unix()
                                                ),
                                                crate::time::label(
                                                    p.stop,
                                                    crate::time::wall_unix()
                                                ),
                                                p.title
                                            )
                                        })
                                    })
                                    .flatten()
                                    .unwrap_or_default()
                                })
                            },
                        )
                        .estimated_item_size(move || size.px(ROW_TWO))
                        .flex_1()
                    },
                ))
                .child(
                    virtual_list(
                        move || state(ui, |s| s.rows().len()).unwrap_or(0),
                        move |at| {
                            let row =
                                move || state(ui, |s| s.rows().get(at.index()).cloned()).flatten();
                            let mark = move || {
                                Mark::of(
                                    state(ui, |s| row().is_some_and(|r| s.focus == r))
                                        .unwrap_or(false)
                                        && ui.keys_shown(),
                                    false,
                                )
                            };
                            list_rows::row(size, ROW_TWO, mark)
                                .role(AccessibilityRole::Button)
                                .test_id("guide-inspection-row")
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
                    .element_ref(&rows)
                    .estimated_item_size(move || size.px(ROW_TWO))
                    .flex_1(),
                )
        },
    )
}
