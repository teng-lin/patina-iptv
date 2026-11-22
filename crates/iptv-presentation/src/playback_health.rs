//! Player health for one playback session: retries with backoff, stall
//! detection, and when the viewer sees a waiting pill or a failure card.
//!
//! A live channel never dead-ends on a failure that a retry can fix: after
//! the fast attempts it retries slowly for as long as it is the session's
//! (the channel stays selected). The card shows only for a failure retrying
//! cannot fix ([`iptv_core::playback::Cause::Fatal`]), for a stream that played after it
//! has been down [`RetryPolicy::card_after_ms`], and for one that never
//! played once the fast attempts are used up; behind the card the retries
//! go on every [`RetryPolicy::background_delay_ms`], and the stream plays
//! again by itself when the provider lets it. Catch-up keeps a finite
//! policy: a program that played retries the fast attempts (the app
//! reopens it where it was), one that never played fails at once (a dead
//! archive must not loop).
//!
//! A played live stream also retries HTTP 404/410: reopening can resolve a
//! fresh resource after the provider's rolling live window advances.

use iptv_core::playback::{ErrorKind, Failure, PlayerState};

mod states;
pub use states::{Phase, Retry, RetryPolicy, Waiting};

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

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy {
            first_delay_ms: 1_000,
            max_delay_ms: 8_000,
            max_attempts: 4,
            slow_first_delay_ms: 5_000,
            slow_delay_ms: 20_000,
            card_after_ms: 120_000,
            background_delay_ms: 30_000,
            stable_ms: 10_000,
            stall_ms: 15_000,
            quiet_reconnect_ms: 1_000,
        }
    }

    fn playing(h: &mut Health, now: u64) {
        h.resolved(now);
        assert_eq!(h.on_state(PlayerState::Buffering, now), None);
        assert_eq!(h.on_state(PlayerState::Ready, now), None);
        assert_eq!(h.phase(), &Phase::Playing);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let p = policy();
        let delays: Vec<u64> = (1..=6).map(|a| p.delay_ms(a)).collect();
        assert_eq!(delays, [1_000, 2_000, 4_000, 8_000, 8_000, 8_000]);
        assert_eq!(RetryPolicy::default().delay_ms(100), 4_000, "no overflow");
        let live: Vec<u64> = (1..=9)
            .map(|a| RetryPolicy::default().live_delay_ms(a))
            .collect();
        assert_eq!(
            live,
            [
                1_000, 2_000, 4_000, 4_000, 4_000, 5_000, 10_000, 20_000, 20_000
            ],
            "fast, then slow and capped"
        );
        assert_eq!(RetryPolicy::default().live_delay_ms(u32::MAX), 20_000);
    }

    #[test]
    fn live_errors_retry_fast_then_show_the_card_and_go_on_retrying() {
        let mut h = Health::new(policy());
        let session = h.start(true, 0);
        h.resolved(0);
        let mut now = 100;
        let mut waits = Vec::new();
        for attempt in 1..=4 {
            let r = h
                .on_error(ErrorKind::Network, "ERROR_CODE_IO".into(), now)
                .expect("retry scheduled");
            assert_eq!((r.attempt, r.session), (attempt, session));
            waits.push(r.at_ms - now);
            assert!(h.waiting(), "retrying shows the waiting indicator");
            assert!(h.failure().is_none(), "no card during the fast attempts");
            assert!(!h.due(r, r.at_ms - 1), "not before its time");
            assert!(h.due(r, r.at_ms));
            assert_eq!(h.phase(), &Phase::Resolving);
            h.resolved(r.at_ms);
            now = r.at_ms + 50;
        }
        assert_eq!(waits, [1_000, 2_000, 4_000, 8_000]);
        // A stream that never played: the card after the fast attempts, and
        // slow retries behind it.
        let r = h
            .on_error(ErrorKind::Network, "x".into(), now)
            .expect("retries go on");
        assert_eq!(r.at_ms - now, 30_000);
        let f = h.failure().expect("the card shows");
        assert_eq!(
            (f.kind.clone(), f.attempts, f.retrying),
            (ErrorKind::Network, 4, true)
        );
        assert!(!h.waiting(), "the card, not a pill");
        assert_eq!(h.indicator(now), None);
        // Further player noise does not drop the card.
        assert_eq!(h.on_state(PlayerState::Buffering, now + 1), None);
        assert!(h.failure().is_some());
        // A background retry that fails keeps the card and waits again.
        assert!(h.due(r, r.at_ms));
        h.resolved(r.at_ms);
        assert!(h.failure().is_some(), "the card stays while it tries");
        let r2 = h
            .on_error(ErrorKind::Network, "x".into(), r.at_ms + 500)
            .unwrap();
        assert_eq!(r2.at_ms - (r.at_ms + 500), 30_000);
        // One that plays clears it.
        assert!(h.due(r2, r2.at_ms));
        playing(&mut h, r2.at_ms);
        assert!(h.failure().is_none());
    }

    #[test]
    fn a_manual_retry_starts_over() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        h.resolved(0);
        for _ in 0..5 {
            if let Some(r) = h.on_error(ErrorKind::Network, "x".into(), 10) {
                assert!(h.due(r, r.at_ms));
            }
        }
        assert!(h.failure().is_some(), "the card, retries behind it");
        let old = h.session();
        let new = h.retry_now(20_000);
        assert!(new > old);
        assert_eq!(h.phase(), &Phase::Resolving);
        assert!(h.failure().is_none(), "Try again drops the card");
        let r = h.on_error(ErrorKind::Network, "x".into(), 20_001).unwrap();
        assert_eq!(r.attempt, 1, "attempts were reset");
        assert_eq!(r.at_ms, 21_001, "the backoff starts again");
    }

    #[test]
    fn only_fatal_failures_stop_a_live_stream() {
        for kind in [
            ErrorKind::Http(404),
            ErrorKind::Http(410),
            ErrorKind::Format,
            ErrorKind::Unavailable,
            ErrorKind::Account,
        ] {
            let mut h = Health::new(policy());
            h.start(true, 0);
            assert_eq!(h.on_error(kind.clone(), "x".into(), 5), None, "{kind:?}");
            let f = h.failure().expect("the card");
            assert_eq!((f.attempts, f.retrying), (0, false), "{kind:?}");
        }
        for kind in [
            ErrorKind::Http(401),
            ErrorKind::Http(403),
            ErrorKind::Http(429),
            ErrorKind::Http(458),
            ErrorKind::Http(503),
            ErrorKind::Http(509),
            ErrorKind::Network,
            ErrorKind::Stalled,
            ErrorKind::Resolve,
        ] {
            let mut h = Health::new(policy());
            h.start(true, 0);
            assert!(
                h.on_error(kind.clone(), "x".into(), 5).is_some(),
                "{kind:?}"
            );
            assert!(h.failure().is_none(), "{kind:?}");
        }
    }

    #[test]
    fn catch_up_retries_a_few_times_only_once_it_played() {
        let mut archive = Health::new(policy());
        archive.start(false, 0);
        assert_eq!(
            archive.on_error(ErrorKind::Network, "x".into(), 5),
            None,
            "an archive that never played fails at once"
        );
        assert!(archive.failure().is_some());
        let mut h = Health::new(policy());
        h.start(false, 0);
        playing(&mut h, 0);
        let mut now = 1_000;
        for attempt in 1..=4 {
            let r = h.on_error(ErrorKind::Http(403), "x".into(), now).unwrap();
            assert_eq!(r.attempt, attempt);
            assert!(h.reconnecting(), "keeps its last frame");
            assert!(h.due(r, r.at_ms));
            h.resolved(r.at_ms);
            now = r.at_ms + 10;
        }
        assert_eq!(h.on_error(ErrorKind::Http(403), "x".into(), now), None);
        let f = h.failure().unwrap();
        assert_eq!((f.attempts, f.retrying), (4, false));
    }

    #[test]
    fn stable_playback_resets_the_attempt_count() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        h.resolved(0);
        let r = h.on_error(ErrorKind::Network, "x".into(), 10).unwrap();
        assert_eq!(r.attempt, 1);
        assert!(h.due(r, r.at_ms));
        playing(&mut h, r.at_ms);
        // Short playback: the next error is attempt 2.
        let r = h
            .on_error(ErrorKind::Network, "x".into(), r.at_ms + 500)
            .unwrap();
        assert_eq!(r.attempt, 2);
        assert!(h.due(r, r.at_ms));
        playing(&mut h, r.at_ms);
        // A rebuffer does not restart the stability clock.
        assert_eq!(h.on_state(PlayerState::Buffering, r.at_ms + 4_000), None);
        assert_eq!(h.on_state(PlayerState::Ready, r.at_ms + 5_000), None);
        let r = h
            .on_error(ErrorKind::Network, "x".into(), r.at_ms + 10_000)
            .unwrap();
        assert_eq!(r.attempt, 1, "played 10 s: counts afresh");
    }

    #[test]
    fn a_live_stream_that_ends_reconnects_at_once() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        playing(&mut h, 0);
        let r = h.on_state(PlayerState::Ended, 30_000).expect("reconnect");
        assert_eq!(r.at_ms, 30_000);
        // An archive that ends is simply over.
        let mut a = Health::new(policy());
        a.start(false, 0);
        playing(&mut a, 0);
        assert_eq!(a.on_state(PlayerState::Ended, 60_000), None);
        assert_eq!(a.phase(), &Phase::Idle);
        assert!(a.failure().is_none());
    }

    #[test]
    fn loading_or_buffering_too_long_is_a_stall() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        h.resolved(100);
        assert_eq!(h.stall_deadline(), Some(15_100));
        assert_eq!(h.tick(15_099), None);
        let r = h.tick(15_100).expect("stall retries");
        assert_eq!(
            h.phase(),
            &Phase::Retrying {
                attempt: 1,
                at_ms: r.at_ms,
                last: ErrorKind::Stalled
            }
        );
        assert!(h.due(r, r.at_ms));
        playing(&mut h, r.at_ms);
        assert_eq!(h.stall_deadline(), None, "no deadline while playing");
        h.on_state(PlayerState::Buffering, 30_000);
        assert_eq!(h.stall_deadline(), Some(45_000));
        // Paused: waiting is expected, the deadline restarts on resume.
        h.set_paused(true, 31_000);
        assert_eq!(h.stall_deadline(), None);
        assert_eq!(h.tick(50_000), None);
        h.set_paused(false, 50_000);
        assert_eq!(h.stall_deadline(), Some(65_000));
    }

    #[test]
    fn stale_retries_and_sessions_are_ignored() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        let r = h.on_error(ErrorKind::Network, "x".into(), 1).unwrap();
        // The viewer zapped meanwhile.
        h.start(true, 500);
        assert!(!h.due(r, r.at_ms), "older session");
        h.stop();
        assert_eq!(h.phase(), &Phase::Idle);
        assert_eq!(h.on_error(ErrorKind::Network, "x".into(), 2), None);
        assert_eq!(h.on_state(PlayerState::Ready, 3), None);
        assert_eq!(h.phase(), &Phase::Idle);
    }

    #[test]
    fn the_previous_stream_reports_nothing_while_resolving() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        assert_eq!(h.on_state(PlayerState::Ready, 5), None);
        assert_eq!(h.on_state(PlayerState::Ended, 6), None);
        assert_eq!(
            h.phase(),
            &Phase::Resolving,
            "still waiting for the address"
        );
        // A resolver failure is an error like any other.
        assert!(
            h.on_error(ErrorKind::Resolve, "Network".into(), 7)
                .is_some()
        );
    }

    #[test]
    fn a_stream_that_played_reconnects_quietly_for_a_moment() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        playing(&mut h, 0);
        assert!(!h.reconnecting());
        assert_eq!(h.indicator(5_000), None, "playing shows nothing");
        // The server closed the live stream: reconnect at once, keep the frame.
        let r = h.on_state(PlayerState::Ended, 20_000).expect("reconnects");
        assert!(h.reconnecting(), "the player keeps the last frame");
        assert_eq!(h.indicator(20_000), None, "no pill for a quick reconnect");
        assert_eq!(h.indicator_due(), Some(21_000));
        assert!(h.due(r, 20_000));
        h.resolved(20_010);
        assert_eq!(h.on_state(PlayerState::Buffering, 20_100), None);
        assert_eq!(h.indicator(20_999), None);
        assert_eq!(
            h.indicator(21_000),
            Some(Waiting::Reconnecting { attempt: 1 }),
            "a slow reconnect says so"
        );
        // Further attempts count from the first one's start.
        let r = h
            .on_error(ErrorKind::Network, "ERROR_CODE_IO".into(), 21_500)
            .expect("retries");
        assert_eq!(h.indicator_due(), Some(21_000));
        assert_eq!(
            h.indicator(21_500),
            Some(Waiting::Reconnecting { attempt: 2 })
        );
        assert!(h.due(r, r.at_ms));
        h.resolved(r.at_ms);
        assert_eq!(h.on_state(PlayerState::Ready, r.at_ms + 300), None);
        assert!(!h.reconnecting(), "it plays again");
        assert_eq!(h.indicator(r.at_ms + 300), None);
        assert_eq!(h.indicator_due(), None);
    }

    #[test]
    fn a_stream_that_never_played_shows_loading_and_retries_openly() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        assert_eq!(h.indicator(0), Some(Waiting::Loading));
        h.resolved(10);
        assert_eq!(h.indicator(10), Some(Waiting::Loading));
        h.on_error(ErrorKind::Network, "ERROR_CODE_IO".into(), 500)
            .expect("retries");
        assert!(!h.reconnecting(), "nothing to keep on screen");
        assert_eq!(h.indicator(500), Some(Waiting::Reconnecting { attempt: 1 }));
        assert_eq!(h.indicator_due(), None);
    }

    #[test]
    fn buffering_failures_and_new_streams_end_a_reconnect() {
        let mut h = Health::new(policy());
        h.start(true, 0);
        playing(&mut h, 0);
        assert_eq!(h.on_state(PlayerState::Buffering, 1_000), None);
        assert_eq!(h.indicator(1_000), Some(Waiting::Buffering));
        assert_eq!(h.on_state(PlayerState::Ready, 1_500), None);
        h.on_state(PlayerState::Ended, 2_000).expect("reconnects");
        assert!(h.reconnecting());
        // Gone: the error stays on screen, with the black shutter.
        h.on_error(ErrorKind::Format, "unsupported format".into(), 2_100);
        assert!(h.failure().is_some());
        assert!(!h.reconnecting());
        assert_eq!(h.indicator(9_000), None, "the failure card, not a pill");
        // A channel change is not a reconnect.
        h.start(true, 10_000);
        playing(&mut h, 10_000);
        h.on_state(PlayerState::Ended, 12_000).expect("reconnects");
        assert!(h.reconnecting());
        h.start(true, 12_100);
        assert!(!h.reconnecting());
        assert_eq!(h.indicator(12_100), Some(Waiting::Loading));
        // Catch-up that ends is over, not reconnected.
        h.start(false, 20_000);
        assert!(!h.live());
        playing(&mut h, 20_000);
        assert_eq!(h.on_state(PlayerState::Ended, 30_000), None);
        assert!(!h.reconnecting());
    }
}

#[cfg(test)]
mod live_tests;
