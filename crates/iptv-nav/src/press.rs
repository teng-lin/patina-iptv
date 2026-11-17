//! Short / long press detection from key down, auto-repeat and release.
//!
//! A key with no long binding in the current context acts immediately on
//! press (and again on each auto-repeat, so holding a direction scrolls).
//! A key with a long binding defers: it becomes `Long` once held for the
//! threshold — detected on an auto-repeat (Android remotes), by [`poll`]
//! (IR remotes that never repeat), or on release if the timer was delayed —
//! and `Short` on an earlier release, so the
//! short action never fires before or alongside the long one.
//!
//! [`poll`]: PressDetector::poll

use crate::RemoteKey;

/// Android's default long-press timeout (`ViewConfiguration`), in ms.
pub const DEFAULT_LONG_PRESS_MS: u64 = 400;

/// A key event's phase as the platform reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    /// An auto-repeat of a held key.
    Repeat,
    Up,
}

/// What a press amounted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Short,
    Long,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    key: RemoteKey,
    since_ms: u64,
    /// Waiting to tell short from long (the key has a long binding).
    deferred: bool,
    long_fired: bool,
}

#[derive(Debug)]
pub struct PressDetector {
    threshold_ms: u64,
    held: Option<Held>,
}

impl PressDetector {
    pub fn new(threshold_ms: u64) -> Self {
        PressDetector {
            threshold_ms,
            held: None,
        }
    }

    pub fn threshold_ms(&self) -> u64 {
        self.threshold_ms
    }

    /// Feeds one key event at `now_ms`. `has_long` says whether `key` has a
    /// long binding in the current context. Returns the press to act on, if any.
    pub fn feed(
        &mut self,
        key: RemoteKey,
        phase: Phase,
        now_ms: u64,
        has_long: bool,
    ) -> Option<(RemoteKey, Press)> {
        match phase {
            Phase::Down => {
                // A new key ends whatever was held (a missed release).
                self.held = Some(Held {
                    key,
                    since_ms: now_ms,
                    deferred: has_long,
                    long_fired: false,
                });
                (!has_long).then_some((key, Press::Short))
            }
            Phase::Repeat => {
                let held = match self.held {
                    Some(h) if h.key == key => h,
                    // A repeat without its press (focus arrived mid-hold): start now.
                    _ => return self.feed(key, Phase::Down, now_ms, has_long),
                };
                if !held.deferred {
                    return Some((key, Press::Short));
                }
                self.long_if_due(now_ms)
            }
            Phase::Up => {
                if self.held.as_ref()?.key != key {
                    return None;
                }
                let held = self.held.take()?;
                if !held.deferred || held.long_fired {
                    return None;
                }
                // A repeat can precede the threshold by a millisecond and the
                // release can arrive before a busy host runs its timer.
                let press = if now_ms.saturating_sub(held.since_ms) >= self.threshold_ms {
                    Press::Long
                } else {
                    Press::Short
                };
                Some((key, press))
            }
        }
    }

    /// Timer fallback: call periodically (or once at `since + threshold`)
    /// while a key is held; fires `Long` for remotes that never auto-repeat.
    pub fn poll(&mut self, now_ms: u64) -> Option<(RemoteKey, Press)> {
        self.long_if_due(now_ms)
    }

    /// When the held key (if deferred and undecided) becomes long.
    /// Returns `None` if that deadline exceeds the `u64` timestamp range.
    pub fn long_deadline_ms(&self) -> Option<u64> {
        self.held
            .filter(|h| h.deferred && !h.long_fired)
            .and_then(|h| h.since_ms.checked_add(self.threshold_ms))
    }

    fn long_if_due(&mut self, now_ms: u64) -> Option<(RemoteKey, Press)> {
        let held = self.held.as_mut()?;
        if held.deferred
            && !held.long_fired
            && now_ms.saturating_sub(held.since_ms) >= self.threshold_ms
        {
            held.long_fired = true;
            return Some((held.key, Press::Long));
        }
        None
    }
}

impl Default for PressDetector {
    fn default() -> Self {
        PressDetector::new(DEFAULT_LONG_PRESS_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RemoteKey::{Down, Left, Ok};

    #[test]
    fn keys_without_long_bindings_act_on_press_and_repeat() {
        let mut d = PressDetector::default();
        assert_eq!(
            d.feed(Down, Phase::Down, 0, false),
            Some((Down, Press::Short))
        );
        assert_eq!(
            d.feed(Down, Phase::Repeat, 450, false),
            Some((Down, Press::Short))
        );
        assert_eq!(
            d.feed(Down, Phase::Repeat, 500, false),
            Some((Down, Press::Short))
        );
        assert_eq!(d.feed(Down, Phase::Up, 520, false), None);
    }

    #[test]
    fn a_quick_tap_of_a_key_with_a_long_binding_is_short_on_release() {
        let mut d = PressDetector::default();
        assert_eq!(d.feed(Ok, Phase::Down, 0, true), None);
        assert_eq!(d.feed(Ok, Phase::Up, 120, true), Some((Ok, Press::Short)));
    }

    #[test]
    fn auto_repeat_past_the_threshold_is_long_once_and_never_short() {
        let mut d = PressDetector::default();
        assert_eq!(d.feed(Ok, Phase::Down, 0, true), None);
        assert_eq!(
            d.feed(Ok, Phase::Repeat, 300, true),
            None,
            "before the threshold"
        );
        assert_eq!(
            d.feed(Ok, Phase::Repeat, 420, true),
            Some((Ok, Press::Long))
        );
        assert_eq!(d.feed(Ok, Phase::Repeat, 470, true), None);
        assert_eq!(d.feed(Ok, Phase::Up, 900, true), None);
    }

    #[test]
    fn ir_remotes_without_repeats_go_long_by_poll() {
        let mut d = PressDetector::default();
        d.feed(Left, Phase::Down, 1_000, true);
        assert_eq!(d.long_deadline_ms(), Some(1_400));
        assert_eq!(d.poll(1_399), None);
        assert_eq!(d.poll(1_400), Some((Left, Press::Long)));
        assert_eq!(d.long_deadline_ms(), None);
        assert_eq!(d.feed(Left, Phase::Up, 2_000, true), None);
    }

    #[test]
    fn a_new_key_replaces_a_held_one_and_stray_releases_are_ignored() {
        let mut d = PressDetector::default();
        d.feed(Ok, Phase::Down, 0, true);
        assert_eq!(
            d.feed(Down, Phase::Down, 50, false),
            Some((Down, Press::Short))
        );
        assert_eq!(d.feed(Ok, Phase::Up, 60, true), None);
        assert_eq!(d.feed(Left, Phase::Up, 70, false), None);
    }

    #[test]
    fn an_overlapping_release_preserves_the_new_keys_short_press() {
        let mut d = PressDetector::default();
        assert_eq!(d.feed(Ok, Phase::Down, 0, true), None);
        assert_eq!(d.feed(Left, Phase::Down, 50, true), None);
        assert_eq!(d.feed(Ok, Phase::Up, 60, true), None);
        assert_eq!(d.feed(Down, Phase::Up, 70, false), None);
        assert_eq!(d.long_deadline_ms(), Some(450));
        assert_eq!(
            d.feed(Left, Phase::Up, 120, true),
            Some((Left, Press::Short))
        );
        assert_eq!(d.poll(450), None);
        assert_eq!(d.feed(Left, Phase::Up, 460, true), None);
    }

    #[test]
    fn an_overlapping_release_preserves_long_press_detection() {
        for phase in [None, Some(Phase::Repeat), Some(Phase::Up)] {
            let mut d = PressDetector::default();
            d.feed(Ok, Phase::Down, 0, true);
            d.feed(Left, Phase::Down, 50, true);
            assert_eq!(d.feed(Ok, Phase::Up, 60, true), None);
            assert_eq!(d.poll(449), None);
            let press = match phase {
                None => d.poll(450),
                Some(phase) => d.feed(Left, phase, 450, true),
            };
            assert_eq!(press, Some((Left, Press::Long)), "{phase:?}");
            assert_eq!(d.long_deadline_ms(), None);
            assert_eq!(d.poll(460), None);
            assert_eq!(d.feed(Left, Phase::Up, 470, true), None);
        }
    }

    #[test]
    fn long_deadlines_handle_the_timestamp_limit() {
        for (threshold, since, deadline) in [
            (u64::MAX, 1, None),
            (400, u64::MAX - 399, None),
            (u64::MAX, 0, Some(u64::MAX)),
            (400, u64::MAX - 400, Some(u64::MAX)),
            (0, u64::MAX, Some(u64::MAX)),
        ] {
            let mut d = PressDetector::new(threshold);
            assert_eq!(d.long_deadline_ms(), None);
            assert_eq!(d.feed(Ok, Phase::Down, since, true), None);
            assert_eq!(d.long_deadline_ms(), deadline);
            if deadline.is_some() {
                assert_eq!(d.poll(u64::MAX), Some((Ok, Press::Long)));
                assert_eq!(d.feed(Ok, Phase::Up, u64::MAX, true), None);
            } else {
                assert_eq!(d.poll(u64::MAX), None);
                assert_eq!(
                    d.feed(Ok, Phase::Up, u64::MAX, true),
                    Some((Ok, Press::Short))
                );
            }
            assert_eq!(d.long_deadline_ms(), None);
        }
    }

    #[test]
    fn a_repeat_without_its_press_starts_a_hold() {
        let mut d = PressDetector::default();
        assert_eq!(
            d.feed(Down, Phase::Repeat, 10, false),
            Some((Down, Press::Short))
        );
        assert_eq!(d.feed(Ok, Phase::Repeat, 20, true), None);
        assert_eq!(d.poll(420), Some((Ok, Press::Long)));
    }
}
