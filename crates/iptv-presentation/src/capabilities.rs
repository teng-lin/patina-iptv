//! Availability of playback features in the current presentation.

use crate::pip::Pip;
use iptv_core::capabilities::{Host, Recording};
use iptv_core::message::key;
use iptv_core::recording::WakeCapability;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    WaitingForPlayer,
    UnsupportedPip,
    PlaybackRequired,
    PipAlreadyActive,
    MainPlayerRequired,
    ExtraPlayerWindows,
    DecoderLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable(Reason),
}
impl Availability {
    pub fn available(self) -> bool {
        self == Self::Available
    }
    pub fn reason(self) -> Option<Reason> {
        match self {
            Self::Available => None,
            Self::Unavailable(reason) => Some(reason),
        }
    }
}

pub fn pip(pip: &Pip, playing: bool) -> Availability {
    use Availability::{Available, Unavailable};
    if !pip.reported {
        Unavailable(Reason::WaitingForPlayer)
    } else if !pip.supported {
        Unavailable(Reason::UnsupportedPip)
    } else if !playing {
        Unavailable(Reason::PlaybackRequired)
    } else if pip.active {
        Unavailable(Reason::PipAlreadyActive)
    } else {
        Available
    }
}

/// Presentation availability. Native playback admission still enforces account/parental policy.
pub fn multiview(
    host: Host,
    main_player: bool,
    playing: bool,
    extra_windows: bool,
) -> Availability {
    use Availability::{Available, Unavailable};
    if !main_player {
        Unavailable(Reason::MainPlayerRequired)
    } else if !playing {
        Unavailable(Reason::PlaybackRequired)
    } else if extra_windows {
        Unavailable(Reason::ExtraPlayerWindows)
    } else if host.decoder_slots() < 2 {
        Unavailable(Reason::DecoderLimit)
    } else {
        Available
    }
}

/// Message ID for the last recording wake report.
pub fn recording_notice(recording: Recording) -> Option<&'static str> {
    match recording.wake {
        Some(wake @ (WakeCapability::Inexact | WakeCapability::Denied)) => wake.notice(),
        Some(WakeCapability::AwakeOnly) if recording.background => Some(key(concat!(
            "Closing the window keeps accepted recordings running. ",
            "Enable start at login to recover after restart. ",
            "Linux and macOS cannot wake a sleeping or powered-off computer; leave it awake. ",
            "Quit stops the recorder.",
        ))),
        Some(WakeCapability::AwakeOnly) => Some(key(
            "Keep Patina open and this device awake for recordings. Recordings use local app storage and are excluded from backups.",
        )),
        None | Some(WakeCapability::Exact) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::capabilities::Platform;

    #[test]
    fn recording_wake_notices_distinguish_exact_from_limited_starts() {
        for wake in [
            WakeCapability::Exact,
            WakeCapability::Inexact,
            WakeCapability::Denied,
            WakeCapability::AwakeOnly,
        ] {
            let state = Recording {
                background: true,
                wake: Some(wake),
            };
            assert_eq!(
                recording_notice(state).is_some(),
                wake != WakeCapability::Exact
            );
        }
        assert!(recording_notice(Recording::default()).is_none());
    }

    #[test]
    fn live_player_reports_replace_unknown_support_and_revoke_actions() {
        let mut state = Pip::default();
        assert_eq!(pip(&state, true).reason(), Some(Reason::WaitingForPlayer));
        state.report(false, false, false);
        assert_eq!(pip(&state, true).reason(), Some(Reason::UnsupportedPip));
        state.report(true, false, false);
        assert_eq!(pip(&state, false).reason(), Some(Reason::PlaybackRequired));
        assert!(pip(&state, true).available());
        state.report(true, false, true);
        assert_eq!(pip(&state, true).reason(), Some(Reason::PipAlreadyActive));
        state.report(false, false, false);
        assert!(!pip(&state, true).available());
        state.report(true, true, false);
        assert_eq!(
            pip(&state, true).reason(),
            Some(Reason::UnsupportedPip),
            "a TV never has it"
        );
    }

    #[test]
    fn multiview_checks_device_and_current_presentation_without_granting_streams() {
        let host = Host {
            platform: Platform::Android,
        };
        assert!(multiview(host, true, true, false).available());
        assert_eq!(
            multiview(host, false, true, false).reason(),
            Some(Reason::MainPlayerRequired)
        );
        assert_eq!(
            multiview(host, true, false, false).reason(),
            Some(Reason::PlaybackRequired)
        );
        assert_eq!(
            multiview(host, true, true, true).reason(),
            Some(Reason::ExtraPlayerWindows)
        );
        assert_eq!(
            multiview(Host::default(), true, true, false).reason(),
            Some(Reason::DecoderLimit)
        );
    }
}
