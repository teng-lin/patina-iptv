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
    AirPlay,
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

/// Controls currently offered for the owned media. Local retirement remains
/// available even when a receiver offers no remote stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    pub play: bool,
    pub pause: bool,
    pub volume: bool,
    pub step_volume: bool,
    pub toggle_mute: bool,
}

impl Controls {
    pub const NONE: Self = Self {
        play: false,
        pause: false,
        volume: false,
        step_volume: false,
        toggle_mute: false,
    };
    pub const ALL: Self = Self {
        play: true,
        pause: true,
        volume: true,
        step_volume: true,
        toggle_mute: true,
    };

    pub fn supports(self, command: Command) -> bool {
        match command {
            Command::Play => self.play,
            Command::Pause => self.pause,
            Command::Volume(_) => self.volume,
            Command::StepVolume(_) => self.step_volume,
            Command::ToggleMute => self.toggle_mute,
            Command::Stop => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    Unavailable,
    Refused,
    Unknown,
    NoRequest { relay_port: u16 },
    AirPlayTiming { port: u16 },
    AirPlayTimingBind { port: u16 },
    InvalidAirPlayTimingPort,
    AirPlayRelayAddress,
    PairingRejected,
    PairingStorage,
    PairingInvalid,
    PairingIdentityChanged,
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
