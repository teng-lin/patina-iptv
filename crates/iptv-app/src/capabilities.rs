//! One host projection for feature controls and action admission.
use crate::Ui;
use iptv_core::capabilities::Host;
use iptv_presentation::capabilities::{Availability, Reason};

/// The host as its build describes it, before the device reports its form
/// factor and input: for the facilities that follow the operating system
/// alone (background recording, login registration, the alarm
/// permission), and for work off the UI thread or before the root mounts.
pub const BUILT: Host = Host::of(crate::host::PLATFORM);

/// The host now (reactive), with the form factor and input the device
/// reports (`device.rs`): what gates that can change while the app runs
/// read.
pub fn host() -> Host {
    crate::device::host()
}

pub fn multiview(ui: Ui) -> Availability {
    iptv_presentation::capabilities::multiview(
        host(),
        ui.player_surface == 0,
        ui.playing.get().is_some(),
        ui.player_windows
            .with(|windows| !windows.sessions().is_empty()),
    )
}

pub fn reason(reason: Reason) -> &'static str {
    use crate::i18n::tr;
    match reason {
        Reason::WaitingForPlayer => tr("Waiting for player availability"),
        Reason::UnsupportedPip => tr("Picture in picture is unavailable on this device"),
        Reason::PlaybackRequired => tr("Play a channel first"),
        Reason::ExtraPlayerWindows => tr("Close the extra player windows before opening multiview"),
        Reason::DecoderLimit => tr("No additional player surface is available"),
        Reason::PipAlreadyActive => tr("Picture in picture is already open"),
        Reason::MainPlayerRequired => tr("Use the main player for multiview"),
    }
}
