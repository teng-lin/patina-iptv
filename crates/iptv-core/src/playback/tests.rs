use super::*;
use crate::guide::{GROUPS, SyntheticSchedule, group_start};

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
fn player_codes_map_to_kinds() {
    use ErrorKind as K;
    let k = ErrorKind::from_player_code;
    assert_eq!(
        k("ERROR_CODE_IO_NETWORK_CONNECTION_FAILED", None),
        K::Network
    );
    assert_eq!(k("ERROR_CODE_IO_BAD_HTTP_STATUS", Some(403)), K::Http(403));
    assert_eq!(k("ERROR_CODE_IO_BAD_HTTP_STATUS", None), K::Http(0));
    assert_eq!(
        k("ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED", None),
        K::Format
    );
    assert_eq!(k("ERROR_CODE_DECODING_FAILED", None), K::Format);
    assert_eq!(k("ERROR_CODE_BEHIND_LIVE_WINDOW", None), K::Network);
    assert_eq!(k("ERROR_CODE_UNSPECIFIED", None), K::Other);
}

#[test]
fn play_source_headers_are_case_insensitive_and_override() {
    let channel = vec![
        ("user-agent".to_owned(), "ChannelAgent/1".to_owned()),
        ("Referer".to_owned(), "http://ref.example.com/".to_owned()),
    ];
    let mut s = PlaySource::new("http://tv.example.com/1.m3u8");
    s.set_header("User-Agent", "SourceAgent/2");
    s.set_header("X-Blank", "  ");
    let s = s.with_headers(&channel);
    assert_eq!(s.header("USER-AGENT"), Some("ChannelAgent/1"));
    assert_eq!(s.header("referer"), Some("http://ref.example.com/"));
    assert_eq!(s.headers.len(), 2, "replaced, blank skipped");
    let mut s = s;
    s.default_header("User-Agent", "Fallback/3");
    assert_eq!(s.header("User-Agent"), Some("ChannelAgent/1"));
}

#[test]
fn previous_channel_toggles() {
    let mut h = ChannelHistory::default();
    assert_eq!(h.previous(), None);
    h.played("a");
    h.played("a");
    assert_eq!(h.previous(), None, "replaying is not a change");
    h.played("b");
    h.played("c");
    assert_eq!((h.current(), h.previous()), (Some("c"), Some("b")));
    let prev = h.previous().unwrap().to_owned();
    h.played(&prev);
    assert_eq!((h.current(), h.previous()), (Some("b"), Some("c")));
}

#[test]
fn recent_channels_are_newest_first_without_repeats_and_bounded() {
    let mut h = ChannelHistory::default();
    for key in ["a", "b", "c", "a"] {
        h.played(key);
    }
    assert_eq!(h.recent(), ["a", "c", "b"]);
    for i in 0..RECENT_MAX + 5 {
        h.played(&format!("k{i}"));
    }
    assert_eq!(h.recent().len(), RECENT_MAX);
    assert_eq!(h.current(), Some(format!("k{}", RECENT_MAX + 4).as_str()));
}

#[test]
fn zapping_wraps_within_the_group() {
    let s = SyntheticSchedule;
    let (g0, g1) = (group_start(0), group_start(1));
    assert_eq!(group_range(&s, g0), g0..g1);
    assert_eq!(group_range(&s, g1 - 1), g0..g1);
    assert_eq!(
        zap_target(&s, g1 - 1, 1),
        g0,
        "down from the last wraps to the first"
    );
    assert_eq!(
        zap_target(&s, g0, -1),
        g1 - 1,
        "up from the first wraps to the last"
    );
    assert_eq!(zap_target(&s, g0, 1), g0 + 1);
    let last = group_start(GROUPS - 1);
    let end = crate::guide::CHANNELS;
    assert_eq!(group_range(&s, end - 1), last..end);
    assert_eq!(zap_target(&s, end - 1, 1), last);
}

/// A schedule with uneven groups, an empty group and one channel groups.
struct Groups(Vec<usize>, usize);

impl Schedule for Groups {
    fn channel_count(&self) -> usize {
        self.1
    }
    fn group_count(&self) -> usize {
        self.0.len()
    }
    fn group_start(&self, group: usize) -> usize {
        self.0[group]
    }
    fn program_at(&self, _: usize, minute: i32) -> crate::guide::Program {
        crate::guide::Program {
            start: minute,
            end: minute + 1,
        }
    }
    fn programs_between(&self, _: usize, _: i32, _: i32) -> Vec<crate::guide::Program> {
        Vec::new()
    }
    fn catchup_days(&self, _: usize) -> u8 {
        0
    }
    fn channel_name(&self, _: usize) -> String {
        String::new()
    }
    fn group_name(&self, _: usize) -> String {
        String::new()
    }
    fn title(&self, _: usize, _: crate::guide::Program) -> String {
        String::new()
    }
}

#[test]
fn zapping_handles_uneven_and_single_channel_groups() {
    // Groups: [0,3) [3,4) [4,8)
    let s = Groups(vec![0, 3, 4], 8);
    assert_eq!(group_range(&s, 3), 3..4);
    assert_eq!(zap_target(&s, 3, 1), 3, "a one-channel group stays");
    assert_eq!(zap_target(&s, 7, 1), 4);
    assert_eq!(zap_target(&s, 2, 1), 0);
    assert_eq!(zap_target(&s, 4, -3), 5);
    let none = Groups(vec![], 5);
    assert_eq!(zap_target(&none, 4, 1), 0, "no groups: the whole list");
    let empty = Groups(vec![], 0);
    assert_eq!(zap_target(&empty, 0, 1), 0);
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
    h.on_error(ErrorKind::Http(404), "404".into(), 2_100);
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

#[test]
fn a_player_state_reads_back_from_its_host_name() {
    let all = [
        PlayerState::Idle,
        PlayerState::Buffering,
        PlayerState::Ready,
        PlayerState::Ended,
    ];
    for state in all {
        assert_eq!(PlayerState::parse(state.name()), Some(state));
    }
    assert_eq!(PlayerState::parse("unknown"), None);
    assert_eq!(PlayerState::parse("Ready"), None, "names are lower case");
}
