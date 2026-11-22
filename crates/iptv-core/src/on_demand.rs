//! Resume points for retained media. Positions are source-relative milliseconds.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub completed: bool,
}
impl Progress {
    pub fn observed(position_ms: u64, duration_ms: Option<u64>, ended: bool) -> Self {
        let duration_ms = duration_ms.filter(|duration| *duration > 0);
        let position_ms = duration_ms.map_or(position_ms, |duration| position_ms.min(duration));
        let completed = ended
            || duration_ms.is_some_and(|duration| {
                position_ms > 0
                    && duration.saturating_sub(position_ms) <= (duration / 20).min(30_000)
            });
        Self {
            position_ms,
            duration_ms,
            completed,
        }
    }
    /// Finished media starts again; otherwise retain even a short saved position.
    pub fn resume_ms(self) -> u64 {
        if self.completed {
            0
        } else {
            self.duration_ms.map_or(self.position_ms, |duration| {
                self.position_ms.min(duration.saturating_sub(1))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Progress;

    #[test]
    fn short_media_and_finished_media_have_sensible_resume_points() {
        assert_eq!(Progress::observed(2300, None, false).resume_ms(), 2300);
        assert_eq!(
            Progress::observed(80_000, Some(100_000), false).resume_ms(),
            80_000
        );
        assert_eq!(
            Progress::observed(99_000, Some(100_000), false).resume_ms(),
            0
        );
        assert_eq!(Progress::observed(5000, None, true).resume_ms(), 0);
        assert_eq!(
            Progress::observed(u64::MAX, Some(100), false).position_ms,
            100
        );
    }
}
