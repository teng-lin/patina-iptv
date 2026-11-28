//! Keeps a window reachable when displays are removed or rearranged.

/// A rectangle in screen points: `(x, y, width, height)`.
pub type Rect = (f32, f32, f32, f32);

/// How much of a window's top edge must stay on a display for the viewer
/// to grab it again (points): a title bar's height, and a width enough to
/// aim at.
pub const GRAB_HEIGHT: f32 = 32.0;
pub const GRAB_WIDTH: f32 = 96.0;

/// Where a window must go so the viewer can reach it again, after a
/// display it was on went away or the displays were arranged anew
/// (`docs/interaction.md`, "Displays"): `None` when enough of its top edge
/// is still on one of `areas` (the displays' usable areas), or when the
/// host reports no display to measure against. Otherwise the window
/// moves onto the area nearest to it, the first (primary) one on a tie,
/// keeping its size unless it is larger than that area. A presentation
/// change only: the caller moves the window, its player plays on.
pub fn reachable(window: Rect, areas: &[Rect]) -> Option<Rect> {
    let finite = |r: &Rect| [r.0, r.1, r.2, r.3].into_iter().all(f32::is_finite);
    // A window with no size (a host's placeholder) is never moved or sized.
    if !finite(&window) || window.2 <= 0.0 || window.3 <= 0.0 {
        return None;
    }
    let areas: Vec<Rect> = areas
        .iter()
        .copied()
        .filter(|a| finite(a) && a.2 > 0.0 && a.3 > 0.0)
        .collect();
    if areas.is_empty() {
        return None;
    }
    let (x, y, w, h) = window;
    let grab_w = GRAB_WIDTH.min(w.max(1.0));
    let grab_h = GRAB_HEIGHT.min(h.max(1.0));
    let grabbable = areas.iter().any(|&(ax, ay, aw, ah)| {
        let across = (x + w).min(ax + aw) - x.max(ax);
        let down = (y + grab_h).min(ay + ah) - y.max(ay);
        across >= grab_w && down >= grab_h
    });
    if grabbable {
        return None;
    }
    // The area whose rectangle is closest to the window's center.
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let distance = |&(ax, ay, aw, ah): &Rect| {
        let dx = (ax - cx).max(cx - (ax + aw)).max(0.0);
        let dy = (ay - cy).max(cy - (ay + ah)).max(0.0);
        dx * dx + dy * dy
    };
    let mut nearest = areas[0];
    for area in &areas[1..] {
        if distance(area) < distance(&nearest) {
            nearest = *area;
        }
    }
    let (ax, ay, aw, ah) = nearest;
    let (w, h) = (w.min(aw), h.min(ah));
    // `ax + (aw - w)` cannot round below `ax` when `w == aw`, as
    // `ax + aw - w` can with fractional origins; `max` keeps `clamp`'s
    // bounds ordered whatever the rounding.
    let fit = |at: f32, start: f32, room: f32| at.clamp(start, (start + room).max(start));
    Some((fit(x, ax, aw - w), fit(y, ay, ah - h), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    const PRIMARY: Rect = (0.0, 0.0, 1920.0, 1040.0);
    const LEFT: Rect = (-1512.0, -200.0, 1512.0, 945.0);

    #[test]
    fn a_window_with_its_top_edge_on_a_display_stays_where_the_viewer_put_it() {
        assert_eq!(reachable((100.0, 80.0, 960.0, 540.0), &[PRIMARY]), None);
        // Mostly off the right edge, but its title bar can still be grabbed.
        assert_eq!(reachable((1800.0, 900.0, 960.0, 540.0), &[PRIMARY]), None);
        // On the second display, left of and above the primary.
        assert_eq!(
            reachable((-1400.0, -150.0, 960.0, 540.0), &[PRIMARY, LEFT]),
            None
        );
        // No display reported (a host that tells none): leave it alone.
        assert_eq!(reachable((-9000.0, 0.0, 640.0, 360.0), &[]), None);
    }

    #[test]
    fn removing_a_display_brings_its_windows_onto_the_nearest_remaining_one() {
        // The left display went away: the window comes onto the primary's
        // left edge, at the same size.
        let moved = reachable((-1400.0, -150.0, 960.0, 540.0), &[PRIMARY]).unwrap();
        assert_eq!(moved, (0.0, 0.0, 960.0, 540.0));
        // A window only a sliver of which shows keeps its size and lands
        // inside, nearest to where it was.
        let moved = reachable((1900.0, 500.0, 640.0, 360.0), &[PRIMARY]).unwrap();
        assert_eq!(moved, (1280.0, 500.0, 640.0, 360.0));
        // Its title bar above the top of every display is unreachable too.
        let moved = reachable((200.0, -500.0, 640.0, 360.0), &[PRIMARY]).unwrap();
        assert_eq!(moved, (200.0, 0.0, 640.0, 360.0));
    }

    #[test]
    fn a_window_larger_than_the_remaining_display_shrinks_to_fit_it() {
        let small = (100.0, 30.0, 1280.0, 690.0);
        let moved = reachable((-1500.0, -190.0, 1500.0, 900.0), &[small]).unwrap();
        assert_eq!(moved, small);
    }

    #[test]
    fn fractional_origins_and_oversized_windows_never_panic() {
        // Scaled displays report fractional points: a window as wide as
        // (or wider than) the remaining display, whose origin and width
        // would round `ax + aw - w` below `ax`.
        let area = (-3.06, -0.333_333_3, 1514.64, 853.333_3);
        for window in [
            (-4000.0, 10.0, 1514.64, 900.0),
            (-4000.0, 10.0, 3000.5, 2000.25),
            (853.333_3, -900.0, 1024.0, 853.333_3),
            (5000.0, 5000.0, 1514.64, 853.333_3),
        ] {
            let (x, y, w, h) = reachable(window, &[area]).expect("moved back");
            assert!(w <= area.2 && h <= area.3, "{window:?} fits");
            assert!(x >= area.0 && y >= area.1, "{window:?} lands on the area");
            assert_eq!(reachable((x, y, w, h), &[area]), None, "and stays");
        }
        // Mixed-DPI origins on a second display.
        let right = (1_706.666_7, 0.0, 1024.0, 640.0);
        let moved = reachable((853.333_3, -700.0, 1024.0, 900.0), &[right]).unwrap();
        assert_eq!(moved.2, 1024.0);
        assert!(moved.0 >= right.0);
    }

    #[test]
    fn the_nearest_display_wins_and_reconnecting_needs_no_move() {
        let right = (1920.0, 0.0, 2560.0, 1400.0);
        // Off the far right: the right display is nearer than the primary.
        let moved = reachable((5000.0, 100.0, 640.0, 360.0), &[PRIMARY, right]).unwrap();
        assert_eq!(moved, (4480.0 - 640.0, 100.0, 640.0, 360.0));
        // After the display returns, the restored placement is reachable.
        assert_eq!(
            reachable((-1400.0, -150.0, 960.0, 540.0), &[PRIMARY, LEFT]),
            None
        );
        // A window with no size is never moved or sized to nothing.
        assert_eq!(reachable((-5000.0, 0.0, 0.0, 360.0), &[PRIMARY]), None);
        assert_eq!(reachable((-5000.0, 0.0, 640.0, -1.0), &[PRIMARY]), None);
        // Garbage from a host never moves a window.
        assert_eq!(reachable((f32::NAN, 0.0, 640.0, 360.0), &[PRIMARY]), None);
        assert_eq!(
            reachable((-5000.0, 0.0, 640.0, 360.0), &[(0.0, 0.0, 0.0, 0.0)]),
            None
        );
    }
}
