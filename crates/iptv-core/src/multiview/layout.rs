//! Where multiview's tiles sit, as fractions of the multiview area (the
//! window). Every tile keeps the video's 16:9 within an area of any
//! shape; the caller insets each rectangle by its gap.
//!
//! - The grid: one tile centred at half the width; two side by side;
//!   three as two above one; four two by two (five to nine would take
//!   three columns). A row that is not full is centred, and so is the
//!   whole grid.
//! - Enlarged: one tile big at the top centre, the others small in a
//!   centred strip under it (the strip's tiles [`THUMB`] of the big one's
//!   width); together they fill the height when the width allows.

/// The video's shape, width over height.
pub const VIDEO: f32 = 16.0 / 9.0;

/// An enlarged layout's small tiles, as a share of the big one's width.
pub const THUMB: f32 = 0.29;

/// A rectangle as fractions (0–1) of the multiview area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn center(self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// How the tiles sit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// `tiles` tiles in rows.
    Grid { tiles: usize },
    /// Tile `big` large at the top, the others under it.
    Enlarged { tiles: usize, big: usize },
}

impl Layout {
    pub fn tiles(self) -> usize {
        match self {
            Layout::Grid { tiles } | Layout::Enlarged { tiles, .. } => tiles,
        }
    }

    /// Where tile `index` goes in an area `aspect` (width over height)
    /// wide.
    pub fn rect(self, index: usize, aspect: f32) -> Rect {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            VIDEO
        };
        match self {
            Layout::Enlarged { tiles, big } if tiles >= 2 && big < tiles => {
                enlarged(tiles, big, index, aspect)
            }
            other => grid(other.tiles().max(1), index, aspect),
        }
    }

    /// The tile next to tile `from` in a direction (`dx`, `dy`: −1, 0 or
    /// 1, one of them zero): of those whose centre lies that way and that
    /// overlap it across, the nearest (the first of equals). Laid out on a
    /// 16:9 area.
    pub fn neighbor(self, from: usize, dx: i8, dy: i8) -> Option<usize> {
        let r = self.rect(from, VIDEO);
        let at = r.center();
        (0..self.tiles())
            .filter(|&i| i != from)
            .filter_map(|i| {
                let o = self.rect(i, VIDEO);
                let c = o.center();
                let (along, across, overlap) = if dx != 0 {
                    let overlap = (r.y + r.h).min(o.y + o.h) - r.y.max(o.y);
                    ((c.0 - at.0) * f32::from(dx), c.1 - at.1, overlap)
                } else {
                    let overlap = (r.x + r.w).min(o.x + o.w) - r.x.max(o.x);
                    ((c.1 - at.1) * f32::from(dy), c.0 - at.0, overlap)
                };
                (along > 1e-3 && overlap > 1e-3).then_some((i, along + 2.0 * across.abs()))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

/// Columns for `tiles` tiles: two up to four, then three.
fn columns(tiles: usize) -> usize {
    if tiles <= 4 { 2 } else { 3 }
}

fn grid(tiles: usize, index: usize, aspect: f32) -> Rect {
    let cols = columns(tiles);
    let rows = tiles.div_ceil(cols).max(1);
    // As wide as a column, unless the rows would not fit the height.
    let w = (1.0 / cols as f32).min(VIDEO / (aspect * rows as f32));
    let h = w * aspect / VIDEO;
    let (row, col) = (index / cols, index % cols);
    let in_row = tiles.saturating_sub(row * cols).clamp(1, cols);
    Rect {
        x: (1.0 - in_row as f32 * w) / 2.0 + col as f32 * w,
        y: (1.0 - rows as f32 * h) / 2.0 + row as f32 * h,
        w,
        h,
    }
}

fn enlarged(tiles: usize, big: usize, index: usize, aspect: f32) -> Rect {
    let small = (tiles - 1) as f32;
    // The big tile and a small one fill the height, unless the width (or
    // the strip's) runs out first.
    let w = (VIDEO / (aspect * (1.0 + THUMB)))
        .min(1.0)
        .min(1.0 / (small * THUMB));
    let h = w * aspect / VIDEO;
    let (sw, sh) = (w * THUMB, h * THUMB);
    let top = (1.0 - h - sh) / 2.0;
    if index == big {
        return Rect {
            x: (1.0 - w) / 2.0,
            y: top,
            w,
            h,
        };
    }
    let k = if index > big { index - 1 } else { index } as f32;
    Rect {
        x: (1.0 - small * sw) / 2.0 + k * sw,
        y: top + h,
        w: sw,
        h: sh,
    }
}
