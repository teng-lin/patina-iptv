//! Shared layout geometry at the current window width and text scale.

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

mod guide_header;
mod list_panel;
pub use guide_header::*;
pub use list_panel::*;

#[cfg(test)]
mod tests {
    use super::*;

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
}
