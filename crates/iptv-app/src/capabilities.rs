//! One host projection for feature controls and action admission.
use crate::Ui;
use iptv_core::capabilities::Host;
use iptv_presentation::capabilities::{Availability, Device, MultiviewOffer, Reason, RecordNow};

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

/// Where Record now is offered on the device as it is now (reactive): by
/// its form factor ([`iptv_presentation::capabilities::record_now`]), and
/// nowhere on a host without workers, which cannot record.
pub fn record_now() -> RecordNow {
    if !crate::host::HAS_WORKERS {
        return RecordNow::Absent;
    }
    iptv_presentation::capabilities::record_now(host())
}

/// Whether the Recordings library shows on the device as it is now
/// (reactive): always, never, or once the library's last read found
/// recordings in it ([`iptv_presentation::capabilities::RecordingsLibrary`]).
pub fn recordings_library(ui: Ui) -> bool {
    crate::host::HAS_WORKERS
        && iptv_presentation::capabilities::recordings_library(host())
            .shown(ui.recordings.with(|s| !s.jobs.is_empty()))
}

/// [`recordings_library`], without subscribing.
pub fn recordings_library_untracked(ui: Ui) -> bool {
    nsh_core::prelude::untracked(|| recordings_library(ui))
}

/// How multiview is offered in `ui`'s window now (reactive): the device's
/// signals, whether the window is wider than tall, and the streams the
/// shown source's account allows (#279).
pub fn multiview_offer(ui: Ui) -> MultiviewOffer {
    multiview_offer_on(ui, crate::device::current())
}

/// [`multiview_offer`] on `device`'s signals.
pub fn multiview_offer_on(ui: Ui, device: Device) -> MultiviewOffer {
    let w = nsh_core::prelude::window();
    iptv_presentation::capabilities::multiview_offer(
        device,
        w.width.0 > w.height.0,
        account_streams(ui),
    )
}

/// The streams the playing channel's account allows at once, when it
/// says: the shown source's, or in All playlists the playing channel's
/// own source's (`data::combined::connection_limit`).
fn account_streams(ui: Ui) -> Option<u32> {
    ui.app.account_streams.get().or_else(|| {
        let channel = ui.playing.get()?;
        let schedule = crate::schedule();
        let all = schedule
            .as_any()?
            .downcast_ref::<iptv_guide::combined::CombinedSchedule>()?;
        let (source, _) = all.channel_source(channel)?;
        crate::data::combined::connection_limit(
            &crate::runtime(),
            crate::runtime::model::SourceId(source.id),
        )
    })
}

/// The reason multiview is drawn disabled in `ui`'s window now (a TV at
/// the account's limit), for the entry points' accessibility descriptions.
pub fn multiview_disabled(ui: Ui) -> Option<&'static str> {
    match multiview_offer(ui) {
        MultiviewOffer::Disabled(why) => Some(reason(why)),
        _ => None,
    }
}

/// Whether multiview can open (or, on a detour, its grid come back) now.
/// A session already open is not refused for a reason that disables it
/// (the account's limit): its streams were admitted.
pub fn multiview(ui: Ui) -> Availability {
    let mut offer = multiview_offer(ui);
    if let MultiviewOffer::Disabled(_) = offer
        && crate::multiview::session(ui)
    {
        offer = MultiviewOffer::Tiles(host().decoder_slots());
    }
    iptv_presentation::capabilities::multiview(
        offer,
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
        Reason::NotOnThisDevice => tr("Multiview is not available on this device yet"),
        Reason::PhoneUpright => tr("Turn the phone sideways for multiview"),
        Reason::WindowTooSmall => tr("Make the window larger for multiview"),
        Reason::ConnectionLimit => tr("Your provider allows one stream at a time"),
    }
}
