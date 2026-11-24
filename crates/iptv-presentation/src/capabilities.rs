//! Availability of playback features in the current presentation.

use crate::layout::Class;
use crate::pip::Pip;
use iptv_core::capabilities::{FormFactor, Host, Input, Recording};
use iptv_core::message::key;
use iptv_core::recording::WakeCapability;

/// The signals features are gated by, as they are now: the device's form
/// factor and connected input (`host`, which decide which capabilities and
/// settings exist), and the window's size class (which decides layouts).
/// All three can change while the app runs, so gates take a `Device`
/// rather than reading a build constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub host: Host,
    pub window: Class,
}

impl Device {
    pub fn form_factor(self) -> FormFactor {
        self.host.form_factor
    }

    pub fn input(self) -> Input {
        self.host.input
    }
}

/// What the keys connected now are set up as in Settings (#280): a
/// keyboard's shortcuts, or a remote's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    /// A keyboard: its single-key shortcuts can be remapped or turned off
    /// (WCAG 2.1.4); keys only a remote has (colour keys, Play, Stop,
    /// Record) are left out.
    Keyboard,
    /// A TV remote's or a game controller's D-pad, with every remote key.
    Remote,
}

/// Which keys `host` has to set up, if any. A TV has its remote and a
/// desktop its keyboard, whatever is reported. A phone or a tablet has
/// them only while one is connected (not when one was used last, so a
/// keyboard-cover user who reaches Settings by touch still finds them):
/// a D-pad makes them a remote's, a keyboard alone a keyboard's, and a
/// touch screen alone has none.
pub fn keys(host: Host) -> Option<Keys> {
    match host.form_factor {
        FormFactor::Tv => Some(Keys::Remote),
        FormFactor::Desktop => Some(Keys::Keyboard),
        FormFactor::Phone | FormFactor::Tablet => {
            let Input { keyboard, dpad } = host.input;
            if dpad {
                Some(Keys::Remote)
            } else if keyboard {
                Some(Keys::Keyboard)
            } else {
                None
            }
        }
    }
}

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
    fn keys_follow_the_form_factor_and_the_connected_input() {
        let host = |form_factor, keyboard, dpad| Host {
            platform: Platform::Android,
            form_factor,
            input: Input { keyboard, dpad },
        };
        for form in [FormFactor::Phone, FormFactor::Tablet] {
            assert_eq!(keys(host(form, false, false)), None, "{form:?}");
            assert_eq!(keys(host(form, true, false)), Some(Keys::Keyboard));
            assert_eq!(keys(host(form, false, true)), Some(Keys::Remote));
            assert_eq!(keys(host(form, true, true)), Some(Keys::Remote));
        }
        for (keyboard, dpad) in [(false, false), (true, false), (false, true)] {
            let tv = host(FormFactor::Tv, keyboard, dpad);
            assert_eq!(keys(tv), Some(Keys::Remote));
            let desktop = host(FormFactor::Desktop, keyboard, dpad);
            assert_eq!(keys(desktop), Some(Keys::Keyboard));
        }
        // Builds before any report.
        assert_eq!(keys(Host::of(Platform::Linux)), Some(Keys::Keyboard));
        assert_eq!(keys(Host::of(Platform::Unknown)), Some(Keys::Remote));
        assert_eq!(keys(Host::of(Platform::Android)), None);
    }

    #[test]
    fn multiview_checks_device_and_current_presentation_without_granting_streams() {
        let host = Host::of(Platform::Android);
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
