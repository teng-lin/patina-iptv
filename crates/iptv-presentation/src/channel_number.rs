//! Timed numeric selection retains content identity rather than a mutable row number.

/// A bounded digit buffer with a caller-resolved stable target and monotonic deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelNumber<T> {
    digits: String,
    deadline: Option<u64>,
    target: Option<T>,
}
impl<T> Default for ChannelNumber<T> {
    fn default() -> Self {
        Self {
            digits: String::new(),
            deadline: None,
            target: None,
        }
    }
}
impl<T> ChannelNumber<T> {
    /// The entered digits for presentation.
    pub fn digits(&self) -> &str {
        &self.digits
    }
    /// Whether a selection is pending, including an unmatched number.
    pub fn active(&self) -> bool {
        !self.digits.is_empty()
    }
    /// Adds a digit and resolves its identity now; expired input starts afresh.
    /// `now` and `timeout` use the same monotonic time unit.
    pub fn push(
        &mut self,
        digit: u8,
        now: u64,
        timeout: u64,
        resolve: impl FnOnce(u32) -> Option<T>,
    ) {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.take();
        }
        if digit > 9 || self.digits.len() == 10 {
            return;
        }
        self.digits.push(char::from(b'0' + digit));
        self.deadline = Some(now.saturating_add(timeout));
        self.target = self.digits.parse().ok().and_then(resolve);
    }
    /// Removes the last digit and resolves the corrected number.
    pub fn backspace(&mut self, now: u64, timeout: u64, resolve: impl FnOnce(u32) -> Option<T>) {
        self.digits.pop();
        self.deadline = self.active().then(|| now.saturating_add(timeout));
        self.target = self.digits.parse().ok().and_then(resolve);
    }
    /// Clears the pending input and returns its resolved identity, if any.
    pub fn take(&mut self) -> Option<T> {
        self.digits.clear();
        self.deadline = None;
        self.target.take()
    }
    /// Commits only after the most recent digit deadline; earlier timers do nothing.
    pub fn expire(&mut self, now: u64) -> Option<T> {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.take()
        } else {
            None
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_tracks_last_digit_and_retains_identity_across_reordering() {
        let mut entry = ChannelNumber::default();
        entry.push(1, 0, 100, |_| Some("first"));
        entry.push(2, 50, 100, |number| {
            (number == 12).then_some("stable-channel")
        });
        assert_eq!(entry.expire(100), None);
        assert_eq!(entry.expire(150), Some("stable-channel"));
        assert!(!entry.active());
        assert_eq!(entry.expire(200), None);
    }
    #[test]
    fn a_digit_after_a_delayed_timer_starts_a_new_number() {
        let mut entry = ChannelNumber::default();
        entry.push(1, 0, 100, |_| Some("expired"));
        entry.push(2, 101, 100, |number| (number == 2).then_some("new"));
        assert_eq!(entry.digits(), "2");
        assert_eq!(entry.expire(201), Some("new"));
    }
    #[test]
    fn correction_cancellation_invalid_and_overflow_never_select_a_fallback() {
        let mut entry = ChannelNumber::default();
        entry.push(9, 0, 100, |_| None::<String>);
        assert_eq!(entry.take(), None);
        entry.push(1, 0, 100, |_| Some("one".into()));
        entry.push(2, 0, 100, |_| None);
        entry.backspace(1, 100, |n| (n == 1).then(|| "one".into()));
        assert_eq!(entry.take().as_deref(), Some("one"));
        for _ in 0..10 {
            entry.push(9, 0, 100, |_| None);
        }
        assert_eq!(entry.expire(100), None);
    }
}
