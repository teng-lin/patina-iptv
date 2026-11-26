//! Identities and state visible to a sender's picker and remote.
use std::net::SocketAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    pub address: SocketAddr,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Volume(u8),
    StepVolume(i8),
    ToggleMute,
    Play,
    Pause,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Refused,
    Unknown,
    NoRequest { relay_port: u16 },
    Busy,
    Denied,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteState {
    Playing,
    Paused,
    Loading,
    Ended,
}
