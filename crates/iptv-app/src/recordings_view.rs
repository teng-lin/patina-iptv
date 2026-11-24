//! One recording workflow for remote, touch and pointer input. State is portable;
//! the shell renders it and sends named runtime commands.
use crate::i18n::trf;
use crate::{Ui, i18n::tr, scale::Scale, ui_kit};
use iptv_core::recording::{Job, JobId, Request, Status};
use iptv_nav::Action;
use iptv_presentation::capabilities::recording_notice;
use iptv_presentation::menu::{MenuItem, Origin};
use iptv_presentation::recording::{Draft, status_label};
use nsh_core::prelude::*;
use std::time::Duration;

mod draft;
mod rows;
mod style;
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
        let background_notice = recording_notice(owner.recording_capabilities()).map(str::to_owned);
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
    // This one-second UI reload is unrelated to a guide/player action. Keep
    // the real native recorder running in explicitly opted-in work-counter runs.
    #[cfg(feature = "test-support")]
    if crate::performance_probe::enabled() {
        return;
    }
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
pub fn try_reserve_viewing(surface: usize, channel: usize, live: bool) -> bool {
    let source = crate::schedule()
        .channel_key(channel)
        .and_then(|key| crate::data::combined::owner(&crate::runtime(), &key));
    source.is_none_or(|(source, channel)| {
        if live {
            crate::runtime().reserve_live_viewing(surface, source, &channel)
        } else {
            crate::runtime().reserve_viewing(surface, source)
        }
    })
}
pub fn reserve_viewing(_ui: Ui, surface: usize, channel: usize, live: bool) -> bool {
    if !try_reserve_viewing(surface, channel, live) {
        crate::settings_view::notify(tr(
            "No provider connection available while recording; stop a recording or check Streams at once",
        ));
        return false;
    }
    true
}
pub fn badge(ui: Ui, channel: usize, program: crate::guide::Program) -> &'static str {
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
        crate::guide::unix_of(&*schedule, program.start),
        crate::guide::unix_of(&*schedule, program.end),
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
        toggle_current(ui);
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
    match selected.airing(crate::time::now_minute()) {
        crate::guide::Airing::Live => record_origin(ui, origin),
        crate::guide::Airing::Upcoming => schedule_program(ui, origin),
        crate::guide::Airing::Ended => program(ui, origin, false),
    }
}

fn playing_channel(ui: Ui) -> Option<(i64, String)> {
    if ui.player_surface != 0 || ui.playback.get() != crate::Playback::Live {
        return None;
    }
    let channel = ui.playing.get()?;
    crate::schedule()
        .channel_key(channel)
        .and_then(|key| crate::data::combined::owner(&crate::runtime(), &key))
        .map(|(source, channel)| (source.0, channel))
}

pub fn recording_active(ui: Ui) -> bool {
    let Some((source, channel)) = playing_channel(ui) else {
        return false;
    };
    ui.recordings.with(|s| {
        s.starting
            .as_ref()
            .is_some_and(|r| r.source == source && r.channel == channel)
            || s.active_on(source, &channel, crate::time::wall_unix())
                .is_some()
    })
}

pub fn toggle_current(ui: Ui) {
    if ui
        .recordings
        .with_untracked(|s| s.busy || s.starting.is_some())
    {
        return;
    }
    let Some((source, channel)) = playing_channel(ui) else {
        return;
    };
    let job = ui.recordings.with_untracked(|s| {
        s.active_on(source, &channel, crate::time::wall_unix())
            .cloned()
    });
    if let Some(job) = job {
        execute(ui, job, false, "Stop recording");
    } else {
        record_now(ui);
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
    let program = schedule.program_at(origin.channel, origin.program_start);
    Some(Request {
        source: source.0,
        channel,
        channel_name: crate::channel_name(origin.channel),
        title: crate::listing_labels::listing(schedule.listing(origin.channel, program)),
        start: crate::guide::unix_of(&*schedule, program.start).max(crate::time::wall_unix()),
        end: crate::guide::unix_of(&*schedule, program.end),
    })
}
/// The active job must belong to the selected occurrence, including its end
/// when an immediate recording starts after the program began.
fn occurrence_job(
    request: &Request,
    stored_start: i64,
    jobs: Vec<Job>,
    padded_matches: impl Fn(&Request) -> bool,
) -> Option<Job> {
    jobs.into_iter()
        .filter(|job| {
            job.source_current
                && job.status.active()
                && job.request.source == request.source
                && job.request.channel == request.channel
                && if let Some(raw) = job.series_start {
                    (stored_start..stored_start + 60).contains(&raw)
                } else {
                    (job.request.end == request.end
                        && match job.status {
                            Status::Scheduled => job.request.start == request.start,
                            _ => (request.start..request.end).contains(&job.request.start),
                        })
                        || padded_matches(&job.request)
                }
        })
        .max_by_key(|job| job.status == Status::Recording)
}

fn padded_occurrence(
    schedule: &dyn crate::guide::Schedule,
    channel: usize,
    program: crate::guide::Program,
    airing: &Request,
    recording: &Request,
) -> bool {
    const MAX_PADDING: i64 = 3600;
    if recording.title != airing.title
        || !(airing.start - MAX_PADDING..=airing.start).contains(&recording.start)
        || !(airing.end..=airing.end + MAX_PADDING).contains(&recording.end)
        || !matches!(
            schedule.listing(channel, program),
            Some(crate::guide::Listing::Title(_))
        )
    {
        return false;
    }
    let epoch = schedule.epoch_unix();
    let from = ((recording.start - epoch).div_euclid(60)) as i32;
    let to = ((recording.end - epoch).div_euclid(60)) as i32;
    // A padded request spanning two same-title airings is ambiguous.
    let mut matches = schedule
        .programs_between(channel, from, to - from)
        .into_iter()
        .filter(|candidate| {
            candidate.start >= from
                && candidate.end <= to
                && crate::listing_labels::listing(schedule.listing(channel, *candidate))
                    == airing.title
        });
    matches.next() == Some(program) && matches.next().is_none()
}

fn channel_action(ui: Ui, origin: Origin) -> Option<(MenuItem, Option<Job>)> {
    if ui.player_surface != 0 || !available(origin.channel) {
        return None;
    }
    let schedule = crate::schedule();
    let program = schedule.program_at(origin.channel, origin.program_start);
    let mut request = request(origin)?;
    request.start = crate::guide::unix_of(&*schedule, program.start);
    let stored_start =
        request.start - i64::from(crate::player::guide_offset(&*schedule, origin.channel)) * 60;
    if let Some(job) = occurrence_job(
        &request,
        stored_start,
        crate::runtime().recording_snapshot(),
        |recording| padded_occurrence(&*schedule, origin.channel, program, &request, recording),
    ) {
        let item = if job.status == Status::Recording {
            MenuItem::StopRecording
        } else {
            MenuItem::CancelRecording
        };
        return Some((item, Some(job)));
    }
    let item = match program.airing(crate::time::now_minute()) {
        crate::guide::Airing::Live => MenuItem::RecordNow,
        crate::guide::Airing::Upcoming => MenuItem::ScheduleRecording,
        crate::guide::Airing::Ended => return None,
    };
    Some((item, None))
}

pub fn channel_menu_action(ui: Ui, origin: Origin) -> Option<MenuItem> {
    channel_action(ui, origin).map(|(item, _)| item)
}

pub fn perform_channel_action(ui: Ui, origin: Origin, item: MenuItem) -> bool {
    let Some((current, job)) = channel_action(ui, origin) else {
        return false;
    };
    if current != item {
        return false;
    }
    match (current, job) {
        (MenuItem::StopRecording, Some(job)) => execute(ui, job, false, "Stop recording"),
        (MenuItem::CancelRecording, Some(job)) => execute(ui, job, false, "Cancel recording"),
        (MenuItem::RecordNow, _) => start_origin(ui, origin, false),
        (MenuItem::ScheduleRecording, _) => {
            if let Some(request) = request(origin) {
                schedule_window(ui, request);
            }
        }
        _ => return false,
    }
    true
}

fn schedule_window(ui: Ui, request: Request) {
    let target = iptv_core::parental::Target::live(request.source, request.channel.clone());
    crate::parental_view::request(ui, target, move || {
        if ui.recordings.with_untracked(|state| state.busy) {
            return;
        }
        ui.recordings.update(|state| state.busy = true);
        let owner = crate::runtime();
        crate::data::spawn_in_app(async move {
            let result = owner.schedule_recording(request).await;
            ui.recordings.update(|state| state.busy = false);
            let message = match &result {
                Ok(_) => tr("Recording scheduled"),
                Err(error) => translated_reason(
                    error
                        .unavailable_reason()
                        .unwrap_or("Recording action failed"),
                ),
            };
            crate::settings_view::notify(message);
            reload(ui);
        });
    });
}

pub fn remember(ui: Ui, origin: Origin) {
    let context = request(origin).map(|request| (origin, request));
    ui.recordings.update(|s| s.context = context);
}
/// EPG scheduling needs no manual date/time form: retain the selected occurrence.
pub fn schedule_program(ui: Ui, origin: Origin) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    if !prepare_program(ui, origin, false) {
        return;
    }
    let draft = ui.recordings.with_untracked(|s| s.draft.clone());
    if draft.is_some() {
        save(ui);
        ui.recordings.update(|s| s.draft = None);
    }
}
pub fn program(ui: Ui, origin: Origin, custom: bool) {
    prepare_program(ui, origin, custom);
}
fn prepare_program(ui: Ui, origin: Origin, custom: bool) -> bool {
    if ui.recordings.with_untracked(|s| s.busy) {
        return false;
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
        return false;
    };
    // A future program keeps its actual window when opened from its context menu.
    let custom = custom && request.start <= crate::time::wall_unix();
    if custom {
        request.start = crate::time::wall_unix();
        request.end = request.start + 3600;
    }
    let program = if custom {
        None
    } else {
        let schedule = crate::schedule();
        let program = schedule.program_at(origin.channel, origin.program_start);
        Some((
            crate::guide::unix_of(&*schedule, program.start),
            crate::guide::unix_of(&*schedule, program.end),
            i64::from(crate::player::guide_offset(&*schedule, origin.channel)) * 60,
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
            draft.program_start = program.map(|(start, _, _)| start);
            draft.program_end = program.map(|(_, end, _)| end);
            draft.guide_offset_seconds = program.map(|(_, _, offset)| offset);
            s.draft = Some(draft);
        }
    });
    true
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
        if draft.immediate {
            s.starting = Some(draft.request.clone());
        }
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
                                program_start: draft.program_start.unwrap_or(draft.request.start),
                                program_end: draft.program_end.unwrap_or(draft.request.end),
                                guide_offset_seconds: draft.guide_offset_seconds,
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
        let jobs = if result.is_ok() {
            owner.recording_jobs().await.ok()
        } else {
            None
        };
        ui.recordings.update(|s| {
            s.busy = false;
            s.starting = None;
            if let Some(jobs) = jobs {
                s.reload(jobs);
            }
            match result {
                Ok(id) => {
                    // A play-bar action owns no library presentation. Keep a
                    // row, editor or confirmation the viewer opened meanwhile.
                    if !draft.immediate {
                        s.draft = None;
                        s.focus = Some(id);
                        s.actions = false;
                    }
                }
                Err(error) => {
                    s.error = Some(
                        error
                            .unavailable_reason()
                            .filter(|reason| iptv_presentation::i18n::is_cataloged(reason))
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
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    ui.recordings.update(|s| {
        s.focus = Some(id);
        s.actions = true;
        s.confirm_delete = false;
        s.action = 0;
        s.error = None;
    });
}
pub(super) fn return_to_player(ui: Ui) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    close(ui);
    ui.mode.set(crate::ViewMode::Fullscreen);
    crate::play_bar::tapped(ui);
}

fn activate_job(ui: Ui, id: JobId) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    let job = ui
        .recordings
        .with_untracked(|s| s.jobs.iter().find(|j| j.id == id).cloned());
    let Some(job) = job else { return };
    if ui
        .player
        .recorded
        .with_untracked(|r| r.as_ref().is_some_and(|(playing, _)| playing.id == id))
    {
        return_to_player(ui);
    } else if job.status == Status::Scheduled {
        execute(ui, job, false, "Edit recording");
    } else if matches!(job.status, Status::Completed | Status::Partial) && job.bytes > 0 {
        ui.recordings.update(|s| {
            s.focus = Some(id);
            s.confirm_delete = false;
        });
        perform(ui, 0);
    } else {
        select_job(ui, id);
    }
}

fn row_action(ui: Ui, id: JobId, action: &'static str) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    let job = ui
        .recordings
        .with_untracked(|s| s.jobs.iter().find(|j| j.id == id).cloned());
    if let Some(job) = job {
        ui.recordings.update(|s| {
            s.focus = Some(id);
            s.confirm_delete = false;
        });
        execute(ui, job, false, action);
    }
}

fn request_delete(ui: Ui, id: JobId) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    ui.recordings.update(|s| {
        s.focus = Some(id);
        s.actions = false;
        s.confirm_delete = true;
        s.delete_selected = false;
        s.error = None;
    });
}

fn confirm_delete(ui: Ui) {
    let job = ui.recordings.with_untracked(|s| {
        if s.busy || !s.confirm_delete {
            None
        } else {
            s.focused().cloned()
        }
    });
    if let Some(job) = job {
        execute(ui, job, true, "Delete recording");
    }
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
        crate::parental_view::request(ui, target, move || execute(ui, job, confirm, action));
    } else {
        execute(ui, job, confirm, action);
    }
}
fn execute(ui: Ui, job: Job, confirm: bool, action: &'static str) {
    if action == "Stop playback" {
        crate::player_keys::stop_playback(ui);
        return;
    }
    if action == "Edit recording" {
        ui.recordings.update(|s| {
            s.focus = Some(job.id);
            s.confirm_delete = false;
            let mut d = Draft::new(job.request);
            d.editing = Some(job.id);
            s.draft = Some(d);
            s.actions = false;
        });
        return;
    }
    if action == "Delete recording" && !confirm {
        request_delete(ui, job.id);
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
            // Consent precedes stopping; deleting another file leaves playback alone.
            if ui
                .player
                .recorded
                .with_untracked(|r| r.as_ref().is_some_and(|(playing, _)| playing.id == job.id))
            {
                crate::player_keys::stop_playback(ui);
            }
            owner.delete_recording(job.id).await
        } else {
            owner.stop_recording(job.id).await
        };
        let jobs = if result.is_ok() {
            owner.recording_jobs().await.ok()
        } else {
            None
        };
        let mut failure_notice = None;
        ui.recordings.update(|s| {
            s.busy = false;
            if let Some(jobs) = jobs {
                s.reload(jobs);
            }
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
                    if s.open {
                        s.error = Some(
                            if action == "Delete recording" {
                                "Recording could not be deleted"
                            } else {
                                "Recording action failed; check whether its file is available"
                            }
                            .into(),
                        );
                    } else {
                        failure_notice = Some(tr("Recording action failed"));
                    }
                }
            }
        });
        if let Some(message) = failure_notice {
            crate::settings_view::notify(message);
        }
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
    if ui.recordings.with_untracked(|s| s.confirm_delete) {
        match action {
            Action::MoveLeft | Action::MoveRight | Action::MoveUp | Action::MoveDown => {
                ui.recordings
                    .update(|s| s.delete_selected = !s.delete_selected);
            }
            Action::Select => {
                if ui.recordings.with_untracked(|s| s.delete_selected) {
                    confirm_delete(ui);
                } else {
                    back(ui);
                }
            }
            _ => {}
        }
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
            Action::ShowMenu => {
                if let Some(id) = ui.recordings.with_untracked(|s| s.focus) {
                    select_job(ui, id);
                }
            }
            Action::Select => {
                if let Some(id) = ui.recordings.with_untracked(|s| s.focus) {
                    activate_job(ui, id);
                }
            }
            _ => {}
        }
    }
}
fn date(unix: i64) -> String {
    crate::time::label(unix, crate::time::wall_unix())
}

fn status(job: &Job) -> &'static str {
    if job.status == Status::Recording && job.bytes == 0 {
        tr("Starting recording")
    } else {
        tr(status_label(job.status))
    }
}

fn action_view(ui: Ui, size: Scale) -> impl IntoElement {
    view()
        .child(
            ui_kit::title(size, move || {
                ui.recordings.with(|s| {
                    s.focused()
                        .map(|j| j.request.title.clone())
                        .unwrap_or_default()
                })
            })
            .px(move || size.px(24.0))
            .py(move || size.px(16.0)),
        )
        .child(
            ui_kit::body(size, move || {
                ui.recordings.with(|s| {
                    s.focused()
                        .map(|j| {
                            let media =
                                format!("{:.2} {}", j.bytes as f64 / (1024.0 * 1024.0), tr("MiB"));
                            format!(
                                "{} · {}\n{}–{} · {}\n{}",
                                tr(status_label(j.status)),
                                j.request.channel_name,
                                date(j.request.start),
                                crate::time::time_only(j.request.end),
                                media,
                                translated_reason(&j.message)
                            )
                        })
                        .unwrap_or_default()
                })
            })
            .px(move || size.px(24.0))
            .pb(move || size.px(16.0)),
        )
        .children((0..4).map(move |index| {
            when(
                move || {
                    ui.recordings
                        .with(|s| s.focused().is_some_and(|j| index < actions(ui, j).len()))
                },
                move || {
                    style::action(
                        ui,
                        size,
                        move || {
                            ui.recordings.with(|s| {
                                s.focused()
                                    .and_then(|j| actions(ui, j).get(index).copied())
                                    .map(|name| tr(name).to_owned())
                                    .unwrap_or_default()
                            })
                        },
                        move || ui.recordings.with(|s| s.action == index),
                        move || perform(ui, index),
                    )
                },
            )
        }))
}

fn requirements() -> &'static str {
    if crate::host::IPAD {
        tr("Keep Patina open and this device awake for recordings. \
             Recordings use local app storage and are excluded from backups.")
    } else {
        tr(
            "Recordings continue when you close the player. Check Recordings settings for startup and device wake requirements. \
             Recordings use local storage and are excluded from backups.",
        )
    }
}

fn message(ui: Ui) -> String {
    ui.recordings.with(|s| {
        s.error
            .as_ref()
            .map(|message| tr(message).to_owned())
            .unwrap_or_default()
    })
}

fn schedule_panel(ui: Ui, size: Scale) -> View {
    view()
        .absolute()
        .inset_0()
        .items_end()
        .bg(crate::tint(0, 0x55))
        .child(
            view()
                .h_full()
                .w(move || size.px(400.0))
                .max_w(percentage(0.9))
                .accessibility_elements_hidden(move || {
                    ui.recordings
                        .with(|s| s.draft.as_ref().is_some_and(|d| d.entry.is_some()))
                })
                .pt_safe_area()
                .pb_safe_area()
                .bg(rgb(style::PANEL))
                .child(style::header(ui, size, tr("Schedule a recording…")))
                .child({
                    let scrolling = ui_kit::FocusScroll::new();
                    scrolling
                        .view()
                        .test_id("recording-draft-scroll")
                        .flex_1()
                        .child(draft_view(ui, size, scrolling))
                })
                .child(
                    ui_kit::body(size, move || message(ui))
                        .text_color(rgb(ui_kit::ERROR))
                        .px(move || size.px(24.0)),
                )
                .child(
                    ui_kit::caption(size, move || {
                        if ui.recordings.with(|s| s.busy) {
                            tr("Saving…")
                        } else {
                            ""
                        }
                    })
                    .px(move || size.px(24.0)),
                )
                .child(
                    ui_kit::caption(size, requirements())
                        .px(move || size.px(24.0))
                        .py(move || size.px(16.0)),
                ),
        )
}

fn library_content(ui: Ui, size: Scale) -> View {
    view().absolute().inset_0().pt_safe_area().pb_safe_area().bg(rgb(style::WINDOW))
        .child(view().flex_row().items_center().px(move || size.px(24.0))
            .min_h(move || size.px(72.0))
            .child(when(move || ui.recordings.with(|s| !s.actions), move || style::action(ui, size, tr("Close"), || false, move || close(ui))))
            .child(view().flex_1())
            .child(ui_kit::title(size, tr("Recordings"))))
        .child(rows::now_playing(ui, size))
        .child(ui_kit::body(size, move || message(ui)).text_color(rgb(ui_kit::ERROR))
            .px(move || size.px(24.0)))
        .child(when(move || ui.recordings.with(|s| s.visible().next().is_none()), move || {
            ui_kit::body(size, tr(
                "No recordings here. Choose a program in the guide, then Record or Schedule a recording."
            )).px(move || size.px(24.0)).py(move || size.px(24.0))
        }))
        .child(recording_list(ui, size))
        .child(ui_kit::caption(size, move || ui.recordings.with(|s| {
            s.background_notice.as_ref().map(|message| tr(message).to_owned()).unwrap_or_default()
        })).px(move || size.px(24.0)))
        .child(ui_kit::caption(size, requirements()).px(move || size.px(24.0)).py(move || size.px(16.0)))
}

fn library(ui: Ui, size: Scale) -> View {
    view()
        .absolute()
        .inset_0()
        .child(
            library_content(ui, size).accessibility_elements_hidden(move || {
                ui.recordings.with(|s| s.actions || s.confirm_delete)
            }),
        )
        .child(when(
            move || ui.recordings.with(|s| s.actions),
            move || {
                view()
                    .absolute()
                    .inset_0()
                    .items_end()
                    .bg(crate::tint(0, 0x55))
                    .child(
                        view()
                            .w(move || size.px(400.0))
                            .max_w(percentage(0.9))
                            .h_full()
                            .pt_safe_area()
                            .pb_safe_area()
                            .bg(rgb(style::PANEL))
                            .child(style::header(ui, size, tr("Recordings")))
                            .child(scroll().flex_1().child(action_view(ui, size))),
                    )
            },
        ))
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let has_draft = move || ui.recordings.with(|s| s.draft.is_some());
    when(
        move || is_open(ui),
        move || {
            view()
                .absolute()
                .inset_0()
                .test_id("recordings-screen")
                .accessibility_elements_hidden(move || {
                    crate::overlays::Overlay::Recordings.accessibility_covered(ui)
                })
                .child(when(has_draft, move || schedule_panel(ui, size)))
                .child(when(move || !has_draft(), move || library(ui, size)))
                .child(draft::entry_layer(ui))
                .child(rows::delete_confirmation(ui, size))
        },
    )
}

fn recording_list(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    // A memo: the list follows the focus when it moves, not on every
    // reload (one a second), which would undo the wheel's scroll.
    let focused = computed(move || {
        ui.recordings
            .with(|s| s.visible().position(|j| Some(j.id) == s.focus))
    });
    effect(move || {
        if let Some(at) = focused.get() {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(
        move || ui.recordings.with(|s| s.visible().count()),
        move |at| rows::job_row(ui, size, at),
    )
    .element_ref(&rows)
    .estimated_item_size(move || size.px(ui_kit::ROW_THREE + 24.0))
    .flex_1()
}

fn translated_reason(message: &str) -> &str {
    if message.is_empty() {
        ""
    } else if iptv_presentation::i18n::is_cataloged(message) {
        tr(message)
    } else {
        tr("Recording action failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_recording_matches_the_airing_and_owning_playlist() {
        let request = Request {
            source: 1,
            channel: "generated-channel".into(),
            channel_name: "Generated channel".into(),
            title: "Generated program".into(),
            start: 1_800_000_000,
            end: 1_800_003_600,
        };
        let raw_start = request.start - 900;
        let mut job = Job {
            id: JobId(7),
            source_current: true,
            series_start: Some(raw_start + 18),
            request: request.clone(),
            status: Status::Scheduled,
            bytes: 0,
            message: String::new(),
        };
        job.request.start -= 600;
        job.request.end += 900;
        let mut neighbor = job.clone();
        neighbor.id = JobId(8);
        neighbor.series_start = Some(raw_start + 3600);
        let mut other_playlist = job.clone();
        other_playlist.id = JobId(9);
        other_playlist.request.source = 2;
        assert_eq!(
            occurrence_job(
                &request,
                raw_start,
                vec![neighbor.clone(), other_playlist, job.clone()],
                |_| false
            )
            .map(|job| job.id),
            Some(JobId(7)),
        );
        assert_eq!(
            occurrence_job(&request, raw_start, vec![neighbor], |_| false),
            None
        );
        job.series_start = None;
        job.status = Status::Recording;
        job.request = request.clone();
        job.request.start += 1200;
        assert_eq!(
            occurrence_job(&request, raw_start, vec![job], |_| false).map(|job| job.id),
            Some(JobId(7))
        );
    }
}
