//! One host projection for feature controls and action admission.
use crate::Ui;
use iptv_core::capabilities::{Availability, Host, Platform, Reason};
use nsh_core::prelude::*;

pub const HOST: Host = Host {
    platform: if cfg!(target_os = "android") {
        Platform::Android
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "ios") {
        Platform::Ipad
    } else {
        Platform::Unknown
    },
};

pub fn multiview(ui: Ui) -> Availability {
    iptv_core::capabilities::multiview(
        HOST,
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
