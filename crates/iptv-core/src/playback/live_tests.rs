//! The live policy: no dead end for a stream that played.

use super::*;

fn policy() -> RetryPolicy {
    RetryPolicy::default()
}

/// Opens the stream and lets it play from `now`.
fn plays(h: &mut Health, now: u64) {
    h.resolved(now);
    h.on_state(PlayerState::Ready, now);
    assert_eq!(h.phase(), &Phase::Playing);
}

/// Fails every retry of `h` with `kind` from `now` until `until`; the
/// waits between them.
fn fail_until(h: &mut Health, kind: ErrorKind, mut now: u64, until: u64) -> Vec<u64> {
    let mut waits = Vec::new();
    while now < until {
        let r = h.on_error(kind.clone(), "x".into(), now).expect("retries");
        waits.push(r.at_ms - now);
        assert!(h.due(r, r.at_ms));
        h.resolved(r.at_ms);
        now = r.at_ms + 100;
    }
    waits
}

#[test]
fn a_live_stream_that_played_retries_for_good_with_a_capped_backoff() {
    let mut h = Health::new(policy());
    h.start(true, 0);
    plays(&mut h, 0);
    let waits = fail_until(&mut h, ErrorKind::Http(403), 60_000, 60_000 + 140_000);
    assert_eq!(
        &waits[..5],
        [1_000, 2_000, 4_000, 4_000, 4_000],
        "fast first"
    );
    assert_eq!(&waits[5..7], [5_000, 10_000], "then slower");
    assert!(waits[7..].iter().all(|w| *w == 20_000), "capped: {waits:?}");
    assert!(h.failure().is_none(), "no card within the first 2.5 min");
    assert!(h.reconnecting(), "the last frame stays");
    assert_eq!(
        h.indicator(200_000),
        Some(Waiting::Reconnecting {
            attempt: waits.len() as u32
        }),
        "a small pill"
    );
}

#[test]
fn the_card_shows_after_a_long_outage_and_retries_go_on_behind_it() {
    let p = policy();
    let mut h = Health::new(p);
    h.start(true, 0);
    plays(&mut h, 0);
    let start = 30_000;
    fail_until(&mut h, ErrorKind::Network, start, start + p.card_after_ms);
    let now = start + p.card_after_ms + 200;
    let r = h.on_error(ErrorKind::Network, "x".into(), now).unwrap();
    assert_eq!(
        r.at_ms - now,
        p.background_delay_ms,
        "slower behind the card"
    );
    let f = h.failure().expect("the card after card_after_ms");
    assert!(f.retrying, "retries go on");
    assert_eq!(f.kind, ErrorKind::Network);
    assert_eq!(h.indicator(now + 1_000), None, "no pill under the card");
    // The background retry plays: no card, no pill, a fresh outage.
    assert!(h.due(r, r.at_ms));
    assert!(h.failure().is_some(), "the card stays while it tries");
    plays(&mut h, r.at_ms);
    assert!(h.failure().is_none());
    assert!(!h.reconnecting());
    let later = r.at_ms + p.stable_ms;
    let r = h.on_error(ErrorKind::Network, "x".into(), later).unwrap();
    assert!(
        h.failure().is_none(),
        "a new outage starts without the card"
    );
    assert!(r.at_ms - later < p.slow_delay_ms);
}

#[test]
fn time_away_does_not_count_towards_the_card() {
    let p = policy();
    let mut h = Health::new(p);
    h.start(true, 0);
    plays(&mut h, 0);
    let r = h.on_error(ErrorKind::Network, "x".into(), 1_000).unwrap();
    h.set_paused(true, 1_500);
    h.set_paused(true, 2_000); // unchanged: the pause still counts from 1.5 s
    let back = 1_500 + 10 * 60_000;
    h.set_paused(false, back);
    assert!(h.due(r, back));
    h.resolved(back);
    h.on_error(ErrorKind::Network, "x".into(), back + 100)
        .expect("retries");
    assert!(h.failure().is_none(), "10 min away is not an outage");
}

#[test]
fn a_paused_live_stream_retries_a_gone_http_resource_after_resuming() {
    let mut h = Health::new(policy());
    h.start(true, 0);
    plays(&mut h, 0);
    h.set_paused(true, 1_000);
    h.set_paused(false, 181_000);

    let retry = h
        .on_error(ErrorKind::Http(410), "HTTP 410".into(), 182_000)
        .expect("a fresh live resolution can recover an expired stream resource");

    assert!(h.failure().is_none(), "no dead-end card while retrying");
    assert!(h.due(retry, retry.at_ms), "the live stream retries");
    plays(&mut h, retry.at_ms);
    assert!(h.failure().is_none(), "playback recovery clears the outage");

    let mut catch_up = Health::new(policy());
    catch_up.start(false, 0);
    plays(&mut catch_up, 0);
    assert_eq!(
        catch_up.on_error(ErrorKind::Http(410), "HTTP 410".into(), 1_000),
        None,
        "an expired catch-up resource remains fatal"
    );
}

#[test]
fn try_again_after_the_card_starts_over_at_once() {
    let mut h = Health::new(policy());
    h.start(true, 0);
    plays(&mut h, 0);
    fail_until(&mut h, ErrorKind::Http(509), 1_000, 200_000);
    assert!(h.failure().is_some());
    let old = h.session();
    assert!(h.retry_now(210_000) > old);
    assert!(h.failure().is_none());
    assert_eq!(h.indicator(210_000), Some(Waiting::Loading));
    let r = h
        .on_error(ErrorKind::Http(509), "x".into(), 210_100)
        .unwrap();
    assert_eq!((r.attempt, r.at_ms), (1, 211_100));
}

#[test]
fn errors_are_classified_by_what_a_retry_can_do() {
    use ErrorKind as K;
    for (kind, cause) in [
        (K::Http(401), Cause::Refused),
        (K::Http(403), Cause::Refused),
        (K::Http(429), Cause::Refused),
        (K::Http(458), Cause::Refused),
        (K::Http(509), Cause::Refused),
        (K::Http(404), Cause::Fatal),
        (K::Http(410), Cause::Fatal),
        (K::Format, Cause::Fatal),
        (K::Unavailable, Cause::Fatal),
        (K::Account, Cause::Fatal),
        (K::Network, Cause::Network),
        (K::Http(500), Cause::Transient),
        (K::Http(0), Cause::Transient),
        (K::Stalled, Cause::Transient),
        (K::Ended, Cause::Transient),
    ] {
        assert_eq!(kind.cause(), cause, "{kind:?}");
        assert_eq!(kind.retryable(), cause != Cause::Fatal);
    }
    assert_eq!(K::from_resolve("Auth"), K::Account);
    assert_eq!(K::from_resolve("Network"), K::Network);
    assert_eq!(K::from_resolve("Server"), K::Resolve);
    assert_eq!(
        K::from_player_code("ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT", None),
        K::Network
    );
}
