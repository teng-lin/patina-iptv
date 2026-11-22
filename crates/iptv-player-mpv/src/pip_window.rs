//! Picture-in-picture window identity and initial native placement.

/// The PiP component shares surface zero's player and stream.
pub const PIP_VIEW: &str = "tv.pip";

/// The window's title: browsers' picture-in-picture title.
pub const TITLE: &str = "Picture-in-Picture";

/// The window's width when it opens (points).
pub const WIDTH: f32 = 600.0;

/// Initial slots for up to four compact players. Native adapters place only
/// newly compact windows; existing user placements remain unchanged.
/// Slots reserve the supported tallest shape (4:3); adapters fit each player's
/// actual shape inside its slot without stretching it.
pub fn popup_rects(area: (f32, f32, f32, f32), count: usize) -> Vec<(f32, f32, f32, f32)> {
    let (x, y, width, height) = area;
    let count = count.min(iptv_core::multiview::MAX_SURFACES);
    if count == 0
        || width <= 0.0
        || height <= 0.0
        || ![x, y, width, height].into_iter().all(f32::is_finite)
    {
        return Vec::new();
    }
    let columns = count.min(2);
    let rows = count.div_ceil(columns);
    let gap = 16.0_f32.min(width / 8.0).min(height / 8.0);
    let w = WIDTH
        .min((width - gap * (columns + 1) as f32) / columns as f32)
        .min((height - gap * (rows + 1) as f32) / rows as f32 * 4.0 / 3.0);
    let h = w * 3.0 / 4.0;
    (0..count)
        .map(|i| {
            (
                x + width - gap - w - (i % columns) as f32 * (w + gap),
                y + height - gap - h - (i / columns) as f32 * (h + gap),
                w,
                h,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_slots_are_separate_and_within_small_and_offset_displays() {
        for area in [(220.0, -482.0, 2560.0, 1440.0), (0.0, 0.0, 800.0, 600.0)] {
            for count in 1..=4 {
                let rects = popup_rects(area, count);
                assert_eq!(rects.len(), count);
                for (i, &(x, y, w, h)) in rects.iter().enumerate() {
                    assert!(x >= area.0 && y >= area.1);
                    assert!(x + w <= area.0 + area.2 + 0.01 && y + h <= area.1 + area.3 + 0.01);
                    for &(a, b, c, d) in &rects[..i] {
                        assert!(x + w <= a || a + c <= x || y + h <= b || b + d <= y);
                    }
                }
            }
        }
        assert!(popup_rects((0.0, 0.0, 0.0, 600.0), 4).is_empty());
        assert!(popup_rects((0.0, 0.0, f32::NAN, 600.0), 4).is_empty());
    }

    #[test]
    fn the_title_is_the_one_browsers_give_their_window() {
        assert_eq!(TITLE, "Picture-in-Picture");
    }
}
