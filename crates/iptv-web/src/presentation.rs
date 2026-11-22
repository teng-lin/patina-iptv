//! Guide and player labels for the bridge's English 24-hour display.

use iptv_core::clock::{self, Style};
use iptv_core::guide::Airing;
use iptv_core::player_info::ProgramTiming;

use crate::host::host;

pub(crate) struct Presentation {
    now_seconds: f64,
    offset_minutes: fn(i64) -> i32,
}

impl Presentation {
    pub(crate) fn from_host() -> Self {
        Self {
            now_seconds: host().wall_ms() / 1000.0,
            offset_minutes: |unix| host().utc_offset_minutes(unix as f64 * 1000.0) as i32,
        }
    }

    #[cfg(test)]
    pub(crate) fn utc(now: i64) -> Self {
        Self {
            now_seconds: now as f64,
            offset_minutes: |_| 0,
        }
    }

    pub(crate) fn time(&self, unix: i64) -> String {
        clock::time_at(unix, (self.offset_minutes)(unix), Style::english_24())
    }

    pub(crate) fn clock(&self) -> String {
        let now = self.now_seconds as i64;
        clock::day_and_time(now, (self.offset_minutes)(now), Style::english_24())
    }

    pub(crate) fn span(&self, start: i64, end: i64, separator: &str) -> String {
        let now = self.now_seconds as i64;
        let day = clock::day_at(start, (self.offset_minutes)(start));
        let today = clock::day_at(now, (self.offset_minutes)(now));
        let time = format!("{}{separator}{}", self.time(start), self.time(end));
        if day == today {
            time
        } else {
            format!("{} {time}", clock::weekday_at(day, Style::english_24()))
        }
    }

    pub(crate) fn timing(&self, start: i64, end: i64) -> ProgramTiming {
        ProgramTiming::at_wall_time(start, end, self.now_seconds)
    }
}

pub(crate) fn timing_label(timing: ProgramTiming) -> String {
    let minutes = timing.minutes_remaining.max(1);
    let hours = minutes / 60;
    let duration = match (hours, minutes % 60) {
        (0, _) => format!("{minutes} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    };
    match timing.airing {
        Airing::Live => format!("{duration} left"),
        Airing::Upcoming => format!("in {duration}"),
        Airing::Ended => "Ended".into(),
    }
}
