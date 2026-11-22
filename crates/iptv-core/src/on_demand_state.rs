//! Seek state and acknowledgements for the on-demand player.

use crate::on_demand::Progress;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seek {
    pub id: u64,
    pub ms: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub seekable: bool,
    pub seek_id: u64,
}
impl Timeline {
    /// Old observations queued before the latest seek cannot move the thumb back.
    pub fn observe(
        &mut self,
        seek_id: u64,
        position_ms: u64,
        duration_ms: Option<u64>,
        seekable: bool,
    ) -> bool {
        if seek_id < self.seek_id {
            return false;
        }
        self.duration_ms = duration_ms.filter(|duration| *duration > 0);
        self.position_ms = self
            .duration_ms
            .map_or(position_ms, |duration| position_ms.min(duration));
        self.seekable = seekable;
        true
    }
    pub fn seek_to(&mut self, ms: u64) -> Option<Seek> {
        if !self.seekable {
            return None;
        }
        let ms = self.duration_ms.map_or(ms, |duration| ms.min(duration));
        self.seek_id = self.seek_id.wrapping_add(1).max(1);
        self.position_ms = ms;
        Some(Seek {
            id: self.seek_id,
            ms,
        })
    }
    pub fn step(&mut self, delta_ms: i64) -> Option<Seek> {
        self.seek_to(self.position_ms.saturating_add_signed(delta_ms))
    }
    pub fn progress(self, ended: bool) -> Progress {
        Progress::observed(self.position_ms, self.duration_ms, ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_observations_cannot_undo_a_seek_and_repeated_seeks_have_new_identity() {
        let mut timeline = Timeline::default();
        timeline.observe(0, 1000, Some(100_000), true);
        let first = timeline.seek_to(70_000).unwrap();
        assert!(!timeline.observe(0, 1200, Some(100_000), true));
        assert_eq!(timeline.position_ms, 70_000);
        let second = timeline.seek_to(70_000).unwrap();
        assert_ne!(first.id, second.id);
        assert!(timeline.observe(second.id, 70_100, Some(100_000), true));
    }
    #[test]
    fn unknown_duration_can_seek_when_supported_and_bounds_saturate() {
        let mut timeline = Timeline {
            seekable: true,
            ..Default::default()
        };
        assert_eq!(timeline.step(i64::MIN).unwrap().ms, 0);
        timeline.observe(timeline.seek_id, 5000, Some(10_000), true);
        assert_eq!(timeline.step(i64::MAX).unwrap().ms, 10_000);
        timeline.seekable = false;
        assert!(timeline.seek_to(0).is_none());
    }
}
