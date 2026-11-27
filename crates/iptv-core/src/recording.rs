//! Portable recording jobs. Times are UTC seconds; hosts own I/O.

pub const MAX_DURATION: i64 = 24 * 60 * 60;
pub const MAX_JOBS: usize = 1000;
pub const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// The recordings library's budget: the recordings kept, plus [`MAX_BYTES`]
/// for each capture still running (less what it has written).
pub const LIBRARY_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// What a capture is taken to write each second when checking free space
/// before it starts: 8 Mbit/s, a high-bitrate HD channel. Lower-rate
/// channels need less; a 4K channel can need more.
pub const ESTIMATED_BYTES_PER_SECOND: u64 = 1_000_000;

/// The space a capture of `seconds` is taken to need: at
/// [`ESTIMATED_BYTES_PER_SECOND`], at most [`MAX_BYTES`].
pub fn estimated_bytes(seconds: i64) -> u64 {
    (seconds.max(0) as u64)
        .saturating_mul(ESTIMATED_BYTES_PER_SECOND)
        .min(MAX_BYTES)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceRefusal {
    LibraryFull,
    InsufficientFreeSpace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestRefusal {
    UnavailableChannel,
    NameTooLong,
    InvalidDuration,
    AlreadyEnded,
}

/// Checks the storage a capture of `seconds` needs before it starts.
/// `library`: what the recordings library holds and reserves now (see
/// [`LIBRARY_BYTES`]); `free`: the free space where it writes, `None` where
/// the host cannot tell (then only the write itself can fail).
pub fn check_space(seconds: i64, library: u64, free: Option<u64>) -> Result<(), SpaceRefusal> {
    if library.saturating_add(MAX_BYTES) > LIBRARY_BYTES {
        return Err(SpaceRefusal::LibraryFull);
    }
    if free.is_some_and(|free| free < estimated_bytes(seconds)) {
        return Err(SpaceRefusal::InsufficientFreeSpace);
    }
    Ok(())
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
    pub fn validate(&self, now: i64) -> Result<(), RequestRefusal> {
        if self.source <= 0 || self.channel.is_empty() || self.channel.len() > 4096 {
            return Err(RequestRefusal::UnavailableChannel);
        }
        if self.title.len() > 4096 || self.channel_name.len() > 4096 {
            return Err(RequestRefusal::NameTooLong);
        }
        if self.start < 0 || self.end <= self.start || self.end - self.start > MAX_DURATION {
            return Err(RequestRefusal::InvalidDuration);
        }
        if self.end <= now {
            return Err(RequestRefusal::AlreadyEnded);
        }
        Ok(())
    }
}

/// How a rule an earlier version stored repeated. Scheduled recording is
/// retired: the store still reads such rules (all disabled by the upgrade)
/// to list what it canceled and to restore older backups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Once,
    Daily,
    Weekly,
    Series,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RuleId(pub i64);

/// A scheduled job or repeating rule an earlier version accepted, which the
/// upgrade that retired scheduled recording canceled. Shown read-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetiredSchedule {
    pub channel_name: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    /// [`Repeat::Once`] for a single job.
    pub repeat: Repeat,
}

/// A repeating rule as an earlier version stored it (see [`Repeat`]).
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: JobId,
    /// A read-time observation for views; native services recheck before I/O.
    pub source_current: bool,
    /// Unshifted airing start of a linked series occurrence, if present.
    pub series_start: Option<i64>,
    pub request: Request,
    pub status: Status,
    pub bytes: u64,
    /// Fixed application messages only; never transport errors or provider replies.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_starts_only_with_room_in_the_library_and_on_the_disk() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let hour = 3600;
        // An hour at the estimated rate is 3.6 GB; a day is capped.
        assert_eq!(estimated_bytes(hour), 3_600_000_000);
        assert_eq!(estimated_bytes(MAX_DURATION), MAX_BYTES);
        assert_eq!(estimated_bytes(-5), 0);
        assert_eq!(check_space(hour, 0, Some(100 * GIB)), Ok(()));
        // A host that cannot tell its free space leaves it to the write.
        assert_eq!(check_space(hour, 0, None), Ok(()));
        // The new capture's reservation must fit the library's budget.
        assert_eq!(check_space(hour, LIBRARY_BYTES - MAX_BYTES, None), Ok(()));
        assert_eq!(
            check_space(hour, LIBRARY_BYTES - MAX_BYTES + 1, Some(100 * GIB)),
            Err(SpaceRefusal::LibraryFull)
        );
        // A full library is reported before a full disk.
        assert_eq!(
            check_space(hour, LIBRARY_BYTES, Some(0)),
            Err(SpaceRefusal::LibraryFull)
        );
        // The disk must hold the estimate for the requested length.
        assert_eq!(check_space(hour, 0, Some(3_600_000_000)), Ok(()));
        assert_eq!(
            check_space(hour, 0, Some(3_599_999_999)),
            Err(SpaceRefusal::InsufficientFreeSpace)
        );
        assert_eq!(check_space(600, 0, Some(600_000_000)), Ok(()));
    }

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
    fn invalid_windows_and_ended_programs_are_rejected() {
        let mut r = job(1).request;
        assert!(r.validate(150).is_ok());
        assert_eq!(r.validate(200), Err(RequestRefusal::AlreadyEnded));
        r.end = r.start;
        assert_eq!(r.validate(0), Err(RequestRefusal::InvalidDuration));
        r.end = i64::MAX;
        assert_eq!(r.validate(0), Err(RequestRefusal::InvalidDuration));
        r.end = 200;
        r.channel.clear();
        assert_eq!(r.validate(0), Err(RequestRefusal::UnavailableChannel));
        r.channel = "channel".into();
        r.channel_name = "x".repeat(4097);
        assert_eq!(r.validate(0), Err(RequestRefusal::NameTooLong));
    }
}
