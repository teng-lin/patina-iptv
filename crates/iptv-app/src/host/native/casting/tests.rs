use super::*;

#[test]
fn cast_relay_uses_a_fixed_default_or_an_explicit_tcp_port() {
    assert_eq!(configured_relay_port(None), Ok(49152));
    assert_eq!(configured_relay_port(Some(OsStr::new("53317"))), Ok(53317));
    for value in ["", "0", "65536", "abc"] {
        assert_eq!(
            configured_relay_port(Some(OsStr::new(value))),
            Err(Error::Unavailable)
        );
    }
}

#[test]
fn reasonless_idle_has_a_grace_period_that_playback_resets() {
    let start = Instant::now();
    let mut idle_since = None;
    let mut sample = |status, seconds| {
        observed_state(
            status,
            &mut idle_since,
            start + Duration::from_secs(seconds),
        )
    };
    assert_eq!(sample(Status::Idle, 0), RemoteState::Loading);
    assert_eq!(sample(Status::Paused, 14), RemoteState::Paused);
    assert_eq!(sample(Status::Idle, 15), RemoteState::Loading);
    assert_eq!(sample(Status::Idle, 29), RemoteState::Loading);
    assert_eq!(sample(Status::Idle, 30), RemoteState::Ended);
    for status in [
        Status::Failed,
        Status::Finished,
        Status::Canceled,
        Status::Interrupted,
    ] {
        assert_eq!(sample(Status::Playing, 31), RemoteState::Playing);
        assert_eq!(sample(Status::Idle, 31), RemoteState::Loading);
        assert_eq!(sample(status, 31), RemoteState::Ended);
    }
}

#[tokio::test]
async fn silent_connection_can_recover_after_its_status_deadline() {
    let recovered = status_or_rejoin(
        Instant::now(),
        std::future::pending(),
        Duration::from_secs(15),
        async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(("replacement connection", Status::Paused))
        },
    )
    .await;
    assert_eq!(
        recovered,
        Ok((Some("replacement connection"), Status::Paused))
    );
}

#[tokio::test]
async fn recovery_has_its_own_bound_and_propagates_lost_ownership() {
    assert_eq!(
        status_or_rejoin::<()>(
            Instant::now() + Duration::from_secs(15),
            std::future::ready(Err(googlecast::Error::Unreachable)),
            Duration::ZERO,
            std::future::pending(),
        )
        .await,
        Err(googlecast::Error::Unknown)
    );
    assert_eq!(
        status_or_rejoin::<()>(
            Instant::now(),
            std::future::pending(),
            Duration::from_secs(15),
            std::future::ready(Err(googlecast::Error::Ended)),
        )
        .await,
        Err(googlecast::Error::Ended)
    );
}

#[tokio::test]
async fn definitive_status_on_the_original_connection_skips_rejoin() {
    for (status, expected) in [
        (Ok(Status::Playing), Ok((None, Status::Playing))),
        (Err(googlecast::Error::Ended), Err(googlecast::Error::Ended)),
    ] {
        let actual = status_or_rejoin::<()>(
            Instant::now() + Duration::from_secs(15),
            std::future::ready(status),
            Duration::from_secs(15),
            async { panic!("unexpected replacement connection") },
        )
        .await;
        assert_eq!(actual, expected);
    }
}
