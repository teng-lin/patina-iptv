//! Patina: ink surfaces, mineral accents and a pale blue focus marker.
//! Designed for this app. Opaque program focus isolates its text from video;
//! dark accent fills keep white control labels readable. Saved color names
//! remain stable so existing preferences select the corresponding new tones.

use crate::i18n::key;

/// An accent bundle: lighter tone and main accent (`0xRRGGBB`). The focus
/// fill is an opaque dark main accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub name: &'static str,
    pub light: u32,
    pub main: u32,
}

/// A background bundle: panel, window and alternate (elevated) surfaces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Background {
    pub name: &'static str,
    pub panel: u32,
    pub window: u32,
    pub alt: u32,
}

const fn a(name: &'static str, light: u32, main: u32) -> Accent {
    Accent { name, light, main }
}
const fn b(name: &'static str, panel: u32, window: u32, alt: u32) -> Background {
    Background {
        name,
        panel,
        window,
        alt,
    }
}

pub const ACCENTS: [Accent; 16] = [
    a(key("Amber"), 0xF2CE8C, 0x72521F),
    a(key("Black"), 0xCED8DE, 0x394650),
    a(key("Blue"), 0x9CCBEB, 0x285575),
    a(key("Blue gray"), 0xB6CBD7, 0x405564),
    a(key("Brown"), 0xDEC1A7, 0x614C3C),
    a(key("Cyan"), 0x99DEDF, 0x265E62),
    a(key("Green"), 0xACD7AE, 0x3B593F),
    a(key("Gray"), 0xCDD2D8, 0x4D545C),
    a(key("Indigo"), 0xBCC1EE, 0x4A4F75),
    a(key("Lime"), 0xD0DB9B, 0x535E32),
    a(key("Orange"), 0xEFBE97, 0x704C30),
    a(key("Pink"), 0xE8B8CF, 0x684354),
    a(key("Purple"), 0xD2BBE9, 0x5D4872),
    a(key("Red"), 0xEDB3AB, 0x70463F),
    a(key("Teal"), 0xA2DACC, 0x315E53),
    a(key("Yellow"), 0xE5D494, 0x635827),
];

pub const BACKGROUNDS: [Background; 18] = [
    b(key("Amber"), 0x24211B, 0x1C1913, 0x2E2B25),
    b(key("Black"), 0x171C20, 0x0F1418, 0x21262A),
    b(key("Blue"), 0x19232C, 0x111B24, 0x232D36),
    b(key("Blue gray"), 0x20262D, 0x181E25, 0x2A3037),
    b(key("Brown"), 0x25211E, 0x1D1916, 0x2F2B28),
    b(key("Cyan"), 0x192728, 0x111F20, 0x233132),
    b(key("Dark"), 0x1C2429, 0x141C21, 0x262E33),
    b(key("Dark gray"), 0x222529, 0x1A1D21, 0x2C2F33),
    b(key("Green"), 0x1D271F, 0x151F17, 0x273129),
    b(key("Gray"), 0x282B2F, 0x202327, 0x323539),
    b(key("Indigo"), 0x212330, 0x191B28, 0x2B2D3A),
    b(key("Lime"), 0x25281C, 0x1D2014, 0x2F3226),
    b(key("Orange"), 0x29221C, 0x211A14, 0x332C26),
    b(key("Pink"), 0x2B2026, 0x23181E, 0x352A30),
    b(key("Purple"), 0x27212D, 0x1F1925, 0x312B37),
    b(key("Red"), 0x2D211F, 0x251917, 0x372B29),
    b(key("Teal"), 0x1C2825, 0x14201D, 0x26322F),
    b(key("Yellow"), 0x29271D, 0x211F15, 0x333127),
];

/// Opaque focus fills keep control contrast independent of the video.
pub const FOCUS_ALPHA: u8 = 0xFF;
/// Alpha of a subtle resting button (`#16FFFFFF`, ~8.6%).
pub const RESTING_ALPHA: u8 = 0x16;
/// Text: primary white; secondary and disabled are white at these alphas.
pub const TEXT_SECONDARY_ALPHA: u8 = 0xC4;
pub const TEXT_DISABLED_ALPHA: u8 = 0x60;
/// Corner radii: focus shape and larger rounded surfaces, in dp.
pub const RADIUS: f32 = 6.0;
pub const RADIUS_LARGE: f32 = 16.0;
/// Base text size (sp) and list padding (dp).
pub const TEXT_SIZE: f32 = 14.0;
pub const LIST_PADDING: f32 = 16.0;
/// Active recording control, distinct from the neutral idle icon.
pub const RECORDING: u32 = 0xF34B4B;

/// The theme in use: one accent and one background bundle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub accent: Accent,
    pub background: Background,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: ACCENTS[2],
            background: BACKGROUNDS[6],
        } // Blue on Dark
    }
}

/// `0xRRGGBB` with alpha `a` as `0xRRGGBBAA` (NSH's `rgba`).
pub const fn with_alpha(rgb: u32, a: u8) -> u32 {
    (rgb << 8) | a as u32
}

/// `over` at alpha `a` drawn on `under` (both `0xRRGGBB`), rounded.
pub const fn blend(under: u32, over: u32, a: u8) -> u32 {
    let mut out = 0;
    let mut shift = 0;
    while shift < 24 {
        let (u, o) = ((under >> shift) & 0xFF, (over >> shift) & 0xFF);
        let c = (o * a as u32 + u * (255 - a as u32) + 127) / 255;
        out |= c << shift;
        shift += 8;
    }
    out
}

const WHITE: u32 = 0xFFFFFF;
/// The focused program: a light pill whatever the background, with
/// near-black text.
pub const FOCUSED_CELL: u32 = 0xC8DDEB;
pub const FOCUSED_TEXT: u32 = 0x171C20;
/// Titles of the focused row and channel names: white, a touch dimmer.
pub const BRIGHT_TEXT: u32 = 0xF5F3EE;
/// Names in the lists (groups, channels, results, menu items) on the
/// window and panel colors.
pub const LIST_TEXT: u32 = 0xE8EDF0;
/// Error text on dark surfaces and its ink counterpart on pale blue focus.
pub const ERROR: u32 = 0xF0B5AC;
pub const ERROR_FOCUSED: u32 = 0x753A32;
/// Lift the focused row slightly above the resting ink surface.
pub const ROW_ALPHA: u8 = 0x18;

/// The guide's colors for a theme (`0xRRGGBB`; titles of rows without the
/// focus and the ruler's times are white at [`TEXT_SECONDARY_ALPHA`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideColors {
    /// Behind everything: header, ruler, channel column, between cells.
    pub window: u32,
    /// A program.
    pub cell: u32,
    /// The other programs of the focused row.
    pub row_cell: u32,
    /// The playing channel's number, name and marker.
    pub playing: u32,
    /// The ruler's date and time.
    pub ruler_date: u32,
    /// The now line and its dot.
    pub now_line: u32,
    /// The line under the ruler.
    pub separator: u32,
}

impl Theme {
    pub const fn guide(&self) -> GuideColors {
        let (bg, accent) = (self.background, self.accent);
        GuideColors {
            window: bg.window,
            cell: bg.panel,
            row_cell: blend(bg.panel, WHITE, ROW_ALPHA),
            playing: accent.light,
            ruler_date: accent.light,
            now_line: accent.light,
            separator: bg.alt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_packs_after_rgb() {
        assert_eq!(with_alpha(0x123456, FOCUS_ALPHA), 0x123456FF);
    }

    #[test]
    fn blend_mixes_each_channel() {
        assert_eq!(blend(0x000000, 0xFFFFFF, 0xFF), 0xFFFFFF);
        assert_eq!(blend(0x123456, 0xFFFFFF, 0), 0x123456);
        assert_eq!(blend(0x000000, 0xFF8000, 0x80), 0x804000);
    }

    fn luminance(rgb: u32) -> f64 {
        let channel = |shift| {
            let c = ((rgb >> shift) & 255_u32) as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_and_focus_contrast_for_every_saved_palette() {
        assert!(contrast(FOCUSED_TEXT, FOCUSED_CELL) >= 7.0);
        assert!(contrast(ERROR_FOCUSED, FOCUSED_CELL) >= 4.5);
        for accent in ACCENTS {
            assert!(
                contrast(LIST_TEXT, accent.main) >= 4.5,
                "{} control",
                accent.name
            );
            for background in BACKGROUNDS {
                let g = Theme { accent, background }.guide();
                for surface in [g.window, g.cell, g.row_cell, background.alt] {
                    assert!(contrast(LIST_TEXT, surface) >= 4.5);
                    assert!(contrast(ERROR, surface) >= 4.5);
                    assert!(contrast(blend(surface, WHITE, TEXT_SECONDARY_ALPHA), surface) >= 4.5);
                    assert!(contrast(FOCUSED_CELL, surface) >= 3.0);
                }
                // The playback panels use E0 opacity. Black/white video
                // bound every channel of their composited surfaces.
                for video in [0x000000, 0xFFFFFF] {
                    let panel = blend(video, background.panel, 0xE0);
                    let track = blend(panel, WHITE, RESTING_ALPHA * 2);
                    assert!(contrast(accent.light, track) >= 3.0);
                    assert!(contrast(blend(panel, WHITE, TEXT_SECONDARY_ALPHA), panel) >= 4.5);
                }
                assert!(contrast(g.playing, g.window) >= 4.5);
                for surface in [g.cell, g.row_cell, FOCUSED_CELL] {
                    assert!(
                        contrast(g.now_line, surface).max(contrast(FOCUSED_TEXT, surface)) >= 3.0
                    );
                }
                assert!(contrast(accent.main, FOCUSED_CELL) >= 3.0);
                assert!(
                    contrast(accent.light, blend(background.panel, WHITE, RESTING_ALPHA)) >= 3.0
                );
                assert!(
                    contrast(
                        accent.light,
                        blend(background.panel, WHITE, RESTING_ALPHA * 2)
                    ) >= 3.0
                );
            }
        }
    }

    #[test]
    fn every_bundle_gives_guide_colors_from_its_own_tones() {
        for accent in ACCENTS {
            for background in BACKGROUNDS {
                let g = Theme { accent, background }.guide();
                assert_eq!((g.window, g.cell), (background.window, background.panel));
                assert_eq!((g.playing, g.ruler_date), (accent.light, accent.light));
            }
        }
        let black = Theme {
            accent: ACCENTS[1],
            background: BACKGROUNDS[1],
        };
        assert_eq!(black.background.name, "Black");
        assert_eq!(black.guide().window, 0x0F1418);
    }

    #[test]
    fn bundles_are_complete_and_unique() {
        let mut names: Vec<_> = ACCENTS.iter().map(|a| a.name).collect();
        names.dedup();
        assert_eq!(names.len(), 16);
        assert_eq!(BACKGROUNDS.len(), 18);
    }
}
