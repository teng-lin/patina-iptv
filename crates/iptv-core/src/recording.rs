//! Portable recording jobs and screen state. Times are UTC seconds; hosts own I/O.

pub const MAX_DURATION: i64 = 24 * 60 * 60;
pub const MAX_JOBS: usize = 1000;
pub const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Scheduled,
    Recording,
    Completed,
    Partial,
    Failed,
    Cancelled,
    Missed,
}
impl Status {
    pub fn active(self) -> bool {
        matches!(self, Self::Scheduled | Self::Recording)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Scheduled => "Scheduled",
            Self::Recording => "Recording",
            Self::Completed => "Completed",
            Self::Partial => "Partial recording",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Missed => "Missed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub source: i64,
    pub channel: String,
    pub channel_name: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
}
impl Request {
    pub fn validate(&self, now: i64) -> Result<(), &'static str> {
        if self.source <= 0 || self.channel.is_empty() || self.channel.len() > 4096 {
            return Err("Choose an available channel");
        }
        if self.title.len() > 4096 || self.channel_name.len() > 4096 {
            return Err("Recording name is too long");
        }
        if self.start < 0 || self.end <= self.start || self.end - self.start > MAX_DURATION {
            return Err("Choose a recording duration between one second and 24 hours");
        }
        if self.end <= now {
            return Err("This recording would already have ended");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: JobId,
    /// A read-time observation for views; native services recheck before I/O.
    pub source_current: bool,
    pub request: Request,
    pub status: Status,
    pub bytes: u64,
    /// Fixed application messages only; never transport errors or provider replies.
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Active,
    Finished,
}
impl Filter {
    pub fn includes(self, job: &Job) -> bool {
        match self {
            Self::All => true,
            Self::Active => job.status.active(),
            Self::Finished => !job.status.active(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub open: bool,
    pub jobs: Vec<Job>,
    pub focus: Option<JobId>,
    pub filter: Filter,
    pub actions: bool,
    pub action: usize,
    pub confirm_delete: bool,
    pub error: Option<String>,
    pub draft: Option<Draft>,
    pub busy: bool,
    pub read_generation: u64,
    /// The content selected when an action menu opened; catalogue row indices
    /// may change while that menu remains visible.
    pub context: Option<(crate::menu::Origin, Request)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub request: Request,
    pub focus: usize,
    pub lead_minutes: i64,
    pub trail_minutes: i64,
    pub editing: Option<JobId>,
}
impl Draft {
    pub fn new(request: Request) -> Self {
        Self {
            request,
            focus: 5,
            lead_minutes: 0,
            trail_minutes: 0,
            editing: None,
        }
    }
    pub fn adjust(&mut self, row: usize, step: i64, now: i64) {
        match row {
            0 | 1 => {
                let delta = step * if row == 0 { 86400 } else { 900 };
                let duration = self.request.end - self.request.start;
                self.request.start = self.request.start.saturating_add(delta).max(now);
                self.request.end = self.request.start.saturating_add(duration);
            }
            2 => {
                self.request.end = self.request.start
                    + (self.request.end - self.request.start + step * 900).clamp(60, MAX_DURATION)
            }
            3 => self.lead_minutes = (self.lead_minutes + step).clamp(0, 60),
            4 => self.trail_minutes = (self.trail_minutes + step).clamp(0, 60),
            _ => {}
        }
    }
    pub fn padded(&self, now: i64) -> Request {
        let mut request = self.request.clone();
        request.start = (request.start - self.lead_minutes * 60).max(now);
        request.end += self.trail_minutes * 60;
        request
    }
}
impl Screen {
    pub fn visible(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|j| self.filter.includes(j))
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
    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        let jobs = std::mem::take(&mut self.jobs);
        self.reload(jobs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(id: i64) -> Job {
        Job {
            id: JobId(id),
            source_current: true,
            request: Request {
                source: 1,
                channel: "synthetic".into(),
                channel_name: "Synthetic channel".into(),
                title: "Generated programme".into(),
                start: 100,
                end: 200,
            },
            status: Status::Scheduled,
            bytes: 0,
            message: String::new(),
        }
    }
    #[test]
    fn invalid_windows_and_ended_programmes_are_rejected() {
        let mut r = job(1).request;
        assert!(r.validate(150).is_ok());
        assert!(r.validate(200).is_err());
        r.end = r.start;
        assert!(r.validate(0).is_err());
        r.end = i64::MAX;
        assert!(r.validate(0).is_err());
    }
    #[test]
    fn refresh_and_filters_restore_focus_by_job_identity() {
        let mut s = Screen::default();
        s.reload(vec![job(1), job(2)]);
        s.move_focus(1);
        s.reload(vec![job(2), job(1)]);
        assert_eq!(s.focus, Some(JobId(2)));
        s.actions = true;
        s.reload(vec![job(1)]);
        assert_eq!(s.focus, Some(JobId(1)));
        assert!(!s.actions);
        s.set_filter(Filter::Finished);
        assert_eq!(s.focus, None);
    }
}
