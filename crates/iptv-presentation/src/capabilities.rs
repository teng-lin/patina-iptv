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

/// Where the host offers Record now (#275): only where the viewer is
/// present for the whole capture and the host can keep it running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordNow {
    /// In the play bar's row, the control strip, the program menu and
    /// details, and on the Record key (Android TV, the desktops).
    Direct,
    /// Under the play bar's More, in the control strip and the program
    /// menu, and on the Record key, but not as the details' button
    /// (Android phones and tablets, where a background capture is
    /// limited).
    UnderMore,
    /// Nowhere: the iPad, which records only while the app stays in front,
    /// and the browser TV hosts, which cannot record.
    Absent,
}

impl RecordNow {
    /// Whether Record now is offered at all.
    pub fn offered(self) -> bool {
        self != Self::Absent
    }
}

/// When the host shows the Recordings library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingsLibrary {
    /// Always (Android TV, the desktops).
    Shown,
    /// Only once a read of the library found recordings in it: media, or
    /// jobs carried over from an earlier version (Android phones and
    /// tablets; the iPad, where development builds could record).
    WhenHolding,
    /// Never: the browser TV hosts, which have no recordings.
    Absent,
}

impl RecordingsLibrary {
    /// Whether the library shows, given whether a read found it holding
    /// recordings.
    pub fn shown(self, holding: bool) -> bool {
        match self {
            Self::Shown => true,
            Self::WhenHolding => holding,
            Self::Absent => false,
        }
    }
}

/// Where `host` offers Record now ([`RecordNow`]).
pub fn record_now(host: Host) -> RecordNow {
    use iptv_core::capabilities::{FormFactor, Platform};
    match (host.platform, host.form_factor) {
        (Platform::Android, FormFactor::Tv) => RecordNow::Direct,
        (Platform::Android, _) => RecordNow::UnderMore,
        (Platform::Linux | Platform::Macos | Platform::Windows, _) => RecordNow::Direct,
        (Platform::Ipad | Platform::Unknown, _) => RecordNow::Absent,
    }
}

/// When `host` shows the Recordings library ([`RecordingsLibrary`]).
pub fn recordings_library(host: Host) -> RecordingsLibrary {
    use iptv_core::capabilities::{FormFactor, Platform};
    match (host.platform, host.form_factor) {
        (Platform::Android, FormFactor::Tv) => RecordingsLibrary::Shown,
        (Platform::Android | Platform::Ipad, _) => RecordingsLibrary::WhenHolding,
        (Platform::Linux | Platform::Macos | Platform::Windows, _) => RecordingsLibrary::Shown,
        (Platform::Unknown, _) => RecordingsLibrary::Absent,
    }
}

/// What the host reports that can stop a capture or make it costly, as
/// Record now starts one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureLimits {
    /// The active network is metered (Android's report; `None` where the
    /// host has no such signal).
    pub metered: Option<bool>,
    /// Turning the TV off stops a capture: Android TV's Low Power Standby
    /// is on and does not exempt Patina.
    pub standby_stops: bool,
    /// The operating system ends a background capture after six hours a
    /// day (Android 15 and later, for a data-sync service).
    pub background_hours: bool,
    /// A desktop could not keep the computer from sleeping while it
    /// records.
    pub sleep_allowed: bool,
}

/// Message IDs for what the viewer must know as Record now starts on
/// `host` with `limits`, most important first: on a TV, that turning it
/// off stops the recording; on a phone or a tablet, a metered network; on
/// Android 15 and later, the background limit; on a desktop, sleep it
/// could not prevent.
pub fn record_now_notices(host: Host, limits: CaptureLimits) -> Vec<&'static str> {
    use iptv_core::capabilities::FormFactor;
    let mut notices = Vec::new();
    let background = key("Android stops recording in the background after six hours a day.");
    match host.form_factor {
        FormFactor::Tv => {
            if limits.standby_stops {
                notices.push(key(
                    "Recording stops when the TV is turned off; what was recorded until then is kept.",
                ));
            }
            // A TV turned off without Low Power Standby, or left on another
            // app, records in the background too.
            if limits.background_hours {
                notices.push(background);
            }
        }
        FormFactor::Phone | FormFactor::Tablet => {
            if limits.metered == Some(true) {
                notices.push(key(
                    "You are on a metered network; a recording can use several gigabytes an hour.",
                ));
            }
            if limits.background_hours {
                notices.push(background);
            }
        }
        FormFactor::Desktop if limits.sleep_allowed => notices.push(key(
            "Patina could not keep this computer awake; keep it awake until the recording ends.",
        )),
        _ => {}
    }
    notices
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
    fn record_now_and_the_library_follow_the_host_table() {
        use RecordNow::{Absent, Direct, UnderMore};
        use RecordingsLibrary as L;
        use iptv_core::capabilities::{FormFactor, Input};
        for (platform, form_factor, record, library) in [
            (Platform::Android, FormFactor::Tv, Direct, L::Shown),
            (
                Platform::Android,
                FormFactor::Phone,
                UnderMore,
                L::WhenHolding,
            ),
            (
                Platform::Android,
                FormFactor::Tablet,
                UnderMore,
                L::WhenHolding,
            ),
            (Platform::Linux, FormFactor::Desktop, Direct, L::Shown),
            (Platform::Macos, FormFactor::Desktop, Direct, L::Shown),
            (Platform::Windows, FormFactor::Desktop, Direct, L::Shown),
            (Platform::Ipad, FormFactor::Tablet, Absent, L::WhenHolding),
            (Platform::Unknown, FormFactor::Tv, Absent, L::Absent),
        ] {
            let host = Host {
                platform,
                form_factor,
                input: Input::default(),
            };
            assert_eq!(record_now(host), record, "{platform:?} {form_factor:?}");
            assert_eq!(
                recordings_library(host),
                library,
                "{platform:?} {form_factor:?}"
            );
        }
        // An Android build counts as a tablet until its activity reports.
        assert_eq!(record_now(Host::of(Platform::Android)), UnderMore);
        assert!(!Absent.offered());
        assert!(UnderMore.offered() && Direct.offered());
        for holding in [false, true] {
            assert!(L::Shown.shown(holding));
            assert_eq!(L::WhenHolding.shown(holding), holding);
            assert!(!L::Absent.shown(holding));
        }
    }

    #[test]
    fn record_now_says_what_will_stop_it_on_each_form_factor() {
        use iptv_core::capabilities::{FormFactor, Input};
        let host = |platform, form_factor| Host {
            platform,
            form_factor,
            input: Input::default(),
        };
        let tv = host(Platform::Android, FormFactor::Tv);
        let phone = host(Platform::Android, FormFactor::Phone);
        let tablet = host(Platform::Android, FormFactor::Tablet);
        let desktop = host(Platform::Linux, FormFactor::Desktop);
        let none = CaptureLimits::default();
        for host in [tv, phone, tablet, desktop] {
            assert!(record_now_notices(host, none).is_empty(), "{host:?}");
        }
        let standby = CaptureLimits {
            standby_stops: true,
            ..none
        };
        assert_eq!(record_now_notices(tv, standby).len(), 1);
        assert!(record_now_notices(tv, standby)[0].contains("TV is turned off"));
        // Standby is a TV's; a handheld's metered network is not checked
        // on a TV, and an unknown network is not called metered.
        assert!(record_now_notices(phone, standby).is_empty());
        let metered = CaptureLimits {
            metered: Some(true),
            ..none
        };
        assert!(record_now_notices(tv, metered).is_empty());
        assert!(
            record_now_notices(
                phone,
                CaptureLimits {
                    metered: None,
                    ..none
                }
            )
            .is_empty()
        );
        let both = CaptureLimits {
            metered: Some(true),
            background_hours: true,
            ..none
        };
        for handheld in [phone, tablet] {
            let notices = record_now_notices(handheld, both);
            assert_eq!(notices.len(), 2);
            assert!(notices[0].contains("metered"));
            assert!(notices[1].contains("six hours"));
        }
        let sleep = CaptureLimits {
            sleep_allowed: true,
            ..none
        };
        // A TV is limited in the background too, after its standby notice.
        let tv_both = CaptureLimits {
            standby_stops: true,
            background_hours: true,
            ..none
        };
        let notices = record_now_notices(tv, tv_both);
        assert_eq!(notices.len(), 2);
        assert!(notices[0].contains("TV is turned off"));
        assert!(notices[1].contains("six hours"));
        assert!(record_now_notices(desktop, sleep)[0].contains("awake"));
        assert!(record_now_notices(tv, sleep).is_empty());
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
