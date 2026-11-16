//! Portable parental access and PIN-entry state. Native storage and hashing are
//! deliberately outside this crate; views render this state and send actions.
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContentKind {
    Live,
    Movie,
    Series,
    Episode,
    Group,
    VodCategory,
    Settings,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Target {
    pub source: i64,
    pub kind: ContentKind,
    pub id: String,
}
impl Target {
    pub fn settings() -> Self {
        Self {
            source: 0,
            kind: ContentKind::Settings,
            id: String::new(),
        }
    }
    pub fn live(source: i64, id: impl Into<String>) -> Self {
        Self {
            source,
            kind: ContentKind::Live,
            id: id.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnlockWindow {
    #[default]
    EveryTime,
    Minutes(u16),
    UntilRestart,
    UntilUnblockedChannel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub live: bool,
    pub vod: bool,
    pub settings: bool,
    pub window: UnlockWindow,
}
impl Policy {
    pub fn protects(self, kind: ContentKind) -> bool {
        match kind {
            ContentKind::Settings => self.settings || self.live || self.vod,
            ContentKind::Live | ContentKind::Group => self.live,
            ContentKind::Movie
            | ContentKind::Series
            | ContentKind::Episode
            | ContentKind::VodCategory => self.vod,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Access {
    pub policy: Policy,
    /// Exact grants for per-content unlocks. Parent/category grants are checked
    /// alongside the requested item by the native runtime's ancestry resolver.
    granted: BTreeSet<Target>,
    all_until: Option<u64>,
    session: bool,
    settings_until: Option<u64>,
    settings_session: bool,
}
impl Access {
    pub fn granted(&self, target: &Target, now: u64) -> bool {
        if target.kind == ContentKind::Settings {
            return self.settings_session
                || self.settings_until.is_some_and(|until| now < until)
                || self.granted.contains(target);
        }
        self.session
            || self.all_until.is_some_and(|until| now < until)
            || self.granted.contains(target)
    }
    pub fn unlock(&mut self, target: Target, now: u64) {
        if target.kind == ContentKind::Settings {
            self.granted.remove(&target);
            self.settings_until = None;
            self.settings_session = false;
            match self.policy.window {
                UnlockWindow::Minutes(minutes) => {
                    self.settings_until = Some(now.saturating_add(u64::from(minutes) * 60_000));
                }
                UnlockWindow::UntilRestart | UnlockWindow::UntilUnblockedChannel => {
                    self.settings_session = true;
                }
                UnlockWindow::EveryTime => {
                    self.granted.insert(target);
                }
            }
            return;
        }
        if self.policy.window != UnlockWindow::EveryTime {
            self.granted
                .retain(|target| target.kind == ContentKind::Settings);
            self.all_until = None;
            self.session = false;
        }
        match self.policy.window {
            UnlockWindow::Minutes(minutes) => {
                self.all_until = Some(now.saturating_add(u64::from(minutes) * 60_000))
            }
            UnlockWindow::UntilRestart | UnlockWindow::UntilUnblockedChannel => self.session = true,
            UnlockWindow::EveryTime => {
                self.all_until = None;
                self.session = false;
                self.granted.insert(target);
            }
        }
    }
    pub fn lock(&mut self) {
        self.granted.clear();
        self.all_until = None;
        self.session = false;
        self.settings_until = None;
        self.settings_session = false;
    }
    pub fn unblocked_channel_played(&mut self) {
        if matches!(
            self.policy.window,
            UnlockWindow::UntilUnblockedChannel | UnlockWindow::EveryTime
        ) {
            self.lock();
        }
    }
    pub fn content_left(&mut self, target: &Target) {
        if self.policy.window == UnlockWindow::EveryTime {
            self.granted.remove(target);
        }
    }
    pub fn content_closed(&mut self) {
        if self.policy.window == UnlockWindow::EveryTime {
            self.lock();
        }
    }
    pub fn backgrounded(&mut self) {
        // Session and timed unlocks survive ordinary backgrounding; per-use
        // authority does not survive navigation away from its UI operation.
        if self.policy.window == UnlockWindow::EveryTime {
            self.lock();
        }
    }
}

/// A PIN is never represented by Debug output or copied into an action label.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Pin {
    bytes: [u8; 8],
    len: usize,
}
impl std::fmt::Debug for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PIN").field(&"***").finish()
    }
}
impl Pin {
    pub fn parse(text: &str) -> Option<Self> {
        if !(4..=8).contains(&text.len()) || !text.bytes().all(|v| v.is_ascii_digit()) {
            return None;
        }
        let mut pin = Self::default();
        pin.bytes[..text.len()].copy_from_slice(text.as_bytes());
        pin.len = text.len();
        Some(pin)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn push(&mut self, digit: u8) -> bool {
        if self.len == self.bytes.len() || digit > 9 {
            return false;
        }
        self.bytes[self.len] = b'0' + digit;
        self.len += 1;
        true
    }
    pub fn erase(&mut self) {
        if self.len != 0 {
            self.len -= 1;
            self.bytes[self.len] = 0;
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn valid(&self) -> bool {
        (4..=8).contains(&self.len)
    }
    pub fn clear(&mut self) {
        self.bytes.fill(0);
        self.len = 0;
    }
    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
    pub fn masked(&self) -> String {
        "•".repeat(self.len)
    }
}
impl Drop for Pin {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.bytes.zeroize();
        self.len.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_grants_never_authorize_or_extend_administration() {
        for window in [
            UnlockWindow::EveryTime,
            UnlockWindow::Minutes(10),
            UnlockWindow::Minutes(30),
            UnlockWindow::Minutes(60),
            UnlockWindow::Minutes(120),
            UnlockWindow::UntilRestart,
            UnlockWindow::UntilUnblockedChannel,
        ] {
            let mut access = Access::default();
            access.policy.window = window;
            let channel = Target::live(1, "generated-channel");
            access.unlock(channel.clone(), 10);
            assert!(access.granted(&channel, 10));
            assert!(!access.granted(&Target::settings(), 10), "{window:?}");
            access.unlock(Target::settings(), 20);
            assert!(access.granted(&Target::settings(), 20));
            access.unlock(Target::live(1, "generated-other"), 30);
            assert!(access.granted(&Target::settings(), 30));
            if let UnlockWindow::Minutes(minutes) = window {
                assert!(!access.granted(&Target::settings(), 20 + u64::from(minutes) * 60_000));
            }
            access.lock();
            assert!(!access.granted(&channel, 40));
            assert!(!access.granted(&Target::settings(), 40));
        }
    }
    #[test]
    fn exact_grants_do_not_unlock_another_source_or_item() {
        let target = Target::live(1, "synthetic-a");
        let mut access = Access::default();
        access.unlock(target.clone(), 1);
        assert!(access.granted(&target, 1));
        assert!(!access.granted(&Target::live(2, "synthetic-a"), 1));
        assert!(!access.granted(&Target::live(1, "synthetic-b"), 1));
        access.unblocked_channel_played();
        assert!(!access.granted(&target, 1));
    }
    #[test]
    fn timed_unlocks_expire_at_the_boundary_and_explicit_lock_revokes_them() {
        let mut access = Access::default();
        access.policy.window = UnlockWindow::Minutes(10);
        let target = Target::live(1, "synthetic-a");
        access.unlock(target.clone(), 100);
        assert!(access.granted(&target, 600_099));
        assert!(!access.granted(&target, 600_100));
        access.unlock(target.clone(), 1);
        access.lock();
        assert!(!access.granted(&target, 2));
    }
    #[test]
    fn until_open_channel_relocks_but_restart_session_is_explicit() {
        let target = Target::live(1, "synthetic-a");
        let mut access = Access::default();
        access.policy.window = UnlockWindow::UntilUnblockedChannel;
        access.unlock(target.clone(), 1);
        access.unblocked_channel_played();
        assert!(!access.granted(&target, 2));
        access.policy.window = UnlockWindow::UntilRestart;
        access.unlock(target.clone(), 1);
        access.unblocked_channel_played();
        assert!(access.granted(&target, 2));
        access.lock();
        assert!(!access.granted(&target, 3));
    }
    #[test]
    fn compound_episode_operation_holds_both_grants_until_content_closes() {
        let mut access = Access::default();
        let series = Target {
            source: 1,
            kind: ContentKind::Series,
            id: "generated-series".into(),
        };
        let episode = Target {
            source: 1,
            kind: ContentKind::Episode,
            id: "generated-episode".into(),
        };
        access.unlock(series.clone(), 1);
        access.unlock(episode.clone(), 2);
        assert!(access.granted(&series, 3) && access.granted(&episode, 3));
        access.content_closed();
        assert!(!access.granted(&series, 4) && !access.granted(&episode, 4));
    }
    #[test]
    fn pin_input_is_bounded_redacted_and_cleared() {
        assert!(Pin::parse("123").is_none());
        assert!(Pin::parse("12345x").is_none());
        let mut pin = Pin::parse("2468").unwrap();
        assert_eq!(format!("{pin:?}"), "PIN(\"***\")");
        assert_eq!(pin.masked(), "••••");
        pin.erase();
        assert!(!pin.valid());
        pin.push(1);
        assert!(pin.valid());
        let taken = pin.take();
        assert!(pin.is_empty());
        assert_eq!(taken.bytes(), b"2461");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Unlock,
    Old,
    New,
    Confirm,
    Recovery,
    RecoveryNotice,
}
/// Entry, focus and the setup/change/recovery transitions are portable state.
/// Native work consumes secrets, so async UI tasks never clone a PIN.
#[derive(Clone, PartialEq, Eq)]
pub struct Dialog {
    pub phase: Phase,
    pub focus: usize,
    pub busy: bool,
    pub message: String,
    pub input: Pin,
    old: Option<Pin>,
    new: Option<Pin>,
    pub recovery: zeroize::Zeroizing<String>,
    changing: bool,
    setup: bool,
}
pub enum Submission {
    None,
    Verify(Pin),
    Change {
        old: Option<Pin>,
        new: Pin,
    },
    Recover {
        code: zeroize::Zeroizing<String>,
        new: Pin,
    },
    Done,
}
impl Dialog {
    pub fn new(phase: Phase) -> Self {
        Self {
            phase,
            focus: 0,
            busy: false,
            message: String::new(),
            input: Pin::default(),
            old: None,
            new: None,
            recovery: zeroize::Zeroizing::new(String::new()),
            changing: phase == Phase::Old,
            setup: phase == Phase::New,
        }
    }
    pub fn digit(&mut self, digit: u8) {
        if !self.busy {
            self.input.push(digit);
        }
    }
    pub fn move_focus(&mut self, step: isize, count: usize) {
        self.focus = self
            .focus
            .saturating_add_signed(step)
            .min(count.saturating_sub(1));
    }
    pub fn recover(&mut self) {
        self.input.clear();
        self.old = None;
        self.new = None;
        self.phase = Phase::Recovery;
        self.focus = 0;
        self.message.clear();
    }
    pub fn fail(&mut self, message: String) {
        self.busy = false;
        self.input.clear();
        self.old = None;
        self.new = None;
        self.recovery.clear();
        self.phase = if self.setup {
            Phase::New
        } else if self.changing {
            Phase::Old
        } else {
            Phase::Unlock
        };
        self.focus = 0;
        self.message = message;
    }
    pub fn notice(&mut self, code: zeroize::Zeroizing<String>) {
        self.busy = false;
        self.phase = Phase::RecoveryNotice;
        self.recovery = code;
        self.message.clear();
        self.focus = 0;
    }
    pub fn submit(&mut self) -> Submission {
        if self.busy {
            return Submission::None;
        }
        if self.phase == Phase::RecoveryNotice {
            return Submission::Done;
        }
        if self.phase == Phase::Recovery {
            if self.recovery.len() != 32 || !self.recovery.bytes().all(|b| b.is_ascii_hexdigit()) {
                self.message = "Enter the 32-character recovery code".into();
                return Submission::None;
            }
            self.phase = Phase::New;
            self.focus = 0;
            self.input.clear();
            return Submission::None;
        }
        if !self.input.valid() {
            self.message = "Use four to eight digits".into();
            return Submission::None;
        }
        self.message.clear();
        match self.phase {
            Phase::Unlock => {
                self.busy = true;
                Submission::Verify(self.input.take())
            }
            Phase::Old => {
                self.old = Some(self.input.take());
                self.phase = Phase::New;
                self.focus = 0;
                Submission::None
            }
            Phase::New => {
                self.new = Some(self.input.take());
                self.phase = Phase::Confirm;
                self.focus = 0;
                Submission::None
            }
            Phase::Confirm => {
                if self
                    .new
                    .as_ref()
                    .is_none_or(|pin| pin.bytes() != self.input.bytes())
                {
                    self.input.clear();
                    self.new = None;
                    self.phase = Phase::New;
                    self.focus = 0;
                    self.message = "PINs did not match; choose a new PIN".into();
                    return Submission::None;
                }
                self.input.clear();
                self.busy = true;
                let new = self.new.take().unwrap();
                if self.recovery.is_empty() {
                    Submission::Change {
                        old: self.old.take(),
                        new,
                    }
                } else {
                    Submission::Recover {
                        code: std::mem::take(&mut self.recovery),
                        new,
                    }
                }
            }
            _ => Submission::None,
        }
    }
}
#[cfg(test)]
mod dialog_tests {
    use super::*;
    fn enter(dialog: &mut Dialog, text: &str) {
        for digit in text.bytes() {
            dialog.digit(digit - b'0');
        }
    }
    #[test]
    fn setup_requires_confirmation_and_clears_mismatch() {
        let mut d = Dialog::new(Phase::New);
        enter(&mut d, "2468");
        d.focus = 11;
        assert!(matches!(d.submit(), Submission::None));
        assert_eq!(d.phase, Phase::Confirm);
        assert_eq!(d.focus, 0);
        enter(&mut d, "2469");
        d.focus = 11;
        assert!(matches!(d.submit(), Submission::None));
        assert_eq!(d.phase, Phase::New);
        assert_eq!(d.focus, 0);
        assert!(d.input.is_empty());
        enter(&mut d, "1357");
        d.submit();
        enter(&mut d, "1357");
        assert!(matches!(d.submit(), Submission::Change { old: None, .. }));
        assert!(d.input.is_empty());
        assert!(d.busy);
        d.focus = 11;
        d.fail("Retry".into());
        assert_eq!(d.focus, 0);
        let mut changing = Dialog::new(Phase::Old);
        enter(&mut changing, "2468");
        changing.focus = 11;
        changing.submit();
        assert_eq!(changing.phase, Phase::New);
        assert_eq!(changing.focus, 0);
    }
    #[test]
    fn recovery_never_disables_protection_without_replacing_pin() {
        let mut d = Dialog::new(Phase::Unlock);
        d.focus = 12;
        d.recover();
        assert_eq!(d.focus, 0, "recovery starts at its own first control");
        d.focus = 1;
        d.recovery.push_str("0123456789abcdef0123456789abcdef");
        assert!(matches!(d.submit(), Submission::None));
        assert_eq!(d.phase, Phase::New);
        assert_eq!(d.focus, 0, "new PIN starts at its own first control");
        enter(&mut d, "2468");
        d.submit();
        enter(&mut d, "2468");
        assert!(matches!(d.submit(), Submission::Recover { .. }));
    }
}
