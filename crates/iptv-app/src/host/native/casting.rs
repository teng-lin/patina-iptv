//! Desktop Cast transport. The runtime authorizes each opening of the relay.
use iptv_cast::googlecast::{self, Session, Status};
use iptv_core::{
    cast::{Command, Error, Receiver, RemoteState},
    playback::PlaySource,
};
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use iptv_runtime::cast::{Admitted, Authorization, Port, Prepared, Remote, Retirement, Task};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::{Instant, timeout};

pub(super) struct Sender;
const DEFAULT_PORT: u16 = 49152;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(15);
fn map_error(error: googlecast::Error) -> Error {
    match error {
        googlecast::Error::Unreachable => Error::Unavailable,
        googlecast::Error::Refused | googlecast::Error::LoadFailed => Error::Refused,
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
    reply: Option<tokio::sync::oneshot::Receiver<Result<i64, googlecast::Error>>>,
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
                let result = session.load(&url).await;
                let _ = loaded.send(result);
                // Until the handoff commits, cancellation revokes ownership of
                // just this capability, including after a delayed LOAD reply.
                if released.await.is_err()
                    && let Ok(Some(id)) = session.lookup(&url).await
                {
                    let _ = session.command(id, googlecast::Command::Stop).await;
                }
            });
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let early_reply = tokio::select! {
                connected = tokio::time::timeout_at(deadline, self.gate.connected()) => {
                    if connected != Ok(true) {
                        return Err(Error::NoRequest);
                    }
                    None
                }
                result = &mut reply => Some(result.map_err(|_| Error::Unknown)?),
            };
            if let Some(result) = early_reply {
                let id = match result {
                    Err(googlecast::Error::LoadFailed) if !self.gate.received_request() => {
                        return Err(Error::NoRequest);
                    }
                    other => other.map_err(map_error)?,
                };
                if tokio::time::timeout_at(deadline, self.gate.connected()).await != Ok(true) {
                    return Err(Error::NoRequest);
                }
                let (send, ready) = tokio::sync::oneshot::channel();
                let _ = send.send(Ok(id));
                reply = ready;
            }
            self.reply = Some(reply);
            Ok(())
        })
    }
    fn allow(self: Box<Self>, authorization: &Authorization) -> Task<'_, Admitted> {
        Box::pin(async move {
            let Handoff {
                address,
                session,
                relay,
                gate,
                reply,
                release,
            } = *self;
            let outcome = authorization
                .while_current(async {
                    if !gate.allow() {
                        return Err(Error::Unknown);
                    }
                    let id = reply
                        .ok_or(Error::Unknown)?
                        .await
                        .map_err(|_| Error::Unknown)?
                        .map_err(map_error)?;
                    timeout(SETTLE_TIMEOUT, async {
                        loop {
                            match session.status(id).await.map_err(map_error)? {
                                Status::Playing => return Ok((id, false)),
                                Status::Paused => return Ok((id, true)),
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
                })
                .await;
            match outcome {
                Ok((id, paused)) => {
                    if let Some(release) = release {
                        let _ = release.send(());
                    }
                    Ok(Admitted {
                        remote: Arc::new(Playing {
                            address,
                            url: relay.url().to_owned(),
                            control: tokio::sync::Mutex::new(Control {
                                session,
                                idle_since: None,
                            }),
                            id,
                            relay: tokio::sync::Mutex::new(Some(relay)),
                        }),
                        paused,
                    })
                }
                Err(e) => {
                    relay.shutdown().await;
                    Err(e)
                }
            }
        })
    }
}
struct Control {
    session: Session,
    idle_since: Option<Instant>,
}
struct Playing {
    address: SocketAddr,
    url: String,
    control: tokio::sync::Mutex<Control>,
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
            timeout(Duration::from_secs(15), async {
                self.control
                    .lock()
                    .await
                    .session
                    .command(self.id, cmd)
                    .await
            })
            .await
            .map_err(|_| Error::Unknown)?
            .map_err(map_error)
        })
    }
    fn state(&self) -> Task<'_, RemoteState> {
        Box::pin(self.observe())
    }
    fn retire(&self) -> Retirement<'_> {
        Box::pin(async move {
            let mut slot = self.relay.lock().await;
            if let Some(relay) = slot.take() {
                relay.shutdown().await;
            }
        })
    }
}

impl Playing {
    async fn observe(&self) -> Result<RemoteState, Error> {
        const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
        const REJOIN_TIMEOUT: Duration = Duration::from_secs(15);

        let deadline = Instant::now() + PROBE_TIMEOUT;
        let mut control = tokio::time::timeout_at(deadline, self.control.lock())
            .await
            .map_err(|_| Error::Unknown)?;
        let result = status_or_rejoin(
            deadline,
            control.session.status(self.id),
            REJOIN_TIMEOUT,
            self.rejoin_owned(),
        )
        .await;
        let (replacement, status) = match result {
            Ok(owned) => owned,
            Err(googlecast::Error::Ended) => return Ok(RemoteState::Ended),
            Err(error) => return Err(map_error(error)),
        };
        if let Some(joined) = replacement {
            control.session = joined;
        }
        Ok(observed_state(
            status,
            &mut control.idle_since,
            Instant::now(),
        ))
    }

    async fn rejoin_owned(&self) -> Result<(Session, Status), googlecast::Error> {
        let joined = Session::rejoin(self.address).await?;
        if joined.lookup(&self.url).await? != Some(self.id) {
            return Err(googlecast::Error::Ended);
        }
        let status = joined.status(self.id).await?;
        Ok((joined, status))
    }
}

// A silent control connection can consume its whole deadline. Start the
// recovery budget only when switching to a fresh connection.
async fn status_or_rejoin<S>(
    deadline: Instant,
    status: impl Future<Output = Result<Status, googlecast::Error>>,
    rejoin_timeout: Duration,
    rejoin: impl Future<Output = Result<(S, Status), googlecast::Error>>,
) -> Result<(Option<S>, Status), googlecast::Error> {
    match tokio::time::timeout_at(deadline, status).await {
        Ok(Ok(status)) => Ok((None, status)),
        Ok(Err(googlecast::Error::Ended)) => Err(googlecast::Error::Ended),
        Ok(Err(_)) | Err(_) => {
            let (session, status) = timeout(rejoin_timeout, rejoin)
                .await
                .map_err(|_| googlecast::Error::Unknown)??;
            Ok((Some(session), status))
        }
    }
}

fn observed_state(status: Status, idle_since: &mut Option<Instant>, now: Instant) -> RemoteState {
    if status != Status::Idle {
        *idle_since = None;
    }
    match status {
        Status::Playing => RemoteState::Playing,
        Status::Paused => RemoteState::Paused,
        Status::Idle if now - *idle_since.get_or_insert(now) >= SETTLE_TIMEOUT => {
            RemoteState::Ended
        }
        Status::Unknown | Status::Buffering | Status::Idle => RemoteState::Loading,
        Status::Failed | Status::Finished | Status::Canceled | Status::Interrupted => {
            RemoteState::Ended
        }
    }
}

#[cfg(test)]
mod tests;
