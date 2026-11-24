//! What can stop a Record now capture on this device, or make it costly,
//! as the host reports it (#275), and what the viewer is told as one starts
//! (`iptv_presentation::capabilities::record_now_notices`).
//!
//! - Android reports through the device observer (`tv.device`,
//!   `DeviceComponent.kt`) when it attaches, on each configuration change
//!   and when the network changes: whether the active network is metered,
//!   whether Low Power Standby stops Patina when the TV is turned off
//!   (Android 13 and later), and whether Android limits a background
//!   capture to six hours a day (Android 15 and later).
//! - A desktop reports whether it could keep the computer awake for the
//!   capture (`host::keep_awake`).
//! - Other hosts report nothing: no metered-network signal, no standby.

use std::cell::Cell;

use iptv_presentation::capabilities::CaptureLimits;

thread_local! {
    static REPORTED: Cell<CaptureLimits> = const {
        Cell::new(CaptureLimits {
            metered: None,
            standby_stops: false,
            background_hours: false,
            sleep_allowed: false,
        })
    };
}

/// The host's report of `data`: `{"metered": bool, "standbyStops": bool,
/// "backgroundHours": bool}`; a field missing keeps what was reported.
pub(crate) fn report(data: &nsh_core::JsonValue) {
    REPORTED.with(|reported| {
        let mut limits = reported.get();
        if let Some(metered) = data["metered"].as_bool() {
            limits.metered = Some(metered);
        }
        if let Some(stops) = data["standbyStops"].as_bool() {
            limits.standby_stops = stops;
        }
        if let Some(hours) = data["backgroundHours"].as_bool() {
            limits.background_hours = hours;
        }
        if limits != reported.get() {
            log::info!(
                "iptv capture limits: metered={:?} standby_stops={} background_hours={}",
                limits.metered,
                limits.standby_stops,
                limits.background_hours
            );
            reported.set(limits);
        }
    });
}

/// The limits now, with the desktop's sleep report.
pub(crate) fn now() -> CaptureLimits {
    let mut limits = REPORTED.with(Cell::get);
    limits.sleep_allowed = crate::host::keep_awake_failed();
    limits
}

/// What the viewer is told as a capture starts on the device as it is
/// now: the notices joined, or that it is starting.
pub(crate) fn start_notice() -> String {
    let host = nsh_core::prelude::untracked(crate::capabilities::host);
    let notices = iptv_presentation::capabilities::record_now_notices(host, now());
    if notices.is_empty() {
        return crate::i18n::tr("Starting recording").to_owned();
    }
    let mut said = Vec::with_capacity(notices.len());
    for notice in notices {
        said.push(crate::i18n::tr(notice));
    }
    said.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_replaces_what_it_names() {
        let json = |text: &str| -> nsh_core::JsonValue { serde_json::from_str(text).unwrap() };
        report(&json(r#"{"metered":true,"standbyStops":true}"#));
        assert_eq!(REPORTED.with(Cell::get).metered, Some(true));
        assert!(REPORTED.with(Cell::get).standby_stops);
        report(&json(r#"{"metered":false}"#));
        let limits = REPORTED.with(Cell::get);
        assert_eq!(limits.metered, Some(false));
        assert!(limits.standby_stops, "kept");
        assert!(!limits.background_hours);
    }
}
