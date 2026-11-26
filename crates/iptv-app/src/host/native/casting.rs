//! Linux Cast transport. The runtime authorizes each opening of the relay.
use iptv_cast::googlecast::{self, Session, Status};
use iptv_core::{
    cast::{Command, Error, Receiver, RemoteState},
    playback::PlaySource,
};
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use iptv_runtime::cast::{Port, Prepared, Remote, Task};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::timeout;

pub(super) struct Sender;
const DEFAULT_PORT: u16 = 49152;
fn map_error(error: googlecast::Error) -> Error {
    match error {
        googlecast::Error::Unreachable => Error::Unavailable,
        googlecast::Error::Refused => Error::Refused,
        googlecast::Error::Unknown | googlecast::Error::Ended => Error::Unknown,
    }
}
impl Port for Sender {
    fn discover(&self) -> Task<'_, Vec<Receiver>> {
        Box::pin(async {
            Ok(iptv_cast::discovery::discover()
                .await
                .map_err(|_| Error::Unavailable)?
                .into_iter()
                .map(|r| Receiver {
                    name: r.name,
                    address: r.address,
                })
                .collect())
        })
    }
    fn prepare(&self, receiver: Receiver, source: PlaySource) -> Task<'_, Box<dyn Prepared>> {
        Box::pin(async move {
            let session = Session::connect(receiver.address)
                .await
                .map_err(map_error)?;
            let port = match std::env::var("PATINA_CAST_RELAY_PORT") {
                Ok(s) => s
                    .parse::<std::num::NonZeroU16>()
                    .map_err(|_| Error::Unavailable)?
                    .get(),
                Err(_) => DEFAULT_PORT,
            };
            let listen = SocketAddr::new(session.local_ip(), port);
            let (relay, gate) = tokio::task::spawn_blocking(move || {
                PlaybackRelay::start_cast(
                    &source.url,
                    &source.headers,
                    super::transport::policy(source.public_only),
                    listen,
                    receiver.address.ip(),
                )
            })
            .await
            .map_err(|_| Error::Unavailable)?
            .map_err(|_| Error::Unavailable)?;
            Ok(Box::new(Handoff {
                address: receiver.address,
                session,
                relay,
                gate,
                reply: None,
                release: None,
            }) as Box<dyn Prepared>)
        })
    }
}
struct Handoff {
    address: SocketAddr,
    session: Session,
    relay: PlaybackRelay,
    gate: ExternalAdmission,
    reply: Option<tokio::sync::oneshot::Receiver<Result<i64, Error>>>,
    release: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Prepared for Handoff {
    fn load(&mut self) -> Task<'_, ()> {
        Box::pin(async move {
            let session = self.session.clone();
            let url = self.relay.url().to_owned();
            let (loaded, mut reply) = tokio::sync::oneshot::channel();
            let (release, released) = tokio::sync::oneshot::channel();
            self.release = Some(release);
            tokio::spawn(async move {
                let result = session.load(&url).await.map_err(map_error);
                let _ = loaded.send(result);
                // Until the handoff commits, cancellation revokes ownership of
                // just this capability, including after a delayed LOAD reply.
                if released.await.is_err()
                    && let Ok(Some(id)) = session.lookup(&url).await
                {
                    let _ = session.command(id, googlecast::Command::Stop).await;
                }
            });
            let wait = timeout(Duration::from_secs(10), self.gate.connected());
            tokio::pin!(wait);
            tokio::select! {
                connected=&mut wait=>{
                    if connected!=Ok(true){return Err(Error::NoRequest);}
                    self.reply=Some(reply);
                }
                result=&mut reply=>{
                    let id=result.map_err(|_|Error::Unknown)??;
                    if wait.await!=Ok(true){return Err(Error::NoRequest);}
                    let (send,ready)=tokio::sync::oneshot::channel();
                    let _=send.send(Ok(id));
                    self.reply=Some(ready);
                }
            }
            Ok(())
        })
    }
    fn allow(self: Box<Self>) -> Task<'static, Arc<dyn Remote>> {
        Box::pin(async move {
            let Handoff {
                address,
                session,
                relay,
                gate,
                reply,
                release,
            } = *self;
            if !gate.allow() {
                return Err(Error::Denied);
            }
            let outcome = async {
                let id = reply
                    .ok_or(Error::Unknown)?
                    .await
                    .map_err(|_| Error::Unknown)??;
                timeout(Duration::from_secs(15), async {
                    loop {
                        match session.status(id).await.map_err(map_error)? {
                            Status::Playing | Status::Paused => return Ok(id),
                            Status::Failed
                            | Status::Finished
                            | Status::Canceled
                            | Status::Interrupted => return Err(Error::Refused),
                            Status::Idle | Status::Unknown | Status::Buffering => {
                                tokio::time::sleep(Duration::from_millis(250)).await
                            }
                        }
                    }
                })
                .await
                .map_err(|_| Error::Unknown)?
            }
            .await;
            match outcome {
                Ok(id) => {
                    if let Some(release) = release {
                        let _ = release.send(());
                    }
                    Ok(Arc::new(Playing {
                        address,
                        url: relay.url().to_owned(),
                        session: tokio::sync::Mutex::new(session),
                        id,
                        relay: tokio::sync::Mutex::new(Some(relay)),
                    }) as Arc<dyn Remote>)
                }
                Err(e) => {
                    relay.shutdown().await;
                    Err(e)
                }
            }
        })
    }
}
struct Playing {
    address: SocketAddr,
    url: String,
    session: tokio::sync::Mutex<Session>,
    id: i64,
    relay: tokio::sync::Mutex<Option<PlaybackRelay>>,
}
impl Remote for Playing {
    fn command(&self, command: Command) -> Task<'_, ()> {
        Box::pin(async move {
            let cmd = match command {
                Command::Volume(v) => googlecast::Command::Volume(v),
                Command::StepVolume(v) => googlecast::Command::StepVolume(v),
                Command::ToggleMute => googlecast::Command::ToggleMute,
                Command::Play => googlecast::Command::Play,
                Command::Pause => googlecast::Command::Pause,
                Command::Stop => googlecast::Command::Stop,
            };
            self.session
                .lock()
                .await
                .command(self.id, cmd)
                .await
                .map_err(map_error)
        })
    }
    fn state(&self) -> Task<'_, RemoteState> {
        Box::pin(async move {
            let mut session = self.session.lock().await;
            let status = match session.status(self.id).await {
                Ok(s) => s,
                Err(googlecast::Error::Ended) => return Ok(RemoteState::Ended),
                Err(_) => {
                    let joined = Session::rejoin(self.address).await.map_err(map_error)?;
                    if joined.lookup(&self.url).await.map_err(map_error)? != Some(self.id) {
                        return Ok(RemoteState::Ended);
                    }
                    *session = joined;
                    session.status(self.id).await.map_err(map_error)?
                }
            };
            Ok(match status {
                Status::Playing => RemoteState::Playing,
                Status::Paused => RemoteState::Paused,
                Status::Unknown | Status::Buffering => RemoteState::Loading,
                _ => RemoteState::Ended,
            })
        })
    }
    fn retire(&self) -> Task<'_, ()> {
        Box::pin(async move {
            if let Some(relay) = self.relay.lock().await.take() {
                relay.shutdown().await;
            }
            Ok(())
        })
    }
}
