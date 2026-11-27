use std::{future::Future, time::Duration};

use iptv_cast::{
    Cast, Command, Connect, Connection, Error, ErrorKind, Media, Pairing, PinCode, Route, RouteId,
    Session, Snapshot, Stage, Updates,
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
async fn configuration_reports_that_the_shared_registry_has_no_production_backend_yet() {
    let Ok(media) = Media::live("http://localhost/generated.m3u8") else {
        panic!("valid public media URL")
    };
    assert!(
        media
            .with_content_type("application/vnd.apple.mpegurl")
            .is_ok()
    );
    let Err(error) = Cast::builder().build() else {
        panic!("shared configuration requires a backend")
    };
    assert_eq!(error.stage(), Stage::Configuration);
    assert_eq!(error.kind(), ErrorKind::Unavailable);
}
