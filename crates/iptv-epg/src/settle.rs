//! The rules that turn one EPG channel's raw programmes into its schedule,
//! shared by [`crate::SourceBuilder`] (in memory) and stores that stream a
//! feed to disk, so both keep exactly the same programmes:
//!
//! 1. As each programme arrives ([`ScheduleRules::admit`]): one whose stop
//!    is not after its start, or that ended before the retention cutoff, is
//!    dropped.
//! 2. Of several programmes with the same start, the longest is kept (a
//!    missing stop counts as shortest; equal ones: the first in input),
//!    see [`ScheduleRules::replaces`].
//! 3. Per channel, in start order ([`ScheduleRules::settle`]): a missing
//!    stop becomes the next programme's start, a programme running into the
//!    next one is cut at the next start; then one still without a stop, one
//!    that ended before the cutoff, or one that does not fit the compact
//!    slot (more than ~136 years from the channel's first start or long) is
//!    dropped.

use crate::store::{IngestReport, StoreOptions};
use crate::time::{DAY, UnixTime};

/// What becomes of one programme when its channel is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// Kept, ending at `stop`.
    Keep { stop: UnixTime },
    /// No stop and no following programme.
    NoStop,
    /// Ended at or before the retention cutoff.
    Retention,
    /// Too far from the channel's first start, or too long.
    Overflow,
}

/// Retention and the schedule rules of one source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduleRules {
    cutoff: Option<UnixTime>,
}

impl ScheduleRules {
    /// `now` is only used for retention (`opts.past_days_to_keep`).
    pub fn new(opts: StoreOptions, now: UnixTime) -> Self {
        Self {
            cutoff: opts
                .past_days_to_keep
                .map(|d| now.saturating_sub(i64::from(d) * DAY)),
        }
    }

    /// Whether a programme arriving with `start`/`stop` is kept for now;
    /// a dropped one is counted in `report` (`programmes_in` counts every
    /// call).
    pub fn admit(
        &self,
        start: UnixTime,
        stop: Option<UnixTime>,
        report: &mut IngestReport,
    ) -> bool {
        report.programmes_in += 1;
        let Some(stop) = stop else {
            return true;
        };
        if stop <= start {
            report.dropped_bad_duration += 1;
            false
        } else if self.cutoff.is_some_and(|c| stop <= c) {
            report.dropped_retention += 1;
            false
        } else {
            true
        }
    }

    /// Whether a programme stopping at `new` replaces an earlier one with
    /// the same start stopping at `old` (strictly longer; `None` is
    /// shortest).
    pub fn replaces(new: Option<UnixTime>, old: Option<UnixTime>) -> bool {
        new.unwrap_or(UnixTime::MIN) > old.unwrap_or(UnixTime::MIN)
    }

    /// Settles one channel. `rows`: its admitted programmes as `(start,
    /// stop)`, sorted by start, one per start ([`Self::replaces`] chose
    /// it). `out` hears each row's fate by index; `report` counts trims,
    /// drops, `kept` and `channels_with_programmes`.
    pub fn settle(
        &self,
        rows: &[(UnixTime, Option<UnixTime>)],
        report: &mut IngestReport,
        mut out: impl FnMut(usize, Settled),
    ) {
        let base = rows.first().map_or(0, |r| r.0);
        let mut kept = 0u64;
        for (i, &(start, stop)) in rows.iter().enumerate() {
            let next = rows.get(i + 1).map(|r| r.0);
            let stop = match (stop, next) {
                (None, next) => next,
                (Some(stop), Some(next)) if stop > next => {
                    report.overlaps_trimmed += 1;
                    Some(next)
                }
                (stop, _) => stop,
            };
            let fate = match stop {
                None => {
                    report.dropped_no_stop += 1;
                    Settled::NoStop
                }
                Some(stop) if self.cutoff.is_some_and(|c| stop <= c) => {
                    report.dropped_retention += 1;
                    Settled::Retention
                }
                Some(stop)
                    if u32::try_from(i128::from(start) - i128::from(base)).is_err()
                        || u32::try_from(i128::from(stop) - i128::from(start)).is_err() =>
                {
                    report.dropped_bad_duration += 1;
                    Settled::Overflow
                }
                Some(stop) => {
                    kept += 1;
                    Settled::Keep { stop }
                }
            };
            out(i, fate);
        }
        report.kept += kept;
        report.channels_with_programmes += u64::from(kept > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(rules: ScheduleRules, rows: &[(i64, Option<i64>)]) -> (Vec<Settled>, IngestReport) {
        let mut report = IngestReport::default();
        let mut fates = Vec::new();
        rules.settle(rows, &mut report, |i, f| {
            assert_eq!(i, fates.len());
            fates.push(f);
        });
        (fates, report)
    }

    #[test]
    fn fills_trims_and_drops() {
        let rules = ScheduleRules::new(
            StoreOptions {
                store_descriptions: true,
                past_days_to_keep: Some(0),
            },
            150,
        );
        let (fates, r) = settle(
            rules,
            &[
                (0, Some(100)),
                (100, None),
                (200, Some(400)),
                (300, Some(350)),
                (500, None),
            ],
        );
        assert_eq!(
            fates,
            [
                Settled::Retention,
                Settled::Keep { stop: 200 },
                Settled::Keep { stop: 300 },
                Settled::Keep { stop: 350 },
                Settled::NoStop,
            ]
        );
        assert_eq!(
            (
                r.kept,
                r.overlaps_trimmed,
                r.dropped_retention,
                r.dropped_no_stop
            ),
            (3, 1, 1, 1)
        );
        assert_eq!(r.channels_with_programmes, 1);
    }

    #[test]
    fn admit_and_replace() {
        let rules = ScheduleRules::new(StoreOptions::default(), 0);
        let mut r = IngestReport::default();
        assert!(rules.admit(10, None, &mut r));
        assert!(!rules.admit(10, Some(10), &mut r));
        assert!(rules.admit(10, Some(11), &mut r));
        assert_eq!((r.programmes_in, r.dropped_bad_duration), (3, 1));
        assert!(ScheduleRules::replaces(Some(5), None));
        assert!(!ScheduleRules::replaces(None, Some(5)));
        assert!(!ScheduleRules::replaces(Some(5), Some(5)));
        assert!(!ScheduleRules::replaces(None, None));
        let (fates, r) = settle(
            rules,
            &[(0, Some(1)), (i64::from(u32::MAX) + 1, Some(1 << 33))],
        );
        assert_eq!(fates[1], Settled::Overflow);
        assert_eq!(r.dropped_bad_duration, 1);
    }
}
