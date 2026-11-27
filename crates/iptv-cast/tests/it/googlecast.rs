use super::receiver::{self, AppState, URL};
use iptv_cast::googlecast::{Command, Error, Session, Status};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, time::timeout};
use tokio_rustls::{TlsAcceptor, rustls};

#[tokio::test]
async fn live_controls_use_the_owned_session_on_both_tls_versions() {
    for (version, initial) in [
        (&rustls::version::TLS12, AppState::AwaitingLaunch),
        (&rustls::version::TLS12, AppState::Running),
        (&rustls::version::TLS13, AppState::AwaitingLaunch),
        (&rustls::version::TLS13, AppState::Running),
    ] {
        let (address, server) = receiver::start(version, initial).await;
        let session = if initial == AppState::Running {
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
        assert_eq!(session.status(id).await, Ok(Status::Playing));
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
    let (address, server) = receiver::start(&rustls::version::TLS13, AppState::MustStayIdle).await;
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
async fn handshake_rejects_a_signature_from_the_wrong_key_on_both_tls_versions() {
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let other_key = rcgen::KeyPair::generate().unwrap();
    let private_key = other_key.serialize_der().try_into().unwrap();
    let wrong_signer = rustls::crypto::ring::sign::any_supported_type(&private_key).unwrap();
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let identity = rustls::sign::CertifiedKey::new(
            vec![certificate.cert.der().clone()],
            wrong_signer.clone(),
        );
        // The resolver lets the fixture present a certificate with a deliberately
        // mismatched key so the client's handshake verification decides the result.
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[version])
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(rustls::sign::SingleCertAndKey::from(identity)));
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let accept = async {
            let (socket, _) = listener.accept().await.unwrap();
            acceptor.accept(socket).await
        };
        let (client, server) = timeout(Duration::from_secs(5), async {
            tokio::join!(Session::connect(address), accept)
        })
        .await
        .expect("a mismatched signature should fail the handshake promptly");
        assert!(matches!(client, Err(Error::Unreachable)), "{version:?}");
        let error = server.unwrap_err();
        let alert = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<rustls::Error>());
        assert!(
            matches!(
                alert,
                Some(rustls::Error::AlertReceived(
                    rustls::AlertDescription::DecryptError
                ))
            ),
            "{version:?}: expected a signature verification alert, got {error:?}",
        );
    }
}

#[tokio::test]
async fn connecting_to_a_running_receiver_reuses_its_transport_without_relaunching() {
    let (address, server) = receiver::start(&rustls::version::TLS13, AppState::Running).await;
    let session = Session::connect(address).await.unwrap();
    assert_eq!(session.lookup(URL).await, Ok(None));
    drop(session);
    timeout(Duration::from_secs(5), server)
        .await
        .expect("sender closes the control socket")
        .unwrap();
}
