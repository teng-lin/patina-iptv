//! The recordings screen and its entry draft.

use iptv_core::recording::{Job, JobId, MAX_DURATION, Repeat, Request, Status};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub open: bool,
    pub jobs: Vec<Job>,
    pub progress: Vec<(JobId, iptv_core::on_demand::Progress)>,
    pub repeating: Vec<JobId>,
    pub focus: Option<JobId>,
    pub actions: bool,
    pub action: usize,
    pub confirm_delete: bool,
    pub delete_selected: bool,
    /// Immediate capture being admitted; separate from library presentation.
    pub starting: Option<Request>,
    pub error: Option<String>,
    pub background_notice: Option<String>,
    pub draft: Option<Draft>,
    pub busy: bool,
    pub read_generation: u64,
    /// The content selected when an action menu opened; catalog row indices
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
    pub repeat: Repeat,
    pub program_start: Option<i64>,
    pub program_end: Option<i64>,
    /// A player action starts immediately without opening the scheduling form.
    pub immediate: bool,
    pub entry: Option<Entry>,
    pub options_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryField {
    Day,
    Time,
    Duration,
    Name,
    LeadMinutes,
    TrailMinutes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub field: EntryField,
    pub text: String,
    pub error: Option<&'static str>,
}

impl Draft {
    pub const SAVE: usize = 7;
    pub const OPTIONS: usize = 8;
    pub fn new(request: Request) -> Self {
        Self {
            request,
            focus: Self::SAVE,
            lead_minutes: 0,
            trail_minutes: 0,
            editing: None,
            repeat: Repeat::Once,
            program_start: None,
            program_end: None,
            immediate: false,
            entry: None,
            options_open: false,
        }
    }
    /// Hidden options never receive remote focus. The order matches the form.
    pub fn move_field(&mut self, step: i32) {
        let rows: &[usize] = if self.options_open {
            &[0, 1, 2, 3, Self::OPTIONS, 4, 5, 6, Self::SAVE]
        } else {
            &[0, 1, 2, 3, Self::OPTIONS, Self::SAVE]
        };
        let at = rows.iter().position(|&row| row == self.focus).unwrap_or(0);
        self.focus = rows[(at as i32 + step).clamp(0, rows.len() as i32 - 1) as usize];
    }
    pub fn adjust(&mut self, row: usize, step: i64, now: i64) {
        match row {
            0 | 1 => {
                let delta = step * if row == 0 { 86400 } else { 900 };
                let duration = self.request.end - self.request.start;
                self.request.start = self.request.start.saturating_add(delta).max(now);
                self.request.end = self.request.start.saturating_add(duration);
                // An explicit date/time edit becomes the new nominal window.
                self.program_start = None;
                self.program_end = None;
            }
            2 => {
                let previous = self.request.end;
                self.request.end = self.request.start
                    + (self.request.end - self.request.start + step * 900).clamp(60, MAX_DURATION);
                if let Some(end) = self.program_end.as_mut() {
                    *end = end.saturating_add(self.request.end - previous);
                }
            }
            4 if self.editing.is_none() => self.repeat = self.repeat.advance(step),
            5 => self.lead_minutes = (self.lead_minutes + step).clamp(0, 60),
            6 => self.trail_minutes = (self.trail_minutes + step).clamp(0, 60),
            _ => {}
        }
    }
    pub fn shift_local_day(
        &mut self,
        step: i64,
        now: i64,
        local_start: i64,
        to_utc: impl Fn(i64) -> Option<i64>,
    ) -> Result<(), &'static str> {
        let start = step
            .checked_mul(86400)
            .and_then(|delta| local_start.checked_add(delta))
            .and_then(to_utc)
            .ok_or("Recording time unavailable")?
            .max(now);
        let duration = self.request.end - self.request.start;
        let end = start
            .checked_add(duration)
            .ok_or("Recording time unavailable")?;
        self.request.start = start;
        self.request.end = end;
        self.program_start = None;
        self.program_end = None;
        Ok(())
    }
    /// Exact time/duration entry is shared by remote keyboards and touch.
    /// Local-time conversion remains a host operation supplied by the caller.
    pub fn commit_entry(
        &mut self,
        now: i64,
        local_start: i64,
        to_utc: impl Fn(i64) -> Option<i64>,
    ) -> Result<(), &'static str> {
        let Some(entry) = &self.entry else {
            return Ok(());
        };
        let clock = |text: &str| -> Option<i64> {
            let (hour, minute) = text.trim().split_once(':')?;
            if hour.is_empty()
                || hour.len() > 2
                || minute.len() != 2
                || !hour
                    .bytes()
                    .chain(minute.bytes())
                    .all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let (hour, minute) = (hour.parse::<i64>().ok()?, minute.parse::<i64>().ok()?);
            (minute < 60).then_some(hour * 60 + minute)
        };
        match entry.field {
            EntryField::LeadMinutes | EntryField::TrailMinutes => {
                let minutes = entry
                    .text
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .filter(|m| (0..=60).contains(m))
                    .ok_or("Choose padding between 0 and 60 minutes")?;
                if entry.field == EntryField::LeadMinutes {
                    self.lead_minutes = minutes;
                } else {
                    self.trail_minutes = minutes;
                }
            }
            EntryField::Day => {
                let local = entry
                    .text
                    .parse::<i64>()
                    .map_err(|_| "Recording time unavailable")?;
                let start = to_utc(local)
                    .filter(|t| *t >= now || now.saturating_sub(*t) < 60)
                    .ok_or("Choose a start time that has not passed")?
                    .max(now);
                let duration = self.request.end - self.request.start;
                let end = start
                    .checked_add(duration)
                    .ok_or("Recording time unavailable")?;
                self.request.start = start;
                self.request.end = end;
                self.program_start = None;
                self.program_end = None;
            }
            EntryField::Name => {
                let name = entry.text.trim();
                if name.is_empty() {
                    return Err("Choose a recording name");
                }
                if name.len() > 4096 {
                    return Err("Recording name is too long");
                }
                self.request.title = name.to_owned();
            }
            EntryField::Time => {
                let minute = clock(&entry.text)
                    .filter(|m| *m < 1440)
                    .ok_or("Enter a time as HH:MM on a 24-hour clock")?;
                let local = local_start.div_euclid(86400) * 86400 + minute * 60;
                let start = to_utc(local)
                    .filter(|t| *t >= now || now.saturating_sub(*t) < 60)
                    .ok_or("Choose a start time that has not passed")?
                    .max(now);
                let duration = self.request.end - self.request.start;
                let end = start
                    .checked_add(duration)
                    .ok_or("Recording time unavailable")?;
                self.request.start = start;
                self.request.end = end;
                self.program_start = None;
                self.program_end = None;
            }
            EntryField::Duration => {
                let duration = clock(&entry.text)
                    .map(|m| m * 60)
                    .filter(|s| (60..=MAX_DURATION).contains(s))
                    .ok_or("Choose a recording duration between one second and 24 hours")?;
                let previous = self.request.end;
                self.request.end = self
                    .request
                    .start
                    .checked_add(duration)
                    .ok_or("Recording time unavailable")?;
                if let Some(end) = self.program_end.as_mut() {
                    *end += self.request.end - previous;
                }
            }
        }
        self.entry = None;
        Ok(())
    }
    pub fn padded(&self, now: i64) -> Request {
        let mut request = self.request.clone();
        request.start = (request.start - self.lead_minutes * 60).max(now);
        request.end += self.trail_minutes * 60;
        request
    }
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
    fn draft_focus_skips_collapsed_options_and_follows_the_visible_order() {
        let mut d = Draft::new(Request {
            source: 1,
            channel: "generated".into(),
            channel_name: "Generated".into(),
            title: "Generated program".into(),
            start: 1000,
            end: 2000,
        });
        d.move_field(-1);
        assert_eq!(d.focus, Draft::OPTIONS);
        d.move_field(-1);
        assert_eq!(d.focus, 3);
        d.options_open = true;
        d.move_field(1);
        assert_eq!(d.focus, Draft::OPTIONS);
        d.move_field(1);
        assert_eq!(d.focus, 4);
        d.move_field(1);
        d.move_field(1);
        d.move_field(1);
        assert_eq!(d.focus, Draft::SAVE);
    }

    #[test]
    fn exact_padding_entry_can_reduce_to_zero_and_rejects_out_of_range_values() {
        let request = Request {
            source: 1,
            channel: "generated".into(),
            channel_name: "Generated".into(),
            title: "Generated program".into(),
            start: 1000,
            end: 2000,
        };
        let mut d = Draft::new(request);
        for field in [EntryField::LeadMinutes, EntryField::TrailMinutes] {
            for text in ["5", "0"] {
                d.entry = Some(Entry {
                    field,
                    text: text.into(),
                    error: None,
                });
                d.commit_entry(500, 1000, Some).unwrap();
            }
            d.entry = Some(Entry {
                field,
                text: "61".into(),
                error: None,
            });
            assert_eq!(
                d.commit_entry(500, 1000, Some),
                Err("Choose padding between 0 and 60 minutes")
            );
            assert_eq!((d.lead_minutes, d.trail_minutes), (0, 0));
        }
    }

    #[test]
    fn exact_schedule_entry_keeps_the_date_duration_and_nominal_window_consistent() {
        let mut request = job(1).request;
        request.start = 86400 + 10 * 3600;
        request.end = request.start + 3600;
        let mut draft = Draft::new(request);
        draft.program_start = Some(draft.request.start);
        draft.program_end = Some(draft.request.end);
        draft.entry = Some(Entry {
            field: EntryField::Time,
            text: "12:05".into(),
            error: None,
        });
        // The host supplies local time at UTC+2; core does no platform I/O.
        draft
            .commit_entry(0, draft.request.start + 7200, |local| Some(local - 7200))
            .unwrap();
        assert_eq!(draft.request.start, 86400 + 10 * 3600 + 5 * 60);
        assert_eq!(draft.request.end - draft.request.start, 3600);
        assert_eq!(draft.program_start, None);
        draft.entry = Some(Entry {
            field: EntryField::Duration,
            text: "1:10".into(),
            error: None,
        });
        draft.commit_entry(0, 0, Some).unwrap();
        assert_eq!(draft.request.end - draft.request.start, 4200);
        draft.entry = Some(Entry {
            field: EntryField::Name,
            text: "  Generated schedule  ".into(),
            error: None,
        });
        draft.commit_entry(0, 0, Some).unwrap();
        assert_eq!(draft.request.title, "Generated schedule");
        assert!(draft.entry.is_none());
    }

    #[test]
    fn invalid_schedule_entry_retains_the_previous_request_and_can_be_corrected() {
        let mut draft = Draft::new(job(1).request);
        let request = draft.request.clone();
        for (field, text) in [
            (EntryField::Time, "24:00"),
            (EntryField::Time, "12:60"),
            (EntryField::Duration, "25:00"),
            (EntryField::Duration, "0:00"),
            (EntryField::Name, " "),
        ] {
            draft.entry = Some(Entry {
                field,
                text: text.into(),
                error: None,
            });
            assert!(draft.commit_entry(0, 0, Some).is_err());
            assert_eq!(draft.request, request);
            assert!(draft.entry.is_some());
        }
        draft.entry = Some(Entry {
            field: EntryField::Time,
            text: "02:30".into(),
            error: None,
        });
        // A missing spring-forward hour is rejected rather than shifted.
        assert!(draft.commit_entry(0, 0, |_| None).is_err());
        assert_eq!(draft.request, request);
    }
    #[test]
    fn shifting_calendar_days_preserves_the_local_hour_across_a_clock_change() {
        let mut request = job(1).request;
        request.start = 10 * 3600;
        request.end = request.start + 3600;
        let mut draft = Draft::new(request);
        draft
            .shift_local_day(1, 0, 12 * 3600, |local| Some(local - 3600))
            .unwrap();
        assert_eq!(draft.request.start, 86400 + 11 * 3600);
        assert_eq!(draft.request.end - draft.request.start, 3600);
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
