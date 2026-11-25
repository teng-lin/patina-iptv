//! The device signals features are gated by, as the host reports them now
//! (`iptv_presentation::capabilities::Device`):
//!
//! - The form factor (phone, tablet, TV, desktop). A desktop build is a
//!   desktop and a browser TV package (Tizen) a TV, from the build. An
//!   iPad build follows the system's idiom, which is the device's, not
//!   the window's. Android's activity reports it ([`bridge`]): a TV by its
//!   UI mode or the leanback feature, otherwise a phone or a tablet by
//!   the physical display's smallest width, so a tablet's split-screen
//!   window stays a tablet. NSH's idiom cannot stand in for it there: it
//!   follows the window's smallest width.
//! - The window size class (`crate::layout::Class`), of the current
//!   window at the guide's scale and the viewer's text size.
//! - The connected input: a hardware keyboard or a D-pad. A desktop has a
//!   keyboard and a browser TV its remote, from the build; Android's
//!   activity reports `Configuration.keyboard` and `navigation`, again on
//!   each configuration change. iPadOS reports `GCKeyboard` on mount and
//!   whenever its connect or disconnect notifications arrive.
//! - The video decoders the device runs at once, where its host measures
//!   them: Android's activity reports its hardware H.264 decoder's
//!   instance limit (`MediaCodecInfo.CodecCapabilities`), which can only
//!   lower the platform's budget (`Host::decoder_slots`).
//!
//! Every value is reactive: a closure that reads one runs again when it
//! changes, so gates follow a foldable opening, a resized window or a
//! keyboard attached during a session without a rebuild.
//!
//! Beside them, what the system's accessibility settings ask of panels
//! that hide by themselves ([`recommended`], [`assistive`]): Android's
//! recommended timeouts and whether Switch Access runs, which the same
//! observer reports, and whether a screen reader runs, which NSH reports
//! on every host.

use std::cell::Cell;

use iptv_core::capabilities::{FormFactor, Host, Input};
use iptv_presentation::capabilities::Device;
use nsh_core::platform::Idiom;
use nsh_core::prelude::*;

use crate::layout::Class;

/// What the host reported: nothing until it does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Report {
    form_factor: Option<FormFactor>,
    input: Option<Input>,
    decoders: Option<usize>,
}

/// How long the system asks panels that hide by themselves to stay, at
/// least, for this viewer (ms; 0 where it asks nothing): Android's
/// `AccessibilityManager.getRecommendedTimeoutMillis`, set in its
/// accessibility settings as the time to take action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recommended {
    /// For panels with controls (`FLAG_CONTENT_CONTROLS`).
    pub controls_ms: u64,
    /// For panels to read (`FLAG_CONTENT_TEXT`).
    pub text_ms: u64,
}

/// The system's accessibility settings, as the host reported them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Accessibility {
    recommended: Recommended,
    /// Switch Access (or another switch service) runs: its user steps
    /// through the controls by switch, as slowly as they need.
    switch_access: bool,
}

thread_local! {
    static REPORTED: Cell<Option<Signal<Report>>> = const { Cell::new(None) };
    static ACCESSIBILITY: Cell<Option<Signal<Accessibility>>> = const { Cell::new(None) };
}

fn accessibility() -> Accessibility {
    ACCESSIBILITY
        .with(Cell::get)
        .map(|a| a.get())
        .unwrap_or_default()
}

/// The panels' timeouts the system recommends now (reactive; nothing
/// until the host reports, and nothing off Android).
pub fn recommended() -> Recommended {
    accessibility().recommended
}

/// Whether assistive technology that moves through the controls at its
/// own pace runs now (reactive): a screen reader (TalkBack, VoiceOver,
/// Narrator, Orca, as NSH reports it), Android's Switch Access or iPadOS's
/// Switch Control. Panels then stay until the viewer hides them.
pub fn assistive() -> bool {
    screen_reader() || accessibility().switch_access
}

/// The host now (reactive): the build's platform, with the form factor
/// and the input the device reports. Off the UI thread, and before the
/// root mounts, it is the build's ([`crate::capabilities::BUILT`]).
pub fn host() -> Host {
    let mut host = built();
    let Some(reported) = REPORTED.with(Cell::get) else {
        return host;
    };
    if crate::host::IPAD {
        host.form_factor = match idiom() {
            Idiom::Phone => FormFactor::Phone,
            Idiom::Tv => FormFactor::Tv,
            Idiom::Mac => FormFactor::Desktop,
            _ => FormFactor::Tablet,
        };
    }
    let report = reported.get();
    if let Some(form_factor) = report.form_factor {
        host.form_factor = form_factor;
    }
    if let Some(input) = report.input {
        host.input = input;
    }
    if report.decoders.is_some() {
        host.decoders = report.decoders;
    }
    host
}

/// The main window's size class (reactive: the window's size, the
/// guide's scale and the viewer's text size). A desktop's auxiliary
/// player windows have their own bounds; a gate that lays them out must
/// read their window, not this. Before the root mounts, and
/// off the UI thread, there is no window: expanded, the class that
/// leaves nothing out.
pub fn window_class() -> Class {
    if REPORTED.with(Cell::get).is_none() {
        return Class::Expanded;
    }
    Class::of(
        window().width.0,
        crate::scale::now() * crate::settings::text_factor(),
    )
}

/// Whether the system's clock counts 24 hours (reactive): Android's
/// 24-hour setting, else its locale's, and iPadOS's, as NSH receives them.
/// `None` before the root mounts and off the UI thread, where the system's
/// settings cannot be read.
pub fn system_24_hour() -> Option<bool> {
    REPORTED.with(Cell::get)?;
    Some(matches!(
        locale_settings().hour_cycle,
        nsh_core::platform::HourCycle::H23 | nsh_core::platform::HourCycle::H24
    ))
}

/// The build's host; in the headless scenarios of other platforms' gates,
/// the one `IPTV_TEST_PLATFORM` names (`android`, `ipad` or `tizen`), with
/// the form factor and input reports still applying
/// (`tests/app/record_now_hosts.rs`).
fn built() -> Host {
    #[cfg(feature = "test-support")]
    if let Some(name) = std::env::var_os("IPTV_TEST_PLATFORM") {
        use iptv_core::capabilities::Platform;
        let platform = match name.to_str() {
            Some("android") => Platform::Android,
            Some("ipad") => Platform::Ipad,
            Some("tizen") => Platform::Unknown,
            _ => panic!("IPTV_TEST_PLATFORM: android, ipad or tizen"),
        };
        return Host::of(platform);
    }
    crate::capabilities::BUILT
}

/// All three signals now (reactive).
pub fn current() -> Device {
    Device {
        host: host(),
        window: window_class(),
    }
}

/// Whether the native observer reports here: on Android, iPadOS, and in the
/// headless scenarios that simulate it (`tests/app/device_signals.rs`).
fn observed() -> bool {
    crate::host::ANDROID
        || crate::host::IPAD
        || (cfg!(feature = "test-support")
            && std::env::var_os("IPTV_TEST_DEVICE_SIGNALS").is_some())
}

/// The host's report of `data`: `{"form": "phone" | "tablet" | "tv" |
/// "desktop", "keyboard": bool, "dpad": bool, "decoders": number}`; a
/// field missing or not understood keeps what was reported before.
fn parse(data: &nsh_core::JsonValue, before: Report) -> Report {
    let form_factor = data["form"]
        .as_str()
        .and_then(FormFactor::from_name)
        .or(before.form_factor);
    let input = match (data["keyboard"].as_bool(), data["dpad"].as_bool()) {
        (Some(keyboard), Some(dpad)) => Some(Input { keyboard, dpad }),
        _ => before.input,
    };
    let decoders = data["decoders"]
        .as_u64()
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
        .or(before.decoders);
    Report {
        form_factor,
        input,
        decoders,
    }
}

/// The host's accessibility report `data`: `{"controls_ms": number,
/// "text_ms": number, "switch_access": bool}`; a field missing or not
/// understood keeps what was reported before.
fn parse_accessibility(data: &nsh_core::JsonValue, before: Accessibility) -> Accessibility {
    let ms = |name: &str, before: u64| data[name].as_u64().unwrap_or(before);
    Accessibility {
        recommended: Recommended {
            controls_ms: ms("controls_ms", before.recommended.controls_ms),
            text_ms: ms("text_ms", before.recommended.text_ms),
        },
        switch_access: data["switch_access"]
            .as_bool()
            .unwrap_or(before.switch_access),
    }
}

/// Keeps the device's reports for [`host`] and [`recommended`]: on
/// Android and iPadOS, a zero-size native view (`tv.device`, `DeviceComponent.kt`)
/// that reports the form factor and the input when it attaches and on
/// every configuration change, and the accessibility settings when it
/// attaches and whenever they change. Mount once, in the main window's
/// root.
pub(crate) fn bridge() -> impl IntoElement {
    crate::capture_limits::init();
    let reported = signal(Report::default());
    REPORTED.with(|cell| cell.set(Some(reported)));
    let accessibility = signal(Accessibility::default());
    ACCESSIBILITY.with(|cell| cell.set(Some(accessibility)));
    let observed = observed();
    let probe = cfg!(feature = "test-support") && observed && !crate::host::ANDROID;
    view()
        .absolute()
        .w(px(0.0))
        .h(px(0.0))
        .pointer_events(PointerEvents::None)
        .child(when(
            move || observed,
            move || {
                native("tv.device")
                    .test_id("device-signals")
                    .absolute()
                    .w(px(0.0))
                    .h(px(0.0))
                    .pointer_events(PointerEvents::None)
                    .accessibility_elements_hidden(true)
                    .on_native_event(move |event: &NativeEvent| {
                        if event.name.as_ref() == "accessibility" {
                            let next =
                                parse_accessibility(&event.data, accessibility.get_untracked());
                            if next != accessibility.get_untracked() {
                                log::info!(
                                    "iptv accessibility: controls={}ms text={}ms switch_access={}",
                                    next.recommended.controls_ms,
                                    next.recommended.text_ms,
                                    next.switch_access,
                                );
                                accessibility.set(next);
                            }
                            return;
                        }
                        // What can stop a capture (`capture_limits.rs`).
                        if event.name.as_ref() == "limits" {
                            crate::capture_limits::report(&event.data);
                            return;
                        }
                        // The headless scenarios' stand-in for an account's
                        // import reporting its connection limit
                        // (`tests/app/multiview_by_device.rs`).
                        #[cfg(feature = "test-support")]
                        if probe && event.name.as_ref() == "accountStreams" {
                            let limit = event.data["limit"]
                                .as_u64()
                                .map(|n| u32::try_from(n).unwrap_or(u32::MAX));
                            crate::runtime_events::account_streams_reported(limit);
                            return;
                        }
                        if event.name.as_ref() != "device" {
                            return;
                        }
                        let next = parse(&event.data, reported.get_untracked());
                        if next != reported.get_untracked() {
                            log::info!(
                                "iptv device: {} keyboard={} dpad={} decoders={:?}",
                                next.form_factor.map_or("unreported", FormFactor::name),
                                next.input.is_some_and(|i| i.keyboard),
                                next.input.is_some_and(|i| i.dpad),
                                next.decoders,
                            );
                            reported.set(next);
                        }
                    })
            },
        ))
        // What the gates see, for the headless scenarios to read.
        .child(when(
            move || probe,
            move || {
                text(move || describe(current()))
                    .test_id("device-signals-now")
                    .absolute()
            },
        ))
        // The settings rows the device decides (#281), as offered now.
        .child(when(
            move || probe,
            move || {
                text(move || offered_rows(current()))
                    .test_id("device-rows-now")
                    .absolute()
            },
        ))
}

/// The rows the device decides and their neighbours (#281), as the
/// settings screen offers them on `device`: `rows: general.clock …` (a
/// headless probe; the screen's own rows are a virtual list).
fn offered_rows(device: Device) -> String {
    const ROWS: &[&str] = &[
        "general.clock",
        "appearance.ui_transparency",
        "guide.browse_mode",
        "guide.list_browse_mode",
        "guide.visible_minutes",
        "guide.after_choosing",
        "guide.fullscreen_timeout",
        "player.info_on_switch",
        "player.info_panel_timeout",
        "player.controls_panel_timeout",
        "catchup.forward_step",
        "catchup.rewind_step",
    ];
    use iptv_presentation::settings::screen::Offer;
    // A row on the Advanced page counts as offered.
    let offered = ROWS.iter().copied().filter(|key| {
        matches!(
            crate::settings::offer(device, key),
            Offer::Active | Offer::Advanced
        )
    });
    std::iter::once("rows:")
        .chain(offered)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The signals in words: `tablet medium keyboard dpad` (a headless probe).
fn describe(device: Device) -> String {
    let window = match device.window {
        Class::Compact => "compact",
        Class::Medium => "medium",
        Class::Expanded => "expanded",
    };
    let input = device.input();
    let mut words = vec![device.form_factor().name(), window];
    if input.keyboard {
        words.push("keyboard");
    }
    if input.dpad {
        words.push("dpad");
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> nsh_core::JsonValue {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_report_replaces_what_it_names_and_keeps_the_rest() {
        let first = parse(
            &json(r#"{"form":"phone","keyboard":false,"dpad":true}"#),
            Report::default(),
        );
        assert_eq!(first.form_factor, Some(FormFactor::Phone));
        assert_eq!(
            first.input,
            Some(Input {
                keyboard: false,
                dpad: true
            })
        );
        // An unknown form keeps the last one; a full input replaces it.
        let second = parse(
            &json(r#"{"form":"watch","keyboard":true,"dpad":false}"#),
            first,
        );
        assert_eq!(second.form_factor, Some(FormFactor::Phone));
        assert_eq!(
            second.input,
            Some(Input {
                keyboard: true,
                dpad: false
            })
        );
        // Half an input report is not one.
        assert_eq!(parse(&json(r#"{"keyboard":false}"#), second), second);
        // A decoder count is kept until the next one.
        let third = parse(&json(r#"{"decoders":3}"#), second);
        assert_eq!(third.decoders, Some(3));
        assert_eq!(third.form_factor, second.form_factor);
        assert_eq!(parse(&json(r#"{"form":"tv"}"#), third).decoders, Some(3));
        assert_eq!(parse(&json(r#"{"decoders":-1}"#), third).decoders, Some(3));
    }

    #[test]
    fn an_accessibility_report_replaces_what_it_names() {
        let first = parse_accessibility(
            &json(r#"{"controls_ms":30000,"text_ms":10000,"switch_access":true}"#),
            Accessibility::default(),
        );
        assert_eq!(
            first,
            Accessibility {
                recommended: Recommended {
                    controls_ms: 30_000,
                    text_ms: 10_000,
                },
                switch_access: true,
            }
        );
        // A field missing or not understood keeps what came before.
        let second = parse_accessibility(&json(r#"{"switch_access":false,"text_ms":-1}"#), first);
        assert_eq!(second.recommended, first.recommended);
        assert!(!second.switch_access);
    }

    #[test]
    fn the_probe_names_every_signal() {
        let device = Device {
            host: Host {
                platform: iptv_core::capabilities::Platform::Android,
                form_factor: FormFactor::Tv,
                input: Input {
                    keyboard: true,
                    dpad: true,
                },
                decoders: None,
            },
            window: Class::Expanded,
        };
        assert_eq!(describe(device), "tv expanded keyboard dpad");
    }
}
