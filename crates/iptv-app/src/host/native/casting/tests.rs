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

    const CHILD: &str = "PATINA_RELAY_RETIREMENT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // A concurrent Linux spawn can inherit the listening socket until exec,
        // retaining its port after the relay has closed its own descriptor.
        // Keep the immediate-reuse assertion strict in a process with no spawns.
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "host::native::casting::tests::locally_rejected_loads_release_the_relay_port_before_returning",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }

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
