//! The layout and input policy every screen shares
//! (`docs/interaction.md`): which input the
//! viewer used last, what that shows, and which arrangement the window
//! space allows. The policy reads the last input, not the device type, so
//! a tablet used with a keyboard, or a TV with a pointer, gets what it needs.

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

/// How much room the window gives the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// One pane at a time.
    Compact,
    /// Panes side by side where both stay usable.
    Medium,
    /// Guide beside the player, or a list beside its details.
    Expanded,
}

/// The widths (logical points, at a text factor of 1) where the classes
/// begin. Compact ends where a header's controls stop fitting beside a
/// query field (Search's measure); Expanded begins at a TV's 960-point
/// screen, which shows the preview layout.
pub const MEDIUM_FROM: f32 = 620.0;
pub const EXPANDED_FROM: f32 = 960.0;

impl Class {
    /// The class of a window `width` points wide whose text is drawn at
    /// `text_factor` (the guide's scale times the viewer's text size):
    /// larger text needs more room, so it reaches each class later.
    pub fn of(width: f32, text_factor: f32) -> Self {
        let factor = if text_factor > 0.0 { text_factor } else { 1.0 };
        let room = width / factor;
        if room >= EXPANDED_FROM {
            Class::Expanded
        } else if room >= MEDIUM_FROM {
            Class::Medium
        } else {
            Class::Compact
        }
    }
}

/// Where the guide's header puts its shortcuts (the list on screen,
/// Movies, Shows, Add playlist).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcuts {
    /// A column at the right, beside the preview and the focused programme.
    Beside,
    /// A row of their own under the preview and the focused programme, so
    /// the programme keeps the width beside the preview: a compact window
    /// has no room for three columns.
    Below,
}

impl Class {
    /// Where the guide's header puts its shortcuts in a window of this
    /// class. The preview keeps its place at the top left either way: the
    /// player under it is only resized, never hidden or moved elsewhere.
    pub fn guide_shortcuts(self) -> Shortcuts {
        match self {
            Class::Compact => Shortcuts::Below,
            Class::Medium | Class::Expanded => Shortcuts::Beside,
        }
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

    #[test]
    fn classes_begin_at_their_widths() {
        assert_eq!(Class::of(320.0, 1.0), Class::Compact);
        assert_eq!(Class::of(619.0, 1.0), Class::Compact);
        assert_eq!(Class::of(620.0, 1.0), Class::Medium);
        assert_eq!(Class::of(959.0, 1.0), Class::Medium);
        assert_eq!(Class::of(960.0, 1.0), Class::Expanded);
        assert_eq!(Class::of(1920.0, 1.0), Class::Expanded);
    }

    #[test]
    fn larger_text_needs_more_room() {
        // A TV at 960 points: expanded at its own scale, medium with large text.
        assert_eq!(Class::of(960.0, 1.0), Class::Expanded);
        assert_eq!(Class::of(960.0, 1.3), Class::Medium);
        // A desktop window shrunk to 0.8 scale fits more per point.
        assert_eq!(Class::of(500.0, 0.8), Class::Medium);
        assert_eq!(Class::of(700.0, 1.3), Class::Compact);
    }

    #[test]
    fn no_text_factor_counts_as_one() {
        assert_eq!(Class::of(700.0, 0.0), Class::Medium);
        assert_eq!(Class::of(700.0, -1.0), Class::Medium);
        assert_eq!(Class::of(700.0, f32::NAN), Class::Medium);
    }

    #[test]
    fn a_compact_guide_header_moves_its_shortcuts_below() {
        assert_eq!(Class::Compact.guide_shortcuts(), Shortcuts::Below);
        assert_eq!(Class::Medium.guide_shortcuts(), Shortcuts::Beside);
        assert_eq!(Class::Expanded.guide_shortcuts(), Shortcuts::Beside);
        // A narrow desktop window, a phone, a portrait tablet, a TV.
        assert_eq!(Class::of(400.0, 0.8).guide_shortcuts(), Shortcuts::Below);
        assert_eq!(Class::of(360.0, 1.0).guide_shortcuts(), Shortcuts::Below);
        assert_eq!(Class::of(800.0, 1.0).guide_shortcuts(), Shortcuts::Beside);
        assert_eq!(Class::of(960.0, 0.8).guide_shortcuts(), Shortcuts::Beside);
    }
}
