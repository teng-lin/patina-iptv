//! Host-supplied casting operations and per-owner handoff state.
use crate::{ExternalPlaybackAuthorization, Runtime};
use iptv_core::{
    cast::{Command, Error, Receiver, RemoteState},
    playback::PlaySource,
};
use std::{future::Future, pin::Pin, sync::Arc};

pub type Task<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;
pub trait Port: Send + Sync {
    fn discover(&self) -> Task<'_, Vec<Receiver>>;
    /// Connects and creates a gated LAN relay without opening the upstream.
    fn prepare(&self, receiver: Receiver, source: PlaySource) -> Task<'_, Box<dyn Prepared>>;
}
pub trait Prepared: Send {
    /// Sends LOAD and waits for the selected TV's first authorized HTTP request.
    fn load(&mut self) -> Task<'_, ()>;
    fn allow(self: Box<Self>) -> Task<'static, Arc<dyn Remote>>;
}
pub trait Remote: Send + Sync {
    fn command(&self, command: Command) -> Task<'_, ()>;
    /// Reports the owned media session, or a transport error if its state is unknown.
    fn state(&self) -> Task<'_, RemoteState>;
    /// Joins upstream retirement before the account claim can be released.
    fn retire(&self) -> Task<'_, ()>;
    /// Revoke relay access even when the control connection has been lost.
    fn stop(&self) -> Task<'_, ()> {
        Box::pin(async {
            self.retire().await?;
            let _ = self.command(Command::Stop).await;
            Ok(())
        })
    }
}
pub struct Destination {
    pub receiver: Receiver,
    pub key: String,
    pub authority: iptv_guide::playback::ResolutionAuthority,
}
pub(crate) struct Active {
    pub receiver: Receiver,
    pub key: String,
    pub authority: iptv_guide::playback::ResolutionAuthority,
    remote: Arc<dyn Remote>,
    failed_polls: u8,
    ticket: u64,
    _claim: Box<dyn Send>,
}

impl Runtime {
    /// Invalidates outstanding handoffs when a new playback intent replaces them.
    pub fn next_cast_intent(&self) -> u64 {
        self.cast_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            .wrapping_add(1)
    }
    pub fn cast_intent_current(&self, ticket: u64) -> bool {
        self.cast_generation
            .load(std::sync::atomic::Ordering::Acquire)
            == ticket
    }
    pub fn cast_available(&self) -> bool {
        self.services().ports().cast.is_some()
    }
    pub async fn cast_receivers(&self) -> Result<Vec<Receiver>, Error> {
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
    pub fn cast_target(&self) -> Option<(Receiver, String)> {
        self.cast
            .borrow()
            .as_ref()
            .map(|active| (active.receiver.clone(), active.key.clone()))
    }
    pub async fn prepare_cast(
        &self,
        receiver: Receiver,
        source: PlaySource,
    ) -> Result<Box<dyn Prepared>, Error> {
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
        prepared: Box<dyn Prepared>,
        destination: Destination,
        claim: Box<dyn Send>,
        grant: ExternalPlaybackAuthorization,
    ) -> Result<(), Error> {
        if self.cast.borrow().is_some() || !grant.is_current() {
            return Err(Error::Denied);
        }
        let generation = self.cast_generation.clone();
        let remote = self
            .spawn_network(async move {
                if generation.load(std::sync::atomic::Ordering::Acquire) != ticket
                    || !grant.is_current()
                {
                    return Err(Error::Denied);
                }
                let remote = prepared.allow().await?;
                if generation.load(std::sync::atomic::Ordering::Acquire) != ticket
                    || !grant.is_current()
                {
                    remote.stop().await?;
                    return Err(Error::Denied);
                }
                Ok(remote)
            })
            .await
            .map_err(|_| Error::Unknown)??;
        if !self.cast_intent_current(ticket) || self.cast.borrow().is_some() {
            self.spawn_network(async move { remote.stop().await })
                .await
                .map_err(|_| Error::Unknown)??;
            return Err(Error::Denied);
        }
        *self.cast.borrow_mut() = Some(Active {
            receiver: destination.receiver,
            key: destination.key,
            authority: destination.authority,
            remote,
            ticket,
            failed_polls: 0,
            _claim: claim,
        });
        Ok(())
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
                worker.stop().await
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
            .map_err(|_| Error::Unknown)??;
            self.clear_cast(&remote);
        }
        Ok(state)
    }
}
