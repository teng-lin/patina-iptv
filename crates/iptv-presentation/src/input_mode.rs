//! The last input method and its shared interaction policy.

/// The kind of input the viewer used last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    /// A remote or a keyboard: focus highlight and key hints.
    Keys,
    /// A finger on a touch screen.
    Touch,
    /// A mouse, a trackpad or a pen.
    Pointer,
}

impl InputMode {
    /// The mode a press from a pointer puts the app in: a finger is touch,
    /// a mouse, trackpad or pen is pointer.
    pub fn of_press(finger: bool) -> Self {
        if finger {
            InputMode::Touch
        } else {
            InputMode::Pointer
        }
    }

    /// Whether focus highlights and key hints show: only while keys drive
    /// the app, as in Android's touch mode.
    pub fn key_hints(self) -> bool {
        self == InputMode::Keys
    }

    /// Whether visible Back and Close controls show. A host whose remote
    /// has a Back key hides them while keys drive it; touch or a pointer
    /// shows them everywhere. Other hosts always show them, including to
    /// keyboard users.
    pub fn back_controls(self, remote_back: bool) -> bool {
        !remote_back || self != InputMode::Keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_follow_the_pointer_that_made_them() {
        assert_eq!(InputMode::of_press(true), InputMode::Touch);
        assert_eq!(InputMode::of_press(false), InputMode::Pointer);
    }

    #[test]
    fn key_hints_show_only_for_keys() {
        assert!(InputMode::Keys.key_hints());
        assert!(!InputMode::Touch.key_hints());
        assert!(!InputMode::Pointer.key_hints());
    }

    #[test]
    fn back_controls_follow_the_input_on_a_remote_host() {
        // A TV: hidden for the remote, shown once touch or a pointer is used.
        assert!(!InputMode::Keys.back_controls(true));
        assert!(InputMode::Touch.back_controls(true));
        assert!(InputMode::Pointer.back_controls(true));
        // A tablet or desktop: always shown, keyboard users included.
        for mode in [InputMode::Keys, InputMode::Touch, InputMode::Pointer] {
            assert!(mode.back_controls(false));
        }
    }
}
