//! The retry policy and the states [`super::Health`] moves through and
//! reports.

use super::error::{ErrorKind, Failure};

/// Retry and stall timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Delay before the first retry; doubles per fast attempt.
    pub first_delay_ms: u64,
    pub max_delay_ms: u64,
    /// Fast retries. Then a live stream retries slowly
    /// ([`RetryPolicy::slow_first_delay_ms`] doubling up to
    /// [`RetryPolicy::slow_delay_ms`]) for good; catch-up gives up.
    pub max_attempts: u32,
    /// Live: the wait before the first retry after the fast ones.
    pub slow_first_delay_ms: u64,
    /// Live: the longest wait between slow retries.
    pub slow_delay_ms: u64,
    /// Live: a stream that played and has been down this long shows the
    /// failure card (its retries go on behind it).
    pub card_after_ms: u64,
    /// Live: the wait between retries behind the failure card.
    pub background_delay_ms: u64,
    /// Playing this long without trouble resets the attempt count.
    pub stable_ms: u64,
    /// Loading or buffering longer than this is a stall.
    pub stall_ms: u64,
    /// A stream that played and is being reconnected keeps its last frame
    /// and shows no waiting indicator for this long (most reconnects of a
    /// live stream the server closed take less).
    pub quiet_reconnect_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            first_delay_ms: 1_000,
            max_delay_ms: 4_000,
            max_attempts: 5,
            slow_first_delay_ms: 5_000,
            slow_delay_ms: 20_000,
            card_after_ms: 150_000,
            background_delay_ms: 30_000,
            stable_ms: 10_000,
            stall_ms: 15_000,
            quiet_reconnect_ms: 1_000,
        }
    }
}

impl RetryPolicy {
    /// The wait before fast retry number `attempt` (1-based).
    pub fn delay_ms(&self, attempt: u32) -> u64 {
        let shift = attempt.saturating_sub(1).min(20);
        self.first_delay_ms
            .saturating_mul(1 << shift)
            .min(self.max_delay_ms)
    }

    /// The wait before live retry number `attempt`: the fast ones (by
    /// default 1, 2, 4, 4, 4 s), then slow ones for good (5, 10, then
    /// every 20 s).
    pub fn live_delay_ms(&self, attempt: u32) -> u64 {
        if attempt <= self.max_attempts {
            return self.delay_ms(attempt);
        }
        let shift = (attempt - self.max_attempts - 1).min(20);
        self.slow_first_delay_ms
            .saturating_mul(1 << shift)
            .min(self.slow_delay_ms)
    }
}

/// The player state the host reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Buffering,
    Ready,
    Ended,
}

impl PlayerState {
    /// The host's state names (`idle`, `buffering`, `ready`, `ended`).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "idle" => Self::Idle,
            "buffering" => Self::Buffering,
            "ready" => Self::Ready,
            "ended" => Self::Ended,
            _ => return None,
        })
    }

    /// The host's name for it: what [`PlayerState::parse`] reads.
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Buffering => "buffering",
            Self::Ready => "ready",
            Self::Ended => "ended",
        }
    }
}

/// Where playback stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing requested.
    Idle,
    /// Getting the stream address (may need a provider round trip).
    Resolving,
    /// The player opened the stream and has not shown video yet.
    Loading,
    Playing,
    /// Playing stopped to refill the buffer.
    Buffering,
    /// Waiting to retry; `at_ms` is when.
    Retrying {
        attempt: u32,
        at_ms: u64,
        last: ErrorKind,
    },
    /// No more retries: stays until the viewer retries or plays something
    /// else.
    Failed(Failure),
}

/// The waiting indicator over the video ([`Health::indicator`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waiting {
    Loading,
    Buffering,
    /// Retry `attempt` of a stream (it played, or failed to start).
    Reconnecting {
        attempt: u32,
    },
}

/// A retry the shell should run at `at_ms` (then call [`Health::due`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retry {
    pub at_ms: u64,
    pub attempt: u32,
    /// The playback session it belongs to; a newer session drops it.
    pub session: u64,
}
