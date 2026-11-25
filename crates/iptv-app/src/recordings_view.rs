//! One recording workflow for remote, touch and pointer input. State is portable;
//! the shell renders it and sends named runtime commands.
use crate::{Ui, i18n::tr, scale::Scale, ui_kit};
use iptv_core::recording::{Job, JobId, Request, Status};
use iptv_nav::Action;
use iptv_presentation::menu::{MenuItem, Origin};
use iptv_presentation::recording::{ActionOutcome, Screen, status_label};
use nsh_core::prelude::*;
use std::time::Duration;

mod rows;
mod style;

pub fn is_open(ui: Ui) -> bool {
    ui.recordings_open.get()
}
pub fn open(ui: Ui) {
    // Not on a device that does not show the library (yet): its entries
    // are gone, and a key bound to it does nothing.
    if !crate::capabilities::recordings_library_untracked(ui) {
        return;
    }
    ui.recordings.update(Screen::open);
    reload(ui);
}
pub fn close(ui: Ui) {
    ui.recordings.update(Screen::close);
}
pub fn back(ui: Ui) -> bool {
    ui.recordings.update(Screen::back);
    true
}
fn reload(ui: Ui) {
    let owner = crate::runtime();
    let mut generation = 0;
    ui.recordings.update(|s| generation = s.begin_reload());
    crate::data::spawn_in_app(async move {
        let result = owner.recording_jobs().await;
        let points = owner.recording_progress_points().await.unwrap_or_default();
        let retired = owner
            .retired_recording_schedules()
            .await
            .unwrap_or_default();
        ui.recordings.update(|s| {
            s.finish_reload(
                generation,
                result.map_err(|_| "Recordings could not be loaded"),
                points,
                retired,
            );
        });
    });
}
/// Scheduled recording is retired. Tells the viewer once that the upgrade
/// canceled what an earlier version had scheduled; Recordings lists it until
/// the viewer has opened and closed that screen.
fn announce_retired(ui: Ui) {
    crate::data::spawn_in_app(async move {
        if crate::runtime()
            .announce_retired_recordings()
            .await
            .unwrap_or(false)
        {
            crate::settings_view::notify(tr(
                "Scheduled recording was removed. Open Recordings to see what was canceled.",
            ));
        }
    });
    effect(move || {
        if ui.recordings_open.get() {
            return;
        }
        untracked(|| {
            if ui.recordings.with(|s| s.retired.is_empty()) {
                return;
            }
            ui.recordings.update(Screen::dismiss_retired);
            crate::data::spawn_in_app(async move {
                // On failure the list stays stored and shows again next time.
                if let Err(error) = crate::runtime().dismiss_retired_recording_schedules().await {
                    log::warn!("iptv recordings: keeping the retired schedule list: {error:?}");
                }
            });
        });
    });
}

pub fn bind(ui: Ui) {
    crate::runtime().start_recordings();
    announce_retired(ui);
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

/// Whether `channel` can be recorded here: on a host with workers (a
/// browser TV has none, `host::HAS_WORKERS`), from a playlist that owns it.
pub fn available(channel: usize) -> bool {
    crate::host::HAS_WORKERS
        && crate::schedule()
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
/// The guide's mark on a program being recorded: "● " while a capture of
/// it is writing, otherwise nothing.
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
    let recording = ui.recordings.with(|s| {
        s.jobs.iter().any(|j| {
            j.status == Status::Recording
                && j.bytes > 0
                && j.source_current
                && j.request.source == source.0
                && j.request.channel == key
                && j.request.start < end
                && j.request.end > start
        })
    });
    if recording { "● " } else { "" }
}
pub fn current(ui: Ui) {
    // The Record key does nothing where Record now is not offered.
    if !untracked(crate::capabilities::record_now).offered() {
        return;
    }
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
    // Record starts recording a program on now. A future or ended one is not
    // scheduled: recording future programs is retired (#273).
    if !can_record_now(origin, crate::time::now_minute()) {
        crate::settings_view::notify(tr("Only a program on now can be recorded"));
        return;
    }
    remember(ui, origin);
    record_origin(ui, origin);
}

/// Whether the program at `origin` is on at guide minute `now` on a channel
/// that can be recorded, so Record can start recording it now.
pub fn can_record_now(origin: Origin, now: i32) -> bool {
    available(origin.channel)
        && crate::schedule()
            .program_at(origin.channel, origin.program_start)
            .airing(now)
            == crate::guide::Airing::Live
}

/// The program details' Record button (and Select or Record over the
/// details): records a program on now from its menu context. It checks the
/// clock itself, not the guide's cached minute: a program that has just ended
/// must not fall back to recording the next hour under its title.
pub fn record_details(ui: Ui, origin: Origin) {
    if ui.player_surface != 0
        || !details_button()
        || !can_record_now(origin, crate::time::now_minute())
    {
        return;
    }
    ui.description.set(None);
    record_origin(ui, origin);
}

/// Whether the program details show Record (and Select or Record over
/// them press it): only where Record now is offered directly
/// (`capabilities::record_now`).
pub fn details_button() -> bool {
    crate::capabilities::record_now() == iptv_presentation::capabilities::RecordNow::Direct
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
/// It never replaces the player surface.
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
    if ui.recordings.with_untracked(|s| s.busy)
        || !untracked(crate::capabilities::record_now).offered()
    {
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
    let target = iptv_core::parental::Target::live(request.source, request.channel.clone());
    crate::parental_view::request(ui, target, move || start(ui, request));
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
/// when a recording started after the program began.
fn occurrence_job(request: &Request, jobs: Vec<Job>) -> Option<Job> {
    jobs.into_iter()
        .filter(|job| {
            job.source_current
                && job.status.active()
                && job.request.source == request.source
                && job.request.channel == request.channel
                && job.request.end == request.end
                && match job.status {
                    Status::Scheduled => job.request.start == request.start,
                    _ => (request.start..request.end).contains(&job.request.start),
                }
        })
        .max_by_key(|job| job.status == Status::Recording)
}

fn channel_action(ui: Ui, origin: Origin) -> Option<(MenuItem, Option<Job>)> {
    if ui.player_surface != 0 || !available(origin.channel) {
        return None;
    }
    let schedule = crate::schedule();
    let program = schedule.program_at(origin.channel, origin.program_start);
    let mut request = request(origin)?;
    request.start = crate::guide::unix_of(&*schedule, program.start);
    if let Some(job) = occurrence_job(&request, crate::runtime().recording_snapshot()) {
        let item = if job.status == Status::Recording {
            MenuItem::StopRecording
        } else {
            MenuItem::CancelRecording
        };
        return Some((item, Some(job)));
    }
    // Only a program on now records, where the device offers Record now. A
    // recording already running or carried over can still be stopped or
    // canceled above.
    (program.airing(crate::time::now_minute()) == crate::guide::Airing::Live
        && crate::capabilities::record_now().offered())
    .then_some((MenuItem::RecordNow, None))
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
        _ => return false,
    }
    true
}

pub fn remember(ui: Ui, origin: Origin) {
    let request = request(origin);
    ui.recordings
        .update(|s| s.remember_context(origin, request));
}
/// Starts recording `request` now, once the parental check has passed.
fn start(ui: Ui, mut request: Request) {
    // A parental PIN may have been entered since the request was made: the
    // capture starts now, or the runtime would count the wait as a gap.
    let now = crate::time::wall_unix();
    request.start = request.start.max(now);
    if let Err(message) = request.validate(now) {
        crate::settings_view::notify(tr(message));
        ui.recordings.update(|s| s.show_error(message));
        return;
    }
    ui.recordings.update(|s| s.begin_start(request.clone()));
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let result = owner.schedule_recording(request).await;
        match &result {
            // What can stop it, said as it starts (#275).
            Ok(_) => {
                crate::settings_view::notify(&crate::capture_limits::start_notice());
                crate::host::request_recording_notification_permission();
            }
            Err(error) => crate::settings_view::notify(translated_reason(
                error
                    .unavailable_reason()
                    .unwrap_or("Recording action failed"),
            )),
        }
        let jobs = if result.is_ok() {
            owner.recording_jobs().await.ok()
        } else {
            None
        };
        let error = result.as_ref().err().map(|error| {
            error
                .unavailable_reason()
                .filter(|reason| iptv_presentation::i18n::is_cataloged(reason))
                .unwrap_or("Recording action failed")
        });
        ui.recordings.update(|s| s.finish_start(jobs, error));
        reload(ui);
    });
}
fn select_job(ui: Ui, id: JobId) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    ui.recordings.update(|s| s.select_job(id));
}
pub(super) fn return_to_player(ui: Ui) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    close(ui);
    ui.mode.apply(crate::mode::Action::EnterFullscreen);
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
    } else if matches!(job.status, Status::Completed | Status::Partial) && job.bytes > 0 {
        ui.recordings.update(|s| s.focus_for_action(id));
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
        ui.recordings.update(|s| s.focus_for_action(id));
        execute(ui, job, false, action);
    }
}

fn request_delete(ui: Ui, id: JobId) {
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    ui.recordings.update(|s| s.request_delete(id));
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
    match job.status {
        Status::Scheduled => vec!["Cancel recording"],
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
    }
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
    if action == "Delete recording" && !confirm {
        request_delete(ui, job.id);
        return;
    }
    if ui.recordings.with_untracked(|s| s.busy) {
        return;
    }
    ui.recordings.update(Screen::begin_action);
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
        let plays = matches!(
            action,
            "Play recording" | "Resume recording" | "Play from beginning"
        );
        let result = if plays {
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
        let outcome = match result {
            Ok(()) if plays => ActionOutcome::Playing,
            Ok(()) => ActionOutcome::Changed,
            Err(_) if action == "Delete recording" => {
                ActionOutcome::Failed("Recording could not be deleted")
            }
            Err(_) => ActionOutcome::Failed(
                "Recording action failed; check whether its file is available",
            ),
        };
        let mut failure_notice = false;
        ui.recordings
            .update(|s| failure_notice = s.finish_action(jobs, outcome));
        if failure_notice {
            crate::settings_view::notify(tr("Recording action failed"));
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
                ui.recordings.update(Screen::toggle_delete_choice);
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
    if ui.recordings.with_untracked(|s| s.actions) {
        let length = ui
            .recordings
            .with_untracked(|s| s.focused().map(|j| actions(ui, j)).map_or(0, |a| a.len()));
        match action {
            Action::MoveUp => ui.recordings.update(Screen::previous_action),
            Action::MoveDown => ui.recordings.update(|s| s.next_action(length)),
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
        tr("Recordings continue when you close the player. \
             Recordings use local storage and are excluded from backups.")
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

fn library_content(ui: Ui, size: Scale) -> View {
    view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .pb_safe_area()
        .bg(rgb(style::WINDOW))
        .child(
            view()
                .flex_row()
                .items_center()
                .px(move || size.px(24.0))
                .min_h(move || size.px(72.0))
                .child(when(
                    move || ui.recordings.with(|s| !s.actions),
                    move || style::action(ui, size, tr("Close"), || false, move || close(ui)),
                ))
                .child(view().flex_1())
                .child(ui_kit::title(size, tr("Recordings"))),
        )
        .child(rows::now_playing(ui, size))
        .child(when(
            move || ui.recordings.with(|s| !s.retired.is_empty()),
            move || retired_schedules(ui, size),
        ))
        .child(
            ui_kit::body(size, move || message(ui))
                .text_color(rgb(ui_kit::ERROR))
                .px(move || size.px(24.0)),
        )
        .child(when(
            move || ui.recordings.with(|s| s.visible().next().is_none()),
            move || {
                ui_kit::body(
                    size,
                    tr("No recordings here. Choose a program on now in the guide, then Record."),
                )
                .px(move || size.px(24.0))
                .py(move || size.px(24.0))
            },
        ))
        .child(recording_list(ui, size))
        .child(
            ui_kit::caption(size, requirements())
                .px(move || size.px(24.0))
                .py(move || size.px(16.0)),
        )
}

/// What the upgrade that retired scheduled recording canceled. Read-only
/// text: nothing here takes focus or acts.
fn retired_schedules(ui: Ui, size: Scale) -> View {
    view()
        .test_id("retired-schedules")
        .px(move || size.px(24.0))
        .py(move || size.px(12.0))
        .gap(move || size.px(6.0))
        .child(ui_kit::label(size, tr("Scheduled recording was removed")))
        .child(ui_kit::caption(
            size,
            tr("These schedules were canceled. Recordings already made are kept."),
        ))
        .child(ui_kit::caption(size, move || {
            ui.recordings.with(|s| {
                // A bounded list keeps the recordings below it in view.
                let mut lines = s
                    .retired
                    .iter()
                    .take(RETIRED_LINES)
                    .map(retired_line)
                    .collect::<Vec<_>>();
                if s.retired.len() > RETIRED_LINES {
                    lines.push(format!("+{}", s.retired.len() - RETIRED_LINES));
                }
                lines.join("\n")
            })
        }))
}

const RETIRED_LINES: usize = 8;

fn retired_line(schedule: &iptv_core::recording::RetiredSchedule) -> String {
    use iptv_core::recording::Repeat;
    format!(
        "{} · {} · {} {} · {}",
        schedule.title,
        schedule.channel_name,
        crate::time::date_only(schedule.start),
        crate::time::time_only(schedule.start),
        match schedule.repeat {
            Repeat::Once => tr("Once"),
            Repeat::Daily => tr("Every day"),
            Repeat::Weekly => tr("Every week"),
            Repeat::Series => tr("Every matching program"),
        }
    )
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
                .child(library(ui, size))
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
        let job = Job {
            id: JobId(7),
            source_current: true,
            series_start: None,
            request: request.clone(),
            status: Status::Scheduled,
            bytes: 0,
            message: String::new(),
        };
        let mut next = job.clone();
        next.id = JobId(8);
        next.request.start += 3600;
        next.request.end += 3600;
        let mut other_playlist = job.clone();
        other_playlist.id = JobId(9);
        other_playlist.request.source = 2;
        assert_eq!(
            occurrence_job(&request, vec![next.clone(), other_playlist, job.clone()])
                .map(|job| job.id),
            Some(JobId(7)),
        );
        assert_eq!(occurrence_job(&request, vec![next]), None);
        // Record now starts after the program began; it still matches.
        let mut started = job;
        started.status = Status::Recording;
        started.request.start += 1200;
        assert_eq!(
            occurrence_job(&request, vec![started]).map(|job| job.id),
            Some(JobId(7))
        );
    }
}
