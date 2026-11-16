//! One recording workflow for remote, touch and pointer input. State is portable;
//! the shell renders it and sends named runtime commands.
use crate::i18n::trf;
use crate::{Ui, i18n::tr, scale::Scale, ui_kit};
use iptv_core::menu::Origin;
use iptv_core::recording::{Draft, Filter, Job, JobId, Request, Status};
use iptv_nav::Action;
use nsh_core::prelude::*;
use std::time::Duration;

mod draft;
use draft::draft_view;

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
    if draft::cancel_entry(ui) {
        return true;
    }
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
    if ui.mode.get_untracked() == crate::ViewMode::Fullscreen {
        record_now(ui);
        return;
    }
    let Some(channel) = crate::lists::focused_channel(ui) else {
        open(ui);
        return;
    };
    let origin = Origin {
        channel,
        program_start: ui.focus.with_untracked(|f| f.cursor),
    };
    remember(ui, origin);
    let selected = crate::schedule().program_at(channel, origin.program_start);
    if selected.airing(crate::time::now_minute()) == crate::guide::Airing::Live {
        record_origin(ui, origin);
    } else {
        programme(ui, origin, false);
    }
}

/// The play bar records its playing channel, independently of guide focus.
/// It never opens the custom schedule form or replaces the player surface.
pub fn record_now(ui: Ui) {
    if ui.player_surface != 0 || ui.playback.get_untracked() != crate::Playback::Live {
        return;
    }
    let Some(channel) = ui.playing.get_untracked() else {
        return;
    };
    start_origin(
        ui,
        Origin {
            channel,
            program_start: crate::time::now_minute(),
        },
        false,
    );
}

pub fn record_origin(ui: Ui, origin: Origin) {
    start_origin(ui, origin, true);
}
fn start_origin(ui: Ui, origin: Origin, remembered: bool) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    let context = remembered
        .then(|| {
            ui.recordings.with_untracked(|s| {
                s.context
                    .as_ref()
                    .filter(|(saved, _)| *saved == origin)
                    .map(|(_, request)| request.clone())
            })
        })
        .flatten();
    let Some(mut request) = context.or_else(|| request(origin)) else {
        crate::settings_view::notify(tr("Choose an available channel"));
        return;
    };
    let now = crate::time::wall_unix();
    // Repeated taps are idempotent, including before the next UI status poll.
    if crate::runtime().recording_snapshot().iter().any(|job| {
        job.source_current
            && job.status.active()
            && job.request.source == request.source
            && job.request.channel == request.channel
            && job.request.start <= now
            && job.request.end > now
    }) {
        crate::settings_view::notify(tr("Recording started"));
        return;
    }
    request.start = now;
    if request.end <= now {
        request.end = now + 3600;
    }
    let mut draft = Draft::new(request);
    draft.immediate = true;
    let target =
        iptv_core::parental::Target::live(draft.request.source, draft.request.channel.clone());
    crate::parental_view::request(ui, target, move || save_allowed(ui, draft));
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
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
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
    // A future programme keeps its actual window when opened from its context menu.
    let custom = custom && request.start <= crate::time::wall_unix();
    if custom {
        request.start = crate::time::wall_unix();
        request.end = request.start + 3600;
    }
    let programme = if custom {
        None
    } else {
        let schedule = crate::schedule();
        let programme = schedule.program_at(origin.channel, origin.program_start);
        Some((
            crate::guide::unix_of(&*schedule, programme.start),
            crate::guide::unix_of(&*schedule, programme.end),
        ))
    };
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
            let mut draft = Draft::new(request);
            draft.programme_start = programme.map(|(start, _)| start);
            draft.programme_end = programme.map(|(_, end)| end);
            s.draft = Some(draft);
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
        if draft.immediate {
            crate::settings_view::notify(tr(message));
        }
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
                    .schedule_repeating_recording_with_timing(
                        request,
                        draft.repeat,
                        (draft.repeat != iptv_core::recording::Repeat::Once).then_some(
                            iptv_core::recording::RecurrenceTiming {
                                programme_start: draft
                                    .programme_start
                                    .unwrap_or(draft.request.start),
                                programme_end: draft.programme_end.unwrap_or(draft.request.end),
                                lead_seconds: draft.lead_minutes.clamp(0, 60) as u32 * 60,
                                trail_seconds: draft.trail_minutes.clamp(0, 60) as u32 * 60,
                            },
                        ),
                    )
                    .await
            }
        };
        if draft.immediate {
            match &result {
                Ok(_) => crate::settings_view::notify(tr("Starting recording")),
                Err(error) => crate::settings_view::notify(translated_reason(
                    error
                        .unavailable_reason()
                        .unwrap_or("Recording action failed"),
                )),
            }
        }
        ui.recordings.update(|s| {
            s.busy = false;
            match result {
                Ok(id) => {
                    s.draft = None;
                    s.filter = Filter::All;
                    s.focus = Some(id);
                    s.actions = false;
                    if draft.immediate {
                        s.open = false;
                    }
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
    if draft::key(ui, action) {
        return;
    }
    if ui.recordings.with_untracked(|s| s.actions) {
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
fn job_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let job = move || ui.recordings.with(|s| s.visible().nth(at.index()).cloned());
    let mark = move || {
        crate::list_rows::Mark::of(
            ui.keys_shown() && job().is_some_and(|j| ui.recordings.with(|s| s.focus == Some(j.id))),
            job().is_some_and(|j| ui.recordings.with(|s| s.focus == Some(j.id))),
        )
    };
    let day = move || {
        ui.recordings.with(|s| {
            let mut jobs = s.visible();
            let previous = at.index().checked_sub(1).and_then(|at| jobs.nth(at));
            let current = s.visible().nth(at.index());
            current
                .filter(|j| {
                    previous.is_none_or(|prev| {
                        crate::time::date_only(prev.request.start)
                            != crate::time::date_only(j.request.start)
                    })
                })
                .map(|j| crate::time::date_only(j.request.start))
                .unwrap_or_default()
        })
    };
    view()
        .gap(move || size.px(6.0))
        .child(when(
            move || !day().is_empty(),
            move || ui_kit::caption(size, day).pt(move || size.px(12.0)),
        ))
        .child(
            crate::list_rows::row(size, ui_kit::ROW_THREE, mark)
                .test_id("recording-job")
                .role(AccessibilityRole::Button)
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
                    if let Some(j) = job() {
                        ui.touched();
                        select_job(ui, j.id);
                    }
                })
                .child(
                    ui_kit::label(size, move || {
                        job().map(|j| j.request.title).unwrap_or_default()
                    })
                    .number_of_lines(1)
                    .text_color(move || mark().text()),
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
                    .number_of_lines(1)
                    .text_color(move || mark().secondary()),
                ),
        )
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
    let requirements = if cfg!(target_os = "ios") {
        tr(
            "Keep Patina open and this device awake for recordings. Recordings use local app storage and are excluded from backups.",
        )
    } else {
        tr(
            "Recordings continue when you close the player. Check Recordings settings for startup and device wake requirements. Recordings use local storage and are excluded from backups.",
        )
    };
    let has_draft = move || ui.recordings.with(|s| s.draft.is_some());
    let message = move || {
        ui.recordings.with(|s| {
            s.error
                .as_ref()
                .map(|message| tr(message).to_owned())
                .unwrap_or_default()
        })
    };
    when(
        move || is_open(ui),
        move || {
            view().absolute().inset_0().test_id("recordings-screen")
            .accessibility_elements_hidden(move || crate::overlays::Overlay::Recordings.accessibility_covered(ui))
            .child(when(has_draft, move || view().absolute().inset_0().items_end().bg(crate::tint(0, 0x55))
                .child(view().h_full().w(move || size.px(640.0)).max_w(percentage(0.9))
                    .accessibility_elements_hidden(move || ui.recordings.with(|s| s.draft.as_ref().is_some_and(|d| d.entry.is_some())))
                    .pt_safe_area().px(move || size.px(24.0)).py(move || size.px(20.0)).gap(move || size.px(12.0))
                    .bg(move || rgb(ui.theme.with(|t| t.background.panel)))
                    .child(ui_kit::header(ui, size, tr("Schedule a recording…"), close))
                    .child(scroll().flex_1().child(draft_view(ui, size)))
                    .child(ui_kit::body(size, message).text_color(rgb(ui_kit::ERROR)))
                    .child(ui_kit::caption(size, move || if ui.recordings.with(|s| s.busy) { tr("Saving…") } else { "" }))
                    .child(ui_kit::caption(size, requirements)))))
            .child(when(move || !has_draft(), move || {
                ui_kit::screen(ui, size)
                    .child(ui_kit::header(ui, size, tr("Recordings"), close))
                    .child(view().flex_row().gap(move || size.px(12.0)).children(
                        [(Filter::All, "All"), (Filter::Active, "Active"), (Filter::Finished, "Finished")].into_iter().map(move |(filter, name)|
                            ui_kit::tab(ui, size, tr(name), move || ui.recordings.with(|s| s.filter == filter), || false,
                                move || ui.recordings.update(|s| { s.set_filter(filter); s.actions = false; s.confirm_delete = false; })))))
                    .child(ui_kit::body(size, message).text_color(rgb(ui_kit::ERROR)))
                    .child(when(move || ui.keys_shown(), move || ui_kit::caption(size, tr("Up/Down selects · Left/Right changes a value or filters the list · Menu shows all recordings · Back returns"))))
                    .child(view().flex_row().flex_1().gap(move || size.px(24.0))
                        .child(view().flex_1().gap(move || size.px(8.0))
                            .child(when(move || ui.recordings.with(|s| s.visible().next().is_none()), move || ui_kit::body(size, tr("No recordings here. Choose a programme in the guide, then Record or Schedule a recording."))))
                            .child(recording_list(ui, size)))
                        .child(when(move || ui.recordings.with(|s| s.actions), move || scroll().w(percentage(0.4)).child(action_view(ui, size)))))
                    .child(ui_kit::caption(size, move || ui.recordings.with(|s| s.background_notice.as_ref().map(|message| tr(message).to_owned()).unwrap_or_default())))
                    .child(ui_kit::caption(size, requirements))
            }))
            .child(draft::entry_layer(ui))
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
    .estimated_item_size(move || size.px(ui_kit::ROW_THREE + 24.0))
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
