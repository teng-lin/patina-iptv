//! Player health for one playback session: retries with backoff, stall
//! detection, and when the viewer sees a waiting pill or a failure card.
//!
//! A live channel never dead-ends on a failure that a retry can fix: after
//! the fast attempts it retries slowly for as long as it is the session's
//! (the channel stays selected). The card shows only for a failure retrying
//! cannot fix ([`super::Cause::Fatal`]), for a stream that played after it
//! has been down [`RetryPolicy::card_after_ms`], and for one that never
//! played once the fast attempts are used up; behind the card the retries
//! go on every [`RetryPolicy::background_delay_ms`], and the stream plays
//! again by itself when the provider lets it. Catch-up keeps a finite
//! policy: a programme that played retries the fast attempts (the app
//! reopens it where it was), one that never played fails at once (a dead
//! archive must not loop).
//!
//! A played live stream also retries HTTP 404/410: reopening can resolve a
//! fresh resource after the provider's rolling live window advances.

use super::error::{ErrorKind, Failure};
use super::states::{Phase, PlayerState, Retry, RetryPolicy, Waiting};

/// Player health for one playback session (see the module's docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub policy: RetryPolicy,
    phase: Phase,
    live: bool,
    paused: bool,
    /// Since when playback is paused (or the app away), while it is.
    paused_at: Option<u64>,
    attempts: u32,
    since_ms: u64,
    ready_since: Option<u64>,
    session: u64,
    /// The session's stream played (a retry then reconnects it).
    played: bool,
    /// Since when a stream that played is being reconnected: from its
    /// first retry until it plays again.
    reconnecting_since: Option<u64>,
    /// Since when the stream has been failing: from the first error after
    /// it played (or started) until it plays again.
    outage_since: Option<u64>,
    /// The failure card shown while retries go on behind it.
    card: Option<Failure>,
}

impl Default for Health {
    fn default() -> Self {
        Self::new(RetryPolicy::default())
    }
}

impl Health {
    pub fn new(policy: RetryPolicy) -> Self {
        Self {
            policy,
            phase: Phase::Idle,
            live: true,
            paused: false,
            paused_at: None,
            attempts: 0,
            since_ms: 0,
            ready_since: None,
            session: 0,
            played: false,
            reconnecting_since: None,
            outage_since: None,
            card: None,
        }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Bumped by every [`Health::start`]; results of older sessions are stale.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// The failure card to show: retries stopped, or go on behind it
    /// ([`Failure::retrying`]).
    pub fn failure(&self) -> Option<&Failure> {
        match &self.phase {
            Phase::Failed(f) => Some(f),
            _ => self.card.as_ref(),
        }
    }

    /// Resolving, loading, buffering or waiting to retry, with no failure
    /// card: the viewer sees a waiting indicator.
    pub fn waiting(&self) -> bool {
        self.card.is_none()
            && matches!(
                self.phase,
                Phase::Resolving | Phase::Loading | Phase::Buffering | Phase::Retrying { .. }
            )
    }

    /// What the viewer sees while waiting, at `now`: nothing while a stream
    /// that played reconnects for less than
    /// [`RetryPolicy::quiet_reconnect_ms`] (its last frame stays), then
    /// "reconnecting" until it plays again; nothing behind the card.
    pub fn indicator(&self, now: u64) -> Option<Waiting> {
        if self.card.is_some() {
            return None;
        }
        let reconnecting = Waiting::Reconnecting {
            attempt: self.attempts,
        };
        match (&self.phase, self.reconnecting_since) {
            (Phase::Buffering, _) => Some(Waiting::Buffering),
            (Phase::Resolving | Phase::Loading | Phase::Retrying { .. }, Some(since)) => {
                let quiet = now.saturating_sub(since) < self.policy.quiet_reconnect_ms;
                (!quiet).then_some(reconnecting)
            }
            (Phase::Retrying { .. }, None) => Some(reconnecting),
            (Phase::Resolving | Phase::Loading, None) => Some(Waiting::Loading),
            _ => None,
        }
    }

    /// When [`Health::indicator`] changes on its own (a quiet reconnect
    /// starts showing), if it will.
    pub fn indicator_due(&self) -> Option<u64> {
        let since = self.reconnecting_since?;
        let due = since + self.policy.quiet_reconnect_ms;
        let waiting = matches!(
            self.phase,
            Phase::Resolving | Phase::Loading | Phase::Retrying { .. }
        );
        (waiting && self.card.is_none()).then_some(due)
    }

    /// A stream that played is being reconnected (the same session): the
    /// player keeps its last frame instead of going black.
    pub fn reconnecting(&self) -> bool {
        self.reconnecting_since.is_some()
    }

    /// The session's stream played at least once.
    pub fn played(&self) -> bool {
        self.played
    }

    /// The session plays live (not catch-up).
    pub fn live(&self) -> bool {
        self.live
    }

    /// A new stream was requested (channel change, live/catch-up switch or a
    /// manual retry): fresh attempt count, new session.
    pub fn start(&mut self, live: bool, now: u64) -> u64 {
        self.session += 1;
        self.live = live;
        self.attempts = 0;
        self.paused = false;
        self.paused_at = None;
        self.played = false;
        self.reconnecting_since = None;
        self.outage_since = None;
        self.card = None;
        self.enter(Phase::Resolving, now);
        self.session
    }

    /// Playback stopped (nothing playing).
    pub fn stop(&mut self) {
        self.session += 1;
        self.attempts = 0;
        self.phase = Phase::Idle;
        self.ready_since = None;
        self.played = false;
        self.reconnecting_since = None;
        self.outage_since = None;
        self.card = None;
    }

    /// The viewer's "Try again": like [`Health::start`] with the same live
    /// flag (at once, with the backoff reset).
    pub fn retry_now(&mut self, now: u64) -> u64 {
        let live = self.live;
        self.start(live, now)
    }

    /// A suspended native player needs fresh authority, without consuming retries.
    /// Stopped sessions must never restart because of a delayed host event.
    pub fn reauthorize(&mut self, now: u64) -> Option<u64> {
        if self.phase == Phase::Idle {
            return None;
        }
        self.session += 1;
        self.card = None;
        self.reconnecting_since = None;
        self.outage_since = None;
        self.enter(Phase::Resolving, now);
        Some(self.session)
    }

    /// The stream address is known and handed to the player.
    pub fn resolved(&mut self, now: u64) {
        if self.phase == Phase::Resolving {
            self.enter(Phase::Loading, now);
        }
    }

    /// Paused by the viewer, or the app is away (its player stopped).
    /// Time held does not count as a stall, nor towards the failure card.
    pub fn set_paused(&mut self, paused: bool, now: u64) {
        if self.paused && !paused {
            // Waiting while paused is not a stall: the clock starts again.
            self.since_ms = now;
            if let (Some(at), Some(since)) = (self.paused_at, self.outage_since) {
                self.outage_since = Some(since + now.saturating_sub(at));
            }
            self.paused_at = None;
        } else if !self.paused && paused {
            self.paused_at = Some(now);
        }
        self.paused = paused;
    }

    /// A scheduled retry fired: `true` if it still applies (then re-resolve
    /// and reopen the stream; the phase is `Resolving` again).
    pub fn due(&mut self, retry: Retry, now: u64) -> bool {
        let current =
            matches!(self.phase, Phase::Retrying { attempt, .. } if attempt == retry.attempt);
        if retry.session != self.session || !current || now < retry.at_ms {
            return false;
        }
        self.enter(Phase::Resolving, now);
        true
    }

    /// The host reported a state change.
    pub fn on_state(&mut self, state: PlayerState, now: u64) -> Option<Retry> {
        match (state, &self.phase) {
            // The player still shows the previous stream until resolved.
            (_, Phase::Idle | Phase::Resolving | Phase::Failed(_) | Phase::Retrying { .. }) => None,
            (PlayerState::Ready, _) => {
                if self.phase != Phase::Playing {
                    // A rebuffer keeps the stability clock running.
                    self.ready_since.get_or_insert(now);
                    self.played = true;
                    self.reconnecting_since = None;
                    self.outage_since = None;
                    self.card = None;
                    self.enter(Phase::Playing, now);
                }
                None
            }
            (PlayerState::Buffering, Phase::Playing) => {
                self.enter(Phase::Buffering, now);
                None
            }
            (PlayerState::Ended, _) if self.live => {
                self.on_error(ErrorKind::Ended, "ended".into(), now)
            }
            (PlayerState::Ended, _) => {
                // A finished archive: nothing more to play, not an error.
                self.enter(Phase::Idle, now);
                None
            }
            _ => None,
        }
    }

    /// The host or the resolver reported an error. Returns the retry to
    /// schedule, or `None` when the failure stays on screen for good.
    pub fn on_error(&mut self, kind: ErrorKind, detail: String, now: u64) -> Option<Retry> {
        if matches!(self.phase, Phase::Idle | Phase::Failed(_)) {
            return None;
        }
        if self
            .ready_since
            .is_some_and(|t| now.saturating_sub(t) >= self.policy.stable_ms)
        {
            self.attempts = 0;
        }
        self.ready_since = None;
        let since = *self.outage_since.get_or_insert(now);
        let p = self.policy;
        // A live URL can expire while paused or behind the provider's
        // rolling window. Reopening it resolves a fresh live resource, so a
        // 404/410 after playback has started is retryable for live sessions.
        let expired_live_resource =
            self.live && self.played && matches!(kind, ErrorKind::Http(404 | 410));
        let retry = (kind.retryable() || expired_live_resource)
            && (self.live || (self.played && self.attempts < p.max_attempts));
        if !retry {
            let attempts = self.attempts;
            self.reconnecting_since = None;
            self.card = None;
            let failure = Failure {
                kind,
                detail,
                attempts,
                retrying: false,
            };
            self.enter(Phase::Failed(failure), now);
            return None;
        }
        let card = self.card.is_some()
            || (self.played && now.saturating_sub(since) >= p.card_after_ms)
            || (!self.played && self.attempts >= p.max_attempts);
        self.attempts += 1;
        let attempt = self.attempts;
        if self.played {
            self.reconnecting_since.get_or_insert(now);
        }
        let delay = if card {
            p.background_delay_ms
        } else if kind == ErrorKind::Ended && attempt == 1 {
            // A live stream that closed after playing reconnects at once.
            0
        } else {
            p.live_delay_ms(attempt)
        };
        if card {
            self.card = Some(Failure {
                kind: kind.clone(),
                detail,
                attempts: attempt - 1,
                retrying: true,
            });
        }
        let at_ms = now + delay;
        self.enter(
            Phase::Retrying {
                attempt,
                at_ms,
                last: kind,
            },
            now,
        );
        Some(Retry {
            at_ms,
            attempt,
            session: self.session,
        })
    }

    /// When [`Health::tick`] should next run (a stall deadline), if ever.
    pub fn stall_deadline(&self) -> Option<u64> {
        match self.phase {
            Phase::Resolving | Phase::Loading | Phase::Buffering if !self.paused => {
                Some(self.since_ms + self.policy.stall_ms)
            }
            _ => None,
        }
    }

    /// Checks for a stall: loading or buffering past the deadline counts as
    /// an error ([`ErrorKind::Stalled`]).
    pub fn tick(&mut self, now: u64) -> Option<Retry> {
        match self.stall_deadline() {
            Some(at) if now >= at => self.on_error(ErrorKind::Stalled, "stalled".into(), now),
            _ => None,
        }
    }

    fn enter(&mut self, phase: Phase, now: u64) {
        self.phase = phase;
        self.since_ms = now;
    }
}

#[cfg(test)]
mod foreground_tests {
    use super::*;

    #[test]
    fn foreground_loading_vod_does_not_consume_retry_budget_or_restart_stop() {
        let mut health = Health::default();
        let session = health.start(false, 0);
        health.resolved(1);
        health.set_paused(true, 2);
        let next = health
            .reauthorize(100)
            .expect("loading session remains active");
        assert_ne!(session, next);
        assert_eq!(health.attempts, 0);
        assert!(health.paused);
        assert!(matches!(health.phase, Phase::Resolving));
        health.resolved(101);
        health.stop();
        assert_eq!(health.reauthorize(200), None);
    }
}
