//! One recording workflow for remote, touch and pointer input. State is portable;
//! the shell renders it and sends named runtime commands.
use crate::i18n::trf;
use crate::{Ui, i18n::tr, scale::Scale, ui_kit};
use iptv_core::menu::Origin;
use iptv_core::recording::{Draft, Filter, Job, JobId, Request, Status};
use iptv_nav::Action;
use nsh_core::prelude::*;
use std::time::Duration;

pub fn is_open(ui: Ui) -> bool {
    ui.recordings_open.get()
}
pub fn open(ui: Ui) {
    ui.recordings.update(|s| {
        s.open = true;
        s.error = None;
        s.actions = false;
        s.confirm_delete = false;
        s.draft = None;
    });
    reload(ui);
}
pub fn close(ui: Ui) {
    ui.recordings.update(|s| {
        if !s.busy {
            s.open = false;
            s.confirm_delete = false;
        }
    });
}
pub fn back(ui: Ui) -> bool {
    ui.recordings.update(|s| {
        if s.busy {
            return;
        }
        s.error = None;
        if s.confirm_delete {
            s.confirm_delete = false;
        } else if s.draft.take().is_some() {
        } else if s.actions {
            s.actions = false;
        } else {
            s.open = false;
        }
    });
    true
}
fn reload(ui: Ui) {
    let owner = crate::runtime();
    let mut generation = 0;
    ui.recordings.update(|s| {
        s.read_generation += 1;
        generation = s.read_generation;
    });
    crate::data::spawn_in_app(async move {
        let result = owner.recording_jobs().await;
        let points = owner.recording_progress_points().await.unwrap_or_default();
        let repeating = owner.repeating_recording_jobs().await.unwrap_or_default();
        let background_notice = owner.recording_wake_notice().map(str::to_owned);
        ui.recordings.update(|s| {
            if s.read_generation != generation {
                return;
            }
            s.repeating = repeating;
            s.background_notice = background_notice;
            match result {
                Ok(jobs) => {
                    s.progress = points;
                    s.reload(jobs);
                }
                Err(_) => s.error = Some("Recordings could not be loaded".into()),
            }
        });
    });
}
pub fn bind(ui: Ui) {
    crate::runtime().start_recordings();
    crate::data::spawn_in_app(async move {
        loop {
            timer(Duration::from_secs(1)).await;
            crate::runtime().sync_recording_contexts();
            reload(ui);
        }
    });
}

pub fn available(channel: usize) -> bool {
    crate::schedule()
        .channel_key(channel)
        .and_then(|key| crate::data::combined::owner(&crate::runtime(), &key))
        .is_some()
}
pub fn try_reserve_viewing(surface: usize, channel: usize) -> bool {
    let source = crate::schedule()
        .channel_key(channel)
        .and_then(|key| crate::data::combined::owner(&crate::runtime(), &key));
    source.is_none_or(|(source, _)| crate::runtime().reserve_viewing(surface, source))
}
pub fn reserve_viewing(_ui: Ui, surface: usize, channel: usize) -> bool {
    if !try_reserve_viewing(surface, channel) {
        crate::settings_view::notify(tr(
            "No provider connection available while recording; stop a recording or check Streams at once",
        ));
        return false;
    }
    true
}
pub fn badge(ui: Ui, channel: usize, programme: crate::guide::Program) -> &'static str {
    if ui.recordings.with(|s| s.jobs.is_empty()) {
        return "";
    }
    let schedule = crate::schedule();
    let Some((source, key)) = schedule
        .channel_key(channel)
        .and_then(|key| crate::data::combined::owner(&crate::runtime(), &key))
    else {
        return "";
    };
    let (start, end) = (
        crate::guide::unix_of(&*schedule, programme.start),
        crate::guide::unix_of(&*schedule, programme.end),
    );
    ui.recordings.with(|s| {
        s.jobs
            .iter()
            .find(|j| {
                j.status.active()
                    && j.source_current
                    && j.request.source == source.0
                    && j.request.channel == key
                    && j.request.start < end
                    && j.request.end > start
            })
            .map_or("", |j| {
                if j.status == Status::Recording && j.bytes > 0 {
                    "● "
                } else {
                    "◷ "
                }
            })
    })
}
pub fn current(ui: Ui) {
    let fullscreen = ui.mode.get_untracked() == crate::ViewMode::Fullscreen;
    let channel = if fullscreen {
        ui.playing.get_untracked()
    } else {
        crate::lists::focused_channel(ui)
    };
    let Some(channel) = channel else {
        open(ui);
        return;
    };
    let origin = Origin {
        channel,
        program_start: if fullscreen {
            crate::time::now_minute()
        } else {
            ui.focus.with_untracked(|f| f.cursor)
        },
    };
    remember(ui, origin);
    programme(ui, origin, false);
}
fn request(origin: Origin) -> Option<Request> {
    let schedule = crate::schedule();
    let (source, channel) = schedule
        .channel_key(origin.channel)
        .and_then(|key| crate::data::combined::owner(&crate::runtime(), &key))?;
    let programme = schedule.program_at(origin.channel, origin.program_start);
    Some(Request {
        source: source.0,
        channel,
        channel_name: crate::channel_name(origin.channel),
        title: schedule.title(origin.channel, programme),
        start: crate::guide::unix_of(&*schedule, programme.start).max(crate::time::wall_unix()),
        end: crate::guide::unix_of(&*schedule, programme.end),
    })
}
pub fn remember(ui: Ui, origin: Origin) {
    let context = request(origin).map(|request| (origin, request));
    ui.recordings.update(|s| s.context = context);
}
pub fn programme(ui: Ui, origin: Origin, custom: bool) {
    let request = ui
        .recordings
        .with_untracked(|s| {
            s.context
                .as_ref()
                .filter(|(saved, _)| *saved == origin)
                .map(|(_, request)| request.clone())
        })
        .or_else(|| request(origin));
    let Some(mut request) = request else {
        return;
    };
    if custom {
        request.start = crate::time::wall_unix();
        request.end = request.start + 3600;
    }
    ui.menu.set(None);
    ui.description.set(None);
    ui.recordings.update(|s| {
        s.open = true;
        s.error = None;
        s.actions = false;
        s.confirm_delete = false;
        let existing = s.jobs.iter().find(|j| {
            j.status.active()
                && j.source_current
                && j.request.source == request.source
                && j.request.channel == request.channel
                && j.request.start < request.end
                && j.request.end > request.start
        });
        if !custom && let Some(job) = existing {
            s.focus = Some(job.id);
            s.actions = true;
            s.action = 0;
            s.draft = None;
        } else {
            s.draft = Some(Draft::new(request));
        }
    });
}
fn save(ui: Ui) {
    let draft = ui
        .recordings
        .with_untracked(|s| if s.busy { None } else { s.draft.clone() });
    let Some(draft) = draft else {
        return;
    };
    let target =
        iptv_core::parental::Target::live(draft.request.source, draft.request.channel.clone());
    crate::parental_view::request(ui, target, move || save_allowed(ui, draft));
}
fn save_allowed(ui: Ui, draft: Draft) {
    let request = draft.padded(crate::time::wall_unix());
    if let Err(message) = request.validate(crate::time::wall_unix()) {
        ui.recordings.update(|s| s.error = Some(message.into()));
        return;
    }
    ui.recordings.update(|s| {
        s.busy = true;
        s.error = None;
    });
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let result = match draft.editing {
            Some(id) => owner.update_recording(id, request).await.map(|()| id),
            None => {
                owner
                    .schedule_repeating_recording(request, draft.repeat)
                    .await
            }
        };
        ui.recordings.update(|s| {
            s.busy = false;
            match result {
                Ok(id) => {
                    s.draft = None;
                    s.filter = Filter::All;
                    s.focus = Some(id);
                    s.actions = false;
                }
                Err(error) => {
                    s.error = Some(
                        error
                            .unavailable_reason()
                            .filter(|reason| iptv_core::i18n::is_catalogued(reason))
                            .unwrap_or("Recording action failed")
                            .into(),
                    )
                }
            }
        });
        reload(ui);
    });
}
fn select_job(ui: Ui, id: JobId) {
    ui.recordings.update(|s| {
        s.focus = Some(id);
        s.actions = true;
        s.confirm_delete = false;
        s.action = 0;
        s.error = None;
    });
}
fn actions(ui: Ui, job: &Job) -> Vec<&'static str> {
    let mut actions = match job.status {
        Status::Scheduled => vec!["Edit recording", "Cancel recording"],
        Status::Recording => vec!["Stop recording"],
        Status::Completed | Status::Partial if job.bytes > 0 => {
            vec![
                if ui
                    .player
                    .recorded
                    .with(|r| r.as_ref().is_some_and(|(playing, _)| playing.id == job.id))
                {
                    "Stop playback"
                } else {
                    if ui.recordings.with(|s| {
                        s.progress
                            .iter()
                            .any(|(id, p)| *id == job.id && p.resume_ms() > 0)
                    }) {
                        "Resume recording"
                    } else {
                        "Play recording"
                    }
                },
                "Play from beginning",
                "Delete recording",
            ]
        }
        _ => vec!["Delete recording"],
    };
    if ui.recordings.with(|s| s.repeating.contains(&job.id)) {
        actions.push("Cancel repeating schedule");
    }
    actions
}
fn perform(ui: Ui, index: usize) {
    let (job, confirm) = ui
        .recordings
        .with_untracked(|s| (s.focused().cloned(), s.confirm_delete));
    let Some(job) = job else {
        return;
    };
    let Some(action) = actions(ui, &job).get(index).copied() else {
        return;
    };
    if matches!(
        action,
        "Play recording" | "Resume recording" | "Play from beginning"
    ) {
        let target =
            iptv_core::parental::Target::live(job.request.source, job.request.channel.clone());
        crate::parental_view::request(ui, target, move || execute(ui, job, confirm, action, index));
    } else {
        execute(ui, job, confirm, action, index);
    }
}
fn execute(ui: Ui, job: Job, confirm: bool, action: &'static str, index: usize) {
    if action == "Stop playback" {
        crate::player_keys::stop_playback(ui);
        return;
    }
    if action == "Edit recording" {
        ui.recordings.update(|s| {
            let mut d = Draft::new(job.request);
            d.editing = Some(job.id);
            s.draft = Some(d);
            s.actions = false;
        });
        return;
    }
    if action == "Delete recording" && !confirm {
        ui.recordings.update(|s| {
            s.confirm_delete = true;
            s.action = index;
        });
        return;
    }
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    ui.recordings.update(|s| {
        s.busy = true;
        s.error = None;
    });
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let result = if matches!(
            action,
            "Play recording" | "Resume recording" | "Play from beginning"
        ) {
            crate::on_demand::flush(ui, false);
            let progress = owner.recording_progress(job.id).await.unwrap_or_default();
            match owner.open_recording(job.id).await {
                Ok(source) => {
                    crate::player::play_recording(ui, job.clone(), source);
                    ui.player
                        .main()
                        .resume_ms
                        .set(Some(if action == "Play from beginning" {
                            0
                        } else {
                            progress.resume_ms()
                        }));
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else if action == "Cancel repeating schedule" {
            owner.stop_repeating_recording(job.id).await
        } else if action == "Delete recording" {
            owner.delete_recording(job.id).await
        } else {
            owner.stop_recording(job.id).await
        };
        ui.recordings.update(|s| {
            s.busy = false;
            s.confirm_delete = false;
            match result {
                Ok(()) => {
                    s.actions = false;
                    if matches!(
                        action,
                        "Play recording" | "Resume recording" | "Play from beginning"
                    ) {
                        s.open = false;
                    }
                }
                Err(_) => {
                    s.error = Some(
                        if action == "Delete recording" {
                            "Recording could not be deleted; stop playback first"
                        } else {
                            "Recording action failed; check whether its file is available"
                        }
                        .into(),
                    )
                }
            }
        });
        reload(ui);
    });
}
pub fn key(ui: Ui, action: Action) {
    if action == Action::Back {
        back(ui);
        return;
    }
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    if ui.recordings.with_untracked(|s| s.draft.is_some()) {
        if action == Action::Select {
            let row = ui
                .recordings
                .with_untracked(|s| s.draft.as_ref().map_or(6, |d| d.focus));
            if row == 6 {
                save(ui);
            } else {
                ui.recordings.update(|s| {
                    if let Some(d) = &mut s.draft {
                        d.adjust(row, 1, crate::time::wall_unix());
                    }
                });
            }
            return;
        }
        ui.recordings.update(|s| {
            if let Some(d) = &mut s.draft {
                match action {
                    Action::MoveUp => d.focus = d.focus.saturating_sub(1),
                    Action::MoveDown => d.focus = (d.focus + 1).min(6),
                    Action::MoveLeft => d.adjust(d.focus, -1, crate::time::wall_unix()),
                    Action::MoveRight => d.adjust(d.focus, 1, crate::time::wall_unix()),
                    _ => {}
                }
            }
        });
    } else if ui.recordings.with_untracked(|s| s.actions) {
        let length = ui
            .recordings
            .with_untracked(|s| s.focused().map(|j| actions(ui, j)).map_or(0, |a| a.len()));
        match action {
            Action::MoveUp => ui
                .recordings
                .update(|s| s.action = s.action.saturating_sub(1)),
            Action::MoveDown => ui
                .recordings
                .update(|s| s.action = (s.action + 1).min(length.saturating_sub(1))),
            Action::Select => perform(ui, ui.recordings.with_untracked(|s| s.action)),
            _ => {}
        }
    } else {
        match action {
            Action::MoveUp => ui.recordings.update(|s| s.move_focus(-1)),
            Action::MoveDown => ui.recordings.update(|s| s.move_focus(1)),
            Action::MoveLeft => ui.recordings.update(|s| s.set_filter(Filter::Active)),
            Action::MoveRight => ui.recordings.update(|s| s.set_filter(Filter::Finished)),
            Action::ShowMenu => ui.recordings.update(|s| s.set_filter(Filter::All)),
            Action::Select => {
                if let Some(id) = ui.recordings.with_untracked(|s| s.focus) {
                    select_job(ui, id);
                }
            }
            _ => {}
        }
    }
}
fn date(unix: i64) -> String {
    crate::time::label(unix, crate::time::wall_unix())
}
fn draft_row(ui: Ui, size: Scale, row: usize) -> impl IntoElement {
    let value = move || {
        ui.recordings.with(|s| {
            s.draft
                .as_ref()
                .map(|d| match row {
                    0 => trf(
                        "Start day · {time}",
                        &[("time", (&date(d.request.start)).into())],
                    ),
                    1 => trf(
                        "Start time · {time}",
                        &[("time", (&date(d.request.start)).into())],
                    ),
                    2 => trf(
                        "Duration · {minutes} min",
                        &[(
                            "minutes",
                            (((d.request.end - d.request.start) / 60) as u64).into(),
                        )],
                    ),
                    3 => trf(
                        "Start early · {minutes} min",
                        &[("minutes", (d.lead_minutes as u64).into())],
                    ),
                    4 => trf(
                        "Finish late · {minutes} min",
                        &[("minutes", (d.trail_minutes as u64).into())],
                    ),
                    _ => trf(
                        "Repeat · {frequency}",
                        &[(
                            "frequency",
                            match d.repeat {
                                iptv_core::recording::Repeat::Once => tr("Once"),
                                iptv_core::recording::Repeat::Daily => tr("Every day"),
                                iptv_core::recording::Repeat::Weekly => tr("Every week"),
                                iptv_core::recording::Repeat::Series => {
                                    tr("Every matching programme")
                                }
                            }
                            .into(),
                        )],
                    ),
                })
                .unwrap_or_default()
        })
    };
    view()
        .flex_row()
        .items_center()
        .gap(move || size.px(12.0))
        .child(ui_kit::label(size, value).flex_1())
        .children([-1, 1].into_iter().map(move |step| {
            ui_kit::button(
                ui,
                size,
                if step < 0 { "−" } else { "+" },
                move || {
                    ui.recordings
                        .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == row))
                },
                move || {
                    ui.touched();
                    ui.recordings.update(|s| {
                        if !s.busy
                            && let Some(d) = &mut s.draft
                        {
                            d.focus = row;
                            d.adjust(row, step, crate::time::wall_unix());
                        }
                    });
                },
            )
            .label(move || {
                if step < 0 {
                    trf("Decrease {field}", &[("field", (&value()).into())])
                } else {
                    trf("Increase {field}", &[("field", (&value()).into())])
                }
            })
        }))
}
fn draft_view(ui: Ui, size: Scale) -> impl IntoElement {
    view()
        .gap(move || size.px(12.0))
        .child(ui_kit::heading(size, move || {
            ui.recordings.with(|s| {
                s.draft
                    .as_ref()
                    .map(|d| d.request.title.clone())
                    .unwrap_or_default()
            })
        }))
        .children((0..6).map(move |row| draft_row(ui, size, row)))
        .child(ui_kit::button(
            ui,
            size,
            tr("Save recording"),
            move || {
                ui.recordings
                    .with(|s| s.draft.as_ref().is_some_and(|d| d.focus == 6))
            },
            move || save(ui),
        ))
}
fn job_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let job = move || ui.recordings.with(|s| s.visible().nth(at.index()).cloned());
    ui_kit::button(
        ui,
        size,
        move || {
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
        },
        move || job().is_some_and(|j| ui.recordings.with(|s| s.focus == Some(j.id))),
        move || {
            if let Some(j) = job() {
                select_job(ui, j.id);
            }
        },
    )
    .test_id("recording-job")
}
fn status(job: &Job) -> &'static str {
    if job.status == Status::Recording && job.bytes == 0 {
        tr("Starting recording")
    } else {
        tr(job.status.label())
    }
}

fn action_view(ui: Ui, size: Scale) -> impl IntoElement {
    view().gap(move || size.px(12.0))
        .child(ui_kit::heading(size, move || ui.recordings.with(|s| s.focused().map(|j| j.request.title.clone()).unwrap_or_default())))
        .child(ui_kit::body(size, move || ui.recordings.with(|s| s.focused().map(|j| format!("{} · {}–{} · {} {}\n{}",
            tr(j.status.label()), date(j.request.start), date(j.request.end), j.bytes / (1024 * 1024), "MiB", translated_reason(&j.message))).unwrap_or_default())))
        .child(when(move || ui.recordings.with(|s| s.confirm_delete), move ||
            ui_kit::body(size, tr("Delete this recording file permanently? Select Delete recording again to confirm. Back cancels."))))
        .children((0..4).map(move |index| when(move || ui.recordings.with(|s| s.focused().is_some_and(|j| index < actions(ui, j).len())), move || {
            ui_kit::button(ui, size, move || ui.recordings.with(|s| s.focused().and_then(|j| actions(ui, j).get(index).copied()).map(|name| tr(name).to_owned()).unwrap_or_default()),
                move || ui.recordings.with(|s| s.action == index), move || perform(ui, index))
        })))
}
pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            ui_kit::screen(ui, size).test_id("recordings-screen")
            .accessibility_elements_hidden(move || crate::overlays::Overlay::Recordings.accessibility_covered(ui))
            .child(ui_kit::header(ui, size, tr("Recordings"), close))
            .child(ui_kit::caption(size, tr("Recordings continue when you close the player. \
                Check Recordings settings for startup and device wake requirements. \
                Recordings use local storage and are excluded from backups.")))
            .child(ui_kit::caption(size,move ||ui.recordings.with(|s|s.background_notice.as_ref().map(|message|tr(message).to_owned()).unwrap_or_default())))
            .child(ui_kit::caption(size, tr("Up/Down selects · Left/Right changes a value or filters the list · Menu shows all recordings · Back returns")))
            .child(ui_kit::body(size, move || ui.recordings.with(|s| s.error.as_ref().map(|message| tr(message).to_owned()).unwrap_or_default())))
            .child(ui_kit::caption(size, move || if ui.recordings.with(|s| s.busy) { tr("Saving…") } else { "" }))
            .child(when(move || ui.recordings.with(|s| s.draft.is_some()), move || scroll().flex_1().child(draft_view(ui, size))))
            .child(when(move || ui.recordings.with(|s| s.draft.is_none() && s.actions), move || action_view(ui, size)))
            .child(when(move || ui.recordings.with(|s| s.draft.is_none() && !s.actions), move || {
                let filters = view().flex_row().gap(move || size.px(12.0))
                    .children([(Filter::All, "All"), (Filter::Active, "Active"), (Filter::Finished, "Finished")].into_iter().map(move |(filter, name)|
                        ui_kit::tab(ui, size, tr(name), move || ui.recordings.with(|s| s.filter == filter), || false,
                            move || ui.recordings.update(|s| s.set_filter(filter)))));
                view().flex_1().gap(move || size.px(8.0)).child(filters)
                    .child(when(move || ui.recordings.with(|s| s.visible().next().is_none()), move || {
                        const EMPTY: &str = "No recordings here. Choose a programme in the guide, then Record or Schedule a recording.";
                        ui_kit::body(size, tr(EMPTY))
                    }))
                    .child(recording_list(ui, size))
            }))
        },
    )
}

fn recording_list(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    effect(move || {
        if let Some(at) = ui
            .recordings
            .with(|s| s.visible().position(|j| Some(j.id) == s.focus))
        {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(
        move || ui.recordings.with(|s| s.visible().count()),
        move |at| job_row(ui, size, at),
    )
    .element_ref(&rows)
    .estimated_item_size(move || size.px(ui_kit::TOUCH))
    .flex_1()
}
pub fn indicator(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || {
            !ui.any_overlay_open()
                && ui.recordings.with(|s| {
                    s.jobs
                        .iter()
                        .any(|j| j.status == Status::Recording && j.bytes > 0)
                })
        },
        move || {
            ui_kit::button(ui, size, tr("● Recording"), || false, move || open(ui))
                .absolute()
                .top(move || size.px(20.0))
                .right(move || size.px(24.0))
                .test_id("recording-indicator")
        },
    )
}

fn translated_reason(message: &str) -> &str {
    if message.is_empty() {
        ""
    } else if iptv_core::i18n::is_catalogued(message) {
        tr(message)
    } else {
        tr("Recording action failed")
    }
}
