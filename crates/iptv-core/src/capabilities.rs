//! Host facts and current feature availability. These values grant no content access.
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

impl Platform {
    /// The form factor a build of this platform starts with, before its
    /// host reports one: an Android build cannot tell a phone from a TV
    /// until its activity says which, and counts as a tablet until then.
    /// A build without a native platform is a browser TV package (Tizen).
    pub const fn form_factor(self) -> FormFactor {
        match self {
            Platform::Android | Platform::Ipad => FormFactor::Tablet,
            Platform::Linux | Platform::Macos | Platform::Windows => FormFactor::Desktop,
            Platform::Unknown => FormFactor::Tv,
        }
    }

    /// The input a build of this platform starts with, before its host
    /// reports any: a desktop has a keyboard and a browser TV its remote's
    /// D-pad; an Android device or an iPad has neither until it reports one.
    pub const fn input(self) -> Input {
        match self {
            Platform::Linux | Platform::Macos | Platform::Windows => Input {
                keyboard: true,
                dpad: false,
            },
            Platform::Unknown => Input {
                keyboard: false,
                dpad: true,
            },
            Platform::Android | Platform::Ipad => Input {
                keyboard: false,
                dpad: false,
            },
        }
    }
}

/// What kind of device the app runs on, whatever the size of its window:
/// a tablet in split screen is still a tablet. It decides which
/// capabilities exist (recording, system picture in picture, how the app
/// exits); the window's size class decides the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFactor {
    Phone,
    Tablet,
    Tv,
    Desktop,
}

impl FormFactor {
    /// A display narrower than this (dp), at its smallest, is a phone's.
    pub const PHONE_MAX_DP: f32 = 600.0;

    /// An Android device's form factor: a TV when its UI mode or the
    /// leanback feature says so, otherwise a phone or a tablet by the
    /// smallest width of its physical display in dp (not of the app's
    /// window, so a tablet's split-screen window stays a tablet, and a
    /// foldable phone unfolded becomes one). An unknown width (zero or
    /// less) counts as a tablet's.
    pub fn android(tv: bool, display_smallest_dp: f32) -> Self {
        if tv {
            FormFactor::Tv
        } else if display_smallest_dp > 0.0 && display_smallest_dp < Self::PHONE_MAX_DP {
            FormFactor::Phone
        } else {
            FormFactor::Tablet
        }
    }

    /// Its name in host reports and logs.
    pub const fn name(self) -> &'static str {
        match self {
            FormFactor::Phone => "phone",
            FormFactor::Tablet => "tablet",
            FormFactor::Tv => "tv",
            FormFactor::Desktop => "desktop",
        }
    }

    /// The form factor `name` names ([`Self::name`]).
    pub fn from_name(name: &str) -> Option<Self> {
        [
            FormFactor::Phone,
            FormFactor::Tablet,
            FormFactor::Tv,
            FormFactor::Desktop,
        ]
        .into_iter()
        .find(|form| form.name() == name)
    }
}

/// The input devices connected now. A touch screen and a pointer are not
/// counted: these are what keyboard and remote features need.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    /// A hardware keyboard is attached and not hidden (Android's
    /// `Configuration.keyboard`, a desktop's).
    pub keyboard: bool,
    /// A D-pad is attached and not hidden: a TV remote or a game pad's
    /// (Android's `Configuration.navigation`, a browser TV's remote).
    pub dpad: bool,
}

/// What the device is and has now. The platform is fixed by the build;
/// the form factor and the input are reported by the host and can change
/// while the app runs (a foldable opens, a keyboard is attached).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Host {
    pub platform: Platform,
    pub form_factor: FormFactor,
    pub input: Input,
}

impl Default for Host {
    fn default() -> Self {
        Self::of(Platform::Unknown)
    }
}

impl Host {
    /// The host a build of `platform` starts as, before any report.
    pub const fn of(platform: Platform) -> Self {
        Self {
            platform,
            form_factor: platform.form_factor(),
            input: platform.input(),
        }
    }

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
            let host = Host::of(platform);
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
    fn an_android_device_is_classed_by_its_display_not_its_window() {
        // A TV whatever its screen; otherwise the physical display's
        // smallest width decides, so a tablet's split-screen window (which
        // the host does not pass) cannot make it a phone.
        assert_eq!(FormFactor::android(true, 360.0), FormFactor::Tv);
        assert_eq!(FormFactor::android(true, 0.0), FormFactor::Tv);
        assert_eq!(FormFactor::android(false, 360.0), FormFactor::Phone);
        assert_eq!(FormFactor::android(false, 599.0), FormFactor::Phone);
        assert_eq!(FormFactor::android(false, 600.0), FormFactor::Tablet);
        assert_eq!(FormFactor::android(false, 800.0), FormFactor::Tablet);
        // A display the host could not measure is not taken for a phone's.
        assert_eq!(FormFactor::android(false, 0.0), FormFactor::Tablet);
        assert_eq!(FormFactor::android(false, f32::NAN), FormFactor::Tablet);
    }

    #[test]
    fn form_factors_round_trip_through_their_names() {
        for form in [
            FormFactor::Phone,
            FormFactor::Tablet,
            FormFactor::Tv,
            FormFactor::Desktop,
        ] {
            assert_eq!(FormFactor::from_name(form.name()), Some(form));
        }
        assert_eq!(FormFactor::from_name("watch"), None);
    }

    #[test]
    fn builds_start_as_their_platform_until_the_host_reports() {
        for (platform, form, keyboard, dpad) in [
            (Platform::Android, FormFactor::Tablet, false, false),
            (Platform::Linux, FormFactor::Desktop, true, false),
            (Platform::Macos, FormFactor::Desktop, true, false),
            (Platform::Windows, FormFactor::Desktop, true, false),
            (Platform::Ipad, FormFactor::Tablet, false, false),
            (Platform::Unknown, FormFactor::Tv, false, true),
        ] {
            let host = Host::of(platform);
            assert_eq!(host.form_factor, form, "{platform:?}");
            assert_eq!(host.input, Input { keyboard, dpad }, "{platform:?}");
        }
        assert_eq!(Host::default(), Host::of(Platform::Unknown));
    }

    #[test]
    fn host_facilities_follow_the_platform_whatever_the_device_reports() {
        // The reports add signals; they do not change a platform's
        // facilities until a feature asks for them.
        for platform in [
            Platform::Android,
            Platform::Linux,
            Platform::Macos,
            Platform::Windows,
            Platform::Ipad,
            Platform::Unknown,
        ] {
            let built = Host::of(platform);
            for form_factor in [
                FormFactor::Phone,
                FormFactor::Tablet,
                FormFactor::Tv,
                FormFactor::Desktop,
            ] {
                for (keyboard, dpad) in [(false, false), (true, false), (false, true)] {
                    let host = Host {
                        platform,
                        form_factor,
                        input: Input { keyboard, dpad },
                    };
                    assert_eq!(host.decoder_slots(), built.decoder_slots());
                    assert_eq!(host.background_recordings(), built.background_recordings());
                    assert_eq!(host.startup_registration(), built.startup_registration());
                    assert_eq!(host.alarm_permission(), built.alarm_permission());
                    assert_eq!(host.automatic_pip(), built.automatic_pip());
                }
            }
        }
    }

    #[test]
    fn only_an_exact_report_implies_an_exact_start() {
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
        }
    }
}
