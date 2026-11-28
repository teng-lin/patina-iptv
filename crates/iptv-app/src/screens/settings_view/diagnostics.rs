//! User-initiated redacted diagnostic export (`general.send_logs`).
//!
//! Confirmed through `Editor::Confirm` before running; never uploads to any server.

use crate::host::diagnostics::{self, DiagnosticHeader, ExportOutcome};
use crate::kit::i18n::tr;
use crate::kit::toast::notify;
use iptv_presentation::layout::Class;

pub(super) fn run(create_picker: bool) {
    let device = crate::wiring::device::current();
    let window_class = match device.window {
        Class::Compact => "compact",
        Class::Medium => "medium",
        Class::Expanded => "expanded",
    };
    let store_lanes = Some(crate::wiring::runtime_owner::runtime().store_queue_stats());
    let header = DiagnosticHeader {
        app_version: env!("CARGO_PKG_VERSION"),
        git_hash: env!("IPTV_GIT_HASH"),
        store_schema_version: iptv_runtime::model::STORE_SCHEMA_VERSION,
        settings_schema_version: iptv_settings::backup::VERSION,
        nsh_rev: include_str!("../../../../../nsh.rev").trim(),
        platform: diagnostics::platform_label(device.host.platform),
        target: env!("IPTV_TARGET"),
        form_factor: device.form_factor().name(),
        window_class,
        uptime: diagnostics::elapsed(),
        store_lanes,
    };
    let file_name = export_file_name(crate::host::since_epoch().map(|d| d.as_secs()));
    let content = diagnostics::export(&header);
    crate::wiring::tasks::spawn_in_app(async move {
        let outcome =
            diagnostics::deliver_export(create_picker, file_name, content, tr("Diagnostic log"))
                .await;
        match outcome {
            ExportOutcome::Saved => notify(tr("Diagnostic log saved")),
            ExportOutcome::Shared => notify(tr("Diagnostic log shared")),
            ExportOutcome::CopiedToClipboard => {
                notify(tr("Diagnostic log copied to the clipboard"))
            }
            ExportOutcome::Canceled => notify(tr("Diagnostic log canceled")),
            ExportOutcome::Failed => notify(tr("Could not save the diagnostic log")),
        }
    });
}

pub(crate) fn export_file_name(epoch_secs: Option<u64>) -> String {
    match epoch_secs.filter(|&s| s > 0) {
        Some(secs) => {
            let (year, month, day, hour, min, sec) = utc_ymdhms(secs);
            format!("patina-diagnostics-{year:04}{month:02}{day:02}-{hour:02}{min:02}{sec:02}.txt")
        }
        None => "patina-diagnostics-00000000-000000.txt".to_owned(),
    }
}

/// Howard Hinnant's civil-from-days algorithm (UTC, proleptic Gregorian).
fn utc_ymdhms(epoch_secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (epoch_secs / 86_400) as i64;
    let rem = (epoch_secs % 86_400) as u32;
    let hour = rem / 3_600;
    let min = (rem % 3_600) / 60;
    let sec = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day, hour, min, sec)
}

#[cfg(test)]
mod tests {
    use super::export_file_name;

    #[test]
    fn utc_filename_formats_known_timestamps() {
        assert_eq!(
            export_file_name(Some(1_700_000_000)),
            "patina-diagnostics-20231114-221320.txt"
        );
        assert_eq!(
            export_file_name(None),
            "patina-diagnostics-00000000-000000.txt"
        );
    }
}
