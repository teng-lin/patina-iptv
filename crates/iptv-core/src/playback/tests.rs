use super::*;

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

#[test]
fn watch_time_counts_forward_playback_bounded_by_the_native_heartbeat() {
    let mut time = super::WatchTime::default();
    time.sample(1_000, 40_000);
    time.sample(2_000, 41_000);
    time.sample(3_000, 41_000); // Playback did not advance.
    time.sample(4_000, 5_000); // The native live position was reset.
    time.sample(14_000, 15_000); // A missing heartbeat leaves the gap uncounted.
    time.sample(15_000, 20_000); // A position jump counts at most the elapsed wall time.
    assert_eq!(time.take_ms(), 2_000);
    assert_eq!(time.pending_ms(), 0);
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
