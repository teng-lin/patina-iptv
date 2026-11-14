//! Stored-guide window policy, independent of UI callbacks.
use iptv_epg::{DAY, UnixTime};
use iptv_guide::pages::PagedWindow;
use iptv_store_sqlite::EpgWindow;
/// Days of finished programmes held in memory with the guide (at most
/// what `epg.keep_past` keeps). Older ones are paged in.
pub const LOADED_PAST_DAYS: u32 = 1;

/// What a schedule shown at `now` holds of the stored guide: from
/// [`LOADED_PAST_DAYS`] (at most `keep_past_days`) before the hour of
/// `now` on; the store keeps nothing that ended before `keep_past_days`.
pub fn window(now: UnixTime, keep_past_days: u32) -> PagedWindow {
    let days = i64::from(LOADED_PAST_DAYS.min(keep_past_days));
    PagedWindow {
        loaded_from: now - now.rem_euclid(3600) - days * DAY,
        oldest: now.saturating_sub(i64::from(keep_past_days).saturating_mul(DAY)),
    }
}

/// The store read of [`window`]'s held part.
pub fn store_window(window: PagedWindow) -> EpgWindow<'static> {
    EpgWindow {
        from: window.loaded_from,
        ..EpgWindow::from(0, 0)
    }
}

/// [`window`] with its oldest time at the first programme stored (`None`:
/// none is), when that is later than what the store would keep: the guide
/// can be browsed back to it and no further (`EpgSchedule::browsable`).
pub fn kept_from(window: PagedWindow, first: Option<UnixTime>, now: UnixTime) -> PagedWindow {
    PagedWindow {
        oldest: window.oldest.max(first.unwrap_or(now)),
        ..window
    }
}
