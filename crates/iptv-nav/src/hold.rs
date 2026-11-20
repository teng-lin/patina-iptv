//! Presses tied to the surface they began on.
//!
//! A key with a long binding waits before it acts ([`PressDetector`]). If
//! the surface that owns input ([`Context`]) changes while it waits (a tap
//! plays a channel full screen, a prompt opens, a timer takes the guide to
//! full screen), the press is dropped: neither its long nor its short action
//! runs on a surface the viewer did not press it on. Keys that act at once
//! (no long binding) act on the surface of the moment, so a held arrow keeps
//! scrolling whatever opens.

use crate::{Context, Phase, Press, PressDetector, RemoteKey};

/// What a key event amounts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Act on `press` of `key` in `context`.
    Act {
        key: RemoteKey,
        press: Press,
        context: Context,
    },
    /// A waiting press resolved after its surface changed: nothing acts.
    Dropped {
        key: RemoteKey,
        press: Press,
        began: Context,
        now: Context,
    },
}

/// A [`PressDetector`] that remembers where a waiting press began.
#[derive(Debug)]
pub struct HeldPresses {
    detector: PressDetector,
    /// The held key and, while it waits, the surface it went down on.
    held: Option<(RemoteKey, Option<Context>)>,
}

impl HeldPresses {
    pub fn new(threshold_ms: u64) -> Self {
        HeldPresses {
            detector: PressDetector::new(threshold_ms),
            held: None,
        }
    }

    pub fn threshold_ms(&self) -> u64 {
        self.detector.threshold_ms()
    }

    /// When the waiting press becomes long ([`PressDetector::long_deadline_ms`]).
    pub fn long_deadline_ms(&self) -> Option<u64> {
        self.detector.long_deadline_ms()
    }

    /// Feeds one key event at `now_ms` on surface `context`; `has_long`:
    /// `key` has a long binding there.
    pub fn feed(
        &mut self,
        context: Context,
        key: RemoteKey,
        phase: Phase,
        now_ms: u64,
        has_long: bool,
    ) -> Option<Decision> {
        let began = match phase {
            Phase::Down => self.start(key, context, has_long),
            // A repeat without its press starts a hold, as the detector does.
            Phase::Repeat => match self.held {
                Some((held, began)) if held == key => began,
                _ => self.start(key, context, has_long),
            },
            Phase::Up => match self.held {
                Some((held, began)) if held == key => {
                    self.held = None;
                    began
                }
                _ => None,
            },
        };
        let (key, press) = self.detector.feed(key, phase, now_ms, has_long)?;
        Some(decide(key, press, began, context))
    }

    /// The long-press timer ([`PressDetector::poll`]), on surface `context`.
    pub fn poll(&mut self, context: Context, now_ms: u64) -> Option<Decision> {
        let (key, press) = self.detector.poll(now_ms)?;
        let began = self.held.and_then(|(_, began)| began);
        Some(decide(key, press, began, context))
    }

    fn start(&mut self, key: RemoteKey, context: Context, has_long: bool) -> Option<Context> {
        let began = has_long.then_some(context);
        self.held = Some((key, began));
        began
    }
}

impl Default for HeldPresses {
    fn default() -> Self {
        HeldPresses::new(crate::DEFAULT_LONG_PRESS_MS)
    }
}

fn decide(key: RemoteKey, press: Press, began: Option<Context>, now: Context) -> Decision {
    match began {
        Some(began) if began != now => Decision::Dropped {
            key,
            press,
            began,
            now,
        },
        _ => Decision::Act {
            key,
            press,
            context: now,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keymap;
    use Context::{Controls, Guide, Menu, Player};
    use RemoteKey::{Down, Ok};

    fn act(key: RemoteKey, press: Press, context: Context) -> Option<Decision> {
        Some(Decision::Act {
            key,
            press,
            context,
        })
    }

    /// Feeds `key` as the app does: `has_long` from the keymap in `context`.
    fn feed(
        h: &mut HeldPresses,
        keys: &Keymap,
        context: Context,
        key: RemoteKey,
        phase: Phase,
        at: u64,
    ) -> Option<Decision> {
        h.feed(context, key, phase, at, keys.has_long(context, key))
    }

    #[test]
    fn long_ok_fires_once_through_repeats_and_its_release_does_nothing() {
        let keys = Keymap::defaults();
        let mut h = HeldPresses::new(400);
        assert_eq!(feed(&mut h, &keys, Player, Ok, Phase::Down, 0), None);
        for at in [50, 100, 150, 200, 250, 300, 350] {
            assert_eq!(feed(&mut h, &keys, Player, Ok, Phase::Repeat, at), None);
        }
        assert_eq!(
            feed(&mut h, &keys, Player, Ok, Phase::Repeat, 400),
            act(Ok, Press::Long, Player)
        );
        // The long action opened the controls: later repeats and the
        // release, now on Controls, act on nothing there.
        for at in [450, 500, 550] {
            assert_eq!(feed(&mut h, &keys, Controls, Ok, Phase::Repeat, at), None);
        }
        assert_eq!(feed(&mut h, &keys, Controls, Ok, Phase::Up, 600), None);
        assert_eq!(h.poll(Controls, 1_000), None);
    }

    #[test]
    fn rapid_taps_each_act_once_on_release() {
        let keys = Keymap::defaults();
        let mut h = HeldPresses::new(400);
        for tap in 0..5u64 {
            let at = tap * 120;
            assert_eq!(feed(&mut h, &keys, Player, Ok, Phase::Down, at), None);
            assert_eq!(
                feed(&mut h, &keys, Player, Ok, Phase::Up, at + 60),
                act(Ok, Press::Short, Player),
                "tap {tap}"
            );
            assert_eq!(h.poll(Player, at + 400), None, "no late long");
        }
        // Keys without a long binding act on press and on each repeat.
        assert_eq!(
            feed(&mut h, &keys, Player, Down, Phase::Down, 1_000),
            act(Down, Press::Short, Player)
        );
        assert_eq!(
            feed(&mut h, &keys, Player, Down, Phase::Repeat, 1_050),
            act(Down, Press::Short, Player)
        );
        assert_eq!(feed(&mut h, &keys, Player, Down, Phase::Up, 1_100), None);
    }

    #[test]
    fn a_surface_change_during_the_hold_drops_the_waiting_press() {
        let keys = Keymap::defaults();
        // OK held in the guide (Long OK: its menu); a tap plays a channel
        // full screen before the press resolves.
        for resolve in ["timer", "repeat", "short release", "late release"] {
            let mut h = HeldPresses::new(400);
            assert_eq!(feed(&mut h, &keys, Guide, Ok, Phase::Down, 0), None);
            let decision = match resolve {
                "timer" => h.poll(Player, 400),
                "repeat" => feed(&mut h, &keys, Player, Ok, Phase::Repeat, 400),
                "short release" => feed(&mut h, &keys, Player, Ok, Phase::Up, 200),
                _ => feed(&mut h, &keys, Player, Ok, Phase::Up, 450),
            };
            let press = if resolve == "short release" {
                Press::Short
            } else {
                Press::Long
            };
            assert_eq!(
                decision,
                Some(Decision::Dropped {
                    key: Ok,
                    press,
                    began: Guide,
                    now: Player
                }),
                "{resolve}"
            );
            // Nothing follows: no short on release, no second long.
            assert_eq!(feed(&mut h, &keys, Player, Ok, Phase::Up, 700), None);
            assert_eq!(h.poll(Player, 800), None);
        }
    }

    #[test]
    fn a_surface_that_comes_back_before_the_press_resolves_keeps_it() {
        let keys = Keymap::defaults();
        let mut h = HeldPresses::new(400);
        assert_eq!(feed(&mut h, &keys, Guide, Ok, Phase::Down, 0), None);
        // A menu opened and closed again mid-hold.
        assert_eq!(feed(&mut h, &keys, Menu, Ok, Phase::Repeat, 200), None);
        assert_eq!(h.poll(Guide, 400), act(Ok, Press::Long, Guide));
        assert_eq!(feed(&mut h, &keys, Guide, Ok, Phase::Up, 500), None);
    }

    #[test]
    fn a_new_press_takes_its_own_surface() {
        let keys = Keymap::defaults();
        let mut h = HeldPresses::new(400);
        assert_eq!(feed(&mut h, &keys, Guide, Ok, Phase::Down, 0), None);
        // A missed release: the next press starts afresh on Player.
        assert_eq!(feed(&mut h, &keys, Player, Ok, Phase::Down, 100), None);
        assert_eq!(
            feed(&mut h, &keys, Player, Ok, Phase::Up, 150),
            act(Ok, Press::Short, Player)
        );
        // A repeat whose press went elsewhere (focus arrived mid-hold).
        assert_eq!(feed(&mut h, &keys, Player, Ok, Phase::Repeat, 200), None);
        assert_eq!(h.poll(Player, 600), act(Ok, Press::Long, Player));
        // A stray release of another key changes nothing.
        assert_eq!(feed(&mut h, &keys, Player, Down, Phase::Up, 650), None);
        assert_eq!(feed(&mut h, &keys, Player, Ok, Phase::Up, 700), None);
    }
}
