//! Focus, confirmation, and retry state for a library that could not open at startup.

/// Why the library could not be opened at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Corrupt,
    KeyUnavailable,
    NewerSchema,
    RestartRequired,
    Other,
}

/// An action on the startup recovery screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Retry,
    Details,
    StartFresh,
    RestoreBackup,
}

/// Focused option in the second confirmation before starting fresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmChoice {
    Cancel,
    StartFresh,
}

/// Host operation requested by activating a recovery control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Retry,
    RestoreBackup,
    StartFresh,
}

/// Availability and outcome of a retry on the current recovery screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retry {
    Unavailable,
    Ready,
    Checking,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveWork {
    None,
    RestoreBackup,
    StartFresh,
}

const CORRUPT_ACTIONS: [Action; 3] = [Action::RestoreBackup, Action::StartFresh, Action::Details];
const CORRUPT_WITHOUT_RESTORE_ACTIONS: [Action; 2] = [Action::StartFresh, Action::Details];
const RETRY_ACTIONS: [Action; 2] = [Action::Retry, Action::Details];
const DETAILS_ONLY_ACTIONS: [Action; 1] = [Action::Details];

/// The visible recovery controls, independent of toolkit presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recovery {
    cause: Cause,
    retry: Retry,
    restore_available: bool,
    focus: Action,
    focus_moved: bool,
    details: bool,
    confirming: Option<ConfirmChoice>,
    work: ActiveWork,
}

impl Recovery {
    pub fn new(retry: Retry) -> Self {
        Self::for_cause(Cause::Other, retry)
    }

    pub fn for_cause(cause: Cause, retry: Retry) -> Self {
        let retry = match cause {
            Cause::Corrupt | Cause::NewerSchema | Cause::RestartRequired => Retry::Unavailable,
            Cause::KeyUnavailable | Cause::Other => retry,
        };
        let mut recovery = Self {
            cause,
            retry,
            restore_available: cause == Cause::Corrupt,
            focus: Action::Details,
            focus_moved: false,
            details: false,
            confirming: None,
            work: ActiveWork::None,
        };
        recovery.focus = recovery.actions()[0];
        recovery
    }

    pub fn cause(self) -> Cause {
        self.cause
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

    pub fn confirming_start_fresh(self) -> Option<ConfirmChoice> {
        self.confirming
    }

    pub fn working(self) -> bool {
        self.work != ActiveWork::None || self.retry == Retry::Checking
    }

    pub fn restoring_backup(self) -> bool {
        self.work == ActiveWork::RestoreBackup
    }

    pub fn starting_fresh(self) -> bool {
        self.work == ActiveWork::StartFresh
    }

    /// Update whether a document picker is available to restore a backup.
    pub fn set_restore_available(&mut self, available: bool) {
        let allowed = self.cause == Cause::Corrupt && available;
        if self.restore_available == allowed {
            return;
        }
        self.restore_available = allowed;
        if self.working() || self.confirming.is_some() {
            return;
        }
        if allowed && !self.focus_moved {
            self.focus = Action::RestoreBackup;
        } else if !self.actions().contains(&self.focus) {
            self.focus = self.actions()[0];
        }
    }

    /// Actions offered in focus order for the current cause.
    pub fn actions(self) -> &'static [Action] {
        match self.cause {
            Cause::Corrupt if self.restore_available => &CORRUPT_ACTIONS,
            Cause::Corrupt => &CORRUPT_WITHOUT_RESTORE_ACTIONS,
            Cause::NewerSchema | Cause::RestartRequired => &DETAILS_ONLY_ACTIONS,
            Cause::KeyUnavailable | Cause::Other => {
                if self.retry == Retry::Unavailable {
                    &DETAILS_ONLY_ACTIONS
                } else {
                    &RETRY_ACTIONS
                }
            }
        }
    }

    /// Move to an available action. Returns whether focus moved.
    pub fn move_to(&mut self, action: Action) -> bool {
        if self.working() || self.confirming.is_some() || action == self.focus {
            return false;
        }
        if !self.actions().contains(&action) {
            return false;
        }
        self.focus = action;
        self.focus_moved = true;
        true
    }

    /// Move focus within the active confirmation step.
    pub fn move_confirm(&mut self, choice: ConfirmChoice) -> bool {
        if self.working() || self.confirming.is_none() || self.confirming == Some(choice) {
            return false;
        }
        self.confirming = Some(choice);
        true
    }

    /// Step focus backward (`delta < 0`) or forward (`delta > 0`) in order.
    pub fn step(&mut self, delta: isize) -> bool {
        if self.working() || delta == 0 {
            return false;
        }
        if let Some(choice) = self.confirming {
            let next = if delta < 0 {
                ConfirmChoice::Cancel
            } else {
                ConfirmChoice::StartFresh
            };
            if choice == next {
                return false;
            }
            self.confirming = Some(next);
            return true;
        }
        let actions = self.actions();
        let Some(index) = actions.iter().position(|item| *item == self.focus) else {
            return false;
        };
        let Some(next) = index.checked_add_signed(delta) else {
            return false;
        };
        let Some(&target) = actions.get(next) else {
            return false;
        };
        self.focus = target;
        self.focus_moved = true;
        true
    }

    /// Activate a specific action and return any host operation to start.
    pub fn activate(&mut self, action: Action) -> Effect {
        if self.working() || self.confirming.is_some() || !self.actions().contains(&action) {
            return Effect::None;
        }
        match action {
            Action::Details => {
                self.focus = action;
                self.focus_moved = true;
                self.details = !self.details;
                Effect::None
            }
            Action::Retry if matches!(self.retry, Retry::Ready | Retry::Failed) => {
                self.focus = action;
                self.focus_moved = true;
                self.retry = Retry::Checking;
                Effect::Retry
            }
            Action::Retry => Effect::None,
            Action::RestoreBackup if self.cause == Cause::Corrupt && self.restore_available => {
                self.focus = action;
                self.focus_moved = true;
                self.work = ActiveWork::RestoreBackup;
                Effect::RestoreBackup
            }
            Action::RestoreBackup => Effect::None,
            Action::StartFresh if self.cause == Cause::Corrupt => {
                self.focus = action;
                self.focus_moved = true;
                self.confirming = Some(ConfirmChoice::Cancel);
                Effect::None
            }
            Action::StartFresh => Effect::None,
        }
    }

    /// Activate the currently focused control or confirmation button.
    pub fn select_focused(&mut self) -> Effect {
        if self.working() {
            return Effect::None;
        }
        if let Some(choice) = self.confirming {
            return match choice {
                ConfirmChoice::Cancel => {
                    self.cancel_start_fresh();
                    Effect::None
                }
                ConfirmChoice::StartFresh => self.confirm_start_fresh(),
            };
        }
        self.activate(self.focus)
    }

    /// Activate `Retry` or `Details`, returning true when the host should retry startup.
    /// Actions with dedicated host effects (`RestoreBackup`, `StartFresh`) require [`Self::activate`].
    pub fn select(&mut self, action: Action) -> bool {
        matches!(action, Action::Retry | Action::Details) && self.activate(action) == Effect::Retry
    }

    /// Dismiss the Start fresh confirmation without modifying storage.
    pub fn cancel_start_fresh(&mut self) -> bool {
        if self.working() || self.confirming.take().is_none() {
            return false;
        }
        self.focus = Action::StartFresh;
        self.focus_moved = true;
        if !self.actions().contains(&self.focus) {
            self.focus = self.actions()[0];
        }
        true
    }

    /// Confirm starting fresh after the second confirmation step was opened.
    pub fn confirm_start_fresh(&mut self) -> Effect {
        if self.working() || self.cause != Cause::Corrupt || self.confirming.is_none() {
            return Effect::None;
        }
        self.confirming = None;
        self.work = ActiveWork::StartFresh;
        Effect::StartFresh
    }

    /// Release the working guard when a recovery action was canceled or failed.
    pub fn finish_work(&mut self) {
        self.work = ActiveWork::None;
        if !self.actions().contains(&self.focus) {
            self.focus = self.actions()[0];
        }
    }

    /// Reconcile the controls with the error from a completed retry.
    pub fn failed(&mut self, retry: Retry) {
        self.work = ActiveWork::None;
        self.retry = match self.cause {
            Cause::Corrupt | Cause::NewerSchema | Cause::RestartRequired => Retry::Unavailable,
            Cause::KeyUnavailable | Cause::Other => retry,
        };
        if !self.actions().contains(&self.focus) {
            self.focus = self.actions()[0];
        }
    }

    /// Dismiss confirmation or details, leaving focus on the originating control.
    pub fn back(&mut self) -> bool {
        if self.working() {
            return true;
        }
        if self.confirming.take().is_some() {
            self.focus = Action::StartFresh;
            self.focus_moved = true;
            return true;
        }
        if !self.details {
            return false;
        }
        self.details = false;
        self.focus = Action::Details;
        self.focus_moved = true;
        true
    }

    /// Whether Back at this point should leave the recovery screen.
    pub fn is_root(self) -> bool {
        !self.working() && self.confirming.is_none() && !self.details
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Cause, ConfirmChoice, Effect, Recovery, Retry};

    #[test]
    fn actions_depend_on_cause_and_wrong_key_never_offers_start_fresh() {
        let corrupt = Recovery::for_cause(Cause::Corrupt, Retry::Ready);
        assert_eq!(corrupt.retry(), Retry::Unavailable);
        assert_eq!(
            corrupt.actions(),
            &[Action::RestoreBackup, Action::StartFresh, Action::Details]
        );
        assert_eq!(corrupt.focus(), Action::RestoreBackup);

        let mut corrupt_no_picker = corrupt;
        corrupt_no_picker.set_restore_available(false);
        assert_eq!(
            corrupt_no_picker.actions(),
            &[Action::StartFresh, Action::Details]
        );
        assert_eq!(corrupt_no_picker.focus(), Action::StartFresh);

        let wrong_key = Recovery::for_cause(Cause::KeyUnavailable, Retry::Ready);
        assert_eq!(wrong_key.actions(), &[Action::Retry, Action::Details]);
        assert_eq!(wrong_key.focus(), Action::Retry);
        assert!(!wrong_key.actions().contains(&Action::StartFresh));
        assert!(!wrong_key.actions().contains(&Action::RestoreBackup));

        for cause in [Cause::NewerSchema, Cause::RestartRequired] {
            let recovery = Recovery::for_cause(cause, Retry::Ready);
            assert_eq!(recovery.retry(), Retry::Unavailable);
            assert_eq!(recovery.actions(), &[Action::Details]);
            assert_eq!(recovery.focus(), Action::Details);
        }

        let other_unretryable = Recovery::for_cause(Cause::Other, Retry::Unavailable);
        assert_eq!(other_unretryable.actions(), &[Action::Details]);
    }

    #[test]
    fn async_picker_arrival_promotes_untouched_focus_and_preserves_active_work() {
        // Simulates Android cold start before `tv.backup` reports `open: true`.
        let mut untouched = Recovery::for_cause(Cause::Corrupt, Retry::Unavailable);
        untouched.set_restore_available(false);
        assert_eq!(untouched.focus(), Action::StartFresh);
        untouched.set_restore_available(true);
        assert_eq!(untouched.focus(), Action::RestoreBackup);

        // Once the user moves focus manually, async picker discovery keeps their focus.
        let mut moved = Recovery::for_cause(Cause::Corrupt, Retry::Unavailable);
        moved.set_restore_available(false);
        assert!(moved.step(1));
        assert_eq!(moved.focus(), Action::Details);
        moved.set_restore_available(true);
        assert_eq!(moved.focus(), Action::Details);

        // Mid-flight picker changes during RestoreBackup never mislabel StartFresh as busy.
        let mut restoring = Recovery::for_cause(Cause::Corrupt, Retry::Unavailable);
        assert_eq!(
            restoring.activate(Action::RestoreBackup),
            Effect::RestoreBackup
        );
        assert!(restoring.working());
        assert!(restoring.restoring_backup());
        assert!(!restoring.starting_fresh());
        restoring.set_restore_available(false);
        assert!(restoring.restoring_backup());
        assert!(!restoring.starting_fresh());
        assert_eq!(restoring.focus(), Action::RestoreBackup);
        restoring.finish_work();
        assert!(!restoring.working());
        assert_eq!(restoring.focus(), Action::StartFresh);
    }

    #[test]
    fn corrupt_focus_moves_in_order_and_start_fresh_requires_cancel_first_confirmation() {
        let mut recovery = Recovery::for_cause(Cause::Corrupt, Retry::Unavailable);
        assert_eq!(recovery.focus(), Action::RestoreBackup);
        assert!(!recovery.select(Action::RestoreBackup));
        assert!(!recovery.working());
        assert!(!recovery.select(Action::StartFresh));
        assert_eq!(recovery.confirming_start_fresh(), None);
        assert!(!recovery.step(-1));
        assert!(recovery.step(1));
        assert_eq!(recovery.focus(), Action::StartFresh);
        assert!(recovery.step(1));
        assert_eq!(recovery.focus(), Action::Details);
        assert!(!recovery.step(1));
        assert!(recovery.step(-1));
        assert_eq!(recovery.focus(), Action::StartFresh);

        // Selecting StartFresh opens confirmation with Cancel focused first,
        // and cannot execute without explicit confirmation.
        assert_eq!(recovery.confirm_start_fresh(), Effect::None);
        assert_eq!(recovery.select_focused(), Effect::None);
        assert_eq!(
            recovery.confirming_start_fresh(),
            Some(ConfirmChoice::Cancel)
        );
        assert!(!recovery.is_root());

        // Pressing Select immediately cancels because Cancel has initial focus.
        assert_eq!(recovery.select_focused(), Effect::None);
        assert_eq!(recovery.confirming_start_fresh(), None);
        assert_eq!(recovery.focus(), Action::StartFresh);
        assert!(recovery.is_root());

        // Re-open confirmation, verify Back also cancels cleanly.
        assert_eq!(recovery.activate(Action::StartFresh), Effect::None);
        assert_eq!(
            recovery.confirming_start_fresh(),
            Some(ConfirmChoice::Cancel)
        );
        assert!(recovery.back());
        assert_eq!(recovery.confirming_start_fresh(), None);
        assert_eq!(recovery.focus(), Action::StartFresh);

        // Re-open confirmation, move focus to StartFresh, and confirm.
        assert_eq!(recovery.select_focused(), Effect::None);
        assert!(!recovery.step(-1));
        assert!(recovery.step(1));
        assert_eq!(
            recovery.confirming_start_fresh(),
            Some(ConfirmChoice::StartFresh)
        );
        assert!(!recovery.step(1));
        assert_eq!(recovery.select_focused(), Effect::StartFresh);
        assert!(recovery.working());
        assert!(recovery.starting_fresh());
        assert!(!recovery.restoring_backup());
        assert_eq!(recovery.select_focused(), Effect::None);
        recovery.finish_work();
        assert!(!recovery.working());
    }

    #[test]
    fn non_corrupt_causes_refuse_start_fresh_and_restore_backup() {
        for cause in [
            Cause::KeyUnavailable,
            Cause::NewerSchema,
            Cause::RestartRequired,
            Cause::Other,
        ] {
            let mut recovery = Recovery::for_cause(cause, Retry::Ready);
            assert_eq!(recovery.activate(Action::StartFresh), Effect::None);
            assert_eq!(recovery.confirming_start_fresh(), None);
            assert_eq!(recovery.confirm_start_fresh(), Effect::None);
            assert_eq!(recovery.activate(Action::RestoreBackup), Effect::None);
            assert!(!recovery.move_to(Action::StartFresh));
            assert!(!recovery.move_to(Action::RestoreBackup));
        }
    }

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
