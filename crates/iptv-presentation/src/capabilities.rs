//! Availability of playback features in the current presentation.

use crate::layout::Class;
use crate::pip::Pip;
use iptv_core::capabilities::{FormFactor, Host, Input, Platform};
use iptv_core::message::key;

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
    /// Not on this device yet (an iPad, until measured on one).
    NotOnThisDevice,
    /// A phone held upright: multiview needs it on its side.
    PhoneUpright,
    /// The window is too small for the tiles (a tablet in split screen).
    WindowTooSmall,
    /// The account allows no second stream at once.
    ConnectionLimit,
    /// A phone avoids the additional stream on a metered network.
    MeteredNetwork,
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
/// source's account allows at once, when it says; `metered` is the host's
/// report of the active network (`None` when unknown).
///
/// - A phone: two tiles, on its side only, and left out at the account's
///   connection limit or on a reported metered network.
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
    metered: Option<bool>,
) -> MultiviewOffer {
    use MultiviewOffer::{Disabled, Omitted, Tiles};
    let host = device.host;
    if host.platform == Platform::Ipad {
        return Omitted(Reason::NotOnThisDevice);
    }
    if !multiview_exists(host) {
        return Omitted(Reason::DecoderLimit);
    }
    if host.form_factor == FormFactor::Phone && metered == Some(true) {
        return Omitted(Reason::MeteredNetwork);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_now_and_the_library_follow_the_host_table() {
        use RecordNow::{Absent, Direct, UnderMore};
        use RecordingsLibrary as L;
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
                form_factor,
                ..Host::of(platform)
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
        let host = |platform, form_factor| Host {
            form_factor,
            ..Host::of(platform)
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
    fn keys_follow_the_form_factor_and_the_connected_input() {
        let host = |form_factor, keyboard, dpad| Host {
            platform: Platform::Android,
            form_factor,
            input: Input { keyboard, dpad },
            decoders: None,
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
            multiview(multiview_offer(tizen, true, None, None), true, true, false).reason(),
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
                multiview_offer(device(platform, form, window), landscape, streams, None),
                offer,
                "{platform:?} {form:?} {window:?} landscape={landscape} streams={streams:?}"
            );
        }
    }

    #[test]
    fn only_phones_omit_multiview_on_a_reported_metered_network() {
        for form in [FormFactor::Phone, FormFactor::Tablet, FormFactor::Tv] {
            let device = device(Platform::Android, form, Class::Expanded);
            for metered in [None, Some(false), Some(true)] {
                let expected = if form == FormFactor::Phone && metered == Some(true) {
                    MultiviewOffer::Omitted(Reason::MeteredNetwork)
                } else {
                    MultiviewOffer::Tiles(2)
                };
                assert_eq!(multiview_offer(device, true, Some(2), metered), expected);
            }
        }
    }

    #[test]
    fn a_measured_decoder_limit_leaves_multiview_out() {
        let mut tv = device(Platform::Android, FormFactor::Tv, Class::Expanded);
        tv.host.decoders = Some(1);
        assert_eq!(
            multiview_offer(tv, true, None, None),
            MultiviewOffer::Omitted(Reason::DecoderLimit)
        );
        assert!(!sound_follows_tile(tv.host));
        tv.host.decoders = Some(8);
        assert_eq!(
            multiview_offer(tv, true, None, None),
            MultiviewOffer::Tiles(2)
        );
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
