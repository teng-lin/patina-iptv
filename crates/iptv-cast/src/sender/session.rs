use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use super::{
    Availability, Command, Confirmation, Delivery, Driver, Error, ErrorKind, Link, Media,
    Observation, PeerTrust, Playback, Snapshot, Stage, StopOutcome, service::Runtime,
};

const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// An idle receiver connection. A load consumes this handle.
pub struct Connection {
    driver: Option<Box<dyn Driver>>,
    runtime: Runtime,
}

impl Connection {
    pub(super) fn new(driver: Box<dyn Driver>, runtime: Runtime) -> Self {
        Self {
            driver: Some(driver),
            runtime,
        }
    }

    pub fn control_addrs(&self) -> Option<(std::net::SocketAddr, std::net::SocketAddr)> {
        self.driver.as_ref().unwrap().control_addrs()
    }

    pub fn peer_trust(&self) -> PeerTrust {
        self.driver.as_ref().unwrap().peer_trust()
    }

    /// Validate and submit the load locally. Receiver acknowledgement arrives
    /// through the new session's updates.
    pub fn load(mut self, media: Media) -> Result<Session, Error> {
        self.runtime.check(Stage::Load)?;
        self.driver.as_ref().unwrap().validate(&media)?;
        Ok(Session::start(
            self.driver.take().unwrap(),
            media,
            &self.runtime,
        ))
    }

    pub async fn close(mut self) {
        // A spawned worker finishes even if the close future is cancelled.
        let _ = self.close_on_runtime().await;
    }

    fn close_on_runtime(&mut self) -> JoinHandle<()> {
        let mut driver = self.driver.take().unwrap();
        self.runtime
            .handle
            .spawn(async move { driver.close().await })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.driver.is_some() {
            self.close_on_runtime();
        }
    }
}

const QUEUED: u8 = 0;
const DISPATCHED: u8 = 1;
const CANCELLED: u8 = 2;

/// The last local point where retiring a queued load can suppress its send.
#[derive(Default)]
pub(crate) struct Dispatch(AtomicU8);

impl Dispatch {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "backend integration follows this contract")
    )]
    pub(crate) fn mark_sent(&self) -> Result<(), Error> {
        match self
            .0
            .compare_exchange(QUEUED, DISPATCHED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(CANCELLED) => Err(Error::new(
                ErrorKind::Cancelled,
                Stage::Load,
                Delivery::NotSent,
            )),
            Err(_) => Err(Error::new(
                ErrorKind::Protocol,
                Stage::Load,
                Delivery::MayHaveReachedReceiver,
            )),
        }
    }

    fn cancel_queued(&self) {
        let _ = self
            .0
            .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
    }

    fn sent(&self) -> bool {
        self.0.load(Ordering::Acquire) == DISPATCHED
    }
}

enum Action {
    Close,
    Stop,
}

type CommandResult = Result<Confirmation, Error>;

struct Request {
    command: Command,
    reply: oneshot::Sender<CommandResult>,
}

pub struct Session {
    snapshot: watch::Receiver<Snapshot>,
    commands: mpsc::Sender<Request>,
    action: Option<oneshot::Sender<Action>>,
    dispatch: Arc<Dispatch>,
    worker: Option<JoinHandle<Result<StopOutcome, Error>>>,
}

impl Session {
    fn start(driver: Box<dyn Driver>, media: Media, runtime: &Runtime) -> Self {
        let (publish, snapshot) = watch::channel(Snapshot::pending(media.is_live()));
        let (commands, requests) = mpsc::channel(1);
        let (action, retire) = oneshot::channel();
        let dispatch = Arc::new(Dispatch::default());
        let worker = runtime.handle.spawn(run(
            driver,
            media,
            publish,
            requests,
            retire,
            dispatch.clone(),
        ));
        Self {
            snapshot,
            commands,
            action: Some(action),
            dispatch,
            worker: Some(worker),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.borrow().clone()
    }

    pub fn subscribe(&self) -> Updates {
        let mut receiver = self.snapshot.clone();
        let initial = receiver.borrow_and_update().clone();
        Updates {
            receiver,
            initial: Some(initial),
        }
    }

    pub async fn command(&mut self, command: Command) -> CommandResult {
        check_command(&self.snapshot(), command)?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Request { command, reply })
            .await
            .map_err(|_| command_error(ErrorKind::Unavailable))?;
        response.await.unwrap_or_else(|_| {
            Err(Error::new(
                ErrorKind::Unavailable,
                Stage::Control,
                Delivery::MayHaveReachedReceiver,
            ))
        })
    }

    pub async fn stop(mut self) -> Result<StopOutcome, Error> {
        self.retire(Action::Stop);
        self.worker
            .take()
            .unwrap()
            .await
            .map_err(|_| stop_error(ErrorKind::Unavailable, Delivery::MayHaveReachedReceiver))?
    }

    pub async fn close(mut self) {
        self.retire(Action::Close);
        let _ = self.worker.take().unwrap().await;
    }

    fn retire(&mut self, action: Action) {
        self.dispatch.cancel_queued();
        if let Some(sender) = self.action.take() {
            let _ = sender.send(action);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.retire(Action::Close);
    }
}

pub struct Updates {
    receiver: watch::Receiver<Snapshot>,
    initial: Option<Snapshot>,
}

impl Updates {
    pub async fn next(&mut self) -> Option<Snapshot> {
        if let Some(initial) = self.initial.take() {
            return Some(initial);
        }
        self.receiver.changed().await.ok()?;
        Some(self.receiver.borrow_and_update().clone())
    }
}

async fn run(
    mut driver: Box<dyn Driver>,
    media: Media,
    publish: watch::Sender<Snapshot>,
    mut commands: mpsc::Receiver<Request>,
    mut retire: oneshot::Receiver<Action>,
    dispatch: Arc<Dispatch>,
) -> Result<StopOutcome, Error> {
    let mut snapshot = publish.borrow().clone();
    let load = tokio::select! {
        biased;
        action = &mut retire => return finish_action(
            &mut *driver, action.unwrap_or(Action::Close), &publish, snapshot, &dispatch,
        ).await,
        result = tokio::time::timeout(LOAD_TIMEOUT, driver.load(media, &dispatch)) => {
            result.unwrap_or_else(|_| Err(Error::new(
                ErrorKind::TimedOut, Stage::Load, if dispatch.sent() {
                    Delivery::MayHaveReachedReceiver
                } else {
                    Delivery::NotSent
                }
            )))
        }
    };
    match load {
        Ok(observation) => snapshot.apply(observation),
        Err(error) => {
            // A caller that explicitly closes concurrently with a failed
            // load has chosen local-only teardown. Otherwise reconcile once.
            snapshot.apply(Observation::playback(Playback::Failed(error)));
            let close = matches!(retire.try_recv(), Ok(Action::Close));
            let result = if close {
                Err(stop_error(ErrorKind::Unavailable, Delivery::NotSent))
            } else {
                stop_owned(&mut *driver, &dispatch).await
            };
            return finish(&mut *driver, &publish, snapshot, result).await;
        }
    }

    loop {
        if let Some(result) = terminal_result(&snapshot) {
            return finish(&mut *driver, &publish, snapshot, result).await;
        }
        publish.send_if_modified(|published| {
            if snapshot.revision() == published.revision() {
                return false;
            }
            *published = snapshot.clone();
            true
        });
        tokio::select! {
            biased;
            action = &mut retire => {
                return finish_action(
                    &mut *driver, action.unwrap_or(Action::Close), &publish, snapshot, &dispatch,
                ).await;
            }
            request = commands.recv() => {
                let Some(request) = request else {
                    return finish_action(&mut *driver, Action::Close, &publish, snapshot, &dispatch).await;
                };
                if request.reply.is_closed() { continue; }
                let result = match check_command(&snapshot, request.command) {
                    Ok(()) => driver.command(request.command).await,
                    Err(error) => Err(error),
                };
                let _ = request.reply.send(result);
            }
            observation = driver.observe() => {
                match observation {
                    Ok(observation) => snapshot.apply(observation),
                    Err(error) => snapshot.apply(Observation::link(Link::Disconnected, Some(error))),
                }
            }
        }
    }
}

async fn finish_action(
    driver: &mut dyn Driver,
    action: Action,
    publish: &watch::Sender<Snapshot>,
    mut snapshot: Snapshot,
    dispatch: &Dispatch,
) -> Result<StopOutcome, Error> {
    let result = match action {
        Action::Stop => stop_owned(driver, dispatch).await,
        Action::Close => Err(stop_error(ErrorKind::Unavailable, Delivery::NotSent)),
    };
    if result.as_ref().is_ok_and(|outcome| {
        *outcome == StopOutcome::StopRequested(Confirmation::ReceiverConfirmed)
    }) {
        snapshot.apply(Observation::playback(Playback::Ended));
    }
    finish(driver, publish, snapshot, result).await
}

async fn stop_owned(driver: &mut dyn Driver, dispatch: &Dispatch) -> Result<StopOutcome, Error> {
    if !dispatch.sent() {
        return Ok(StopOutcome::CancelledBeforeSend);
    }
    tokio::time::timeout(STOP_TIMEOUT, driver.stop())
        .await
        .unwrap_or_else(|_| {
            Err(stop_error(
                ErrorKind::TimedOut,
                Delivery::MayHaveReachedReceiver,
            ))
        })
}

fn terminal_result(snapshot: &Snapshot) -> Option<Result<StopOutcome, Error>> {
    match snapshot.playback() {
        Playback::Ended | Playback::NoLongerPresent => Some(Ok(StopOutcome::AlreadyEnded)),
        Playback::OwnershipLost => {
            Some(Err(stop_error(ErrorKind::OwnershipLost, Delivery::NotSent)))
        }
        Playback::Failed(error) => Some(Err(stop_error(error.kind(), Delivery::NotSent))),
        _ if snapshot.link() == Link::Disconnected => {
            let kind = snapshot
                .link_error()
                .map_or(ErrorKind::Unavailable, Error::kind);
            Some(Err(stop_error(kind, Delivery::NotSent)))
        }
        _ => None,
    }
}

async fn finish(
    driver: &mut dyn Driver,
    publish: &watch::Sender<Snapshot>,
    mut snapshot: Snapshot,
    result: Result<StopOutcome, Error>,
) -> Result<StopOutcome, Error> {
    driver.close().await;
    let link_error = snapshot.link_error().cloned();
    snapshot.apply(Observation::link(Link::Disconnected, link_error));
    publish.send_replace(snapshot);
    result
}

fn check_command(snapshot: &Snapshot, command: Command) -> Result<(), Error> {
    if matches!(snapshot.playback(), Playback::OwnershipLost) {
        return Err(command_error(ErrorKind::OwnershipLost));
    }
    if snapshot.playback().is_terminal() || snapshot.link() != Link::Connected {
        return Err(command_error(ErrorKind::Unavailable));
    }
    if let Command::Output { scope, .. } = command {
        match snapshot.volume_scope() {
            Some(established) if established == scope => {}
            Some(_) => return Err(command_error(ErrorKind::Unsupported)),
            None => return Err(command_error(ErrorKind::Unavailable)),
        }
    }
    match snapshot.availability(command.feature()) {
        Availability::Unsupported => Err(command_error(ErrorKind::Unsupported)),
        Availability::Unavailable => Err(command_error(ErrorKind::Unavailable)),
        Availability::Available | Availability::Unknown => Ok(()),
    }
}

fn command_error(kind: ErrorKind) -> Error {
    Error::new(kind, Stage::Control, Delivery::NotSent)
}

fn stop_error(kind: ErrorKind, delivery: Delivery) -> Error {
    Error::new(kind, Stage::Stop, delivery)
}
