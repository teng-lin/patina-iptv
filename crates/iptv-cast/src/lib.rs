//! Native discovery and control of casting receivers through [Cast].

#[cfg(feature = "airplay")]
mod airplay;
#[cfg(feature = "googlecast")]
#[doc(hidden)]
pub mod discovery;
#[cfg(feature = "googlecast")]
#[doc(hidden)]
pub mod googlecast;
pub(crate) mod sender;
#[cfg(any(all(test, feature = "googlecast"), feature = "test-support"))]
#[doc(hidden)]
#[path = "googlecast/tests/fixture.rs"]
pub mod test_support;
#[cfg(feature = "googlecast")]
mod tls;
#[cfg(feature = "googlecast")]
mod wire;

pub use sender::{
    Availability, Cast, CastBuilder, Command, Confirmation, Connect, Connection, Credential,
    CredentialKey, CredentialStore, Delivery, Diagnostic, Discovery, DiscoveryIssue, Error,
    ErrorKind, Feature, Link, Media, OutputChange, Pairing, PeerTrust, PinCode, Playback,
    ProtocolId, Route, RouteId, Session, Snapshot, Stage, StopOutcome, StoreError, Updates, Volume,
    VolumeScope,
};
