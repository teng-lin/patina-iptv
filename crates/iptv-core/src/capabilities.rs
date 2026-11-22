//! Host facts and current feature availability. These values grant no content access.
use crate::message::key;
use crate::recording::WakeCapability;

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

pub use crate::capabilities_state::{Availability, Reason, multiview, pip};

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
