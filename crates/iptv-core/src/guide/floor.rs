//! The guide can browse retained listings independently of replay availability.

use super::{Focus, Program, Schedule, VISIBLE, home_window};

/// The minutes the guide can show at `now`, including its default window.
pub fn view_range(s: &dyn Schedule, now: i32) -> (i32, i32) {
    let (first, end) = s.browsable(now);
    (first.min(home_window(now)), end.max(now + VISIBLE))
}

/// The first minute the focused row can inspect, up to the present.
pub fn floor_of(s: &dyn Schedule, channel: usize, now: i32) -> i32 {
    if channel >= s.channel_count() {
        return now;
    }
    s.listing_start(channel)
        .map_or(now, |start| start.max(s.browsable(now).0).min(now))
}

impl Focus {
    /// [`floor_of`] the focused row.
    pub fn floor(&self, s: &dyn Schedule) -> i32 {
        floor_of(s, self.channel, self.now)
    }

    /// The previous programme within retained guide coverage.
    pub(crate) fn previous(&self, s: &dyn Schedule) -> Option<Program> {
        let current = self.program(s);
        if current.start <= self.floor(s) {
            return None;
        }
        let p = s.program_at(self.channel, current.start - 1);
        (p.start < current.start).then_some(p)
    }

    /// Keeps the cursor on or after the row's first inspectable minute.
    pub(crate) fn lift_to_floor(&mut self, s: &dyn Schedule) {
        let before = self.cursor;
        self.cursor = self.cursor.max(self.floor(s));
        if self.cursor == before {
            return;
        }
        if self.cursor == self.now {
            self.window = home_window(self.now);
        } else {
            self.keep_cursor_visible(s);
        }
    }
}

#[cfg(test)]
mod tests;
