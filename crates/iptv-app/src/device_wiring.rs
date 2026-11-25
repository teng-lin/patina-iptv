//! Dispatches reports from the main window's native device observer.

use nsh_core::prelude::*;

pub(crate) fn bridge() -> View {
    crate::capture_limits::init();
    #[cfg(feature = "test-support")]
    let probe = probe_enabled();
    let bridge = crate::device::bridge(move |event| match event.name.as_ref() {
        "limits" => crate::capture_limits::report(&event.data),
        // Simulate an import reporting its connection limit in headless tests.
        #[cfg(feature = "test-support")]
        "accountStreams" if probe => {
            let limit = event.data["limit"]
                .as_u64()
                .map(|n| u32::try_from(n).unwrap_or(u32::MAX));
            crate::runtime_events::account_streams_reported(limit);
        }
        _ => {}
    });
    #[cfg(feature = "test-support")]
    let bridge = bridge
        .child(when(
            move || probe,
            || {
                text(move || describe(crate::device::current()))
                    .test_id("device-signals-now")
                    .absolute()
            },
        ))
        .child(when(
            move || probe,
            || {
                text(move || offered_rows(crate::device::current()))
                    .test_id("device-rows-now")
                    .absolute()
            },
        ));
    bridge
}

#[cfg(feature = "test-support")]
fn probe_enabled() -> bool {
    std::env::var_os("IPTV_TEST_DEVICE_SIGNALS").is_some()
        && !crate::host::ANDROID
        && !crate::host::IPAD
}

/// The rows the device decides and their neighbours (#281), as the
/// settings screen offers them on `device`: `rows: general.clock …` (a
/// headless probe; the screen's own rows are a virtual list).
#[cfg(feature = "test-support")]
fn offered_rows(device: iptv_presentation::capabilities::Device) -> String {
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
            crate::settings_offer::offer(device, key),
            Offer::Active | Offer::Advanced
        )
    });
    std::iter::once("rows:")
        .chain(offered)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The signals in words: `tablet medium keyboard dpad` (a headless probe).
#[cfg(any(test, feature = "test-support"))]
fn describe(device: iptv_presentation::capabilities::Device) -> String {
    use crate::layout::Class;

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
    use crate::layout::Class;
    use iptv_core::capabilities::{FormFactor, Host, Input, Platform};
    use iptv_presentation::capabilities::Device;

    #[test]
    fn the_probe_names_every_signal() {
        let device = Device {
            host: Host {
                platform: Platform::Android,
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
