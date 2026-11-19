//! Cancelable tuning and inactivity policy for browsing live channels.
//! Callers supply a monotonic clock; stale timer callbacks are harmless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Guide,
    Overlay,
    List,
    Player,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub key: String,
    pub source: Option<i64>,
    pub surface: Surface,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub autoplay: bool,
    pub delay_ms: u64,
    pub fullscreen_ms: u64,
    pub stay: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Tune { target: Target, fullscreen: bool },
    Fullscreen,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowsePlayback {
    target: Option<Target>,
    policy: Policy,
    pending: Option<(u64, bool)>,
    fullscreen_at: Option<u64>,
}
impl BrowsePlayback {
    /// A different identity/surface/policy cancels every previous request.
    /// An ineligible surface (menus, future/past cells, fullscreen) cancels too.
    pub fn focus(&mut self, target: Option<Target>, policy: Policy, now: u64) {
        if self.target == target && self.policy == policy {
            return;
        }
        self.target = target;
        self.policy = policy;
        self.pending = None;
        self.fullscreen_at = None;
        if self.target.is_some() && policy.autoplay {
            self.pending = Some((now.saturating_add(policy.delay_ms), false));
        }
    }
    /// Explicit OK still debounces rapid choices. Replacing it never tunes
    /// the earlier channel, and automatic preview never closes the browser.
    pub fn select(&mut self, now: u64) {
        if self.target.is_some() {
            self.pending = Some((now.saturating_add(self.policy.delay_ms), !self.policy.stay));
            self.fullscreen_at = None;
        }
    }
    pub fn activity(&mut self, now: u64) {
        if self.fullscreen_at.is_some() {
            self.arm_fullscreen(now);
        }
    }
    fn arm_fullscreen(&mut self, now: u64) {
        self.fullscreen_at = (self.policy.fullscreen_ms > 0 && self.target.is_some())
            .then(|| now.saturating_add(self.policy.fullscreen_ms));
    }
    /// The visible preview has this identity (also handles an already tuned
    /// channel). Merely focusing a different channel does not start its timer.
    pub fn playing(&mut self, key: &str, now: u64) {
        if self.target.as_ref().is_some_and(|t| t.key == key) && self.fullscreen_at.is_none() {
            self.arm_fullscreen(now);
        }
    }
    pub fn deadline(&self) -> Option<u64> {
        self.pending
            .map(|p| p.0)
            .into_iter()
            .chain(self.fullscreen_at)
            .min()
    }
    pub fn tick(&mut self, now: u64) -> Option<Effect> {
        if let Some((at, fullscreen)) = self.pending
            && now >= at
        {
            self.pending = None;
            if fullscreen {
                self.fullscreen_at = None;
            }
            return self
                .target
                .clone()
                .map(|target| Effect::Tune { target, fullscreen });
        }
        if self.fullscreen_at.is_some_and(|at| now >= at) {
            self.fullscreen_at = None;
            return Some(Effect::Fullscreen);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(key: &str) -> Option<Target> {
        Some(Target {
            key: key.into(),
            source: None,
            surface: Surface::Guide,
        })
    }
    fn policy() -> Policy {
        Policy {
            autoplay: true,
            delay_ms: 500,
            fullscreen_ms: 2_000,
            stay: true,
        }
    }
    #[test]
    fn rapid_navigation_and_surface_exit_cancel_pending_tunes() {
        let mut b = BrowsePlayback::default();
        b.focus(target("a"), policy(), 0);
        b.focus(target("b"), policy(), 450);
        assert_eq!(b.tick(500), None);
        assert_eq!(
            b.tick(950),
            Some(Effect::Tune {
                target: target("b").unwrap(),
                fullscreen: false
            })
        );
        b.focus(target("c"), policy(), 1000);
        b.focus(None, policy(), 1200);
        assert_eq!(b.tick(10000), None);
    }
    #[test]
    fn input_rearms_timeout_but_not_tuning_and_zero_disables_timeout() {
        let mut b = BrowsePlayback::default();
        b.focus(target("a"), policy(), 0);
        b.activity(400);
        assert!(matches!(
            b.tick(500),
            Some(Effect::Tune {
                fullscreen: false,
                ..
            })
        ));
        assert_eq!(b.deadline(), None);
        b.playing("a", 500);
        b.activity(2400);
        assert_eq!(b.tick(2500), None);
        assert_eq!(b.tick(4400), Some(Effect::Fullscreen));
        assert_eq!(b.tick(5000), None);
        b.focus(
            target("b"),
            Policy {
                fullscreen_ms: 0,
                ..policy()
            },
            5000,
        );
        b.tick(5500);
        assert_eq!(b.deadline(), None);
    }
    #[test]
    fn explicit_selection_respects_stay_and_cancels_older_requests() {
        for stay in [true, false] {
            let p = Policy {
                autoplay: false,
                stay,
                ..policy()
            };
            let mut b = BrowsePlayback::default();
            b.focus(target("a"), p, 0);
            assert_eq!(b.tick(1000), None);
            b.select(1000);
            b.focus(target("b"), p, 1200);
            assert_eq!(b.tick(1500), None);
            b.select(1600);
            assert!(
                matches!(b.tick(2100), Some(Effect::Tune { fullscreen, .. }) if fullscreen == !stay)
            );
        }
    }
    #[test]
    fn policy_change_cancels_and_already_playing_preview_can_time_out() {
        let mut b = BrowsePlayback::default();
        b.focus(target("a"), policy(), 0);
        let p = Policy {
            autoplay: false,
            delay_ms: 0,
            ..policy()
        };
        b.focus(target("a"), p, 200);
        assert_eq!(b.tick(500), None);
        b.playing("other", 600);
        assert_eq!(b.deadline(), None);
        b.playing("a", 600);
        b.playing("a", 700);
        assert_eq!(b.deadline(), Some(2600));
        b.focus(None, p, 900);
        assert_eq!(b.tick(3000), None);
    }
}
