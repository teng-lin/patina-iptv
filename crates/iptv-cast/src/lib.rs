//! Native discovery and control of casting receivers.
//!
//! The shared sender contract is being integrated with the existing Google
//! Cast engine. Until then, use the `googlecast` and `discovery` modules for
//! network playback. A shared [Cast] cannot be built without a backend.

pub mod discovery;
pub mod googlecast;
pub(crate) mod sender;
mod tls;
mod wire;

pub use sender::{
    Availability, Cast, CastBuilder, Command, Confirmation, Connect, Connection, Credential,
    CredentialKey, CredentialStore, Delivery, Diagnostic, Discovery, DiscoveryIssue, Error,
    ErrorKind, Feature, Link, Media, OutputChange, Pairing, PeerTrust, PinCode, Playback,
    ProtocolId, Route, RouteId, Session, Snapshot, Stage, StopOutcome, StoreError, Updates, Volume,
    VolumeScope,
};
