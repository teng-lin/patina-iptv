//! The guide's channel-number column: as wide as the widest number of the
//! list on screen, so four- and five-digit numbers show whole.

use nsh_core::prelude::*;

/// Digits the column always has room for.
const MIN_DIGITS: usize = 3;
/// A digit's width in em where the platform measures less: tabular
/// digits are about 0.55 em in the common sans-serif faces.
const DIGIT_EM: f32 = 0.6;
/// Room for rounding in the platform's layout.
const SLACK: f32 = 2.0;

/// The width (px) of a number column for rows numbered up to `widest`,
/// set at `size` sp. The platform draws text at the user's text size
/// (Android's font scale, 1.15 on some tablets by default), so the column
/// grows with it (reading subscribes).
pub fn column_width(widest: usize, size: f32) -> f32 {
    let size = size * font_scale();
    let digits = digits(widest).max(MIN_DIGITS);
    let measured = measure_text(&format!("{size}px Roboto"), &"0".repeat(digits)).0;
    width(digits, measured, size)
}

fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |d| d as usize + 1)
}

fn width(digits: usize, measured: f32, size: f32) -> f32 {
    measured.max(digits as f32 * size * DIGIT_EM).ceil() + SLACK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::CHANNEL;

    #[test]
    fn digits_count_the_widest_number() {
        assert_eq!(digits(0), 1);
        assert_eq!(digits(9), 1);
        assert_eq!(digits(10), 2);
        assert_eq!(digits(999), 3);
        assert_eq!(digits(6105), 4);
        assert_eq!(digits(10_000), 5);
        assert_eq!(digits(99_999), 5);
    }

    #[test]
    fn the_column_grows_with_the_digits_and_trusts_a_wider_measure() {
        let three = width(3, 0.0, CHANNEL);
        let five = width(5, 0.0, CHANNEL);
        assert!(five > three);
        // Five digits at 0.55 em (a common face) fit.
        assert!(five >= 5.0 * CHANNEL * 0.55 + 1.0);
        // A face measured wider than the estimate wins.
        assert_eq!(width(5, 100.0, CHANNEL), 102.0);
        // A smaller size (a scaled window) narrows the column.
        assert!(width(5, 0.0, CHANNEL * 0.8) < five);
    }
}
