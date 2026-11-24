//! Availability of playback features in the current presentation.

use crate::layout::Class;
use crate::pip::Pip;
use iptv_core::capabilities::{FormFactor, Host, Input, Platform, Recording};
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
    /// Not on this device yet (an iPad, until measured on one).
    NotOnThisDevice,
    /// A phone held upright: multiview needs it on its side.
    PhoneUpright,
    /// The window is too small for the tiles (a tablet in split screen).
    WindowTooSmall,
    /// The account allows no second stream at once.
    ConnectionLimit,
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

/// How multiview is offered on a device now (#279).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiviewOffer {
    /// Offered, with up to this many tiles.
    Tiles(usize),
    /// Shown disabled with its reason, which can change while it shows.
    Disabled(Reason),
    /// Left out of every entry point; the reason is for a key binding
    /// that asks anyway.
    Omitted(Reason),
}

impl MultiviewOffer {
    /// Whether multiview is drawn at all (enabled or disabled).
    pub fn shown(self) -> bool {
        !matches!(self, MultiviewOffer::Omitted(_))
    }
}

/// The tiles a form factor's screen holds at most: a phone's, a tablet's
/// and a TV's two (a phone's, on its side, are each about as wide as its
/// upright video); a desktop's four.
fn form_tiles(form_factor: FormFactor) -> usize {
    match form_factor {
        FormFactor::Phone | FormFactor::Tablet | FormFactor::Tv => 2,
        FormFactor::Desktop => 4,
    }
}

/// Whether multiview can ever exist on `host`, whatever its window: not on
/// a host with one decoder (the browser TV packages), and not on an iPad
/// until it is measured on one (it draws every mpv frame on the main
/// thread).
pub fn multiview_exists(host: Host) -> bool {
    host.platform != Platform::Ipad && host.decoder_slots() >= 2
}

/// Whether "Sound follows the selected tile" is offered: not on a phone,
/// whose two tiles take a tap to choose the sound, nor where multiview
/// never exists. The stored value is kept, unused there.
pub fn sound_follows_tile(host: Host) -> bool {
    host.form_factor != FormFactor::Phone && multiview_exists(host)
}

/// How multiview is offered on `device` now: `landscape` says whether its
/// window is wider than tall, `account_streams` the streams the shown
/// source's account allows at once, when it says.
///
/// - A phone: two tiles, on its side only, and left out at the account's
///   connection limit. A metered network should also leave it out; no
///   host reports one yet.
/// - A tablet: two tiles in an expanded window; left out in a smaller one
///   (split screen, a compact window).
/// - A TV: two tiles; disabled with its reason at the account's limit.
/// - A desktop: four tiles (Windows has no separate player windows).
/// - Left out where it never exists ([`multiview_exists`]).
///
/// The account's limit is reached when it allows one stream: the one
/// playing full screen. Elsewhere, adding a stream past the limit asks
/// first (`Multiview::over_connections`).
pub fn multiview_offer(
    device: Device,
    landscape: bool,
    account_streams: Option<u32>,
) -> MultiviewOffer {
    use MultiviewOffer::{Disabled, Omitted, Tiles};
    let host = device.host;
    if host.platform == Platform::Ipad {
        return Omitted(Reason::NotOnThisDevice);
    }
    if !multiview_exists(host) {
        return Omitted(Reason::DecoderLimit);
    }
    let at_limit = account_streams.is_some_and(|allowed| allowed <= 1);
    match device.form_factor() {
        FormFactor::Phone if !landscape => Omitted(Reason::PhoneUpright),
        FormFactor::Phone if at_limit => Omitted(Reason::ConnectionLimit),
        FormFactor::Tablet if device.window < Class::Expanded => Omitted(Reason::WindowTooSmall),
        FormFactor::Tv if at_limit => Disabled(Reason::ConnectionLimit),
        form => Tiles(form_tiles(form).min(host.decoder_slots())),
    }
}

/// Presentation availability. Native playback admission still enforces
/// account/parental policy.
pub fn multiview(
    offer: MultiviewOffer,
    main_player: bool,
    playing: bool,
    extra_windows: bool,
) -> Availability {
    use Availability::{Available, Unavailable};
    if let MultiviewOffer::Omitted(reason) = offer {
        Unavailable(reason)
    } else if !main_player {
        Unavailable(Reason::MainPlayerRequired)
    } else if !playing {
        Unavailable(Reason::PlaybackRequired)
    } else if extra_windows {
        Unavailable(Reason::ExtraPlayerWindows)
    } else if let MultiviewOffer::Disabled(reason) = offer {
        Unavailable(reason)
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

    fn device(platform: Platform, form_factor: FormFactor, window: Class) -> Device {
        Device {
            host: Host {
                form_factor,
                ..Host::of(platform)
            },
            window,
        }
    }

    #[test]
    fn multiview_checks_device_and_current_presentation_without_granting_streams() {
        let offer = MultiviewOffer::Tiles(2);
        assert!(multiview(offer, true, true, false).available());
        assert_eq!(
            multiview(offer, false, true, false).reason(),
            Some(Reason::MainPlayerRequired)
        );
        assert_eq!(
            multiview(offer, true, false, false).reason(),
            Some(Reason::PlaybackRequired)
        );
        assert_eq!(
            multiview(offer, true, true, true).reason(),
            Some(Reason::ExtraPlayerWindows)
        );
        let tizen = Device {
            host: Host::default(),
            window: Class::Expanded,
        };
        assert_eq!(
            multiview(multiview_offer(tizen, true, None), true, true, false).reason(),
            Some(Reason::DecoderLimit)
        );
        // A disabled offer names its reason once the rest allows it; an
        // omitted one before anything else.
        let disabled = MultiviewOffer::Disabled(Reason::ConnectionLimit);
        assert_eq!(
            multiview(disabled, true, false, false).reason(),
            Some(Reason::PlaybackRequired)
        );
        assert_eq!(
            multiview(disabled, true, true, false).reason(),
            Some(Reason::ConnectionLimit)
        );
        let omitted = MultiviewOffer::Omitted(Reason::PhoneUpright);
        assert_eq!(
            multiview(omitted, false, false, false).reason(),
            Some(Reason::PhoneUpright)
        );
        assert!(disabled.shown() && offer.shown() && !omitted.shown());
    }

    #[test]
    fn each_host_offers_multiview_as_its_screen_allows() {
        use Class::{Compact, Expanded, Medium};
        use FormFactor::{Desktop, Phone, Tablet, Tv};
        use MultiviewOffer::{Disabled, Omitted, Tiles};
        use Platform::{Android, Ipad, Linux, Macos, Unknown, Windows};
        let upright = Omitted(Reason::PhoneUpright);
        let small = Omitted(Reason::WindowTooSmall);
        let limit = Reason::ConnectionLimit;
        for (platform, form, window, landscape, streams, offer) in [
            // An Android phone: two tiles on its side, never upright, and
            // not at the account's limit.
            (Android, Phone, Medium, true, None, Tiles(2)),
            (Android, Phone, Compact, true, Some(2), Tiles(2)),
            (Android, Phone, Compact, false, None, upright),
            (Android, Phone, Medium, true, Some(1), Omitted(limit)),
            // An Android tablet: two tiles in an expanded window only.
            (Android, Tablet, Expanded, true, None, Tiles(2)),
            (Android, Tablet, Expanded, false, Some(1), Tiles(2)),
            (Android, Tablet, Medium, true, None, small),
            (Android, Tablet, Compact, false, None, small),
            // An Android TV: two tiles, disabled at the account's limit.
            (Android, Tv, Expanded, true, None, Tiles(2)),
            (Android, Tv, Medium, true, Some(4), Tiles(2)),
            (Android, Tv, Expanded, true, Some(1), Disabled(limit)),
            (Android, Tv, Expanded, true, Some(0), Disabled(limit)),
            // An iPad: not until measured, in any window.
            (
                Ipad,
                Tablet,
                Expanded,
                true,
                None,
                Omitted(Reason::NotOnThisDevice),
            ),
            (
                Ipad,
                Tablet,
                Compact,
                false,
                None,
                Omitted(Reason::NotOnThisDevice),
            ),
            // Desktops keep four tiles, whatever the window.
            (Linux, Desktop, Expanded, true, None, Tiles(4)),
            (Macos, Desktop, Compact, false, Some(1), Tiles(4)),
            (Windows, Desktop, Medium, true, None, Tiles(4)),
            // A browser TV package (Tizen): left out, never disabled.
            (
                Unknown,
                Tv,
                Expanded,
                true,
                None,
                Omitted(Reason::DecoderLimit),
            ),
            (
                Unknown,
                Tv,
                Expanded,
                true,
                Some(1),
                Omitted(Reason::DecoderLimit),
            ),
        ] {
            assert_eq!(
                multiview_offer(device(platform, form, window), landscape, streams),
                offer,
                "{platform:?} {form:?} {window:?} landscape={landscape} streams={streams:?}"
            );
        }
    }

    #[test]
    fn a_measured_decoder_limit_leaves_multiview_out() {
        let mut tv = device(Platform::Android, FormFactor::Tv, Class::Expanded);
        tv.host.decoders = Some(1);
        assert_eq!(
            multiview_offer(tv, true, None),
            MultiviewOffer::Omitted(Reason::DecoderLimit)
        );
        assert!(!sound_follows_tile(tv.host));
        tv.host.decoders = Some(8);
        assert_eq!(multiview_offer(tv, true, None), MultiviewOffer::Tiles(2));
    }

    #[test]
    fn sound_follows_tile_only_where_a_grid_has_a_choice_to_follow() {
        use FormFactor::{Desktop, Phone, Tablet, Tv};
        for (platform, form, offered) in [
            (Platform::Android, Phone, false),
            (Platform::Android, Tablet, true),
            (Platform::Android, Tv, true),
            (Platform::Linux, Desktop, true),
            (Platform::Linux, Phone, false),
            (Platform::Ipad, Tablet, false),
            (Platform::Unknown, Tv, false),
        ] {
            let host = Host {
                form_factor: form,
                ..Host::of(platform)
            };
            assert_eq!(sound_follows_tile(host), offered, "{platform:?} {form:?}");
        }
    }
}
