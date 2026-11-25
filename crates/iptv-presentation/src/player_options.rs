//! Player options chosen over full-screen video: picture size (per channel
//! or for all channels) and the sleep timer, plus the small chooser model
//! their translucent lists use. Option sets follow the choosers observed
//! in the reference UX; times are the caller's monotonic
//! milliseconds (no clock access here).

use std::collections::BTreeMap;

/// How the video fills the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PictureSize {
    /// Fit inside the screen, keeping the video's shape.
    #[default]
    Normal,
    /// Stretch to the whole screen.
    Full,
    /// Fill the screen keeping the shape, cropping what spills over.
    Zoom,
    /// Zoom a letterboxed wide film so its picture fills the width.
    CinemaZoom,
    /// Smaller than Normal, with a margin all round.
    Reduced,
    /// Force a 4:3 picture.
    Ratio4x3,
    /// Force a 16:9 picture.
    Ratio16x9,
    /// Force a 2.35:1 picture.
    Ratio235,
}

impl PictureSize {
    pub const ALL: [PictureSize; 8] = [
        PictureSize::Normal,
        PictureSize::Full,
        PictureSize::Zoom,
        PictureSize::CinemaZoom,
        PictureSize::Reduced,
        PictureSize::Ratio4x3,
        PictureSize::Ratio16x9,
        PictureSize::Ratio235,
    ];

    /// The stable key the player component understands (and settings store).
    pub fn key(self) -> &'static str {
        match self {
            PictureSize::Normal => "normal",
            PictureSize::Full => "full",
            PictureSize::Zoom => "zoom",
            PictureSize::CinemaZoom => "cinema",
            PictureSize::Reduced => "reduced",
            PictureSize::Ratio4x3 => "4:3",
            PictureSize::Ratio16x9 => "16:9",
            PictureSize::Ratio235 => "2.35:1",
        }
    }
}

/// The picture size for all channels, with per-channel choices on top.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PictureSettings {
    pub all: PictureSize,
    /// Opaque stable identities scoped by source and content, never list offsets.
    pub per_channel: BTreeMap<String, PictureSize>,
}

impl PictureSettings {
    /// Missing live identity (such as on-demand playback) uses the global size.
    pub fn for_channel(&self, channel: Option<&str>) -> PictureSize {
        channel
            .and_then(|key| self.per_channel.get(key))
            .copied()
            .unwrap_or(self.all)
    }

    /// Chooses a size for a live identity, or the global default without one.
    pub fn set_channel(&mut self, channel: Option<&str>, size: PictureSize) {
        let Some(channel) = channel else {
            self.all = size;
            return;
        };
        if size == self.all {
            self.per_channel.remove(channel);
        } else {
            self.per_channel.insert(channel.to_owned(), size);
        }
    }

    /// "Apply to all channels": the channel's size becomes everyone's, and
    /// per-channel choices are cleared.
    pub fn apply_to_all(&mut self, channel: Option<&str>) {
        self.all = self.for_channel(channel);
        self.per_channel.clear();
    }
}

/// Sleep timer choices in minutes (after Off).
pub const SLEEP_MINUTES: [u32; 8] = [15, 30, 45, 60, 90, 120, 180, 240];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SleepTimer {
    ends_at_ms: Option<u64>,
    minutes: Option<u32>,
}

impl SleepTimer {
    pub fn set(&mut self, minutes: u32, now_ms: u64) {
        self.minutes = Some(minutes);
        self.ends_at_ms = Some(now_ms + u64::from(minutes) * 60_000);
    }

    pub fn cancel(&mut self) {
        *self = SleepTimer::default();
    }

    /// The chosen duration, if running.
    pub fn minutes(&self) -> Option<u32> {
        self.minutes
    }

    pub fn ends_at_ms(&self) -> Option<u64> {
        self.ends_at_ms
    }

    /// Whole minutes left, rounded up (0 once due); `None` when off.
    pub fn remaining_minutes(&self, now_ms: u64) -> Option<u64> {
        self.ends_at_ms
            .map(|end| end.saturating_sub(now_ms).div_ceil(60_000))
    }

    /// Whether the timer has run out (it then turns itself off).
    pub fn fire_if_due(&mut self, now_ms: u64) -> bool {
        match self.ends_at_ms {
            Some(end) if now_ms >= end => {
                self.cancel();
                true
            }
            _ => false,
        }
    }
}

/// A short list chooser: focus moves, the current choice is marked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chooser {
    pub len: usize,
    pub focus: usize,
    /// The entry currently in effect (marked in the list).
    pub selected: usize,
}

impl Chooser {
    /// Focus starts on the current choice.
    pub fn new(len: usize, selected: usize) -> Chooser {
        let selected = selected.min(len.saturating_sub(1));
        Chooser {
            len,
            focus: selected,
            selected,
        }
    }

    /// Replaces action entries, keeping the focused action when it remains.
    pub fn replace_actions<T: PartialEq>(&mut self, before: &[T], after: &[T]) {
        let focus = before
            .get(self.focus)
            .and_then(|focused| after.iter().position(|entry| entry == focused))
            .unwrap_or(self.focus);
        *self = Self::new(after.len(), focus);
    }

    pub fn move_focus(&mut self, step: isize) {
        let last = self.len.saturating_sub(1) as isize;
        self.focus = (self.focus as isize + step).clamp(0, last) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_focus_stays_with_the_action_and_falls_back_when_it_leaves() {
        let mut chooser = Chooser::new(3, 1);
        chooser.replace_actions(&["Tracks", "Record", "Display"], &["Record", "Display"]);
        assert_eq!((chooser.focus, chooser.len), (0, 2));
        chooser.replace_actions(&["Record", "Display"], &["Display"]);
        assert_eq!((chooser.focus, chooser.len), (0, 1));
    }

    #[test]
    fn picture_size_is_per_channel_until_applied_to_all() {
        let mut p = PictureSettings::default();
        p.set_channel(Some("source:3:channel"), PictureSize::Zoom);
        assert_eq!(p.for_channel(Some("source:3:channel")), PictureSize::Zoom);
        assert_eq!(p.for_channel(Some("source:4:channel")), PictureSize::Normal);
        p.apply_to_all(Some("source:3:channel"));
        assert_eq!(
            (p.for_channel(Some("source:4:channel")), p.per_channel.len()),
            (PictureSize::Zoom, 0)
        );
        p.set_channel(Some("source:4:channel"), PictureSize::Zoom);
        assert!(
            p.per_channel.is_empty(),
            "same as everyone's: no override kept"
        );
    }

    #[test]
    fn missing_live_identity_uses_global_without_aliasing_a_channel() {
        let mut p = PictureSettings::default();
        p.set_channel(Some("source:1:same-name"), PictureSize::Zoom);
        p.set_channel(Some("source:2:same-name"), PictureSize::Full);
        assert_eq!(p.for_channel(None), PictureSize::Normal);
        p.set_channel(None, PictureSize::Reduced);
        assert_eq!(p.for_channel(None), PictureSize::Reduced);
        assert_eq!(p.for_channel(Some("source:1:same-name")), PictureSize::Zoom);
        assert_eq!(p.for_channel(Some("source:2:same-name")), PictureSize::Full);
        p.apply_to_all(None);
        assert_eq!(
            p.for_channel(Some("source:1:same-name")),
            PictureSize::Reduced
        );
    }

    #[test]
    fn picture_keys_are_unique() {
        let mut keys: Vec<_> = PictureSize::ALL.iter().map(|p| p.key()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), PictureSize::ALL.len());
    }

    #[test]
    fn the_sleep_timer_counts_down_and_fires_once() {
        let mut t = SleepTimer::default();
        assert_eq!(t.remaining_minutes(0), None);
        t.set(15, 1_000);
        assert_eq!(t.minutes(), Some(15));
        assert_eq!(t.remaining_minutes(1_000), Some(15));
        assert_eq!(t.remaining_minutes(1_000 + 14 * 60_000 + 1), Some(1));
        assert!(!t.fire_if_due(1_000 + 15 * 60_000 - 1));
        assert!(t.fire_if_due(1_000 + 15 * 60_000));
        assert_eq!(t, SleepTimer::default(), "off after firing");
        assert!(!t.fire_if_due(u64::MAX));
    }

    #[test]
    fn choosers_start_on_the_current_choice_and_clamp() {
        let mut c = Chooser::new(9, 2);
        assert_eq!(c.focus, 2);
        c.move_focus(-5);
        assert_eq!(c.focus, 0);
        c.move_focus(50);
        assert_eq!(c.focus, 8);
        assert_eq!(Chooser::new(3, 9).selected, 2);
    }
}
