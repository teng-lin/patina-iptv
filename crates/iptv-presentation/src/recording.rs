//! The recordings screen.

use iptv_core::recording::{Job, JobId, Request, Status};

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
    pub progress: Vec<(JobId, iptv_core::on_demand::Progress)>,
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
    pub retired: Vec<iptv_core::recording::RetiredSchedule>,
    pub busy: bool,
    pub read_generation: u64,
    /// The content selected when an action menu opened; catalog row indices
    /// may change while that menu remains visible.
    pub context: Option<(crate::menu::Origin, Request)>,
}

impl Screen {
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
