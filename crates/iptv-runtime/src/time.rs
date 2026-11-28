//! The runtime's clocks: a monotonic [`Instant`] and the wall clock.
//!
//! Natively (the `native` feature) they are the operating system's:
//! [`Instant`] is `std::time::Instant`. A browser build has no clock of its
//! own (`std::time::Instant::now` and `SystemTime::now` panic on
//! `wasm32-unknown-unknown`), so its host installs one at startup
//! ([`install`]); until then reading the time is a bug and panics saying
//! so. The clock is a fact of the process, like the operating system's, not
//! of an owner.

use std::time::Duration;

/// Common resolver clock for viewing and recording. `at` selects the local
/// offset at a program's start for catch-up; live callers use `None`.
pub fn playback_clock(now: i64, at: Option<i64>) -> iptv_guide::playback::Clock {
    clock_with_offset(now, at, |instant| {
        #[cfg(all(windows, feature = "test-support"))]
        if let Some(offset) = std::env::var("IPTV_TEST_UTC_OFFSET_MINUTES")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|minutes| (-720..=720).contains(minutes))
        {
            return offset * 60;
        }
        use chrono::{Local, TimeZone};
        Local
            .timestamp_opt(instant, 0)
            .single()
            .map_or(0, |time| time.offset().local_minus_utc() / 60 * 60)
    })
}

fn clock_with_offset(
    now: i64,
    at: Option<i64>,
    offset: impl FnOnce(i64) -> i32,
) -> iptv_guide::playback::Clock {
    iptv_guide::playback::Clock {
        now,
        local_offset_secs: offset(at.unwrap_or(now)),
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    #[test]
    fn shared_live_resolver_uses_the_same_nonzero_local_offset_as_viewing() {
        let clock = clock_with_offset(0, None, |instant| {
            assert_eq!(instant, 0);
            5 * 3600 + 30 * 60
        });
        assert_eq!(clock.now, 0);
        assert_eq!(clock.local_offset_secs, 19_800);
        let live = "https://example.test/generated/{lutc:H_M}.ts";
        assert_eq!(
            iptv_providers::m3u::expand_live_url(live, clock.now, clock.local_offset_secs),
            "https://example.test/generated/05_30.ts"
        );
        assert_eq!(
            iptv_providers::m3u::expand_live_url(live, clock.now, 0),
            "https://example.test/generated/00_00.ts"
        );
        let archive = clock_with_offset(172_800, Some(86_400), |instant| {
            assert_eq!(instant, 86_400);
            -4 * 3600
        });
        assert_eq!(archive.now, 172_800);
        assert_eq!(archive.local_offset_secs, -14_400);
    }
}

#[cfg(feature = "native")]
pub use std::time::Instant;

#[cfg(not(feature = "native"))]
pub use host::Instant;

/// A host's clocks, for a build without the native ones.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    /// Time since an origin of the host's that does not move backwards.
    pub monotonic: fn() -> Duration,
    /// Time since the Unix epoch.
    pub wall: fn() -> Duration,
}

/// Installs the host's clocks. Natively the operating system's are used and
/// this does nothing; otherwise the first installed clock stays.
pub fn install(clock: Clock) {
    #[cfg(not(feature = "native"))]
    let _ = host::CLOCK.set(clock);
    #[cfg(feature = "native")]
    let _ = clock;
}

/// Time since the Unix epoch, zero for a clock before it.
pub fn since_epoch() -> Duration {
    #[cfg(feature = "native")]
    return std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    #[cfg(not(feature = "native"))]
    (host::clock().wall)()
}

/// Wall-clock seconds since the Unix epoch.
pub(crate) fn now_unix() -> i64 {
    since_epoch().as_secs() as i64
}

/// Whether `deadline` has passed, as the store's bounded reads ask it.
pub(crate) fn expiry(deadline: Instant) -> impl Fn() -> bool + Send + Sync + 'static {
    move || Instant::now() >= deadline
}

#[cfg(not(feature = "native"))]
mod host {
    use std::ops::{Add, AddAssign, Sub, SubAssign};
    use std::sync::OnceLock;
    use std::time::Duration;

    pub(super) static CLOCK: OnceLock<super::Clock> = OnceLock::new();

    pub(super) fn clock() -> super::Clock {
        // Unit tests of a browser build run on a host that has the
        // standard clocks.
        #[cfg(test)]
        let _ = CLOCK.set(super::Clock {
            monotonic: || {
                static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
                ORIGIN.get_or_init(std::time::Instant::now).elapsed()
            },
            wall: || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
            },
        });
        *CLOCK
            .get()
            .expect("the host installs a clock before the runtime reads the time")
    }

    /// A point on the host's monotonic clock; the same API as
    /// `std::time::Instant`'s, as far as the runtime uses it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Instant(Duration);

    impl Instant {
        pub fn now() -> Self {
            Self((clock().monotonic)())
        }
        pub fn duration_since(&self, earlier: Self) -> Duration {
            self.0.saturating_sub(earlier.0)
        }
        pub fn saturating_duration_since(&self, earlier: Self) -> Duration {
            self.0.saturating_sub(earlier.0)
        }
        pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
            self.0.checked_sub(earlier.0)
        }
        pub fn elapsed(&self) -> Duration {
            Self::now().duration_since(*self)
        }
        pub fn checked_add(&self, duration: Duration) -> Option<Self> {
            self.0.checked_add(duration).map(Self)
        }
        pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
            self.0.checked_sub(duration).map(Self)
        }
    }

    impl Add<Duration> for Instant {
        type Output = Self;
        fn add(self, duration: Duration) -> Self {
            Self(self.0 + duration)
        }
    }
    impl AddAssign<Duration> for Instant {
        fn add_assign(&mut self, duration: Duration) {
            self.0 += duration;
        }
    }
    impl Sub<Duration> for Instant {
        type Output = Self;
        fn sub(self, duration: Duration) -> Self {
            Self(self.0 - duration)
        }
    }
    impl SubAssign<Duration> for Instant {
        fn sub_assign(&mut self, duration: Duration) {
            self.0 -= duration;
        }
    }
    impl Sub for Instant {
        type Output = Duration;
        fn sub(self, earlier: Self) -> Duration {
            self.duration_since(earlier)
        }
    }
}
