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

/// `deadline` as the store's inspection API takes it (a standard
/// `Instant`). Natively that is the same value.
#[cfg(feature = "native")]
pub(crate) fn store_deadline(deadline: Instant) -> std::time::Instant {
    deadline
}

/// `deadline` as the store's inspection API takes it. The store crate still
/// bounds inspections with `std::time::Instant`, which cannot be read in a
/// browser build: until it takes a portable deadline, a bounded inspection
/// there is a bug, and says so.
#[cfg(not(feature = "native"))]
pub(crate) fn store_deadline(_deadline: Instant) -> std::time::Instant {
    unimplemented!("the store's inspection deadlines need a portable clock")
}

#[cfg(not(feature = "native"))]
mod host {
    use std::ops::{Add, AddAssign, Sub, SubAssign};
    use std::sync::OnceLock;
    use std::time::Duration;

    pub(super) static CLOCK: OnceLock<super::Clock> = OnceLock::new();

    pub(super) fn clock() -> super::Clock {
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
