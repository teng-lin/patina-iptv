//! Host-supplied casting operations and per-owner handoff state.
use crate::{ExternalPlaybackAuthorization, Runtime};
use futures_channel::oneshot;
use iptv_core::{
    cast::{Command, Error, Receiver, RemoteState, Target},
    playback::PlaySource,
};
use iptv_import::CancelToken;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, mpsc},
};

pub type Task<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;
pub trait Port: Send + Sync {
    fn discover(&self) -> Task<'_, Vec<Receiver>>;
    /// Connects and creates a gated LAN relay without opening the upstream.
    fn prepare(&self, receiver: Receiver, source: PlaySource) -> Task<'_, Box<dyn Prepared>>;
}
/// A system route picker is mounted by the host; its player remains owned by
/// this runtime's transport. The opaque view identity carries no media metadata.
pub trait NativePort: Send + Sync {
    fn view_id(&self) -> Option<u64>;
    fn prepare(&self, source: PlaySource) -> Task<'_, Box<dyn Prepared>>;
    fn cancel_pending(&self);
}
pub trait Prepared: Send {
    /// Recheck policy and intent while a system picker waits for owner approval.
    fn guard(&mut self, _current: Box<dyn Fn() -> bool + Send + Sync>) {}
    /// Sends LOAD and waits for the selected TV's first authorized HTTP request.
    fn load(&mut self) -> Task<'_, ()>;
    /// Open and settle under the grant, joining relay retirement on denial.
    fn allow(self: Box<Self>, authorization: &Authorization) -> Task<'_, Admitted>;
}
/// Monitors the playback grant while the receiver owns an unpublished relay.
pub struct Authorization {
    grant: ExternalPlaybackAuthorization,
    services: crate::store_service::Services,
}
impl Authorization {
    /// Drop the pending operation on revocation, including while a LOAD reply is outstanding.
    pub async fn while_current<T>(
        &self,
        work: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        let mut work = std::pin::pin!(work);
        while self.grant.is_current() {
            let deadline = crate::time::Instant::now() + std::time::Duration::from_millis(250);
            if let Ok(result) = self.services.timeout_at(deadline, work.as_mut()).await {
                return result;
            }
        }
        Err(Error::Denied)
    }
}
pub struct Admitted {
    pub remote: Arc<dyn Remote>,
    pub paused: bool,
}
pub type Retirement<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
pub trait Remote: Send + Sync {
    fn command(&self, command: Command) -> Task<'_, ()>;
    /// Reports the owned media session, or a transport error if its state is unknown.
    fn state(&self) -> Task<'_, RemoteState>;
    /// Joins upstream retirement before the account claim can be released.
    fn retire(&self) -> Retirement<'_>;
    /// Revoke relay access even when the control connection has been lost.
    fn stop(&self) -> Retirement<'_> {
        Box::pin(async {
            self.retire().await;
            let _ = self.command(Command::Stop).await;
        })
    }
}
pub struct Destination {
    pub receiver: Target,
    pub key: String,
    pub authority: iptv_guide::playback::ResolutionAuthority,
}
pub(crate) struct Active {
    pub receiver: Target,
    pub key: String,
    pub authority: iptv_guide::playback::ResolutionAuthority,
    remote: Arc<dyn Remote>,
    failed_polls: u8,
    ticket: u64,
    claim: Box<dyn Send>,
}

// The owner can cancel an admission on teardown even when its UI future is suspended.
pub(crate) struct Worker {
    cancel: CancelToken,
    done: mpsc::Receiver<()>,
}

/// Releasing provider capacity depends on closing the local relay. A receiver that has
/// left the network can take much longer to answer STOP; its owned media ID scopes that command.
async fn retire_abandoned(
    services: &crate::store_service::Services,
    remote: Arc<dyn Remote>,
    claim: Box<dyn Send>,
) {
    remote.retire().await;
    drop(claim);
    drop(services.spawn(async move { remote.command(Command::Stop).await }));
}

struct CancelOnDrop(CancelToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Runtime {
    /// Invalidates outstanding handoffs when a new playback intent replaces them.
    pub fn next_cast_intent(&self) -> u64 {
        let next = self
            .cast_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            .wrapping_add(1);
        for worker in self.cast_workers.borrow().iter() {
            worker.cancel.cancel();
        }
        next
    }
    pub fn cast_intent(&self) -> u64 {
        self.cast_generation
            .load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn cast_intent_current(&self, ticket: u64) -> bool {
        self.cast_intent() == ticket
    }
    pub fn chromecast_available(&self) -> bool {
        self.services().ports().cast.is_some()
    }
    pub fn cast_available(&self) -> bool {
        self.services().ports().cast.is_some() || self.native_cast_available()
    }
    pub fn native_cast_available(&self) -> bool {
        self.services().ports().native_cast.is_some()
    }
    pub fn native_cast_view(&self) -> Option<u64> {
        self.services()
            .ports()
            .native_cast
            .as_ref()
            .and_then(|port| port.view_id())
    }
    pub fn cancel_pending_native_cast(&self) {
        if let Some(port) = &self.services().ports().native_cast {
            port.cancel_pending();
        }
    }
    pub async fn cast_receivers(&self) -> Result<Vec<Receiver>, Error> {
        if self.services().ports().cast.is_none() && self.native_cast_available() {
            return Ok(Vec::new());
        }
        let port = self
            .services()
            .ports()
            .cast
            .clone()
            .ok_or(Error::Unavailable)?;
        self.spawn_network(async move { port.discover().await })
            .await
            .map_err(|_| Error::Unavailable)?
    }
    pub fn cast_target(&self) -> Option<(Target, String)> {
        self.cast
            .borrow()
            .as_ref()
            .map(|active| (active.receiver.clone(), active.key.clone()))
    }
    pub async fn prepare_cast(
        &self,
        receiver: Target,
        source: PlaySource,
    ) -> Result<Box<dyn Prepared>, Error> {
        if receiver == Target::AirPlay {
            let port = self
                .services()
                .ports()
                .native_cast
                .clone()
                .ok_or(Error::Unavailable)?;
            return self
                .spawn_network(async move { port.prepare(source).await })
                .await
                .map_err(|_| Error::Unavailable)?;
        }
        let Target::Chromecast(receiver) = receiver else {
            unreachable!()
        };
        let port = self
            .services()
            .ports()
            .cast
            .clone()
            .ok_or(Error::Unavailable)?;
        self.spawn_network(async move { port.prepare(receiver, source).await })
            .await
            .map_err(|_| Error::Unavailable)?
    }
    pub async fn load_cast(
        &self,
        ticket: u64,
        mut prepared: Box<dyn Prepared>,
        grant: ExternalPlaybackAuthorization,
    ) -> Result<Box<dyn Prepared>, Error> {
        let generation = self.cast_generation.clone();
        self.spawn_network(async move {
            if generation.load(std::sync::atomic::Ordering::Acquire) != ticket
                || !grant.is_current()
            {
                return Err(Error::Denied);
            }
            prepared.load().await?;
            Ok(prepared)
        })
        .await
        .map_err(|_| Error::Unknown)?
    }
    pub async fn admit_cast(
        &self,
        ticket: u64,
        mut prepared: Box<dyn Prepared>,
        destination: Destination,
        claim: Box<dyn Send>,
        grant: ExternalPlaybackAuthorization,
    ) -> Result<bool, Error> {
        if self.shutdown_requested.get() || self.cast.borrow().is_some() || !grant.is_current() {
            return Err(Error::Denied);
        }
        let generation = self.cast_generation.clone();
        let guarded_generation = generation.clone();
        let guarded_grant = grant.clone();
        prepared.guard(Box::new(move || {
            guarded_generation.load(std::sync::atomic::Ordering::Acquire) == ticket
                && guarded_grant.is_current()
        }));
        let cancel = CancelOnDrop(CancelToken::new());
        let signal = cancel.0.clone();
        let (offer, offered) = oneshot::channel();
        let services = self.services();
        let authorization = Arc::new(Authorization {
            grant,
            services: services.clone(),
        });
        let worker_authorization = authorization.clone();
        let admission = async move {
            // Even if the UI drops its future, allow() must finish on this executor:
            // it may have opened the upstream already and must join its retirement.
            if signal.is_canceled()
                || generation.load(std::sync::atomic::Ordering::Acquire) != ticket
                || !worker_authorization.grant.is_current()
            {
                return Err(Error::Denied);
            }
            let admitted = prepared.allow(&worker_authorization).await?;
            let remote = admitted.remote.clone();
            let claim = Arc::new(Mutex::new(Some(claim)));
            if !signal.is_canceled()
                && generation.load(std::sync::atomic::Ordering::Acquire) == ticket
                && worker_authorization.grant.is_current()
                && offer.send((admitted, claim.clone())).is_ok()
            {
                // The owner may be suspended after the offer. Keep watching its grant
                // until the owner has taken the claim or the relay must be retired.
                let _ = worker_authorization
                    .while_current(async {
                        signal.canceled().await;
                        Ok(())
                    })
                    .await;
            }
            // Publication takes the claim without yielding. Every other path
            // leaves it here until the admitted relay has actually closed.
            let unclaimed = claim.lock().expect("cast claim poisoned").take();
            if let Some(claim) = unclaimed {
                retire_abandoned(&services, remote, claim).await;
                return Err(Error::Denied);
            }
            Ok(())
        };
        let job = self.spawn_cast_worker(cancel.0.clone(), admission);
        if let Ok((admitted, claim)) = offered.await
            && self.cast_intent_current(ticket)
            && !self.shutdown_requested.get()
            && self.cast.borrow().is_none()
            && let Some(claim) = claim
                .lock()
                .expect("cast claim poisoned")
                .take_if(|_| authorization.grant.is_current())
        {
            *self.cast.borrow_mut() = Some(Active {
                receiver: destination.receiver,
                key: destination.key,
                authority: destination.authority,
                remote: admitted.remote,
                ticket,
                failed_polls: 0,
                claim,
            });
            return Ok(admitted.paused);
        }
        drop(cancel);
        job.await.map_err(|_| Error::Unknown)??;
        Err(Error::Denied)
    }

    fn spawn_cast_worker(
        &self,
        cancel: CancelToken,
        work: impl Future<Output = Result<(), Error>> + crate::MaybeSend + 'static,
    ) -> crate::execution::Join<Result<(), Error>> {
        let (done, waiting) = mpsc::channel();
        let mut workers = self.cast_workers.borrow_mut();
        workers.retain(|worker| matches!(worker.done.try_recv(), Err(mpsc::TryRecvError::Empty)));
        workers.push(Worker {
            cancel,
            done: waiting,
        });
        let services = self.services();
        self.spawn_network(async move {
            // A bounded drain can time out. Keep its executor alive until this relay is closed.
            let result = work.await;
            let _ = done.send(());
            drop(services);
            result
        })
    }

    /// Stop admission and revoke open relays while the other services start draining.
    pub(crate) fn close_cast(&self) {
        self.next_cast_intent();
        if let Some(active) = self.cast.borrow_mut().take() {
            self.parental_end_transfer(&iptv_core::parental::Target::live(
                active.authority.source(),
                active.authority.channel().id.as_str(),
            ));
            let services = self.services();
            drop(self.spawn_cast_worker(CancelToken::new(), async move {
                retire_abandoned(&services, active.remote, active.claim).await;
                Ok(())
            }));
        }
    }

    pub(crate) fn drain_cast(&self, deadline: crate::time::Instant) -> bool {
        let mut complete = true;
        self.cast_workers.borrow_mut().retain(|worker| {
            let left = deadline.saturating_duration_since(crate::time::Instant::now());
            let result = if self.services().has_workers() {
                worker.done.recv_timeout(left)
            } else {
                worker.done.try_recv().map_err(|err| match err {
                    mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
                    mpsc::TryRecvError::Disconnected => mpsc::RecvTimeoutError::Disconnected,
                })
            };
            complete &= result.is_ok();
            result == Err(mpsc::RecvTimeoutError::Timeout)
        });
        complete
    }
    pub async fn cast_command(&self, command: Command) -> Result<(), Error> {
        let remote = self
            .cast
            .borrow()
            .as_ref()
            .ok_or(Error::Unavailable)?
            .remote
            .clone();
        let worker = remote.clone();
        self.spawn_network(async move {
            if command == Command::Stop {
                worker.stop().await;
                Ok(())
            } else {
                worker.command(command).await
            }
        })
        .await
        .map_err(|_| Error::Unknown)??;
        if command == Command::Stop {
            self.clear_cast(&remote);
        }
        Ok(())
    }
    /// Retire the old media before a channel replacement while keeping the
    /// native player's selected route. Explicit Stop always disconnects it.
    pub async fn replace_native_cast(&self) -> Result<(), Error> {
        let remote = self
            .cast
            .borrow()
            .as_ref()
            .filter(|active| active.receiver == Target::AirPlay)
            .map(|active| active.remote.clone())
            .ok_or(Error::Unavailable)?;
        let worker = remote.clone();
        self.spawn_network(async move { worker.retire().await })
            .await
            .map_err(|_| Error::Unknown)?;
        self.clear_cast(&remote);
        Ok(())
    }
    fn clear_cast(&self, remote: &Arc<dyn Remote>) {
        let mut slot = self.cast.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(&active.remote, remote))
            && let Some(active) = slot.take()
        {
            self.parental_end_transfer(&iptv_core::parental::Target::live(
                active.authority.source(),
                active.authority.channel().id.as_str(),
            ));
        }
    }
    pub fn cast_parental_target(&self) -> Option<iptv_core::parental::Target> {
        self.cast.borrow().as_ref().map(|a| {
            iptv_core::parental::Target::live(
                a.authority.source(),
                a.authority.channel().id.as_str(),
            )
        })
    }
    pub fn active_cast_ticket(&self) -> Option<u64> {
        self.cast.borrow().as_ref().map(|active| active.ticket)
    }

    pub async fn poll_cast(&self, ticket: u64) -> Result<RemoteState, Error> {
        let (authority, remote) = self
            .cast
            .borrow()
            .as_ref()
            .filter(|active| active.ticket == ticket)
            .map(|active| (active.authority.clone(), active.remote.clone()))
            .ok_or(Error::Unavailable)?;
        let allowed = self
            .authorize_cast_playback(authority)
            .await
            .ok()
            .flatten()
            .is_some_and(|grant| grant.is_current());
        if self.active_cast_ticket() != Some(ticket) {
            return Err(Error::Unavailable);
        }
        let state = if allowed {
            let worker = remote.clone();
            let observed = self
                .spawn_network(async move { worker.state().await })
                .await
                .map_err(|_| Error::Unknown)?;
            let mut slot = self.cast.borrow_mut();
            let active = slot
                .as_mut()
                .filter(|active| active.ticket == ticket)
                .ok_or(Error::Unavailable)?;
            match observed {
                Ok(observed) => {
                    active.failed_polls = 0;
                    observed
                }
                Err(_) => {
                    // Each poll already tries to rejoin once. Keep serving during
                    // short control outages, then retire an unreachable receiver.
                    active.failed_polls = active.failed_polls.saturating_add(1);
                    if active.failed_polls < 3 {
                        return Ok(RemoteState::Loading);
                    }
                    RemoteState::Ended
                }
            }
        } else {
            RemoteState::Ended
        };
        if state == RemoteState::Ended {
            let worker = remote.clone();
            self.spawn_network(async move {
                if allowed {
                    worker.retire().await
                } else {
                    worker.stop().await
                }
            })
            .await
            .map_err(|_| Error::Unknown)?;
            self.clear_cast(&remote);
        }
        Ok(state)
    }
}

#[cfg(all(test, feature = "native"))]
mod tests;
