//! Host facts and current feature availability. These values grant no content access.
use crate::message::key;
use crate::{pip::Pip, recording::WakeCapability};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Platform {
    Android,
    Linux,
    Macos,
    Windows,
    Ipad,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Host {
    pub platform: Platform,
}
impl Host {
    pub const fn decoder_slots(self) -> usize {
        match self.platform {
            Platform::Android => 2,
            Platform::Linux | Platform::Macos | Platform::Windows | Platform::Ipad => 4,
            Platform::Unknown => 1,
        }
    }
    pub const fn background_recordings(self) -> bool {
        matches!(
            self.platform,
            Platform::Android | Platform::Linux | Platform::Macos | Platform::Windows
        )
    }
    pub const fn startup_registration(self) -> bool {
        matches!(
            self.platform,
            Platform::Linux | Platform::Macos | Platform::Windows
        )
    }
    pub const fn alarm_permission(self) -> bool {
        matches!(self.platform, Platform::Android)
    }
    pub const fn automatic_pip(self) -> bool {
        matches!(self.platform, Platform::Android)
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

/// Current native scheduling report, independent of whether a particular job is allowed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recording {
    pub background: bool,
    /// None until the host has acknowledged registration, including a denied report.
    pub wake: Option<WakeCapability>,
}
impl Recording {
    /// Exact starts are a host report, never inferred from the operating system name.
    pub fn exact_start(self) -> bool {
        self.wake == Some(WakeCapability::Exact)
    }
    pub fn notice(self) -> Option<&'static str> {
        match self.wake {
            Some(wake @ (WakeCapability::Inexact | WakeCapability::Denied)) => wake.notice(),
            Some(WakeCapability::AwakeOnly) if self.background => Some(key(concat!(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_profiles_do_not_imply_permissions_or_exact_wakes() {
        for (platform, slots, background, startup, alarms) in [
            (Platform::Android, 2, true, false, true),
            (Platform::Linux, 4, true, true, false),
            (Platform::Macos, 4, true, true, false),
            (Platform::Windows, 4, true, true, false),
            (Platform::Ipad, 4, false, false, false),
            (Platform::Unknown, 1, false, false, false),
        ] {
            let host = Host { platform };
            assert_eq!(host.decoder_slots(), slots);
            assert_eq!(host.background_recordings(), background);
            assert_eq!(host.startup_registration(), startup);
            assert_eq!(host.alarm_permission(), alarms);
            assert_eq!(host.automatic_pip(), alarms);
            let name = match platform {
                Platform::Android => "Android TV/tablet",
                Platform::Linux => "Linux",
                Platform::Macos => "macOS",
                Platform::Windows => "Windows preview",
                Platform::Ipad => "iPad",
                Platform::Unknown => "Unknown",
            };
            let yn = |value| if value { "Yes" } else { "No" };
            let row = format!(
                "| {name} | {slots} | {} | {} | {} |",
                yn(background),
                yn(startup),
                yn(alarms)
            );
            assert!(
                include_str!("../../../docs/capabilities.md").contains(&row),
                "documented profile drifted: {row}"
            );
            assert!(
                !Recording {
                    background,
                    wake: None
                }
                .exact_start()
            );
        }
    }

    #[test]
    fn live_player_reports_replace_unknown_support_and_revoke_actions() {
        let mut state = Pip::default();
        assert_eq!(pip(&state, true).reason(), Some(Reason::WaitingForPlayer));
        state.report(false, true, false);
        assert_eq!(pip(&state, true).reason(), Some(Reason::UnsupportedPip));
        state.report(true, true, false);
        assert_eq!(pip(&state, false).reason(), Some(Reason::PlaybackRequired));
        assert!(pip(&state, true).available());
        state.report(true, true, true);
        assert_eq!(pip(&state, true).reason(), Some(Reason::PipAlreadyActive));
        state.report(false, true, false);
        assert!(!pip(&state, true).available());
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

    #[test]
    fn wake_permission_reports_keep_awake_and_denial_distinct() {
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
            assert_eq!(state.exact_start(), wake == WakeCapability::Exact);
            assert_eq!(state.notice().is_some(), wake != WakeCapability::Exact);
        }
        assert!(Recording::default().notice().is_none());
    }
}
