//! Keeps the guide's displayed minute in sync with the app clock.

use nsh_core::prelude::*;

use crate::Ui;
use crate::clock::{next_tick, now_minute};
use crate::host::reload_time_zone;

/// Moves the guide's now when the minute changed.
pub fn tick(ui: Ui) {
    reload_time_zone();
    let now = now_minute();
    if ui.focus.with(|f| f.now) != now {
        // The focus is on a row of the list on screen.
        let view = crate::lists::view(ui);
        ui.focus.update(|f| {
            f.tick(now, &view);
        });
    }
}

/// Ticks at every minute boundary of the current schedule's clock.
pub fn start(ui: Ui) {
    // Synthetic work-counter journeys exclude unrelated minute-boundary work.
    #[cfg(feature = "test-support")]
    if crate::performance_probe::enabled() {
        return;
    }
    let _task = set_timeout(next_tick(), move || {
        tick(ui);
        start(ui);
    });
}
