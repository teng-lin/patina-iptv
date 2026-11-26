use super::receiver::{self, URL};
use iptv_cast::googlecast::{Command, Error, Session, Status};
use std::time::Duration;
use tokio::{net::TcpListener, time::timeout};
use tokio_rustls::rustls;

#[tokio::test]
async fn live_controls_use_the_owned_session_on_both_tls_versions() {
    for (version, launched) in [
        (&rustls::version::TLS12, false),
        (&rustls::version::TLS12, true),
        (&rustls::version::TLS13, false),
        (&rustls::version::TLS13, true),
    ] {
        let (address, server) = receiver::start(version, launched).await;
        let session = if launched {
            Session::rejoin(address).await
        } else {
            Session::connect(address).await
        }
        .unwrap();
        assert!(session.local_ip().is_loopback());
        assert_eq!(session.lookup(URL).await, Ok(None));
        let id = session.load(URL).await.unwrap();
        assert_eq!(session.lookup(URL).await, Ok(Some(id)));
        assert_eq!(session.status(id).await, Ok(Status::Playing));
        session.command(id, Command::Pause).await.unwrap();
        assert_eq!(session.status(id).await, Ok(Status::Paused));
        for command in [
            Command::Volume(45),
            Command::StepVolume(5),
            Command::ToggleMute,
            Command::Play,
        ] {
            session.command(id, command).await.unwrap();
        }
        session.command(id, Command::Stop).await.unwrap();
        assert_eq!(session.status(id).await, Err(Error::Ended));
        drop(session);
        timeout(Duration::from_secs(5), server)
            .await
            .expect("sender closes the control socket")
            .unwrap();
    }
}

#[tokio::test]
async fn rejoining_an_empty_receiver_leaves_it_idle() {
    let (address, server) = receiver::start(&rustls::version::TLS13, false).await;
    assert!(matches!(Session::rejoin(address).await, Err(Error::Ended)));
    timeout(Duration::from_secs(5), server)
        .await
        .expect("sender closes the control socket")
        .unwrap();
}

#[tokio::test]
async fn a_peer_that_closes_during_tls_never_opens_a_cast_session() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let disconnect = async {
        drop(listener.accept().await.unwrap());
    };
    let (result, ()) = tokio::join!(Session::connect(address), disconnect);
    assert!(matches!(result, Err(Error::Unreachable)));
}

#[tokio::test]
async fn connecting_to_a_running_receiver_reuses_its_transport_without_relaunching() {
    let (address, server) = receiver::start(&rustls::version::TLS13, true).await;
    let session = Session::connect(address).await.unwrap();
    assert_eq!(session.lookup(URL).await, Ok(None));
    drop(session);
    timeout(Duration::from_secs(5), server)
        .await
        .expect("sender closes the control socket")
        .unwrap();
}
