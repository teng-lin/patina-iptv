//! Portable recording jobs and recurrence. Times are UTC seconds; hosts own I/O.

pub const MAX_DURATION: i64 = 24 * 60 * 60;
pub const MAX_JOBS: usize = 1000;
pub const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// What the native host actually registered for the next recording wake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WakeCapability {
    #[default]
    AwakeOnly,
    Exact,
    Inexact,
    Denied,
}
impl WakeCapability {
    pub fn notice(self) -> Option<&'static str> {
        match self {
            Self::AwakeOnly | Self::Exact => None,
            Self::Inexact => Some(
                "Precise background starts are unavailable. Allow alarms and reminders in Android settings.",
            ),
            Self::Denied => Some(
                "The recording wake could not be registered. Check Recordings settings before relying on unattended starts.",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Scheduled,
    Recording,
    Completed,
    Partial,
    Failed,
    Canceled,
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
            Self::Canceled => "Canceled",
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

/// A repeating intent is separate from its materialized recording jobs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Once,
    Daily,
    Weekly,
    Series,
}
impl Repeat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Once => "Once",
            Self::Daily => "Every day",
            Self::Weekly => "Every week",
            Self::Series => "Every matching program",
        }
    }
    pub fn advance(self, step: i64) -> Self {
        let values = [Self::Once, Self::Daily, Self::Weekly, Self::Series];
        let index = values.iter().position(|value| *value == self).unwrap_or(0) as i64;
        values[(index + step).rem_euclid(values.len() as i64) as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RuleId(pub i64);

/// A guide occurrence is identified by its original start, independently of
/// padding or a record-now capture that begins after the program started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecurrenceTiming {
    pub program_start: i64,
    pub program_end: i64,
    pub lead_seconds: u32,
    pub trail_seconds: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: RuleId,
    pub request: Request,
    pub repeat: Repeat,
    /// Wall-clock timestamp encoded as seconds since the local Unix epoch.
    /// Hosts convert each occurrence separately so DST does not shift its hour.
    pub local_start: i64,
    pub enabled: bool,
    pub lead_seconds: u32,
    pub trail_seconds: u32,
}
impl Rule {
    /// The first daily/weekly local window whose end is after `local_now`.
    /// Series rules are matched against the current EPG by the native store.
    pub fn local_window(&self, local_now: i64) -> Option<(i64, i64)> {
        let interval = match self.repeat {
            Repeat::Daily => 86400,
            Repeat::Weekly => 7 * 86400,
            _ => return None,
        };
        let duration = self.request.end.checked_sub(self.request.start)?;
        if !self.enabled || !(1..=MAX_DURATION).contains(&duration) {
            return None;
        }
        let elapsed = local_now.saturating_sub(self.local_start).max(0);
        let mut start = self
            .local_start
            .checked_add((elapsed / interval).checked_mul(interval)?)?;
        if start.checked_add(duration)? <= local_now {
            start = start.checked_add(interval)?;
        }
        Some((start, start.checked_add(duration)?))
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
    fn recurrence_uses_wall_clock_windows_and_skips_elapsed_occurrences() {
        let mut rule = Rule {
            id: RuleId(1),
            request: job(1).request,
            repeat: Repeat::Daily,
            local_start: 100,
            enabled: true,
            lead_seconds: 0,
            trail_seconds: 0,
        };
        assert_eq!(rule.local_window(150), Some((100, 200)));
        assert_eq!(rule.local_window(200), Some((86500, 86600)));
        assert_eq!(rule.local_window(172950), Some((172900, 173000)));
        rule.repeat = Repeat::Weekly;
        assert_eq!(rule.local_window(200), Some((604900, 605000)));
        rule.enabled = false;
        assert_eq!(rule.local_window(0), None);
        rule.enabled = true;
        rule.local_start = i64::MAX - 10;
        assert_eq!(rule.local_window(0), None);
    }

    #[test]
    fn invalid_windows_and_ended_programs_are_rejected() {
        let mut r = job(1).request;
        assert!(r.validate(150).is_ok());
        assert!(r.validate(200).is_err());
        r.end = r.start;
        assert!(r.validate(0).is_err());
        r.end = i64::MAX;
        assert!(r.validate(0).is_err());
    }
}
