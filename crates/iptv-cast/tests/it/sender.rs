use std::{future::Future, time::Duration};

use iptv_cast::{
    Cast, Command, Connect, Connection, Error, Media, Pairing, PinCode, Route, RouteId, Session,
    Snapshot, Updates,
};

fn cloneable<T: Clone + Send + Sync + 'static>() {}
fn owned<T: Send + 'static>() {}
fn send<T: Future + Send>(future: T) {
    drop(future);
}

fn operation_futures(
    cast: &Cast,
    route: &Route,
    connection: Connection,
    pairing: Pairing,
    mut session: Session,
    mut updates: Updates,
    cancel: bool,
) {
    send(cast.discover(Duration::from_secs(1)));
    send(cast.connect(route));
    send(cast.forget(route));
    send(connection.close());
    send(session.command(Command::Play));
    send(updates.next());
    if cancel {
        send(pairing.cancel());
        send(session.close());
    } else {
        send(pairing.submit(PinCode::parse("0000").unwrap()));
        send(session.stop());
    }
}

#[test]
fn public_handles_and_futures_have_the_documented_thread_safety() {
    cloneable::<Cast>();
    cloneable::<Route>();
    cloneable::<RouteId>();
    cloneable::<Snapshot>();
    cloneable::<Error>();
    owned::<Connection>();
    owned::<Pairing>();
    owned::<Connect>();
    owned::<Session>();
    owned::<Updates>();
    let _ = operation_futures;
}

#[tokio::test]
async fn configuration_respects_the_enabled_protocol_features() {
    let Ok(media) = Media::live("http://localhost/generated.m3u8") else {
        panic!("valid public media URL")
    };
    assert!(
        media
            .with_content_type("application/vnd.apple.mpegurl")
            .is_ok()
    );
    #[cfg(feature = "googlecast")]
    assert!(Cast::builder().build().is_ok());
    #[cfg(not(feature = "googlecast"))]
    {
        let error = Cast::builder()
            .build()
            .err()
            .expect("a protocol is required");
        assert_eq!(error.stage(), iptv_cast::Stage::Configuration);
        assert_eq!(error.kind(), iptv_cast::ErrorKind::Unavailable);
    }
}
