mod credentials;
mod model;
mod service;
mod session;

#[cfg(test)]
mod tests;

use std::{future::Future, net::SocketAddr, pin::Pin, time::Instant};

pub use credentials::{Credential, CredentialKey, CredentialStore, Pairing, PinCode, StoreError};
pub use model::{
    Availability, Command, Confirmation, Delivery, Diagnostic, Error, ErrorKind, Feature, Link,
    Media, OutputChange, PeerTrust, Playback, Snapshot, Stage, StopOutcome, Volume, VolumeScope,
};
pub use service::{
    Cast, CastBuilder, Connect, Discovery, DiscoveryIssue, ProtocolId, Route, RouteId,
};
pub use session::{Connection, Session, Updates};

#[cfg_attr(
    not(test),
    allow(unused_imports, reason = "backend integration follows this contract")
)]
pub(crate) use model::{Features, Observation};
#[cfg_attr(
    not(test),
    allow(unused_imports, reason = "backend integration follows this contract")
)]
pub(crate) use service::{Candidate, ScanSink};
pub(crate) use session::Dispatch;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub(crate) type VerifiedConnection = (Box<dyn Driver>, Credential);

pub(crate) trait Backend: Send + Sync {
    fn protocol(&self) -> ProtocolId;

    fn uses_credentials(&self) -> bool {
        false
    }

    /// Browse through the caller's deadline, publishing partial discoveries as
    /// they arrive. Finish normally once browsing for this window is complete.
    fn discover(
        &self,
        deadline: tokio::time::Instant,
        sink: ScanSink,
    ) -> BoxFuture<'_, Result<(), Error>>;

    /// Revalidate the selected service identity for its payload. A backend
    /// must verify an existing credential before allowing media submission or
    /// a replacement pairing.
    fn connect<'a>(
        &'a self,
        route: &'a Route,
        credential: Option<Credential>,
    ) -> BoxFuture<'a, Result<BackendConnect, Error>>;
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "backend integration follows this contract")
)]
pub(crate) enum BackendConnect {
    Ready(Box<dyn Driver>),
    PinRequired(Box<dyn PairDriver>),
}

pub(crate) trait PairDriver: Send {
    fn expires_at(&self) -> Option<Instant>;

    /// Return an identity-verified connection and its opaque credential.
    /// Cancellation may interrupt network I/O, then call `cancel`.
    fn submit(&mut self, pin: PinCode) -> BoxFuture<'_, Result<VerifiedConnection, Error>>;

    /// Retire provisional resources without waiting for further user action.
    /// Any network exchange used for cancellation must have its own deadline.
    fn cancel(&mut self) -> BoxFuture<'_, ()>;
}

pub(crate) trait Driver: Send {
    fn control_addrs(&self) -> Option<(SocketAddr, SocketAddr)>;
    fn peer_trust(&self) -> PeerTrust;
    fn validate(&self, media: &Media) -> Result<(), Error>;

    /// Call `dispatch.mark_sent()` immediately before first possible wire
    /// submission. A dropped load future may leave a request in flight.
    /// Retain the correlation needed by `stop` on the driver itself.
    fn load<'a>(
        &'a mut self,
        media: Media,
        dispatch: &'a Dispatch,
    ) -> BoxFuture<'a, Result<Observation, Error>>;

    /// This wait can be dropped in favor of a command or cleanup. Preserve
    /// protocol state and pending messages across cancellation.
    fn observe(&mut self) -> BoxFuture<'_, Result<Observation, Error>>;

    /// Recheck current ownership and capability for each write. Serialize
    /// protocol messages and report the command's own delivery evidence.
    /// The session may drop this wait on timeout or local retirement, so
    /// keep framing and ownership state usable for subsequent stop or close.
    fn command(&mut self, command: Command) -> BoxFuture<'_, Result<Confirmation, Error>>;

    /// Stop only the proven owned load. Reconcile late replies within a bound
    /// and treat missing correlation as uncertainty, even for equal URLs.
    fn stop(&mut self) -> BoxFuture<'_, Result<StopOutcome, Error>>;

    /// Join local resources without issuing a remote stop command.
    /// Closing must retire resources even if the peer never answers.
    fn close(&mut self) -> BoxFuture<'_, ()>;
}
