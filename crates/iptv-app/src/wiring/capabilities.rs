//! One host projection for feature controls and action admission.
use iptv_presentation::capabilities::{Reason, RecordNow};

/// Where Record now is offered on the device as it is now (reactive): by
/// its form factor ([`iptv_presentation::capabilities::record_now`]), and
/// nowhere on a host without workers, which cannot record.
pub fn record_now() -> RecordNow {
    if !crate::host::HAS_WORKERS {
        return RecordNow::Absent;
    }
    iptv_presentation::capabilities::record_now(crate::wiring::device::host())
}

/// Whether the Recordings library shows on the device as it is now
/// (reactive): always, never, or once the library's last read found
/// recordings in it ([`iptv_presentation::capabilities::RecordingsLibrary`]).
pub fn recordings_library(screens: crate::ScreensUi) -> bool {
    crate::host::HAS_WORKERS
        && iptv_presentation::capabilities::recordings_library(crate::wiring::device::host())
            .shown(screens.recordings.with(|s| !s.jobs.is_empty()))
}

/// [`recordings_library`], without subscribing.
pub fn recordings_library_untracked(screens: crate::ScreensUi) -> bool {
    nsh_core::prelude::untracked(|| recordings_library(screens))
}

pub fn reason(reason: Reason) -> &'static str {
    use crate::kit::i18n::tr;
    match reason {
        Reason::Casting => tr("Stop casting to open multiview."),
        Reason::WaitingForPlayer => tr("Waiting for player availability"),
        Reason::UnsupportedPip => tr("Picture in picture is unavailable on this device"),
        Reason::PlaybackRequired => tr("Play a channel first"),
        Reason::ExtraPlayerWindows => tr("Close the extra player windows before opening multiview"),
        Reason::DecoderLimit => tr("No additional player surface is available"),
        Reason::PipAlreadyActive => tr("Picture in picture is already open"),
        Reason::MainPlayerRequired => tr("Use the main player for multiview"),
        Reason::NotOnThisDevice => tr("Multiview is not available on this device yet"),
        Reason::PhoneUpright => tr("Turn the phone sideways for multiview"),
        Reason::WindowTooSmall => tr("Make the window larger for multiview"),
        Reason::MeteredNetwork => tr("Multiview is unavailable on a metered network"),
        Reason::ConnectionLimit => tr("Your provider allows one stream at a time"),
    }
}
