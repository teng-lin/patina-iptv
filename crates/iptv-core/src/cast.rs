//! Identities and state visible to a sender's picker and remote.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    pub id: String,
    pub protocol: NetworkProtocol,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkProtocol {
    GoogleCast,
}

/// The native AirPlay destination is selected by Apple's system picker.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Network(Receiver),
    AirPlay,
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
    Cancelled,
    Unavailable,
    Refused,
    Unknown,
    NoRequest { relay_port: u16 },
    StopUnconfirmed,
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
