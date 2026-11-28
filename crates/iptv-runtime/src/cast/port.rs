//! Leaf cast port traits shared by `ports.rs` and `cast.rs`.
use super::pairing::PairingPrompt;
use crate::parental::gate::ExternalPlaybackAuthorization;
use iptv_core::{
    cast::{Command, Controls, Error, Receiver, RemoteState},
    playback::PlaySource,
};
use std::{any::Any, future::Future, pin::Pin, sync::Arc};

/// Whether the host cast port includes AirPlay receivers alongside Chromecast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirPlayMode {
    Disabled,
    Enabled,
}

pub type Task<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

pub trait Port: Send + Sync {
    fn configure_airplay(&self, _mode: AirPlayMode) {}
    fn last_peer_ip(&self) -> Option<String> {
        None
    }
    fn discover(&self) -> Task<'_, Vec<Receiver>>;
    fn forget(&self, _receiver: Receiver) -> Task<'_, ()> {
        Box::pin(std::future::ready(Err(Error::Unsupported)))
    }
    /// Connects and prepares the handoff without opening the upstream.
    fn prepare(
        &self,
        receiver: Receiver,
        source: PlaySource,
        pairing: PairingPrompt,
    ) -> Task<'_, Box<dyn Prepared>>;
}

/// A system route picker is mounted by the host; its player remains owned by
/// this runtime's transport. The opaque view identity carries no media metadata.
pub trait NativePort: Send + Sync {
    fn view_id(&self) -> Option<u64>;
    fn prepare(&self, source: PlaySource) -> Task<'_, Box<dyn Prepared>>;
    fn cancel_pending(&self);
    /// Join queued player cleanup before local audio resumes after a failed handoff.
    fn flush(&self) -> Retirement<'_> {
        Box::pin(std::future::ready(()))
    }
}

pub trait Prepared: Send {
    /// Recheck policy and intent while a system picker waits for owner approval.
    fn guard(&mut self, _current: Box<dyn Fn() -> bool + Send + Sync>) {}
    /// Sends LOAD and waits for the selected TV's first authorized HTTP request.
    fn load(&mut self) -> Task<'_, ()>;
    /// Open and settle under the grant, joining relay retirement on denial.
    fn allow(self: Box<Self>, authorization: &Authorization) -> Task<'_, Admitted>;
}

/// Sealed marker implemented only by `store_service::Services` in `cast.rs`.
pub(super) trait AuthorizationServices: Any + Send + Sync {}

/// Monitors the playback grant while the receiver owns an unpublished relay.
pub struct Authorization {
    pub(super) grant: ExternalPlaybackAuthorization,
    pub(super) services: Box<dyn AuthorizationServices>,
}

pub struct Admitted {
    pub remote: Arc<dyn Remote>,
    pub paused: bool,
}

pub type Retirement<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub trait Remote: Send + Sync {
    fn command(&self, command: Command) -> Task<'_, ()>;
    /// The most recent session capabilities, available without making a network request.
    fn controls(&self) -> Option<Controls> {
        Some(Controls::ALL)
    }
    /// Reports the owned media session, or a transport error if its state is unknown.
    fn state(&self) -> Task<'_, RemoteState>;
    /// Joins upstream retirement before the account claim can be released.
    fn retire(&self) -> Retirement<'_>;
    /// A native receiver can keep its route for the immediately following channel.
    fn retire_for_replacement(&self) -> Retirement<'_> {
        self.retire()
    }
    /// Revoke relay access even when the control connection has been lost.
    fn stop(&self) -> Retirement<'_> {
        Box::pin(async {
            self.retire().await;
            let _ = self.command(Command::Stop).await;
        })
    }
    /// Joins retirement and tells the viewer if the remote outcome is uncertain.
    fn stop_checked(&self) -> Task<'_, ()> {
        Box::pin(async {
            self.stop().await;
            Ok(())
        })
    }
}
