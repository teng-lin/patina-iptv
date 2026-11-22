//! Dialog flow for setting, changing and entering a parental PIN.

use iptv_core::parental::Pin;

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
    // A credential result hidden at an authentication boundary is never
    // rendered until the shell has verified the current PIN again.
    held_notice: zeroize::Zeroizing<String>,
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
    /// Display only the number of entered digits.
    pub fn masked_input(&self) -> String {
        "•".repeat(self.input.len())
    }

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
            held_notice: zeroize::Zeroizing::new(String::new()),
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
        self.held_notice.clear();
        self.busy = false;
        self.phase = Phase::RecoveryNotice;
        self.recovery = code;
        self.message.clear();
        self.focus = 0;
    }
    /// Keep a committed recovery result private while the window is hidden.
    pub fn hold_notice(&mut self, code: zeroize::Zeroizing<String>) {
        self.held_notice = code;
        self.recovery.clear();
        self.input.clear();
        self.old = None;
        self.new = None;
        self.busy = false;
        self.phase = Phase::Unlock;
        self.focus = 0;
        self.message.clear();
        self.setup = false;
        self.changing = false;
    }
    pub fn has_held_notice(&self) -> bool {
        !self.held_notice.is_empty()
    }
    pub fn suspend_notice(&mut self) {
        if self.phase == Phase::RecoveryNotice {
            let code = std::mem::take(&mut self.recovery);
            self.hold_notice(code);
        }
    }
    /// The shell calls this only after authoritative PIN verification.
    pub fn resume_notice(&mut self) -> bool {
        if self.held_notice.is_empty() {
            return false;
        }
        let code = std::mem::take(&mut self.held_notice);
        self.notice(code);
        true
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
    fn hidden_notice_is_private_until_verified_and_survives_a_failed_pin() {
        let code = "0123456789abcdef0123456789abcdef";
        let mut d = Dialog::new(Phase::New);
        d.hold_notice(zeroize::Zeroizing::new(code.into()));
        assert_eq!(d.phase, Phase::Unlock);
        assert!(d.recovery.is_empty());
        d.fail("Incorrect PIN".into());
        assert_eq!(d.phase, Phase::Unlock);
        assert!(d.has_held_notice());
        assert!(d.recovery.is_empty());
        assert!(d.resume_notice()); // the shell has now verified the PIN
        assert_eq!(d.phase, Phase::RecoveryNotice);
        assert_eq!(d.recovery.as_str(), code);
        d.suspend_notice();
        assert!(d.recovery.is_empty());
        assert_eq!(d.phase, Phase::Unlock);
        assert!(d.resume_notice());
        assert_eq!(d.recovery.as_str(), code);
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
