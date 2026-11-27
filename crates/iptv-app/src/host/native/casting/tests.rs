use super::*;

#[test]
fn a_sender_local_ipv6_zone_cannot_escape_into_the_receiver_url() {
    for address in ["[fe80::1%3]:7000", "[fe80::1]:7000", "[fd12::1%5]:7000"] {
        assert_eq!(
            relay_listen_addr(address.parse().unwrap(), 49152),
            Err(Error::AirPlayRelayAddress)
        );
    }
    for (control, expected) in [
        ("192.0.2.10:7000", "192.0.2.10:49152"),
        ("[fd12::1]:7000", "[fd12::1]:49152"),
    ] {
        assert_eq!(
            relay_listen_addr(control.parse().unwrap(), 49152).unwrap(),
            expected.parse().unwrap()
        );
    }
}

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

#[cfg(feature = "test-support")]
#[tokio::test]
async fn locally_rejected_loads_release_the_relay_port_before_returning() {
    use iptv_cast::test_support;

    for media in [
        Media::live("invalid media URL"),
        Media::live("http://example.test/generated-without-content-type"),
    ] {
        let key_pair = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let receiver = test_support::Receiver::start(
            test_support::Load::Immediate,
            key_pair.cert.der().to_vec(),
            key_pair.key_pair.serialize_der(),
        )
        .await;
        let cast = test_support::sender(receiver.address()).unwrap();
        let routes = cast.discover(Duration::from_secs(1)).await.unwrap();
        let Connect::Ready(connection) = cast.connect(&routes.routes()[0]).await.unwrap() else {
            panic!("unexpected pairing for loopback Chromecast");
        };
        let loopback = receiver.address().ip();
        let (relay, gate) = PlaybackRelay::start_cast(
            "http://example.test/generated.m3u8",
            &[],
            Default::default(),
            SocketAddr::new(loopback, 0),
            loopback,
        )
        .unwrap();
        let listen: SocketAddr = relay.url().split('/').nth(2).unwrap().parse().unwrap();
        assert_eq!(
            Pending::submit(connection, relay, gate, media).await.err(),
            Some(Error::Unsupported),
        );
        let listener = std::net::TcpListener::bind(listen)
            .expect("returning from a rejected load must make the same relay port reusable");
        drop(listener);
        assert!(receiver.events().is_empty());
        timeout(Duration::from_secs(5), receiver.finish())
            .await
            .unwrap()
            .unwrap();
    }
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "current_thread")]
async fn canceled_retirement_and_stop_waiters_share_one_relay_shutdown() {
    use iptv_cast::test_support::{self, Event};
    use std::{future::poll_fn, task::Poll};

    let key_pair = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let (release, stop) = tokio::sync::oneshot::channel();
    let receiver = test_support::Receiver::start_with_rejected_stop(
        key_pair.cert.der().to_vec(),
        key_pair.key_pair.serialize_der(),
        stop,
    )
    .await;
    let cast = test_support::sender(receiver.address()).unwrap();
    let routes = cast.discover(Duration::from_secs(1)).await.unwrap();
    let Connect::Ready(connection) = cast.connect(&routes.routes()[0]).await.unwrap() else {
        panic!("expected loopback Chromecast connection");
    };
    let loopback = receiver.address().ip();
    let (relay, gate) = PlaybackRelay::start_cast(
        "http://example.test/generated.m3u8",
        &[],
        Default::default(),
        SocketAddr::new(loopback, 0),
        loopback,
    )
    .unwrap();
    let listen: SocketAddr = relay.url().split('/').nth(2).unwrap().parse().unwrap();
    let media = Media::live(relay.url())
        .unwrap()
        .with_content_type("application/x-mpegURL")
        .unwrap();
    let session = connection.load(media).unwrap();
    timeout(
        Duration::from_secs(5),
        initial_playback(&mut session.subscribe()),
    )
    .await
    .unwrap()
    .unwrap();
    let playing = Arc::new(Playing::new(session, relay));

    let mut joined_retirement = playing.retire();
    poll_fn(|cx| {
        // There is no executor turn between starting shutdown, canceling the
        // waiter, and joining it again. The real listener is still bound.
        let mut abandoned_retirement = playing.retire();
        assert!(abandoned_retirement.as_mut().poll(cx).is_pending());
        drop(abandoned_retirement);
        assert!(joined_retirement.as_mut().poll(cx).is_pending());
        assert_eq!(
            std::net::TcpListener::bind(listen).unwrap_err().kind(),
            std::io::ErrorKind::AddrInUse,
        );
        Poll::Ready(())
    })
    .await;

    let first = tokio::spawn({
        let playing = playing.clone();
        async move { playing.stop_checked().await }
    });
    timeout(Duration::from_secs(5), joined_retirement)
        .await
        .unwrap();
    drop(std::net::TcpListener::bind(listen).expect("retirement must close the listener"));
    timeout(Duration::from_secs(5), async {
        while !receiver.events().contains(&Event::Stop) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    // Retirement was joined before even the first STOP reached the receiver.
    drop(std::net::TcpListener::bind(listen).expect("cast relay must already be retired"));
    drop(gate);

    let mut second = playing.stop_checked();
    poll_fn(|cx| {
        assert!(second.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(playing.state().await, Ok(RemoteState::Loading));
    release.send(()).unwrap();
    assert_eq!(second.await, Err(Error::StopUnconfirmed));
    assert_eq!(playing.stop_checked().await, Err(Error::StopUnconfirmed));
    assert_eq!(playing.state().await, Err(Error::StopUnconfirmed));
    assert_eq!(receiver.events(), [Event::Load, Event::Stop]);
    timeout(Duration::from_secs(5), receiver.finish())
        .await
        .unwrap()
        .unwrap();
}
