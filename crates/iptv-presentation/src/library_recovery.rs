//! Focus and retry state for a library that could not open at startup.

/// An action on the startup recovery screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Retry,
    Details,
}

/// Availability and outcome of a retry on the current recovery screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retry {
    Unavailable,
    Ready,
    Checking,
    Failed,
}

/// The visible recovery controls, independent of the cause and presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recovery {
    retry: Retry,
    focus: Action,
    details: bool,
}

impl Recovery {
    pub fn new(retry: Retry) -> Self {
        Self {
            retry,
            focus: if retry == Retry::Unavailable {
                Action::Details
            } else {
                Action::Retry
            },
            details: false,
        }
    }

    pub fn retry(self) -> Retry {
        self.retry
    }

    pub fn focus(self) -> Action {
        self.focus
    }

    pub fn details(self) -> bool {
        self.details
    }

    /// Move to an available action. Returns whether focus moved.
    pub fn move_to(&mut self, action: Action) -> bool {
        if action == self.focus || action == Action::Retry && self.retry == Retry::Unavailable {
            return false;
        }
        self.focus = action;
        true
    }

    /// Activate the action, returning true when the host should retry startup.
    pub fn select(&mut self, action: Action) -> bool {
        match action {
            Action::Details => {
                self.focus = action;
                self.details = !self.details;
                false
            }
            Action::Retry if matches!(self.retry, Retry::Ready | Retry::Failed) => {
                self.focus = action;
                self.retry = Retry::Checking;
                true
            }
            Action::Retry => false,
        }
    }

    /// Reconcile the controls with the error from a completed retry.
    pub fn failed(&mut self, retry: Retry) {
        self.retry = retry;
        if retry == Retry::Unavailable {
            self.focus = Action::Details;
        }
    }

    /// Close the details, leaving focus on their disclosure.
    pub fn back(&mut self) -> bool {
        if !self.details {
            return false;
        }
        self.details = false;
        self.focus = Action::Details;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Recovery, Retry};

    #[test]
    fn recovery_allows_one_retry_and_preserves_the_details() {
        let mut recovery = Recovery::new(Retry::Ready);
        assert!(!recovery.select(Action::Details));
        assert!(recovery.select(Action::Retry));
        assert!(!recovery.select(Action::Retry));
        recovery.failed(Retry::Unavailable);
        assert!(recovery.details());
        assert_eq!(recovery.focus(), Action::Details);
        assert!(!recovery.move_to(Action::Retry));
        assert!(recovery.back());
        assert!(!recovery.back());
    }
}
