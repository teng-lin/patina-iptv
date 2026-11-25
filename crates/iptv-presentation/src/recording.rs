//! The recordings screen.

use iptv_core::{
    on_demand::Progress,
    recording::{Job, JobId, Request, RetiredSchedule, Status},
};

use crate::menu::Origin;

pub fn status_label(status: Status) -> &'static str {
    match status {
        Status::Scheduled => "Scheduled",
        Status::Recording => "Recording",
        Status::Completed => "Completed",
        Status::Partial => "Partial recording",
        Status::Failed => "Failed",
        Status::Canceled => "Canceled",
        Status::Missed => "Missed",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub open: bool,
    pub jobs: Vec<Job>,
    pub progress: Vec<(JobId, Progress)>,
    pub focus: Option<JobId>,
    pub actions: bool,
    pub action: usize,
    pub confirm_delete: bool,
    pub delete_selected: bool,
    /// Immediate capture being admitted; separate from library presentation.
    pub starting: Option<Request>,
    pub error: Option<String>,
    /// Schedules the retirement of scheduled recording canceled, listed
    /// read-only until the viewer has seen them.
    pub retired: Vec<RetiredSchedule>,
    pub busy: bool,
    read_generation: u64,
    /// The content selected when an action menu opened; catalog row indices
    /// may change while that menu remains visible.
    pub context: Option<(Origin, Request)>,
}

/// How a recording action completed, independently of host playback and notices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionOutcome {
    Playing,
    Changed,
    Failed(&'static str),
}

impl Screen {
    /// Open the library with its last focused recording.
    pub fn open(&mut self) {
        self.open = true;
        self.error = None;
        self.actions = false;
        self.confirm_delete = false;
    }

    /// Close the library if it is safe to leave the current action.
    pub fn close(&mut self) {
        if !self.busy {
            self.open = false;
            self.confirm_delete = false;
        }
    }

    /// Dismiss the active confirmation, action menu, or library in order.
    pub fn back(&mut self) {
        if self.busy {
            return;
        }
        self.error = None;
        if self.confirm_delete {
            self.confirm_delete = false;
        } else if self.actions {
            self.actions = false;
        } else {
            self.open = false;
        }
    }

    /// Start a new reload, superseding older reads.
    pub fn begin_reload(&mut self) -> u64 {
        self.read_generation += 1;
        self.read_generation
    }

    /// Apply only the latest read, keeping the visible jobs if the read failed.
    pub fn finish_reload(
        &mut self,
        generation: u64,
        jobs: Result<Vec<Job>, &'static str>,
        progress: Vec<(JobId, Progress)>,
        retired: Vec<RetiredSchedule>,
    ) {
        if self.read_generation != generation {
            return;
        }
        self.retired = retired;
        match jobs {
            Ok(jobs) => {
                self.progress = progress;
                self.reload(jobs);
            }
            Err(error) => self.show_error(error),
        }
    }

    /// Dismiss the retired schedules currently shown by the library.
    pub fn dismiss_retired(&mut self) {
        self.retired.clear();
    }

    /// Remember the recording request captured by the now-open channel menu.
    pub fn remember_context(&mut self, origin: Origin, request: Option<Request>) {
        self.context = request.map(|request| (origin, request));
    }

    /// Discard a menu context whose catalog row is no longer valid.
    pub fn invalidate_context(&mut self, shifted: impl FnOnce(usize) -> bool) {
        if self
            .context
            .as_ref()
            .is_some_and(|(origin, _)| shifted(origin.channel))
        {
            self.context = None;
        }
    }

    /// Set a catalog message for the library to render.
    pub fn show_error(&mut self, message: &str) {
        self.error = Some(message.into());
    }

    /// Start a recording action and clear the error from a previous action.
    pub fn begin_action(&mut self) {
        self.busy = true;
        self.error = None;
    }

    /// Mark an immediate capture as awaiting admission.
    pub fn begin_start(&mut self, request: Request) {
        self.begin_action();
        self.starting = Some(request);
    }

    /// Complete an immediate capture without dismissing the library's controls.
    pub fn finish_start(&mut self, jobs: Option<Vec<Job>>, error: Option<&str>) {
        self.busy = false;
        self.starting = None;
        if let Some(jobs) = jobs {
            self.reload(jobs);
        }
        if let Some(error) = error {
            self.show_error(error);
        }
    }

    /// Focus a recording before running a direct action on it.
    pub fn focus_for_action(&mut self, id: JobId) {
        self.focus = Some(id);
        self.confirm_delete = false;
    }

    /// Open the recording's action menu with its first command focused.
    pub fn select_job(&mut self, id: JobId) {
        self.focus_for_action(id);
        self.actions = true;
        self.action = 0;
        self.error = None;
    }

    /// Ask for confirmation before deleting this recording.
    pub fn request_delete(&mut self, id: JobId) {
        self.focus = Some(id);
        self.actions = false;
        self.confirm_delete = true;
        self.delete_selected = false;
        self.error = None;
    }

    /// Finish a library action; return whether its failure needs a host notice.
    pub fn finish_action(&mut self, jobs: Option<Vec<Job>>, outcome: ActionOutcome) -> bool {
        self.busy = false;
        if let Some(jobs) = jobs {
            self.reload(jobs);
        }
        self.confirm_delete = false;
        match outcome {
            ActionOutcome::Playing => {
                self.actions = false;
                self.open = false;
            }
            ActionOutcome::Changed => self.actions = false,
            ActionOutcome::Failed(message) => {
                if !self.open {
                    return true;
                }
                self.show_error(message);
            }
        }
        false
    }

    /// Change the focused confirmation choice.
    pub fn toggle_delete_choice(&mut self) {
        self.delete_selected = !self.delete_selected;
    }

    /// Move to the previous command in the action menu.
    pub fn previous_action(&mut self) {
        self.action = self.action.saturating_sub(1);
    }

    /// Move to the next available command in the action menu.
    pub fn next_action(&mut self, length: usize) {
        self.action = (self.action + 1).min(length.saturating_sub(1));
    }

    pub fn active_on(&self, source: i64, channel: &str, now: i64) -> Option<&Job> {
        self.jobs.iter().find(|j| {
            j.source_current
                && j.status.active()
                && j.request.source == source
                && j.request.channel == channel
                && j.request.start <= now
                && (j.status == Status::Recording || j.request.end > now)
        })
    }
    pub fn visible(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter()
    }
    pub fn focused(&self) -> Option<&Job> {
        self.visible().find(|j| Some(j.id) == self.focus)
    }
    pub fn reload(&mut self, jobs: Vec<Job>) {
        self.jobs = jobs;
        if self.focused().is_none() {
            let first = self.visible().next().map(|j| j.id);
            self.focus = first;
            self.actions = false;
            self.confirm_delete = false;
        }
    }
    pub fn move_focus(&mut self, step: isize) {
        let ids: Vec<_> = self.visible().map(|j| j.id).collect();
        let at = ids
            .iter()
            .position(|id| Some(*id) == self.focus)
            .unwrap_or(0);
        self.focus = ids
            .get(
                at.saturating_add_signed(step)
                    .min(ids.len().saturating_sub(1)),
            )
            .copied();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(id: i64) -> Job {
        Job {
            id: JobId(id),
            source_current: true,
            series_start: None,
            request: Request {
                source: 1,
                channel: "synthetic".into(),
                channel_name: "Synthetic channel".into(),
                title: "Generated program".into(),
                start: 100,
                end: 200,
            },
            status: Status::Scheduled,
            bytes: 0,
            message: String::new(),
        }
    }
    #[test]
    fn record_toggle_only_matches_the_current_channel_and_incarnation() {
        let mut s = Screen {
            jobs: vec![job(1)],
            ..Default::default()
        };
        assert!(s.active_on(1, "synthetic", 99).is_none());
        assert_eq!(s.active_on(1, "synthetic", 150).unwrap().id, JobId(1));
        assert!(s.active_on(2, "synthetic", 150).is_none());
        assert!(s.active_on(1, "other", 150).is_none());
        s.jobs[0].source_current = false;
        assert!(s.active_on(1, "synthetic", 150).is_none());
        s.jobs[0].source_current = true;
        s.jobs[0].status = Status::Recording;
        assert!(s.active_on(1, "synthetic", 201).is_some());
        s.jobs[0].status = Status::Completed;
        assert!(s.active_on(1, "synthetic", 150).is_none());
    }

    #[test]
    fn a_failed_latest_read_keeps_jobs_and_ignores_an_older_success() {
        let mut screen = Screen::default();
        screen.reload(vec![job(1)]);
        let older = screen.begin_reload();
        let latest = screen.begin_reload();
        screen.finish_reload(latest, Err("Failed"), Vec::new(), Vec::new());
        screen.finish_reload(older, Ok(vec![job(2)]), Vec::new(), Vec::new());
        assert_eq!(screen.focus, Some(JobId(1)));
        assert_eq!(screen.jobs, vec![job(1)]);
        assert_eq!(screen.error.as_deref(), Some("Failed"));
    }

    #[test]
    fn library_navigation_keeps_the_confirmation_until_the_action_finishes() {
        let mut screen = Screen::default();
        screen.open();
        screen.reload(vec![job(1), job(2)]);
        screen.select_job(JobId(2));
        screen.next_action(2);
        screen.next_action(2);
        assert_eq!(screen.action, 1);
        screen.previous_action();
        assert_eq!(screen.action, 0);
        screen.request_delete(JobId(2));
        screen.toggle_delete_choice();
        screen.begin_action();
        screen.close();
        screen.back();
        assert!(screen.open && screen.confirm_delete && screen.delete_selected);
        let notice = screen.finish_action(None, ActionOutcome::Failed("Failed"));
        assert!(!notice);
        assert!(!screen.confirm_delete);
        assert_eq!(screen.focus, Some(JobId(2)));
        assert_eq!(screen.error.as_deref(), Some("Failed"));
        screen.back();
        assert!(!screen.open);
        assert!(screen.finish_action(None, ActionOutcome::Failed("Failed")));
    }

    #[test]
    fn immediate_capture_completion_preserves_library_controls_and_context() {
        let mut screen = Screen::default();
        screen.open();
        let origin = Origin {
            channel: 3,
            program_start: 10,
        };
        let request = job(1).request;
        screen.remember_context(origin, Some(request.clone()));
        screen.invalidate_context(|channel| channel == 2);
        assert_eq!(
            screen.context.as_ref().map(|(origin, _)| origin.channel),
            Some(3)
        );
        screen.begin_start(request);
        screen.reload(vec![job(1)]);
        screen.select_job(JobId(1));
        screen.finish_start(Some(vec![job(1), job(2)]), None);
        assert!(screen.open && screen.actions);
        assert!(!screen.busy && screen.starting.is_none());
        assert_eq!(screen.focus, Some(JobId(1)));
        screen.invalidate_context(|channel| channel == 3);
        assert!(screen.context.is_none());
    }

    #[test]
    fn refresh_restores_focus_by_job_identity_across_status_changes() {
        let mut s = Screen::default();
        s.reload(vec![job(1), job(2)]);
        s.move_focus(1);
        s.reload(vec![job(2), job(1)]);
        assert_eq!(s.focus, Some(JobId(2)));
        s.actions = true;
        s.reload(vec![job(1)]);
        assert_eq!(s.focus, Some(JobId(1)));
        assert!(!s.actions);
        let mut completed = job(1);
        completed.status = Status::Completed;
        s.reload(vec![completed, job(3)]);
        assert_eq!(s.focus, Some(JobId(1)));
        assert_eq!(s.visible().count(), 2);
    }
}
