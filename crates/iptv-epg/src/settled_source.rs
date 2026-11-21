//! Building an [`EpgSource`] from programmes that are already settled: a
//! stored guide read back in start order per channel, whose rows passed
//! [`crate::ScheduleRules`] when they were written (sorted, one per start,
//! each with a stop at or before the next start).
//!
//! [`crate::SourceBuilder`] holds a 24-byte raw row per programme until
//! [`crate::SourceBuilder::finish`] sorts and settles them into 16-byte
//! slots, so reading a guide back peaked at about two and a half times what
//! it keeps. Settled rows go straight into their slots: memory grows only
//! by what the source keeps (and one channel's spare capacity).

use std::collections::HashMap;

use crate::intern::Interner;
use crate::matching::FeedId;
use crate::store::{EpgSource, IngestReport, ProgramData, Schedule, Slot, intern_program};

/// Builds an [`EpgSource`] one channel at a time from settled programmes.
///
/// ```
/// use iptv_epg::*;
/// let mut b = SettledBuilder::new(FeedId(1), 10, false);
/// b.channel("one.example");
/// for (start, stop, title) in [(1000, 2000, "News"), (2000, 2600, "Weather")] {
///     b.push(&ProgramData { start, stop: Some(stop), title, ..Default::default() });
/// }
/// let src = b.finish();
/// let p = src.schedule("one.example").unwrap().at(2100).unwrap();
/// assert_eq!((p.title(), p.start(), p.stop()), ("Weather", 2000, 2600));
/// ```
pub struct SettledBuilder {
    id: FeedId,
    priority: i32,
    descriptions: bool,
    strings: Interner,
    extras: Vec<u32>,
    schedules: Vec<Schedule>,
    by_epg_id: HashMap<Box<str>, u32>,
    /// The schedule being filled.
    current: Option<usize>,
    report: IngestReport,
}

impl SettledBuilder {
    /// `descriptions`: keep them in memory (else they are left out).
    pub fn new(id: FeedId, priority: i32, descriptions: bool) -> Self {
        SettledBuilder {
            id,
            priority,
            descriptions,
            strings: Interner::new(),
            extras: Vec::new(),
            schedules: Vec::new(),
            by_epg_id: HashMap::new(),
            current: None,
            report: IngestReport::default(),
        }
    }

    /// The programmes that follow belong to EPG channel `channel` (a
    /// channel named again continues its schedule).
    pub fn channel(&mut self, channel: &str) {
        self.end_channel();
        let next = self.schedules.len();
        let at = *self
            .by_epg_id
            .entry(channel.into())
            .or_insert_with(|| next as u32) as usize;
        if at == next {
            self.schedules.push(Schedule::default());
        }
        self.current = Some(at);
    }

    /// Adds the next programme of the current channel. One that is not
    /// settled after the ones before it (no stop, not after its start,
    /// starting before the previous one ends, or beyond the compact
    /// slot's range) is dropped and counted; returns whether it was kept.
    pub fn push(&mut self, p: &ProgramData<'_>) -> bool {
        let r = &mut self.report;
        r.programs_in += 1;
        let Some(at) = self.current else {
            r.dropped_no_stop += 1;
            return false;
        };
        let Some(stop) = p.stop.filter(|&s| s > p.start) else {
            r.dropped_bad_duration += 1;
            return false;
        };
        let sched = &mut self.schedules[at];
        let (base, after) = match sched.slots.last() {
            Some(last) => (
                sched.base,
                i128::from(sched.base) + i128::from(last.start) + i128::from(last.dur),
            ),
            None => (p.start, i128::from(i64::MIN)),
        };
        if i128::from(p.start) < after {
            r.dropped_duplicate += 1;
            return false;
        }
        let (Ok(start), Ok(dur)) = (
            u32::try_from(i128::from(p.start) - i128::from(base)),
            u32::try_from(i128::from(stop) - i128::from(p.start)),
        ) else {
            r.dropped_bad_duration += 1;
            return false;
        };
        let (title, extra) =
            intern_program(&mut self.strings, &mut self.extras, p, self.descriptions);
        if sched.slots.is_empty() {
            sched.base = base;
            r.channels_with_programs += 1;
        }
        sched.slots.push(Slot {
            start,
            dur,
            title,
            extra,
        });
        r.kept += 1;
        true
    }

    /// Trims the finished channel's spare capacity.
    fn end_channel(&mut self) {
        if let Some(at) = self.current.take() {
            self.schedules[at].slots.shrink_to_fit();
        }
    }

    pub fn finish(mut self) -> EpgSource {
        self.end_channel();
        self.schedules.shrink_to_fit();
        EpgSource::from_parts(
            self.id,
            self.priority,
            self.strings,
            self.extras,
            self.schedules,
            self.by_epg_id,
            self.report,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SourceBuilder, StoreOptions};

    fn data(start: i64, stop: i64, title: &str) -> ProgramData<'_> {
        ProgramData {
            start,
            stop: Some(stop),
            title,
            desc: Some("About it"),
            season: Some(2),
            episode: Some(5),
            ..Default::default()
        }
    }

    #[test]
    fn settled_rows_read_back_as_the_source_builder_keeps_them() {
        let rows = [(1000, 1600, "A"), (1600, 2000, "B"), (2500, 3000, "C")];
        let mut settled = SettledBuilder::new(FeedId(1), 5, false);
        let mut built = SourceBuilder::new(FeedId(1), 5, StoreOptions::default(), 0);
        settled.channel("x");
        for &(a, b, t) in &rows {
            assert!(settled.push(&data(a, b, t)));
            built.push_data("x", &data(a, b, t).as_stored(false));
        }
        let (settled, built) = (settled.finish(), built.finish());
        let both = [settled.schedule("x").unwrap(), built.schedule("x").unwrap()];
        for t in [999, 1000, 1700, 2200, 2999, 3000] {
            let found = both.map(|s| s.at(t).map(|p| p.data()));
            assert_eq!(found[0], found[1], "at {t}");
        }
        let p = both[0].at(1000).unwrap();
        assert_eq!(p.desc(), None, "descriptions left out");
        assert_eq!(p.season_episode(), (Some(2), Some(5)));
        assert_eq!(settled.priority(), 5);
        assert_eq!(settled.report().kept, 3);
    }

    #[test]
    fn compact_boundaries_and_full_width_timestamps_do_not_overflow() {
        let mut b = SettledBuilder::new(FeedId(1), 0, false);
        b.channel("synthetic");
        assert!(b.push(&data(0, 1, "A")));
        assert!(b.push(&data(i64::from(u32::MAX) - 1, i64::from(u32::MAX) + 2, "B")));
        assert!(!b.push(&data(i64::from(u32::MAX) + 2, i64::from(u32::MAX) + 3, "C")));
        let mut extremes = SettledBuilder::new(FeedId(1), 0, false);
        extremes.channel("synthetic");
        assert!(!extremes.push(&data(i64::MIN, i64::MAX, "Too long")));
        assert!(extremes.push(&data(i64::MIN, i64::MIN + 1, "First")));
        assert!(!extremes.push(&data(i64::MAX - 1, i64::MAX, "Too far")));
    }

    #[test]
    fn rows_out_of_order_or_without_duration_are_dropped() {
        let mut b = SettledBuilder::new(FeedId(1), 0, true);
        assert!(!b.push(&data(0, 10, "before any channel")));
        b.channel("x");
        assert!(b.push(&data(1000, 2000, "A")));
        assert!(!b.push(&data(1500, 2500, "overlaps A")));
        assert!(!b.push(&data(3000, 3000, "no duration")));
        assert!(b.push(&data(2000, 2100, "B")));
        b.channel("y");
        assert!(b.push(&data(500, 600, "Y")));
        // Named again: continues after B.
        b.channel("x");
        assert!(b.push(&data(2100, 2200, "C")));
        let src = b.finish();
        let x = src.schedule("x").unwrap();
        assert_eq!(x.len(), 3);
        assert_eq!(x.at(1000).unwrap().desc(), Some("About it"));
        assert_eq!(src.schedule("y").unwrap().at(550).unwrap().title(), "Y");
        let r = src.report();
        assert_eq!((r.programs_in, r.kept, r.channels_with_programs), (7, 4, 2));
        assert_eq!((r.dropped_duplicate, r.dropped_bad_duration), (1, 1));
    }
}
